//! CurseForge 整合包（`.zip` + `manifest.json`）解析与下载地址解析。
//!
//! 这个格式比 mrpack 难一个量级：manifest 只给 `projectID` + `fileID`，
//! 没有文件名、没有路径、没有下载地址，全部得回 API 换。而且作者可以
//! 关掉第三方分发 —— 那时 `downloadUrl` 是空的，只能记下来跳过。
//!
//! 三级兜底（顺序照抄 conic，四家实现的共识）：
//! 1. `POST /v1/mods/files` 批量，每 100 个一批
//! 2. 批量没返回的 → `GET /v1/mods/{project}/files/{file}`
//! 3. `downloadUrl` 仍为空 → `GET .../download-url`
//!
//! 全部只走镜像：CF 官方无 key 必 403，`post_mod_json` 已经把官方候选剔掉了。

use std::collections::{HashMap, HashSet};
use std::fs;

use serde::Deserialize;
use serde_json::Value;
use xcl_core::AppError;
use xcl_loader::LoaderKind;

use super::extract;
use super::{MissingFile, PackFile, PackManifest};
use crate::AppState;

/// manifest 上限。真实包几十 KB。
const MANIFEST_LIMIT: u64 = 4 * 1024 * 1024;
/// 批量接口一次最多查多少个文件 id。
const BATCH: usize = 100;

const FILES_API: &str = "https://api.curseforge.com/v1/mods/files";
const MOD_API: &str = "https://api.curseforge.com/v1/mods";

/// 清单里的一对 id。
#[derive(Debug, Clone)]
pub(crate) struct RawFile {
    pub project_id: i64,
    pub file_id: i64,
    pub required: bool,
}

/// 还没换下载地址的包。
#[derive(Debug)]
pub(crate) struct RawPack {
    pub name: String,
    pub game: String,
    pub loader: Option<(LoaderKind, String)>,
    pub overrides: Vec<String>,
    pub files: Vec<RawFile>,
}

#[derive(Deserialize)]
struct Manifest {
    #[serde(default)]
    name: String,
    minecraft: ManifestMinecraft,
    #[serde(default)]
    files: Vec<ManifestFile>,
    #[serde(default = "default_overrides")]
    overrides: String,
}

#[derive(Deserialize)]
struct ManifestMinecraft {
    version: String,
    #[serde(default, rename = "modLoaders")]
    mod_loaders: Vec<ManifestLoader>,
}

#[derive(Deserialize)]
struct ManifestLoader {
    id: String,
    #[serde(default)]
    primary: bool,
}

#[derive(Deserialize)]
struct ManifestFile {
    #[serde(rename = "projectID")]
    project_id: i64,
    #[serde(rename = "fileID")]
    file_id: i64,
    #[serde(default = "default_true")]
    required: bool,
}

fn default_overrides() -> String {
    "overrides".to_string()
}

fn default_true() -> bool {
    true
}

/// 解析一个已经打开的 CF 整合包。
pub(crate) fn parse(archive: &mut zip::ZipArchive<fs::File>) -> Result<RawPack, AppError> {
    let (index, base) = extract::find_entry(archive, "manifest.json")?.ok_or_else(|| {
        AppError::unsupported("这不是 CurseForge 整合包", "压缩包里找不到 manifest.json")
    })?;
    let text = extract::read_entry(archive, index, MANIFEST_LIMIT)?;
    let manifest: Manifest = serde_json::from_str(&text)
        .map_err(|err| AppError::parse("整合包信息损坏", err.to_string()))?;

    if manifest.minecraft.version.trim().is_empty() {
        return Err(AppError::unsupported(
            "整合包没写游戏版本",
            "manifest.json 的 minecraft.version 是空的",
        ));
    }
    let loader = loader_of(&manifest)?;

    // overrides 是相对包根的目录名，可能自定义（"overrides-dir" 之类）
    let overrides_name = manifest.overrides.trim().trim_matches('/');
    let overrides = if overrides_name.is_empty() || overrides_name == "." {
        Vec::new()
    } else {
        vec![format!("{base}{overrides_name}/")]
    };

    let name = if manifest.name.trim().is_empty() {
        "整合包".to_string()
    } else {
        manifest.name
    };

    let files = manifest
        .files
        .into_iter()
        .map(|file| RawFile {
            project_id: file.project_id,
            file_id: file.file_id,
            required: file.required,
        })
        .collect();

    Ok(RawPack {
        name,
        game: manifest.minecraft.version,
        loader,
        overrides,
        files,
    })
}

/// `modLoaders[]` → 加载器。
///
/// 优先 `primary`；老包会写成 `forge-1.12.2-14.23.5.2859`，把 mc 版本前缀剥掉。
fn loader_of(manifest: &Manifest) -> Result<Option<(LoaderKind, String)>, AppError> {
    let Some(entry) = manifest
        .minecraft
        .mod_loaders
        .iter()
        .find(|loader| loader.primary)
        .or_else(|| manifest.minecraft.mod_loaders.first())
    else {
        return Ok(None);
    };

    let (kind, version) = entry.id.split_once('-').ok_or_else(|| {
        AppError::unsupported("整合包的加载器信息看不懂", entry.id.clone())
    })?;
    let kind = LoaderKind::parse(kind).ok_or_else(|| {
        AppError::unsupported(
            "这个整合包要的加载器还没支持",
            format!("加载器 id：{}", entry.id),
        )
        .with_hint("目前只支持 Fabric / Forge / NeoForge")
    })?;

    let prefix = format!("{}-", manifest.minecraft.version);
    let version = version.strip_prefix(&prefix).unwrap_or(version);
    if version.is_empty() {
        return Err(AppError::unsupported(
            "整合包没写加载器版本",
            format!("加载器 id：{}", entry.id),
        ));
    }
    Ok(Some((kind, version.to_string())))
}

impl AppState {
    /// 把 `RawPack` 的文件 id 换成真正的下载信息。
    ///
    /// 返回 `(可下载的文件, 拿不到的文件)`。拿不到的那部分不阻断安装 ——
    /// 少几个 Mod 的整合包通常还能进，别的实现会弹窗问，我们没有阻塞弹窗。
    pub(crate) async fn resolve_pack_files(
        &self,
        raw: &RawPack,
    ) -> Result<(Vec<PackFile>, Vec<MissingFile>), AppError> {
        if raw.files.is_empty() {
            return Ok((Vec::new(), Vec::new()));
        }

        let mut metadata: HashMap<i64, Value> = HashMap::new();
        let mut last_error: Option<String> = None;

        // 第一级：批量
        for chunk in raw.files.chunks(BATCH) {
            let ids: Vec<i64> = chunk.iter().map(|file| file.file_id).collect();
            let body = serde_json::json!({ "fileIds": ids });
            match self.post_mod_json(FILES_API, &body).await {
                Ok(text) => {
                    if let Ok(value) = serde_json::from_str::<Value>(&text) {
                        if let Some(data) = value.get("data").and_then(Value::as_array) {
                            for item in data {
                                if let Some(id) = item.get("id").and_then(Value::as_i64) {
                                    metadata.insert(id, item.clone());
                                }
                            }
                        }
                    }
                }
                Err(err) => {
                    // 镜像不支持 POST 时就是这里失败，后面靠逐个查兜底
                    tracing::warn!(error = %err, "CurseForge 批量查文件失败");
                    last_error = Some(format!("{}：{}", err.title, err.message));
                }
            }
        }

        // 第二级：逐个查
        let missing: Vec<RawFile> = raw
            .files
            .iter()
            .filter(|file| !metadata.contains_key(&file.file_id))
            .cloned()
            .collect();
        for file in missing {
            let url = format!("{MOD_API}/{}/files/{}", file.project_id, file.file_id);
            match self.fetch_mod_text(&url, true).await {
                Ok(text) => {
                    if let Ok(value) = serde_json::from_str::<Value>(&text) {
                        if let Some(data) = value.get("data").filter(|data| !data.is_null()) {
                            metadata.insert(file.file_id, data.clone());
                        }
                    }
                }
                Err(err) => {
                    tracing::warn!(project = file.project_id, file = file.file_id, error = %err, "CurseForge 查单个文件失败");
                    last_error = Some(format!("{}：{}", err.title, err.message));
                }
            }
        }

        // 一个都没查到 = 整条链路挂了，别装作「文件都被删了」
        if metadata.is_empty() {
            return Err(AppError::network(
                "CurseForge 暂时不可用",
                last_error.unwrap_or_else(|| "拿不到整合包里的文件信息".to_string()),
            )
            .with_hint("检查网络后重试，或去 CurseForge 手动下载"));
        }

        // 第三级：downloadUrl 为空时问一次 download-url
        let mut fallback: HashMap<i64, Option<String>> = HashMap::new();
        for file in &raw.files {
            let Some(item) = metadata.get(&file.file_id) else {
                continue;
            };
            let empty = item
                .get("downloadUrl")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .is_empty();
            if !empty {
                continue;
            }
            let url = format!(
                "{MOD_API}/{}/files/{}/download-url",
                file.project_id, file.file_id
            );
            let resolved = self
                .fetch_mod_text(&url, true)
                .await
                .ok()
                .and_then(|text| serde_json::from_str::<Value>(&text).ok())
                .and_then(|value| {
                    value
                        .get("data")
                        .and_then(Value::as_str)
                        .filter(|url| !url.is_empty())
                        .map(str::to_string)
                });
            fallback.insert(file.file_id, resolved);
        }

        let mut files = Vec::new();
        let mut missing = Vec::new();
        let mut seen: HashSet<String> = HashSet::new();

        for file in &raw.files {
            let Some(item) = metadata.get(&file.file_id) else {
                missing.push(MissingFile {
                    name: format!("project {} / file {}", file.project_id, file.file_id),
                    reason: "这个文件在 CurseForge 上已经没有了".to_string(),
                    required: file.required,
                });
                continue;
            };

            if item
                .get("isAvailable")
                .and_then(Value::as_bool)
                .is_some_and(|available| !available)
            {
                missing.push(MissingFile {
                    name: file_name_of(item).unwrap_or_else(|| format!("file {}", file.file_id)),
                    reason: "作者把这个文件标为不可用".to_string(),
                    required: file.required,
                });
                continue;
            }

            let Some(file_name) = file_name_of(item) else {
                missing.push(MissingFile {
                    name: format!("project {} / file {}", file.project_id, file.file_id),
                    reason: "CurseForge 给的文件名不合法".to_string(),
                    required: file.required,
                });
                continue;
            };

            if !seen.insert(file_name.to_ascii_lowercase()) {
                missing.push(MissingFile {
                    name: file_name,
                    reason: "整合包里有两个同名的文件".to_string(),
                    required: file.required,
                });
                continue;
            }

            let url = item
                .get("downloadUrl")
                .and_then(Value::as_str)
                .filter(|url| !url.is_empty())
                .map(str::to_string)
                .or_else(|| fallback.get(&file.file_id).cloned().flatten());

            let Some(url) = url else {
                missing.push(MissingFile {
                    name: file_name,
                    reason: "作者关闭了第三方下载".to_string(),
                    required: file.required,
                });
                continue;
            };

            let subdir = subdir_of(item);
            files.push(PackFile {
                path: format!("{subdir}/{file_name}"),
                urls: vec![self.mod_asset_url(&url)],
                sha1: sha1_of(item),
                size: item
                    .get("fileLength")
                    .and_then(Value::as_u64)
                    .unwrap_or(0),
            });
        }

        Ok((files, missing))
    }
}

/// CF 返回的文件名要直接当路径用，所以必须是一个纯文件名。
fn file_name_of(item: &Value) -> Option<String> {
    let raw = item.get("fileName").and_then(Value::as_str)?.trim();
    if raw.is_empty() || raw == "." || raw == ".." {
        return None;
    }
    if raw.contains('/') || raw.contains('\\') {
        return None;
    }
    Some(raw.to_string())
}

/// 落点：CF 只给 `modules`（文件内部结构），据此猜它该放哪。
///
/// 猜错也无所谓 —— 放错的资源包不会让游戏崩，只是没生效。
fn subdir_of(item: &Value) -> &'static str {
    let Some(modules) = item.get("modules").and_then(Value::as_array) else {
        return "mods";
    };
    let names: Vec<&str> = modules
        .iter()
        .filter_map(|module| module.get("name").and_then(Value::as_str))
        .collect();
    if names.contains(&"pack.mcmeta") {
        return "resourcepacks";
    }
    if names.contains(&"shaders") {
        return "shaderpacks";
    }
    "mods"
}

/// `hashes[]` 里 `algo == 1` 是 sha1。
fn sha1_of(item: &Value) -> Option<String> {
    item.get("hashes")?.as_array()?.iter().find_map(|hash| {
        (hash.get("algo").and_then(Value::as_i64) == Some(1))
            .then(|| hash.get("value").and_then(Value::as_str))
            .flatten()
            .map(|value| value.to_ascii_lowercase())
    })
}

impl RawPack {
    /// 转成统一产物（还没有文件信息时用）。
    pub(crate) fn into_manifest(
        self,
        files: Vec<PackFile>,
        missing: Vec<MissingFile>,
    ) -> PackManifest {
        PackManifest {
            name: self.name,
            game: self.game,
            loader: self.loader,
            files,
            missing,
            overrides: self.overrides,
            icon: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{file_name_of, loader_of, sha1_of, subdir_of, Manifest};
    use serde_json::json;

    fn manifest(text: &str) -> Manifest {
        serde_json::from_str(text).expect("一份合法清单")
    }

    #[test]
    fn reads_a_forge_pack() {
        let pack = manifest(
            r#"{
                "name": "DeceasedCraft",
                "minecraft": {
                    "version": "1.20.1",
                    "modLoaders": [{ "id": "forge-47.4.0", "primary": true }]
                },
                "files": [{ "projectID": 280510, "fileID": 7612291, "required": true }]
            }"#,
        );
        let (kind, version) = loader_of(&pack).unwrap().unwrap();
        assert_eq!(kind, xcl_loader::LoaderKind::Forge);
        assert_eq!(version, "47.4.0");
        assert_eq!(pack.files.len(), 1);
    }

    #[test]
    fn strips_a_legacy_minecraft_prefix() {
        let pack = manifest(
            r#"{
                "minecraft": {
                    "version": "1.12.2",
                    "modLoaders": [{ "id": "forge-1.12.2-14.23.5.2859", "primary": true }]
                }
            }"#,
        );
        let (_, version) = loader_of(&pack).unwrap().unwrap();
        assert_eq!(version, "14.23.5.2859");
    }

    #[test]
    fn allows_a_vanilla_pack() {
        let pack = manifest(r#"{"minecraft": {"version": "1.20.1"}}"#);
        assert!(loader_of(&pack).unwrap().is_none());
    }

    #[test]
    fn rejects_an_unknown_loader() {
        let pack = manifest(
            r#"{
                "minecraft": {
                    "version": "1.20.1",
                    "modLoaders": [{ "id": "liteloader-1.0", "primary": true }]
                }
            }"#,
        );
        assert!(loader_of(&pack).is_err());
    }

    #[test]
    fn rejects_path_like_file_names() {
        assert_eq!(file_name_of(&json!({ "fileName": "a.jar" })).as_deref(), Some("a.jar"));
        assert_eq!(file_name_of(&json!({ "fileName": "../a.jar" })), None);
        assert_eq!(file_name_of(&json!({ "fileName": "a/b.jar" })), None);
        assert_eq!(file_name_of(&json!({ "fileName": "" })), None);
    }

    #[test]
    fn guesses_the_target_folder_from_modules() {
        assert_eq!(
            subdir_of(&json!({ "modules": [{ "name": "pack.mcmeta" }] })),
            "resourcepacks"
        );
        assert_eq!(subdir_of(&json!({ "modules": [{ "name": "shaders" }] })), "shaderpacks");
        assert_eq!(subdir_of(&json!({ "modules": [{ "name": "META-INF" }] })), "mods");
        assert_eq!(subdir_of(&json!({ "modules": null })), "mods");
    }

    #[test]
    fn reads_the_sha1_hash() {
        assert_eq!(
            sha1_of(&json!({
                "hashes": [
                    { "algo": 1, "value": "ABCDEF" },
                    { "algo": 2, "value": "ignored" }
                ]
            }))
            .as_deref(),
            Some("abcdef")
        );
    }
}
