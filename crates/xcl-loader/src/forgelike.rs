//! Forge / NeoForge 的安装器元数据与清单计算。
//!
//! 只做纯逻辑，不做 IO：请求由 `xcl-service` 发，镜像改写交给 `xcl-source`。
//!
//! 与 Fabric 的根本差别：Fabric 官方直接给完整 profile JSON，Forge 系必须
//! 跑官方安装器（`java -jar installer.jar --installClient <目录>`）才能拿到
//! 处理器产物。实测依据见 `docs/LOADER-FORGELIKE.md` 第 9 节：
//! - 安装器的 `--mirror` 参数不生效，逐条库仍打官方域名 → 必须先把库预下载齐；
//! - 预下载清单要取 `install_profile.libraries` 与 `version.json.libraries` 的并集；
//! - 写出来的版本 JSON 用安装器生成的那份，不往里加库。

use std::collections::HashMap;

use serde::Deserialize;

use xcl_core::{AppError, Library};

use crate::fabric::LoaderVersion;
use crate::LoaderKind;

/// Forge maven 根地址。
pub const FORGE_MAVEN_BASE: &str = "https://maven.minecraftforge.net";

/// NeoForge maven 根地址（镜像里没有 `/releases` 这一段，剥掉交给 `xcl-source` 做）。
pub const NEOFORGED_MAVEN_BASE: &str = "https://maven.neoforged.net/releases";

/// 安装器 jar 里的版本 JSON 条目名。
pub const VERSION_JSON_ENTRY: &str = "version.json";

/// Legacy 安装器里内嵌 maven 的目录前缀（Forge 1.12.2 及更早）。
pub const EMBEDDED_MAVEN_PREFIX: &str = "maven/";

// ---------- 地址 ----------

/// Forge 全量版本元数据（一个 XML 里含所有 MC 版本的构建号）。
pub fn forge_metadata_url() -> String {
    format!("{FORGE_MAVEN_BASE}/net/minecraftforge/forge/maven-metadata.xml")
}

/// NeoForge 版本元数据。
///
/// `legacy` 是 1.20.1 那一支：它从 Forge 分叉出来，artifact 还叫 `forge`，
/// 版本名也带着 `1.20.1-` 前缀（同样按这个特例处理）。
pub fn neoforge_metadata_url(legacy: bool) -> String {
    let artifact = if legacy { "forge" } else { "neoforge" };
    format!("{NEOFORGED_MAVEN_BASE}/net/neoforged/{artifact}/maven-metadata.xml")
}

/// 安装器 jar 的官方地址。`loader` 用 [`display_version`] 那种剥掉 MC 前缀的形态。
pub fn installer_url(kind: LoaderKind, game: &str, loader: &str) -> String {
    match kind {
        LoaderKind::Forge => {
            let raw = format!("{game}-{loader}");
            format!("{FORGE_MAVEN_BASE}/net/minecraftforge/forge/{raw}/forge-{raw}-installer.jar")
        }
        LoaderKind::NeoForge => {
            let (artifact, raw) = neoforge_artifact(game, loader);
            format!("{NEOFORGED_MAVEN_BASE}/net/neoforged/{artifact}/{raw}/{artifact}-{raw}-installer.jar")
        }
        LoaderKind::Fabric => unreachable!("Fabric 没有安装器"),
    }
}

/// 装出来的版本 id（目录名 + JSON 名）。
///
/// 统一 `{加载器}-{加载器版本}-{原版}`，和 Fabric 的
/// `fabric-loader-0.16.5-1.20.1` 同一个形状：都以 `-<原版 id>` 结尾，
/// 于是「这个原版装过哪个加载器」只需要一套判定（见 [`crate::loader_label_of`]）。
///
/// 不照抄安装器自己生成的 id（`1.20.1-forge-47.4.5`）：NeoForge 1.20.1 那一支
/// 生成的也是 `1.20.1-forge-47.1.106`，跟真 Forge 撞名。
pub fn instance_id(kind: LoaderKind, game: &str, loader: &str) -> String {
    format!("{}-{loader}-{game}", kind.as_str())
}

/// NeoForge 用哪个 artifact 目录：1.20.1 那一支仍是 `forge`。
fn neoforge_artifact(game: &str, loader: &str) -> (&'static str, String) {
    if game == "1.20.1" && !loader.starts_with("1.20.1-") {
        return ("forge", format!("1.20.1-{loader}"));
    }
    ("neoforge", loader.to_string())
}

// ---------- 版本列表 ----------

/// 展示与回传用的版本串：剥掉「`{原版}-`」这一段前缀，界面上不出现重复的 MC 版本号。
///
/// 只在已知原版的前提下剥，绝不猜：`14.23.5.2860-1.7.10` 里的尾巴是分支名，
/// 不是 MC 前缀，剥错就拼不出下载地址。
fn strip_game_prefix<'a>(raw: &'a str, game: &str) -> &'a str {
    let prefix = format!("{game}-");
    raw.strip_prefix(prefix.as_str()).unwrap_or(raw)
}

/// 版本名按「数字段比数值、其余比文本」自然排序，返回 `a` 相对 `b` 的顺序。
///
/// Forge 的构建号位数不固定（`47.4.5` / `14.23.5.2860`），按字符串比会把
/// `14.23.5.2860` 排到 `47.4.5` 前面。
fn version_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    let split = |v: &str| -> Vec<String> {
        v.split(['.', '-', '_', '+'])
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
            .collect()
    };
    let left = split(a);
    let right = split(b);
    for i in 0..left.len().max(right.len()) {
        let l = left.get(i).map(String::as_str);
        let r = right.get(i).map(String::as_str);
        let ord = match (l, r) {
            (Some(l), Some(r)) => match (l.parse::<u64>(), r.parse::<u64>()) {
                (Ok(x), Ok(y)) => x.cmp(&y),
                (Ok(_), Err(_)) => std::cmp::Ordering::Greater,
                (Err(_), Ok(_)) => std::cmp::Ordering::Less,
                (Err(_), Err(_)) => l.cmp(r),
            },
            // 段数少的算旧（`47.4` < `47.4.5`）
            (Some(_), None) => std::cmp::Ordering::Greater,
            (None, Some(_)) => std::cmp::Ordering::Less,
            (None, None) => std::cmp::Ordering::Equal,
        };
        if ord != std::cmp::Ordering::Equal {
            return ord;
        }
    }
    std::cmp::Ordering::Equal
}

/// 按版本号从新到老排（元数据文件里的顺序不可依赖）。
fn sort_desc(versions: Vec<String>) -> Vec<String> {
    let mut versions = versions;
    versions.sort_by(|a, b| version_cmp(b, a));
    versions
}

/// 某个原版的 Forge 版本列表（已剥前缀、按新→老）。
///
/// 入参是 [`parse_metadata_versions`] 的全量结果：那份文件包含所有 MC 版本的
/// 构建（2000 多条），服务层按类型缓存一次，切原版只在内存里筛。
pub fn forge_loader_versions(raw: &[String], game: &str) -> Vec<LoaderVersion> {
    let prefix = format!("{game}-");
    let picked: Vec<String> = raw
        .iter()
        .filter(|v| v.starts_with(&prefix))
        .map(|v| strip_game_prefix(v, game).to_string())
        .collect();
    // Forge 元数据不标稳定版，全给 true：`latest` 取第一条就是最新构建
    stabilize(sort_desc(picked), |_| true)
}

/// 某个原版的 NeoForge 版本列表（新老两支合起来筛）。
pub fn neoforge_loader_versions(raw: &[String], game: &str) -> Vec<LoaderVersion> {
    let picked: Vec<String> = raw
        .iter()
        .filter(|v| neoforge_game(v).as_deref() == Some(game))
        .map(|v| strip_game_prefix(v, game).to_string())
        .collect();
    stabilize(sort_desc(picked), |v| {
        !v.contains("beta") && !v.contains("alpha")
    })
}

/// 从 maven 元数据 XML 里取 `<version>` 列表。
///
/// 不用正则：这个文件的结构一直是 `<versions><version>…</version></versions>`，
/// 手写扫描比引一个正则依赖更好控（未知标签一律忽略）。
pub fn parse_metadata_versions(xml: &str) -> Result<Vec<String>, AppError> {
    const OPEN: &str = "<version>";
    const CLOSE: &str = "</version>";
    let mut out = Vec::new();
    let mut rest = xml;
    while let Some(start) = rest.find(OPEN) {
        let after = &rest[start + OPEN.len()..];
        let Some(end) = after.find(CLOSE) else { break };
        let value = after[..end].trim();
        if !value.is_empty() {
            out.push(value.to_string());
        }
        rest = &after[end + CLOSE.len()..];
    }
    if out.is_empty() {
        return Err(AppError::parse(
            "加载器版本列表解析失败",
            "元数据里一个版本号都没有",
        ));
    }
    Ok(out)
}

/// NeoForge 版本名 → 它对应的原版。认不出来就 `None`（当没有这条）。
fn neoforge_game(api: &str) -> Option<String> {
    if let Some(rest) = api.strip_prefix("1.20.1-") {
        // 老一支：`1.20.1-47.1.106`
        let _ = rest;
        return Some("1.20.1".to_string());
    }
    if api.contains('+') || api.contains("w") {
        // `…+snapshot-1` 这类未发布构建、以及 25w14craftmine 愚人节版：不猜
        return None;
    }
    let head = api.split('-').next()?;
    let segs: Vec<&str> = head.split('.').collect();
    if segs.len() < 3 {
        return None;
    }
    let major: u32 = segs[0].parse().ok()?;
    let minor: u32 = segs[1].parse().ok()?;
    if major >= 24 {
        // 2024 起 MC 改成 24.1 / 26.1 这种年号，NeoForge 主版本跟着走
        let build = segs[2];
        let mut inherit = format!("{major}.{minor}.{build}");
        if inherit.ends_with(".0") {
            inherit.truncate(inherit.len() - 2);
        }
        return Some(inherit);
    }
    Some(format!("1.{major}.{minor}"))
}

fn stabilize(versions: Vec<String>, stable: impl Fn(&str) -> bool) -> Vec<LoaderVersion> {
    versions
        .into_iter()
        .map(|version| LoaderVersion {
            stable: stable(&version),
            version,
        })
        .collect()
}

// ---------- 安装器内部结构 ----------

#[derive(Debug, Clone, Deserialize)]
struct DataEntry {
    #[serde(default)]
    client: String,
}

/// `install_profile.json`。只建模我们用得到的字段。
#[derive(Debug, Clone, Deserialize)]
pub struct InstallProfile {
    /// 安装器自己生成的版本 id，如 `1.20.1-forge-47.4.5` / `neoforge-21.1.265`
    pub version: String,
    /// 它要求的原版，如 `1.20.1`
    #[serde(default)]
    pub minecraft: Option<String>,
    /// Legacy 指向安装器内的 `version.json` 条目
    #[serde(default)]
    pub json: Option<String>,
    /// `{KEY}` 的取值表；`client` / `server` 各一份。
    /// 类型细节不外泄，只通过 [`InstallProfile::mojmaps_relative_path`] 这类方法读。
    #[serde(default)]
    data: HashMap<String, DataEntry>,
    /// 处理器条目。为空 = 老式安装器（不需要跑 JVM）
    #[serde(default)]
    pub processors: Vec<serde_json::Value>,
    /// 安装期要用的库（含安装工具本身，比 `version.json` 多）
    #[serde(default)]
    pub libraries: Vec<Library>,
}

impl InstallProfile {
    /// 老式（Forge ≤1.12.2）：没有处理器，库直接内嵌在安装器的 `maven/` 里。
    pub fn is_legacy_extract(&self) -> bool {
        self.processors.is_empty() && self.json.is_some()
    }

    /// 既没有处理器、又指不到 `version.json`：这套我们没做（1.7.10 及更早）。
    pub fn is_unsupported(&self) -> bool {
        self.processors.is_empty() && self.json.is_none()
    }

    /// 需要 Mojang 映射表的路径（1.17+ / NeoForge 的 `DOWNLOAD_MOJMAPS`）。
    ///
    /// 这一步由处理器自己联网打 Mojang，不受任何镜像控制，
    /// 所以要提前把文件放到这个相对路径上（数据来自原版 JSON 的 `client_mappings`）。
    pub fn mojmaps_relative_path(&self) -> Option<String> {
        let raw = self.data.get("MOJMAPS")?.client.trim();
        let body = raw.trim_start_matches('[').trim_end_matches(']').trim_end_matches('\'');
        xcl_core::path::maven_to_relative_path(body)
    }
}

pub fn parse_install_profile(text: &str) -> Result<InstallProfile, AppError> {
    serde_json::from_str(text).map_err(|e| AppError::parse("安装器描述解析失败", e.to_string()))
}

/// 预下载 / 收编要用的一条库。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrefetchItem {
    /// `libraries/` 下的相对路径
    pub relative: String,
    /// `None` = 安装器 / 处理器本地产物，下载不了，只能事后校验存在
    pub url: Option<String>,
    pub sha1: Option<String>,
    pub size: u64,
}

/// 安装期库清单 = 两份清单的并集，按相对路径去重。
///
/// 实测（Forge 1.20.1）`version.json` 只有 29 条，`install_profile` 独有 34 条
/// （`universal`、`fmlcore`、各代 `installertools`、`mcp_config@zip`…）；
/// 少下任何一条，安装器就要自己联网，国内基本等不到。
pub fn prefetch_manifest(
    profile_libraries: &[Library],
    version_libraries: &[Library],
) -> Vec<PrefetchItem> {
    let mut out: Vec<PrefetchItem> = Vec::new();
    for library in profile_libraries.iter().chain(version_libraries) {
        let Some(artifact) = library.resolved_artifact() else {
            continue;
        };
        let Some(relative) = artifact
            .path
            .clone()
            .or_else(|| xcl_core::path::maven_to_relative_path(&library.name))
        else {
            continue;
        };
        let url = Some(artifact.url).filter(|u| !u.trim().is_empty());
        if out.iter().any(|item| item.relative == relative) {
            continue;
        }
        out.push(PrefetchItem {
            relative,
            url,
            sha1: artifact.sha1.clone(),
            size: artifact.size.unwrap_or(0),
        });
    }
    out
}

/// 清单里必须由安装器生成的条目（收编后要拿它们校验「装没装坏」）。
pub fn generated_artifacts(manifest: &[PrefetchItem]) -> Vec<String> {
    manifest
        .iter()
        .filter(|item| item.url.is_none())
        .map(|item| item.relative.clone())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// maven 元数据的真实形状（`<latest>` / `<release>` 不能被当成版本）。
    const FORGE_META: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<metadata>
  <groupId>net.minecraftforge</groupId>
  <artifactId>forge</artifactId>
  <versioning>
    <latest>1.21.1-52.0.17</latest>
    <release>1.21.1-52.0.17</release>
    <versions>
      <version>1.12.2-14.23.5.2860</version>
      <version>1.20.1-47.4.5</version>
      <version>1.20.1-47.4.4</version>
      <version>1.20.1-47.2.0</version>
      <version>1.20.1-14.23.5.2860-1.7.10</version>
    </versions>
    <lastUpdated>20250801</lastUpdated>
  </versioning>
</metadata>"#;

    #[test]
    fn metadata_parser_only_reads_version_items() {
        let got = parse_metadata_versions(FORGE_META).unwrap();
        assert_eq!(got.len(), 5);
        assert!(!got.iter().any(|v| v.contains("1.21.1-52")), "latest 混进来了");
    }

    #[test]
    fn forge_list_is_filtered_and_newest_first() {
        let raw = parse_metadata_versions(FORGE_META).unwrap();
        let list = forge_loader_versions(&raw, "1.20.1");
        let versions: Vec<&str> = list.iter().map(|v| v.version.as_str()).collect();
        // 47.4.5 > 47.4.4 > 47.2.0，且 1.12.2 的条目被滤掉
        assert_eq!(versions[0], "47.4.5");
        assert_eq!(versions[1], "47.4.4");
        assert!(versions.contains(&"14.23.5.2860-1.7.10"), "分支尾巴整条要留着");
        assert!(!versions.iter().any(|v| v.starts_with("1.20.1-")), "前缀必须剥掉");
    }

    #[test]
    fn version_cmp_is_natural_not_lexicographic() {
        assert_eq!(version_cmp("47.4.5", "14.23.5.2860"), std::cmp::Ordering::Greater);
        assert_eq!(version_cmp("36.2.9", "36.2.39"), std::cmp::Ordering::Less);
        assert_eq!(version_cmp("47.4", "47.4.5"), std::cmp::Ordering::Less);
    }

    #[test]
    fn neoforge_maps_loader_to_minecraft() {
        assert_eq!(neoforge_game("21.1.265").as_deref(), Some("1.21.1"));
        assert_eq!(neoforge_game("20.4.30-beta").as_deref(), Some("1.20.4"));
        assert_eq!(neoforge_game("1.20.1-47.1.106").as_deref(), Some("1.20.1"));
        // 2024 起的年号版本：26.1.0.0 → 26.1
        assert_eq!(neoforge_game("26.1.0.0").as_deref(), Some("26.1"));
        // 认不出的不猜
        assert_eq!(neoforge_game("0.25w14craftmine.3-beta"), None);
        assert_eq!(neoforge_game("26.1.0.0-alpha.1+snapshot-1"), None);
    }

    /// 两支元数据合起来（服务层就是这么喂的：老一支镜像上可能没有）。
    fn neoforge_raw(modern: &str, legacy: &str) -> Vec<String> {
        let mut raw = parse_metadata_versions(modern).unwrap_or_default();
        raw.extend(parse_metadata_versions(legacy).unwrap_or_default());
        raw
    }

    #[test]
    fn neoforge_list_strips_prefix_and_marks_beta_unstable() {
        let modern = "<versions><version>21.1.265</version><version>21.1.266-beta</version></versions>";
        let legacy = "<versions><version>1.20.1-47.1.106</version></versions>";
        let list = neoforge_loader_versions(&neoforge_raw(modern, legacy), "1.21.1");
        assert_eq!(list.len(), 2);
        // 266-beta 比 265 新（构建号更大），所以排在前面 —— 但它不是稳定版，
        // 服务层的默认选中项会跳过它取 265
        assert_eq!(list[0].version, "21.1.266-beta");
        assert!(!list[0].stable);
        assert_eq!(list[1].version, "21.1.265");
        assert!(list[1].stable);

        let legacy_list = neoforge_loader_versions(&neoforge_raw("", legacy), "1.20.1");
        assert_eq!(legacy_list[0].version, "47.1.106", "老一支要剥掉 1.20.1- 前缀");
    }

    #[test]
    fn neoforge_list_of_other_game_is_empty_not_an_error() {
        // 1.12.2 这类根本没有 NeoForge 的原版：空列表，界面上写「没有支持」
        let modern = "<versions><version>21.1.265</version></versions>";
        assert!(neoforge_loader_versions(&neoforge_raw(modern, ""), "1.12.2").is_empty());
    }

    #[test]
    fn installer_urls_match_real_maven_layout() {
        // 这些地址我 2026-10-27 逐个下载验证过
        assert_eq!(
            installer_url(LoaderKind::Forge, "1.20.1", "47.4.5"),
            "https://maven.minecraftforge.net/net/minecraftforge/forge/1.20.1-47.4.5/forge-1.20.1-47.4.5-installer.jar"
        );
        assert_eq!(
            installer_url(LoaderKind::NeoForge, "1.21.1", "21.1.265"),
            "https://maven.neoforged.net/releases/net/neoforged/neoforge/21.1.265/neoforge-21.1.265-installer.jar"
        );
        // 1.20.1 那一支的 artifact 仍叫 forge
        assert_eq!(
            installer_url(LoaderKind::NeoForge, "1.20.1", "47.1.106"),
            "https://maven.neoforged.net/releases/net/neoforged/forge/1.20.1-47.1.106/forge-1.20.1-47.1.106-installer.jar"
        );
    }

    #[test]
    fn instance_ids_do_not_collide_on_1_20_1() {
        // 安装器自己给 NeoForge 1.20.1 生成的 id 是 `1.20.1-forge-47.1.106`，
        // 跟真 Forge 撞名，所以我们的 id 必须带加载器名，且形状与 Fabric 一致
        assert_eq!(instance_id(LoaderKind::Forge, "1.20.1", "47.1.106"), "forge-47.1.106-1.20.1");
        assert_eq!(
            instance_id(LoaderKind::NeoForge, "1.20.1", "47.1.106"),
            "neoforge-47.1.106-1.20.1"
        );
        assert_eq!(
            instance_id(LoaderKind::NeoForge, "1.21.1", "21.1.265"),
            "neoforge-21.1.265-1.21.1"
        );
    }

    /// 真实 `install_profile.json` 的裁剪片段（Forge 1.20.1-47.4.5）。
    const FORGE_PROFILE: &str = r#"{
      "version": "1.20.1-forge-47.4.5",
      "minecraft": "1.20.1",
      "json": "/version.json",
      "data": {
        "MOJMAPS": { "client": "[net.minecraft:client:1.20.1-20230612.114412:mappings@txt]", "server": "x" },
        "BINPATCH": { "client": "/data/client.lzma", "server": "/data/server.lzma" }
      },
      "processors": [
        { "sides": ["server"], "jar": "net.minecraftforge:installertools:1.4.1", "classpath": [], "args": [] },
        { "jar": "net.minecraftforge:jarsplitter:1.1.4", "classpath": [], "args": [] }
      ],
      "libraries": [
        { "name": "net.minecraftforge:forge:1.20.1-47.4.5:universal",
          "downloads": { "artifact": { "path": "net/minecraftforge/forge/1.20.1-47.4.5/forge-1.20.1-47.4.5-universal.jar",
                                       "url": "https://maven.minecraftforge.net/net/minecraftforge/forge/1.20.1-47.4.5/forge-1.20.1-47.4.5-universal.jar",
                                       "size": 6293303 } } },
        { "name": "net.minecraftforge:forge:1.20.1-47.4.5:client",
          "downloads": { "artifact": { "path": "net/minecraftforge/forge/1.20.1-47.4.5/forge-1.20.1-47.4.5-client.jar",
                                       "url": "", "size": 0 } } }
      ]
    }"#;

    /// 真实片段（Forge 1.12.2-14.23.5.2860）：没有处理器，库内嵌在安装器里。
    const LEGACY_PROFILE: &str = r#"{
      "version": "1.12.2-forge-14.23.5.2860",
      "minecraft": "1.12.2",
      "json": "/version.json",
      "path": "net.minecraftforge:forge:1.12.2-14.23.5.2860",
      "data": {}, "processors": [], "libraries": []
    }"#;

    #[test]
    fn modern_profile_is_not_legacy_and_knows_its_mappings_path() {
        let profile = parse_install_profile(FORGE_PROFILE).unwrap();
        assert!(!profile.is_legacy_extract());
        assert!(!profile.is_unsupported());
        assert_eq!(profile.version, "1.20.1-forge-47.4.5");
        assert_eq!(
            profile.mojmaps_relative_path().as_deref(),
            Some("net/minecraft/client/1.20.1-20230612.114412/client-1.20.1-20230612.114412-mappings.txt")
        );
    }

    #[test]
    fn legacy_profile_skips_the_jvm() {
        let profile = parse_install_profile(LEGACY_PROFILE).unwrap();
        assert!(profile.is_legacy_extract());
    }

    #[test]
    fn ancient_profile_without_version_json_is_reported() {
        // 1.7.10 及更早是 install + versionInfo 那一套，这里不当它支持
        let profile = parse_install_profile(
            r#"{"version":"1.7.10-forge1.7.10-10.13.4.1614","processors":[],"libraries":[]}"#,
        )
        .unwrap();
        assert!(profile.is_unsupported());
    }

    #[test]
    fn prefetch_manifest_is_unioned_and_separates_generated_files() {
        let profile = parse_install_profile(FORGE_PROFILE).unwrap();
        let version_json: serde_json::Value = serde_json::from_str(
            r#"{"libraries":[
              {"name":"cpw.mods:bootstraplauncher:1.1.2",
               "downloads":{"artifact":{"path":"cpw/mods/bootstraplauncher/1.1.2/bootstraplauncher-1.1.2.jar",
                                          "url":"https://libraries.minecraft.net/cpw/mods/bootstraplauncher/1.1.2/bootstraplauncher-1.1.2.jar","sha1":"aa"}}},
              {"name":"net.minecraftforge:forge:1.20.1-47.4.5:universal",
               "downloads":{"artifact":{"path":"net/minecraftforge/forge/1.20.1-47.4.5/forge-1.20.1-47.4.5-universal.jar",
                                          "url":"https://maven.minecraftforge.net/dup.jar"}}}
            ]}"#,
        )
        .unwrap();
        let libs: Vec<Library> =
            serde_json::from_value(version_json["libraries"].clone()).unwrap();

        let manifest = prefetch_manifest(&profile.libraries, &libs);
        // universal 两份都声明 → 只留一条（去重按相对路径）
        assert_eq!(manifest.len(), 3, "清单应去重：{manifest:?}");
        assert!(manifest.iter().all(|item| item.relative.ends_with(".jar") || item.relative.ends_with(".txt")));

        // 空 url 的 `:client` 是处理器产物，不能进下载清单
        let generated = generated_artifacts(&manifest);
        assert_eq!(
            generated,
            vec!["net/minecraftforge/forge/1.20.1-47.4.5/forge-1.20.1-47.4.5-client.jar".to_string()]
        );
    }
}
