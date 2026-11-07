//! `.mrpack`（Modrinth 整合包）解析。
//!
//! 规范：<https://support.modrinth.com/en/articles/8802351-modrinth-modpack-format-mrpack>
//!
//! 这个格式是白送的：`modrinth.index.json` 自带每个文件的路径、sha1 和
//! 下载地址，不需要回 API 换。真正要小心的只有三件事：
//! 1. `formatVersion` 必须是 1，`game` 必须是 `minecraft`；
//! 2. `dependencies` 里可能出现 `quilt-loader` 或别的键 —— 认不得就明确拒绝，
//!    不能装作纯原版装下去；
//! 3. `env.client == "unsupported"` 的文件要跳过（那是服务端专属的）。

use std::collections::BTreeMap;
use std::fs;

use serde::Deserialize;
use xcl_core::AppError;
use xcl_loader::LoaderKind;

use super::extract;
use super::{MissingFile, PackFile, PackManifest};

/// 清单本身的上限。真实 mrpack 的 index 几十 KB，1 MB 已经极宽松。
const INDEX_LIMIT: u64 = 1024 * 1024;

const OVERRIDES: [&str; 2] = ["overrides/", "client-overrides/"];

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Index {
    format_version: u32,
    #[serde(default)]
    name: String,
    files: Vec<IndexFile>,
    dependencies: BTreeMap<String, String>,
}

#[derive(Debug, Deserialize)]
struct IndexFile {
    path: String,
    #[serde(default)]
    hashes: Hashes,
    #[serde(default)]
    downloads: Vec<String>,
    #[serde(default)]
    env: Option<Env>,
    #[serde(default, rename = "fileSize")]
    file_size: Option<u64>,
}

#[derive(Debug, Default, Deserialize)]
struct Hashes {
    #[serde(default)]
    sha1: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Env {
    #[serde(default)]
    client: Option<String>,
}

/// 解析一个已经打开的 mrpack。
pub(crate) fn parse(archive: &mut zip::ZipArchive<fs::File>) -> Result<PackManifest, AppError> {
    let (index, _base) = extract::find_entry(archive, "modrinth.index.json")?
        .ok_or_else(|| {
            AppError::unsupported(
                "这不是 Modrinth 整合包",
                "压缩包里找不到 modrinth.index.json",
            )
        })?;
    let text = extract::read_entry(archive, index, INDEX_LIMIT)?;
    let index: Index = serde_json::from_str(&text)
        .map_err(|err| AppError::parse("整合包信息损坏", err.to_string()))?;

    if index.format_version != 1 {
        return Err(AppError::unsupported(
            "不支持的整合包格式",
            format!("格式版本 {}，只认 1", index.format_version),
        ));
    }

    let game = index
        .dependencies
        .get("minecraft")
        .cloned()
        .ok_or_else(|| AppError::unsupported("整合包没写游戏版本", "dependencies 里没有 minecraft"))?;

    let loader = loader_of(&index.dependencies)?;

    let mut files = Vec::new();
    let mut missing = Vec::new();
    for file in index.files {
        // 服务端专属的文件不装
        let client_flag = file.env.as_ref().and_then(|env| env.client.as_deref());
        if client_flag == Some("unsupported") {
            continue;
        }
        // 路径先校验，坏包不往下走
        extract::safe_relative(&file.path)?;
        if file.downloads.is_empty() {
            missing.push(MissingFile {
                name: file.path.clone(),
                reason: "整合包没给下载地址".to_string(),
                required: client_flag != Some("optional"),
            });
            continue;
        }
        files.push(PackFile {
            path: file.path,
            urls: file.downloads,
            sha1: file.hashes.sha1.map(|hash| hash.to_ascii_lowercase()),
            size: file.file_size.unwrap_or(0),
        });
    }

    let overrides = OVERRIDES.iter().map(|prefix| prefix.to_string()).collect();
    let icon = extract::read_icon(archive, &OVERRIDES.map(str::to_string));

    let name = if index.name.trim().is_empty() {
        "整合包".to_string()
    } else {
        index.name
    };

    Ok(PackManifest {
        name,
        game,
        loader,
        files,
        missing,
        overrides,
        icon,
    })
}

/// `dependencies` → 加载器。
///
/// 认不得的键（`quilt-loader`、`liteloader`…）直接拒绝：我们装不了，
/// 装作纯原版装下去只会得到一个进不去的实例。
fn loader_of(deps: &BTreeMap<String, String>) -> Result<Option<(LoaderKind, String)>, AppError> {
    if let Some(version) = deps.get("fabric-loader") {
        return Ok(Some((LoaderKind::Fabric, version.clone())));
    }
    if let Some(version) = deps.get("forge") {
        return Ok(Some((LoaderKind::Forge, version.clone())));
    }
    if let Some(version) = deps.get("neoforge").or_else(|| deps.get("neo-forge")) {
        return Ok(Some((LoaderKind::NeoForge, version.clone())));
    }
    let known = ["minecraft", "fabric-loader", "forge", "neoforge", "neo-forge"];
    if let Some(other) = deps.keys().find(|key| !known.contains(&key.as_str())) {
        return Err(AppError::unsupported(
            "这个整合包要的加载器还没支持",
            format!("dependencies 里有 {other}"),
        )
        .with_hint("目前只支持 Fabric / Forge / NeoForge"));
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::loader_of;
    use std::collections::BTreeMap;

    fn deps(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect()
    }

    #[test]
    fn picks_the_declared_loader() {
        let got = loader_of(&deps(&[("minecraft", "1.20.1"), ("fabric-loader", "0.16.5")]))
            .unwrap()
            .unwrap();
        assert_eq!(got.0, xcl_loader::LoaderKind::Fabric);
        assert_eq!(got.1, "0.16.5");
    }

    #[test]
    fn allows_a_vanilla_pack() {
        assert!(loader_of(&deps(&[("minecraft", "1.20.1")])).unwrap().is_none());
    }

    #[test]
    fn rejects_quilt() {
        assert!(loader_of(&deps(&[("minecraft", "1.20.1"), ("quilt-loader", "0.24.0")])).is_err());
    }

    /// 造一个真的 zip 走一遍完整解析 —— 单测 `loader_of` 抓不到
    /// 「条目找不着 / JSON 读串了 / 前缀拼错」这类问题。
    fn make_pack(prefix: &str, index: &str) -> std::fs::File {
        use std::io::Write;
        use zip::write::SimpleFileOptions;

        let path = std::env::temp_dir().join(format!(
            "xcl-mrpack-{}-{}.mrpack",
            std::process::id(),
            prefix.replace('/', "_")
        ));
        let file = std::fs::File::create(&path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let options = SimpleFileOptions::default();
        zip.start_file(format!("{prefix}modrinth.index.json"), options)
            .unwrap();
        zip.write_all(index.as_bytes()).unwrap();
        zip.start_file(format!("{prefix}overrides/config/a.toml"), options)
            .unwrap();
        zip.write_all(b"x = 1").unwrap();
        zip.finish().unwrap();
        // ZipWriter 已经关了文件，重新打开读
        std::fs::File::open(&path).unwrap()
    }

    #[test]
    fn reads_a_pack_at_the_root() {
        let index = r#"{
            "formatVersion": 1,
            "name": "Cool Pack",
            "files": [
                { "path": "mods/a.jar", "hashes": { "sha1": "ABCDEF" },
                  "downloads": ["https://cdn.modrinth.com/a.jar"], "fileSize": 12 },
                { "path": "mods/server.jar", "hashes": { "sha1": "X" },
                  "downloads": ["https://cdn.modrinth.com/s.jar"],
                  "env": { "client": "unsupported" } }
            ],
            "dependencies": { "minecraft": "1.20.1", "fabric-loader": "0.16.5" }
        }"#;
        let file = make_pack("", index);
        let mut archive = zip::ZipArchive::new(file).unwrap();
        let pack = super::parse(&mut archive).unwrap();
        assert_eq!(pack.name, "Cool Pack");
        assert_eq!(pack.game, "1.20.1");
        assert_eq!(pack.loader.unwrap().1, "0.16.5");
        // 服务端专属的那个被跳过，sha1 归一成小写
        assert_eq!(pack.files.len(), 1);
        assert_eq!(pack.files[0].path, "mods/a.jar");
        assert_eq!(pack.files[0].sha1.as_deref(), Some("abcdef"));
        assert!(pack.overrides.iter().any(|prefix| prefix == "overrides/"));
    }

    #[test]
    fn reads_a_pack_nested_one_level() {
        let index = r#"{
            "formatVersion": 1,
            "name": "Nested",
            "files": [],
            "dependencies": { "minecraft": "1.21.1" }
        }"#;
        let file = make_pack("pack/", index);
        let mut archive = zip::ZipArchive::new(file).unwrap();
        let pack = super::parse(&mut archive).unwrap();
        assert_eq!(pack.name, "Nested");
        assert!(pack.loader.is_none());
    }
}
