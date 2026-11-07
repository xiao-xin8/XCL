//! Mojang 官方 Java 运行时清单。
//!
//! 两个 JSON：
//! - 索引 `all.json`：按平台列出可用组件，每个组件再指向自己的 manifest
//! - 组件 manifest：一份文件清单，key 就是相对路径，值里带 `downloads.raw`
//!
//! 关键点：这不是压缩包。`files` 的每个 key 都能直接当落地路径用，
//! 所以整个「安装 Java」就是一次普通的并发下载 —— 不用解压、不用处理 zip。
//!
//! 实测（2026-10-24，windows-x64）：
//! ```text
//! jre-legacy       8u51        192 个文件 / 143 MB
//! java-runtime-alpha   16.0.1
//! java-runtime-gamma   17.0.15
//! java-runtime-delta   21.0.7
//! java-runtime-epsilon 25.0.1
//! ```
//!
//! 为什么用 Mojang 的而不是 Adoptium：这套运行时是官方针对具体游戏版本验证过的，
//! 版本组合确定；而且有 BMCLAPI 镜像，国内下得动。Adoptium 版本全但慢得多。

use std::collections::HashMap;

use serde::Deserialize;

use crate::error::AppError;
use crate::launch::JavaRequirement;

// ---------- 索引（all.json） ----------

/// 顶层是「平台 → 组件名 → 该组件的若干版本」，用 flatten 兜住动态平台键。
#[derive(Debug, Clone, Deserialize, serde::Serialize)]
pub struct RuntimeIndex {
    #[serde(flatten)]
    pub platforms: HashMap<String, HashMap<String, Vec<RuntimeEntry>>>,
}

#[derive(Debug, Clone, Deserialize, serde::Serialize)]
pub struct RuntimeEntry {
    pub manifest: ManifestRef,
    pub version: RuntimeVersion,
}

#[derive(Debug, Clone, Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ManifestRef {
    pub url: String,
    #[serde(default)]
    pub sha1: Option<String>,
    #[serde(default)]
    pub size: Option<u64>,
}

#[derive(Debug, Clone, Deserialize, serde::Serialize)]
pub struct RuntimeVersion {
    pub name: String,
    #[serde(default)]
    pub released: Option<String>,
}

impl RuntimeIndex {
    pub fn parse(json: &str) -> Result<Self, AppError> {
        serde_json::from_str(json)
            .map_err(|e| AppError::parse("Java 运行时清单解析失败", e.to_string()))
    }

    /// 当前平台下所有可用组件，按组件名排序（保证界面顺序稳定）。
    pub fn components(&self) -> Vec<(String, RuntimeEntry)> {
        let Some(map) = self.platforms.get(platform_key()) else {
            return Vec::new();
        };
        let mut out: Vec<(String, RuntimeEntry)> = map
            .iter()
            // 同一个组件可能有多个版本条目（如 gamma 和 gamma-snapshot），
            // 只取第一个（Mojang 保证最新的在前）
            .filter_map(|(name, entries)| {
                if !is_usable_component(name) {
                    return None;
                }
                entries.first().map(|entry| (name.clone(), entry.clone()))
            })
            .collect();
        out.sort_by(|a, b| a.0.cmp(&b.0));
        out
    }

    /// 查某个组件的 manifest 地址。
    pub fn find(&self, component: &str) -> Option<&RuntimeEntry> {
        self.platforms
            .get(platform_key())?
            .get(component)?
            .first()
    }
}

/// 当前平台在清单里的键名。
///
/// 注意不是 `windows` —— 官方按位宽/架构分：`windows-x64`、`windows-x86`、
/// `windows-arm64`、`mac-os`、`mac-os-arm64`、`linux`、`linux-i386`。
pub fn platform_key() -> &'static str {
    #[cfg(all(target_os = "windows", target_arch = "x86_64"))]
    {
        return "windows-x64";
    }
    #[cfg(all(target_os = "windows", target_arch = "x86"))]
    {
        return "windows-x86";
    }
    #[cfg(all(target_os = "windows", target_arch = "aarch64"))]
    {
        return "windows-arm64";
    }
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    {
        return "mac-os-arm64";
    }
    #[cfg(all(target_os = "macos", not(target_arch = "aarch64")))]
    {
        return "mac-os";
    }
    #[cfg(all(target_os = "linux", target_arch = "x86"))]
    {
        return "linux-i386";
    }
    #[cfg(all(
        target_os = "linux",
        not(target_arch = "x86"),
        not(target_os = "macos")
    ))]
    {
        return "linux";
    }
    #[allow(unreachable_code)]
    "linux"
}

// ---------- 组件 manifest ----------

#[derive(Debug, Clone, Deserialize, serde::Serialize)]
pub struct RuntimeManifest {
    pub files: HashMap<String, RuntimeFile>,
}

#[derive(Debug, Clone, Deserialize, serde::Serialize)]
pub struct RuntimeFile {
    /// `file` / `directory` / `link`
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub executable: bool,
    #[serde(default)]
    pub downloads: Option<RuntimeDownloads>,
}

#[derive(Debug, Clone, Default, Deserialize, serde::Serialize)]
pub struct RuntimeDownloads {
    #[serde(default)]
    pub raw: Option<RuntimeDownload>,
}

#[derive(Debug, Clone, Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeDownload {
    pub url: String,
    pub sha1: String,
    #[serde(default)]
    pub size: u64,
}

/// 展开后的一行：可以直接变成下载任务。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeFileEntry {
    /// 相对路径，如 `bin/java.exe`
    pub path: String,
    pub url: String,
    pub sha1: String,
    pub size: u64,
    pub executable: bool,
}

/// 已知无意义的大文件，直接跳过。
///
/// 这几个 hash 在清单里
/// 以不同路径重复出现几十次，不跳过会白下几十 MB。
pub const SKIPPED_HASHES: [&str; 3] = [
    "12976a6c2b227cbac58969c1455444596c894656",
    "c80e4bab46e34d02826eab226a4441d0970f2aba",
    "84d2102ad171863db04e7ee22a259d1f6c5de4a5",
];

impl RuntimeManifest {
    pub fn parse(json: &str) -> Result<Self, AppError> {
        serde_json::from_str(json)
            .map_err(|e| AppError::parse("Java 运行时文件清单解析失败", e.to_string()))
    }

    /// 展开成可下载的文件列表，按路径排序（下载顺序稳定，日志好读）。
    pub fn download_entries(&self) -> Vec<RuntimeFileEntry> {
        let mut out: Vec<RuntimeFileEntry> = self
            .files
            .iter()
            .filter_map(|(path, file)| {
                // 目录没有内容可下，create_dir_all 会在写文件时自动建
                if file.kind != "file" {
                    return None;
                }
                let raw = file.downloads.as_ref()?.raw.as_ref()?;
                if SKIPPED_HASHES.contains(&raw.sha1.as_str()) {
                    return None;
                }
                Some(RuntimeFileEntry {
                    path: path.clone(),
                    url: raw.url.clone(),
                    sha1: raw.sha1.clone(),
                    size: raw.size,
                    executable: file.executable,
                })
            })
            .collect();
        out.sort_by(|a, b| a.path.cmp(&b.path));
        out
    }

    /// 需要下载的总字节数。
    pub fn total_size(&self) -> u64 {
        self.download_entries().iter().map(|entry| entry.size).sum()
    }
}

// ---------- 组件与 Java 版本的对应 ----------

/// 组件名 → 它提供的 Java 主版本。
///
/// 硬编码是安全的：这些名字（`jre-legacy`、`java-runtime-gamma`）是 Mojang
/// 的固定语义，不会变。
pub fn component_java_major(component: &str) -> Option<u32> {
    match component {
        "jre-legacy" => Some(8),
        "java-runtime-alpha" => Some(16),
        "java-runtime-beta" | "java-runtime-gamma" | "java-runtime-gamma-snapshot" => Some(17),
        "java-runtime-delta" => Some(21),
        "java-runtime-epsilon" => Some(25),
        _ => None,
    }
}

/// 给某个 Java 主版本挑组件，按优先级排列（前面没有才试后面）。
pub fn components_for_java(major: u32) -> &'static [&'static str] {
    match major {
        0..=8 => &["jre-legacy"],
        9..=16 => &["java-runtime-alpha"],
        17..=20 => &["java-runtime-gamma", "java-runtime-beta"],
        21..=24 => &["java-runtime-delta"],
        _ => &["java-runtime-epsilon", "java-runtime-delta"],
    }
}

/// 按某个版本的 Java 需求挑一个可下载的组件。
///
/// 关键是上限也要满足：1.12.2 要 Java 8，就不能给它下 Java 17 —— 那等于
/// 换个人再踩一遍同样的坑。
pub fn choose_component(requirement: &JavaRequirement) -> Option<(&'static str, u32)> {
    // 从需求的下限往上找，优先给刚好满足的最低版本
    for major in requirement.min..=requirement.min.saturating_add(16) {
        if !requirement.accepts(major) {
            break;
        }
        for component in components_for_java(major) {
            if component_java_major(component) == Some(major) {
                return Some((component, major));
            }
        }
    }
    None
}

/// 这个组件该不该出现在界面上。
///
/// 清单里混着两类不该给的东西：
/// - `minecraft-java-exe`：启动器自己的 exe 包装器，不是 JRE
/// - `java-runtime-gamma-snapshot`：快照版，版本号跟正式版一样但可能随时变
fn is_usable_component(name: &str) -> bool {
    component_java_major(name).is_some() && !name.ends_with("-snapshot")
}

/// 给界面看的组件标签。
pub fn component_label(component: &str) -> String {
    match component_java_major(component) {
        Some(major) => format!("Java {major}（{component}）"),
        None => component.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const INDEX_JSON: &str = r#"{
      "windows-x64": {
        "jre-legacy": [{"manifest":{"url":"https://x/jre.json","sha1":"aa"},"version":{"name":"8u51-cacert462b08"}}],
        "java-runtime-gamma": [{"manifest":{"url":"https://x/gamma.json"},"version":{"name":"17.0.15"}}],
        "java-runtime-delta": [{"manifest":{"url":"https://x/delta.json"},"version":{"name":"21.0.7"}}],
        "empty-component": []
      },
      "linux": {}
    }"#;

    const MANIFEST_JSON: &str = r#"{
      "files": {
        "bin/java.exe": {"type":"file","executable":true,"downloads":{"raw":{"url":"https://d/java.exe","sha1":"1111","size":100}}},
        "bin/server.exe": {"type":"file","downloads":{"raw":{"url":"https://d/server.exe","sha1":"2222","size":50}}},
        "lib": {"type":"directory"},
        "lib/modules": {"type":"file","downloads":{"raw":{"url":"https://d/modules","sha1":"3333","size":1000}}},
        "release": {"type":"file"},
        "dupe": {"type":"file","downloads":{"raw":{"url":"https://d/dupe","sha1":"12976a6c2b227cbac58969c1455444596c894656","size":99999}}}
      }
    }"#;

    #[test]
    fn parses_index_and_lists_components() {
        let index = RuntimeIndex::parse(INDEX_JSON).unwrap();
        let components = index.components();
        // linux 平台下取不到 windows-x64 的东西，测试跟着编译平台走
        if cfg!(all(target_os = "windows", target_arch = "x86_64")) {
            assert_eq!(components.len(), 3, "空数组的组件不该出现");
            assert_eq!(components[0].0, "java-runtime-delta");
            assert_eq!(components[2].0, "jre-legacy");
        }
    }

    #[test]
    fn finds_component_manifest() {
        let index = RuntimeIndex::parse(INDEX_JSON).unwrap();
        if cfg!(all(target_os = "windows", target_arch = "x86_64")) {
            let entry = index.find("jre-legacy").unwrap();
            assert_eq!(entry.manifest.url, "https://x/jre.json");
            assert_eq!(entry.version.name, "8u51-cacert462b08");
        }
        assert!(index.find("nope").is_none());
    }

    #[test]
    fn rejects_broken_json() {
        assert!(RuntimeIndex::parse("not json").is_err());
        assert!(RuntimeManifest::parse("{").is_err());
    }

    #[test]
    fn expands_only_real_files_and_skips_duplicates() {
        let manifest = RuntimeManifest::parse(MANIFEST_JSON).unwrap();
        let entries = manifest.download_entries();

        let paths: Vec<&str> = entries.iter().map(|e| e.path.as_str()).collect();
        assert_eq!(paths, vec!["bin/java.exe", "bin/server.exe", "lib/modules"]);
        // directory / 没有 raw 的 / hash 在跳过表里的都不出现
        assert!(!paths.contains(&"lib"));
        assert!(!paths.contains(&"release"));
        assert!(!paths.contains(&"dupe"));

        assert!(entries.iter().find(|e| e.path == "bin/java.exe").unwrap().executable);
        assert_eq!(manifest.total_size(), 1150);
    }

    #[test]
    fn maps_components_to_java_majors() {
        assert_eq!(component_java_major("jre-legacy"), Some(8));
        assert_eq!(component_java_major("java-runtime-alpha"), Some(16));
        assert_eq!(component_java_major("java-runtime-gamma"), Some(17));
        assert_eq!(component_java_major("java-runtime-delta"), Some(21));
        assert_eq!(component_java_major("java-runtime-epsilon"), Some(25));
        assert_eq!(component_java_major("unknown"), None);
    }

    #[test]
    fn chooses_component_respecting_upper_bound() {
        // 1.12.2：只要 Java 8，不能给 17
        let req = JavaRequirement {
            min: 8,
            max: Some(8),
        };
        assert_eq!(choose_component(&req), Some(("jre-legacy", 8)));

        // 1.20.1：17 起步，无上限
        let req = JavaRequirement {
            min: 17,
            max: None,
        };
        assert_eq!(choose_component(&req), Some(("java-runtime-gamma", 17)));

        // 1.21：21 起步
        let req = JavaRequirement {
            min: 21,
            max: None,
        };
        assert_eq!(choose_component(&req), Some(("java-runtime-delta", 21)));
    }

    #[test]
    fn label_is_readable() {
        assert_eq!(component_label("jre-legacy"), "Java 8（jre-legacy）");
        assert_eq!(component_label("weird"), "weird");
    }

    #[test]
    fn filters_non_runtime_components() {
        // 启动器包装器不是 JRE
        assert!(!is_usable_component("minecraft-java-exe"));
        // 快照版不该出现在可选列表里
        assert!(!is_usable_component("java-runtime-gamma-snapshot"));
        // 正常的都要留
        assert!(is_usable_component("jre-legacy"));
        assert!(is_usable_component("java-runtime-gamma"));
        assert!(is_usable_component("java-runtime-delta"));
    }

    #[test]
    fn components_list_excludes_junk() {
        let json = r#"{
          "windows-x64": {
            "jre-legacy": [{"manifest":{"url":"https://x/a"},"version":{"name":"8u51"}}],
            "minecraft-java-exe": [{"manifest":{"url":"https://x/b"},"version":{"name":"14"}}],
            "java-runtime-gamma": [{"manifest":{"url":"https://x/c"},"version":{"name":"17.0.15"}}],
            "java-runtime-gamma-snapshot": [{"manifest":{"url":"https://x/d"},"version":{"name":"17.0.15"}}]
          }
        }"#;
        let index = RuntimeIndex::parse(json).unwrap();
        if cfg!(all(target_os = "windows", target_arch = "x86_64")) {
            let names: Vec<String> = index.components().into_iter().map(|(n, _)| n).collect();
            assert_eq!(names, vec!["java-runtime-gamma", "jre-legacy"]);
        }
    }
}
