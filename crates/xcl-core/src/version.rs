//! 版本 JSON 模型。
//!
//! 覆盖三种形态：
//! - 版本清单（`version_manifest_v2.json`）
//! - 版本详情（`versions/<id>/<id>.json`）
//! - 旧版详情（用 `minecraftArguments` 字符串而非 `arguments` 对象）
//!
//! 所有字段尽量宽松（`Option` / `default`）：Mojang 随时会加字段，我们不能因为
//! 多一个不认识的键就整份解析失败。

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::error::AppError;
use crate::rule::{rules_match, Env, Rule};

// ---------- 版本清单 ----------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VersionManifest {
    pub latest: LatestVersions,
    pub versions: Vec<VersionSummary>,
}

impl VersionManifest {
    pub fn parse(json: &str) -> Result<Self, AppError> {
        serde_json::from_str(json).map_err(|e| AppError::parse("版本清单解析失败", e.to_string()))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LatestVersions {
    pub release: String,
    pub snapshot: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VersionSummary {
    pub id: String,
    #[serde(rename = "type")]
    pub version_type: String,
    pub url: String,
    #[serde(default)]
    pub sha1: Option<String>,
    #[serde(default)]
    pub time: Option<String>,
    #[serde(default)]
    pub release_time: Option<String>,
}

// ---------- 版本详情 ----------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VersionDetail {
    pub id: String,
    #[serde(rename = "type", default)]
    pub version_type: Option<String>,
    /// 继承自哪个版本（Forge/Fabric 生成的 JSON 常见）
    #[serde(default)]
    pub inherits_from: Option<String>,
    #[serde(default)]
    pub main_class: Option<String>,
    #[serde(default)]
    pub assets: Option<String>,
    #[serde(default)]
    pub asset_index: Option<AssetIndex>,
    #[serde(default)]
    pub downloads: Option<VersionDownloads>,
    #[serde(default)]
    pub libraries: Vec<Library>,
    /// 1.13+ 的结构化参数
    #[serde(default)]
    pub arguments: Option<Arguments>,
    /// 1.12- 的字符串参数，空格分隔
    #[serde(default)]
    pub minecraft_arguments: Option<String>,
    #[serde(default)]
    pub java_version: Option<JavaVersion>,
}

impl VersionDetail {
    pub fn parse(json: &str) -> Result<Self, AppError> {
        serde_json::from_str(json).map_err(|e| AppError::parse("版本 JSON 解析失败", e.to_string()))
    }

    /// 按当前平台过滤出真正需要下载/进 classpath 的库。
    pub fn applicable_libraries(&self, env: &Env) -> Vec<&Library> {
        self.libraries
            .iter()
            .filter(|lib| rules_match(lib.rules.as_deref(), env))
            .collect()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AssetIndex {
    pub id: String,
    pub url: String,
    #[serde(default)]
    pub sha1: Option<String>,
    #[serde(default)]
    pub size: Option<u64>,
    #[serde(default)]
    pub total_size: Option<u64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VersionDownloads {
    #[serde(default)]
    pub client: Option<Artifact>,
    #[serde(default)]
    pub server: Option<Artifact>,
    /// 官方的混淆映射表（`piston-data` 上的 `client_mappings`）。
    ///
    /// Forge / NeoForge 的 `DOWNLOAD_MOJMAPS` 处理器会自己联网抓这份文件，
    /// 不受镜像控制；我们提前用它填好目标路径，安装器就不会去打 Mojang。
    #[serde(default)]
    pub client_mappings: Option<Artifact>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Arguments {
    #[serde(default)]
    pub game: Vec<Argument>,
    #[serde(default)]
    pub jvm: Vec<Argument>,
}

/// 参数项：要么是纯字符串，要么是带规则的对象。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Argument {
    Plain(String),
    Conditional {
        rules: Vec<Rule>,
        value: ArgumentValue,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ArgumentValue {
    Single(String),
    Many(Vec<String>),
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JavaVersion {
    pub major_version: u32,
    #[serde(default)]
    pub component: Option<String>,
}

// ---------- 库 ----------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Library {
    /// maven 坐标，如 `com.google.code.gson:gson:2.10.1`
    pub name: String,
    #[serde(default)]
    pub downloads: Option<LibraryDownloads>,
    /// 形如 `{"windows": "natives-windows-${arch}"}`
    #[serde(default)]
    pub natives: Option<HashMap<String, String>>,
    #[serde(default)]
    pub rules: Option<Vec<Rule>>,
    #[serde(default)]
    pub extract: Option<Extract>,
    /// 极老的 JSON 只有 base url，没有 downloads
    #[serde(default)]
    pub url: Option<String>,
    /// maven 坐标式条目的校验信息（Fabric / NeoForge 的 profile JSON 把
    /// sha1 / size 直接写在库对象上，而不是塞进 `downloads.artifact`）。
    #[serde(default)]
    pub sha1: Option<String>,
    #[serde(default)]
    pub size: Option<u64>,
}

impl Library {
    pub fn is_applicable(&self, env: &Env) -> bool {
        rules_match(self.rules.as_deref(), env)
    }

    /// 主 artifact（进 classpath 的那个 jar）。
    pub fn artifact(&self) -> Option<&Artifact> {
        self.downloads.as_ref()?.artifact.as_ref()
    }

    /// 实际要下载的那个 jar，带 maven 坐标式兜底。
    ///
    /// 官方 JSON 走 `downloads.artifact`；Fabric / NeoForge 的 profile JSON
    /// 只给 `name` + `url`（maven 根地址），没有 `downloads`。只认前者会把
    /// 整个加载器的库静默丢掉 —— 装了不下、启动了崩。
    ///
    /// 没有 `downloads` 也没有 `url` 时返回 `None`（无法构造地址）。
    pub fn resolved_artifact(&self) -> Option<Artifact> {
        if let Some(artifact) = self.artifact() {
            return Some(artifact.clone());
        }

        let base = self.url.as_deref()?.trim().trim_end_matches('/');
        if base.is_empty() {
            return None;
        }
        let relative = crate::path::maven_to_relative_path(&self.name)?;

        Some(Artifact {
            path: Some(relative.clone()),
            url: format!("{base}/{relative}"),
            sha1: self.sha1.clone(),
            size: self.size,
        })
    }

    /// natives 的 classifier 名，已把 `${arch}` 替换成 `32` / `64`。
    pub fn native_classifier(&self, os_name: &str) -> Option<String> {
        let raw = self.natives.as_ref()?.get(os_name)?;
        Some(raw.replace("${arch}", native_arch()))
    }

    /// 某个平台对应的 natives 包。
    pub fn native_artifact(&self, os_name: &str) -> Option<&Artifact> {
        let classifier = self.native_classifier(os_name)?;
        self.downloads
            .as_ref()?
            .classifiers
            .as_ref()?
            .get(&classifier)
    }

    /// 解压 natives 时要跳过的条目。
    pub fn extract_excludes(&self) -> &[String] {
        self.extract
            .as_ref()
            .map(|e| e.exclude.as_slice())
            .unwrap_or(&[])
    }
}

/// natives 文件名里的架构位宽：`32` / `64`。
fn native_arch() -> &'static str {
    if cfg!(target_pointer_width = "64") {
        "64"
    } else {
        "32"
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LibraryDownloads {
    #[serde(default)]
    pub artifact: Option<Artifact>,
    #[serde(default)]
    pub classifiers: Option<HashMap<String, Artifact>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Artifact {
    /// 相对 `libraries/` 的路径，官方 JSON 会给；没有时用 maven 坐标推导
    #[serde(default)]
    pub path: Option<String>,
    pub url: String,
    #[serde(default)]
    pub sha1: Option<String>,
    #[serde(default)]
    pub size: Option<u64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Extract {
    #[serde(default)]
    pub exclude: Vec<String>,
}

// ---------- 资源索引 ----------

/// `assets/indexes/<id>.json` 的结构。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AssetIndexFile {
    #[serde(default)]
    pub objects: HashMap<String, AssetObject>,
    /// pre-1.6 的索引会把资源摊平到 assets/virtual/<index>/
    #[serde(default, rename = "virtual")]
    pub virtual_: Option<bool>,
    #[serde(default)]
    pub map_to_resources: Option<bool>,
}

impl AssetIndexFile {
    pub fn parse(json: &str) -> Result<Self, AppError> {
        serde_json::from_str(json)
            .map_err(|e| AppError::parse("资源索引解析失败", e.to_string()))
    }

    /// 资源数量，用于清单统计。
    pub fn len(&self) -> usize {
        self.objects.len()
    }

    pub fn is_empty(&self) -> bool {
        self.objects.is_empty()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AssetObject {
    /// sha1，同时也是文件名
    pub hash: String,
    pub size: u64,
}

// ---------- 继承合并 ----------

/// 把「原版 JSON」与「加载器 JSON」合并成一份自包含的版本 JSON。
///
/// 为什么不是直接序列化 [`merge_versions`]：那条路走类型，会把类型里没建模的键
/// 整批丢掉（`checksums`、`logging`、`complianceLevel` …）。装加载器时我们要落盘的
/// 就是这份合并结果，它此后是启动链路的唯一依据，原文里的键一个都不能少。
///
/// 规则：
/// - 以父版本为底，父版本独有的键原样保留
/// - `id` 强制写成目标版本 id，`inheritsFrom` 删掉（自包含）
/// - `libraries`：子在前、父在后（与 [`merge_versions`] 一致）
/// - `arguments.game` / `arguments.jvm`：子在前、父在后拼接
/// - 其余标量（`mainClass` / `assets` / `assetIndex` / `downloads` /
///   `javaVersion` / `minecraftArguments` / `type`）：子有则用子
pub fn merge_version_json(
    child_text: &str,
    parent_text: &str,
    id: &str,
) -> Result<String, AppError> {
    let child: serde_json::Value = serde_json::from_str(child_text)
        .map_err(|e| AppError::parse("加载器 JSON 解析失败", e.to_string()))?;
    let parent: serde_json::Value = serde_json::from_str(parent_text)
        .map_err(|e| AppError::parse("原版 JSON 解析失败", e.to_string()))?;

    let mut out = parent.clone();
    let obj = out
        .as_object_mut()
        .ok_or_else(|| AppError::parse("原版 JSON 不是对象", "顶层必须是 JSON 对象"))?;

    obj.insert("id".into(), serde_json::json!(id));
    obj.remove("inheritsFrom");

    let array_of = |value: &serde_json::Value, key: &str| -> Vec<serde_json::Value> {
        value
            .get(key)
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default()
    };

    // libraries：子在前；子版本声明过的 组:构件 以子为准（见 `library_group_artifact`）
    let child_libraries = array_of(&child, "libraries");
    let provided: std::collections::HashSet<String> = child_libraries
        .iter()
        .filter_map(|lib| lib.get("name").and_then(|v| v.as_str()))
        .map(library_dedupe_key)
        .collect();
    let mut libraries = child_libraries;
    libraries.extend(
        array_of(&parent, "libraries")
            .into_iter()
            .filter(|lib| {
                lib.get("name")
                    .and_then(|v| v.as_str())
                    .is_none_or(|name| !provided.contains(&library_dedupe_key(name)))
            }),
    );
    obj.insert("libraries".into(), serde_json::Value::Array(libraries));

    // arguments：只合并两边都有的子表，避免凭空造出空数组
    let child_args = child.get("arguments");
    let parent_args = parent.get("arguments");
    if child_args.is_some() || parent_args.is_some() {
        let mut merged = serde_json::Map::new();
        for key in ["game", "jvm"] {
            let mut list: Vec<serde_json::Value> = Vec::new();
            if let Some(value) = child_args.and_then(|a| a.get(key)).and_then(|v| v.as_array()) {
                list.extend(value.iter().cloned());
            }
            if let Some(value) = parent_args.and_then(|a| a.get(key)).and_then(|v| v.as_array()) {
                list.extend(value.iter().cloned());
            }
            if !list.is_empty() {
                merged.insert(key.to_string(), serde_json::Value::Array(list));
            }
        }
        obj.insert("arguments".into(), serde_json::Value::Object(merged));
    }

    // 标量覆盖：子有则用子
    for key in [
        "mainClass",
        "assets",
        "assetIndex",
        "downloads",
        "javaVersion",
        "minecraftArguments",
        "type",
    ] {
        if let Some(value) = child.get(key) {
            if !value.is_null() {
                obj.insert(key.to_string(), value.clone());
            }
        }
    }

    serde_json::to_string_pretty(&out)
        .map_err(|e| AppError::parse("合并后的版本 JSON 无法序列化", e.to_string()))
}

/// 把子版本与父版本合并（子版本优先）。
///
/// 这是 Forge / Fabric 生成的 JSON 的基础：它们只写差异，其余靠 `inheritsFrom`。
///
/// 合并规则：
/// - 标量字段：子有则用子，否则用父
/// - `libraries`：子在前、父在后（顺序影响 classpath 查找优先级）
/// - `arguments`：两边拼接，子在前
///
/// 返回值的 `inherits_from` 会被清空，调用方需要自己记录下一个父版本。
/// 库的判重键：`组:构件[:classifier]` —— 去掉版本，保留 classifier。
///
/// 合并 / 拼 classpath 时按它判重：原版 1.21.10 自带 `asm:9.6`、Fabric 自带
/// `asm:9.10.1`，两份都进 classpath 会让 Fabric 的自检直接拒绝启动
/// （`duplicate ASM classes found on classpath`）。
///
/// classifier 必须参与：`org.lwjgl:lwjgl-glfw:3.3.3` 和
/// `org.lwjgl:lwjgl-glfw:3.3.3:natives-windows` 是同组同构件的两个不同文件，
/// 后者是 LWJGL 运行时解压 dll 的来源；只看组:构件会把它当重复删掉，
/// 启动就报 `Failed to locate library: lwjgl.dll`。
pub fn library_dedupe_key(name: &str) -> String {
    let parts: Vec<&str> = name.split(':').collect();
    match parts.len() {
        0 | 1 => name.to_string(),
        2 | 3 => format!("{}:{}", parts[0], parts[1]),
        _ => format!("{}:{}:{}", parts[0], parts[1], parts[3..].join(":")),
    }
}

pub fn merge_versions(child: VersionDetail, parent: VersionDetail) -> VersionDetail {
    let libraries = {
        let mut merged = child.libraries;
        // 子版本（加载器）声明过的 组:构件 一律以子为准，原版里同名的丢掉
        let provided: std::collections::HashSet<String> = merged
            .iter()
            .map(|lib| library_dedupe_key(&lib.name))
            .collect();
        merged.extend(
            parent
                .libraries
                .into_iter()
                .filter(|lib| !provided.contains(&library_dedupe_key(&lib.name))),
        );
        merged
    };

    let arguments = match (child.arguments, parent.arguments) {
        (Some(c), Some(p)) => Some(Arguments {
            game: c.game.into_iter().chain(p.game).collect(),
            jvm: c.jvm.into_iter().chain(p.jvm).collect(),
        }),
        (child_args, parent_args) => child_args.or(parent_args),
    };

    VersionDetail {
        id: child.id,
        version_type: child.version_type.or(parent.version_type),
        inherits_from: None,
        main_class: child.main_class.or(parent.main_class),
        assets: child.assets.or(parent.assets),
        asset_index: child.asset_index.or(parent.asset_index),
        downloads: child.downloads.or(parent.downloads),
        libraries,
        arguments,
        minecraft_arguments: child.minecraft_arguments.or(parent.minecraft_arguments),
        java_version: child.java_version.or(parent.java_version),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MANIFEST: &str = r#"{
        "latest": { "release": "1.21.1", "snapshot": "24w33a" },
        "versions": [
            { "id": "1.21.1", "type": "release", "url": "https://x/1.21.1.json",
              "time": "2024-08-08T12:00:00+00:00", "releaseTime": "2024-08-08T12:00:00+00:00",
              "sha1": "abc", "complianceLevel": 1 }
        ]
    }"#;

    const DETAIL: &str = r#"{
        "id": "1.20.1",
        "type": "release",
        "mainClass": "net.minecraft.client.main.Main",
        "assets": "5",
        "assetIndex": { "id": "5", "sha1": "s", "size": 10, "totalSize": 20, "url": "https://x/assets.json" },
        "downloads": { "client": { "sha1": "c", "size": 100, "url": "https://x/client.jar" } },
        "javaVersion": { "component": "java-runtime-gamma", "majorVersion": 17 },
        "libraries": [
            { "name": "com.google.code.gson:gson:2.10.1",
              "downloads": { "artifact": { "path": "com/google/code/gson/gson/2.10.1/gson-2.10.1.jar",
                                            "sha1": "g", "size": 1, "url": "https://x/gson.jar" } } },
            { "name": "org.lwjgl:lwjgl:3.3.1",
              "downloads": {
                "artifact": { "path": "org/lwjgl/lwjgl/3.3.1/lwjgl-3.3.1.jar", "url": "https://x/l.jar" },
                "classifiers": { "natives-windows-64": { "path": "org/lwjgl/lwjgl/3.3.1/lwjgl-3.3.1-natives-windows-64.jar", "url": "https://x/ln.jar" } }
              },
              "natives": { "windows": "natives-windows-${arch}" },
              "extract": { "exclude": ["META-INF/"] } },
            { "name": "only.on.mac:foo:1.0", "rules": [ { "action": "allow", "os": { "name": "osx" } } ] }
        ],
        "arguments": {
            "game": [ "--username", "${auth_player_name}", { "rules": [ { "action": "allow", "features": { "has_custom_resolution": true } } ], "value": [ "--width", "800" ] } ],
            "jvm": [ "-Djava.library.path=${natives_directory}" ]
        }
    }"#;

    fn env(os: &str) -> Env {
        Env {
            os_name: os.to_string(),
            os_version: String::new(),
            os_arch: "x86_64".to_string(),
            features: HashMap::new(),
        }
    }

    #[test]
    fn manifest_parses() {
        let m = VersionManifest::parse(MANIFEST).unwrap();
        assert_eq!(m.latest.release, "1.21.1");
        assert_eq!(m.versions[0].version_type, "release");
        assert_eq!(m.versions[0].release_time.as_deref(), Some("2024-08-08T12:00:00+00:00"));
    }

    #[test]
    fn detail_parses() {
        let d = VersionDetail::parse(DETAIL).unwrap();
        assert_eq!(d.id, "1.20.1");
        assert_eq!(d.main_class.as_deref(), Some("net.minecraft.client.main.Main"));
        assert_eq!(d.java_version.unwrap().major_version, 17);
        assert_eq!(d.asset_index.unwrap().id, "5");
        assert_eq!(d.libraries.len(), 3);
    }

    #[test]
    fn rules_filter_libraries_by_platform() {
        let d = VersionDetail::parse(DETAIL).unwrap();
        let windows = d.applicable_libraries(&env("windows"));
        let macos = d.applicable_libraries(&env("osx"));
        assert_eq!(windows.len(), 2, "windows 上应排除只在 osx 生效的库");
        assert_eq!(macos.len(), 3);
    }

    #[test]
    fn natives_classifier_replaces_arch() {
        let d = VersionDetail::parse(DETAIL).unwrap();
        let lib = d
            .libraries
            .iter()
            .find(|l| l.natives.is_some())
            .expect("应有一个带 natives 的库");

        let expect = if cfg!(target_pointer_width = "64") {
            "natives-windows-64"
        } else {
            "natives-windows-32"
        };
        assert_eq!(lib.native_classifier("windows").as_deref(), Some(expect));
        assert!(lib.native_artifact("windows").is_some());
        assert!(lib.native_artifact("linux").is_none());
        assert_eq!(lib.extract_excludes(), ["META-INF/"]);
    }

    #[test]
    fn arguments_parse_both_forms() {
        let d = VersionDetail::parse(DETAIL).unwrap();
        let args = d.arguments.unwrap();
        assert_eq!(args.jvm.len(), 1);
        assert_eq!(args.game.len(), 3);
        assert!(matches!(args.game[0], Argument::Plain(_)));
        assert!(matches!(args.game[2], Argument::Conditional { .. }));
    }

    #[test]
    fn unknown_fields_do_not_break_parsing() {
        let json = r#"{ "id": "x", "brandNewField": 123 }"#;
        assert!(VersionDetail::parse(json).is_ok());
    }

    #[test]
    fn invalid_json_returns_parse_error() {
        let err = VersionDetail::parse("{ bad json").unwrap_err();
        assert!(err.hint.is_none());
    }

    #[test]
    fn merge_child_overrides_scalars_and_prepends_libraries() {
        let parent = VersionDetail::parse(DETAIL).unwrap();
        let child = VersionDetail::parse(
            r#"{
                "id": "1.20.1-forge-47.2.0",
                "inheritsFrom": "1.20.1",
                "mainClass": "cpw.mods.bootstraplauncher.BootstrapLauncher",
                "libraries": [
                    { "name": "net.minecraftforge:forge:1.20.1-47.2.0", "downloads": { "artifact": { "url": "https://x/forge.jar" } } }
                ],
                "arguments": { "game": ["--launchTarget", "forgeclient"] }
            }"#,
        )
        .unwrap();

        let merged = merge_versions(child, parent);

        // 子版本覆盖 mainClass
        assert_eq!(
            merged.main_class.as_deref(),
            Some("cpw.mods.bootstraplauncher.BootstrapLauncher")
        );
        // 子版本的库排在最前，父版本库追加在后面
        assert_eq!(merged.libraries.len(), 4);
        assert!(merged.libraries[0].name.contains("forge"));
        // arguments 拼接：子在前
        let args = merged.arguments.unwrap();
        assert_eq!(args.game[0], Argument::Plain("--launchTarget".into()));
        assert_eq!(args.game.len(), 5); // 2 子 + 3 父
        // 合并不再保留 inheritsFrom，避免调用方重复解析
        assert!(merged.inherits_from.is_none());
        // 父版本的 assetIndex 被继承下来
        assert_eq!(merged.asset_index.unwrap().id, "5");
    }

    /// 真实 Fabric profile 的库条目：只有 `name` + `url`，没有 `downloads`。
    ///
    /// 这条不测好，加载器的 7 个库会被静默丢掉：装了不下、classpath 里也没有，
    /// 启动直接崩。所以它和普通官方库必须区别对待。
    #[test]
    fn fabric_maven_style_library_resolves_artifact() {
        let detail = VersionDetail::parse(
            r#"{
                "id": "fabric-loader-0.16.5-1.20.1",
                "inheritsFrom": "1.20.1",
                "mainClass": "net.fabricmc.loader.impl.launch.knot.KnotClient",
                "arguments": { "game": [], "jvm": ["-DFabricMcEmu= net.minecraft.client.main.Main "] },
                "libraries": [
                    { "name": "org.ow2.asm:asm:9.6", "url": "https://maven.fabricmc.net/",
                      "sha1": "aa205cf0a06dbd8e04ece91c0b37c3f5d567546a", "size": 123598 },
                    { "name": "net.fabricmc:intermediary:1.20.1", "url": "https://maven.fabricmc.net/" }
                ]
            }"#,
        )
        .unwrap();

        let asm = &detail.libraries[0];
        assert!(asm.artifact().is_none(), "Fabric 的库没有 downloads");

        let artifact = asm
            .resolved_artifact()
            .expect("maven 坐标式库必须能解析出地址");
        assert_eq!(
            artifact.url,
            "https://maven.fabricmc.net/org/ow2/asm/asm/9.6/asm-9.6.jar"
        );
        assert_eq!(
            artifact.path.as_deref(),
            Some("org/ow2/asm/asm/9.6/asm-9.6.jar")
        );
        assert_eq!(
            artifact.sha1.as_deref(),
            Some("aa205cf0a06dbd8e04ece91c0b37c3f5d567546a")
        );
        assert_eq!(artifact.size, Some(123598));

        // intermediary / fabric-loader 本身连 sha1 都没有，但地址照样要拼得出来
        let artifact = detail.libraries[1].resolved_artifact().unwrap();
        assert_eq!(
            artifact.url,
            "https://maven.fabricmc.net/net/fabricmc/intermediary/1.20.1/intermediary-1.20.1.jar"
        );
        assert!(artifact.sha1.is_none());
        assert!(artifact.size.is_none());
    }

    #[test]
    fn official_library_prefers_downloads_artifact() {
        let detail = VersionDetail::parse(DETAIL).unwrap();
        let gson = &detail.libraries[0];
        // downloads 里有地址时不该走 maven 推导（官方会给 CDN 地址）
        assert_eq!(
            gson.resolved_artifact().unwrap().url,
            "https://x/gson.jar"
        );
    }

    #[test]
    fn library_without_downloads_or_url_stays_unresolved() {
        let detail =
            VersionDetail::parse(r#"{ "id": "x", "libraries": [{ "name": "a:b:1" }] }"#).unwrap();
        assert!(detail.libraries[0].resolved_artifact().is_none());
    }

    #[test]
    fn asset_index_parses_and_counts() {
        let json = r#"{
            "objects": {
                "icons/icon_16x16.png": { "hash": "a1b2c3", "size": 123 },
                "sounds/step/stone1.ogg": { "hash": "d4e5f6", "size": 456 }
            },
            "virtual": false,
            "map_to_resources": false
        }"#;
        let index = AssetIndexFile::parse(json).unwrap();
        assert_eq!(index.len(), 2);
        assert_eq!(index.objects["icons/icon_16x16.png"].size, 123);
        assert_eq!(index.virtual_, Some(false));
    }

    /// 用真实抓取的官方清单（截取前 8 条）做回归，防止 Mojang 改字段后
    /// 我们的宽松解析悄悄失效。
    #[test]
    fn parses_real_official_manifest() {
        let json = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/version_manifest.json"
        ));
        let manifest = VersionManifest::parse(json).expect("真实清单必须能解析");

        assert!(!manifest.versions.is_empty());
        assert!(!manifest.latest.release.is_empty());
        assert!(!manifest.latest.snapshot.is_empty());

        for version in &manifest.versions {
            assert!(!version.id.is_empty());
            assert!(!version.url.is_empty());
            assert!(
                version.url.starts_with("https://"),
                "官方地址应为 https：{}",
                version.url
            );
            assert!(version.release_time.is_some(), "{} 缺少 releaseTime", version.id);
            assert!(version.sha1.is_some(), "{} 缺少 sha1", version.id);
        }
    }
}

#[cfg(test)]
mod merge_json_tests {
    use super::*;

    const PARENT: &str = r#"{
        "id": "1.20.1",
        "type": "release",
        "logging": { "client": { "argument": "-Dlog4j.configurationFile=${path}",
                                  "file": { "id": "client-1.12.xml", "sha1": "x", "size": 1, "url": "https://x/log.xml" } } },
        "mainClass": "net.minecraft.client.main.Main",
        "assets": "5",
        "assetIndex": { "id": "5", "url": "https://x/assets.json", "sha1": "a", "size": 1 },
        "downloads": { "client": { "url": "https://x/client.jar", "sha1": "c", "size": 100 } },
        "javaVersion": { "majorVersion": 17, "component": "java-runtime-gamma" },
        "libraries": [
            { "name": "a:b:1",
              "downloads": { "artifact": { "path": "a/b/1/b-1.jar", "url": "https://x/b.jar", "sha1": "s", "size": 2 } } }
        ],
        "arguments": { "game": ["--username"], "jvm": ["-cp", "${classpath}"] }
    }"#;

    const CHILD: &str = r#"{
        "id": "fabric-loader-0.16.5-1.20.1",
        "inheritsFrom": "1.20.1",
        "mainClass": "net.fabricmc.loader.impl.launch.knot.KnotClient",
        "arguments": { "game": [], "jvm": ["-DFabricMcEmu= net.minecraft.client.main.Main "] },
        "libraries": [
            { "name": "net.fabricmc:fabric-loader:0.16.5", "url": "https://maven.fabricmc.net/", "sha1": "f" }
        ]
    }"#;

    #[test]
    fn merged_json_is_self_contained_and_keeps_parent_only_keys() {
        let text = merge_version_json(CHILD, PARENT, "fabric-loader-0.16.5-1.20.1").unwrap();
        let value: serde_json::Value = serde_json::from_str(&text).unwrap();

        assert_eq!(value["id"], "fabric-loader-0.16.5-1.20.1");
        assert!(value.get("inheritsFrom").is_none(), "自包含版本不能留 inheritsFrom");
        assert_eq!(
            value["mainClass"],
            "net.fabricmc.loader.impl.launch.knot.KnotClient"
        );

        // 子在前、父在后
        let libraries = value["libraries"].as_array().unwrap();
        assert_eq!(libraries.len(), 2);
        assert_eq!(libraries[0]["name"], "net.fabricmc:fabric-loader:0.16.5");
        assert_eq!(libraries[1]["name"], "a:b:1");

        // arguments 拼接，子在前
        assert_eq!(value["arguments"]["jvm"].as_array().unwrap().len(), 3);
        assert_eq!(value["arguments"]["game"].as_array().unwrap().len(), 1);

        // 父版本独有的键必须原样保留（走类型序列化会把它们丢掉）
        assert_eq!(value["logging"]["client"]["file"]["id"], "client-1.12.xml");
        assert_eq!(value["assetIndex"]["id"], "5");
        assert_eq!(value["downloads"]["client"]["size"], 100);
        assert_eq!(value["javaVersion"]["majorVersion"], 17);
        assert_eq!(value["assets"], "5");
    }

    #[test]
    fn merged_json_round_trips_through_launch_model() {
        let text = merge_version_json(CHILD, PARENT, "fabric-loader-0.16.5-1.20.1").unwrap();
        let detail = VersionDetail::parse(&text).unwrap();

        assert_eq!(detail.id, "fabric-loader-0.16.5-1.20.1");
        assert_eq!(detail.inherits_from, None);
        assert_eq!(detail.libraries.len(), 2);
        assert_eq!(
            detail.main_class.as_deref(),
            Some("net.fabricmc.loader.impl.launch.knot.KnotClient")
        );
        assert_eq!(detail.java_version.unwrap().major_version, 17);
        assert!(detail.downloads.and_then(|d| d.client).is_some());
    }

    #[test]
    fn library_dedupe_key_drops_version_but_keeps_classifier() {
        // 同组同构件、不同版本 → 同一个键（asm 9.6 与 9.10.1 才算重复）
        assert_eq!(library_dedupe_key("org.ow2.asm:asm:9.10.1"), "org.ow2.asm:asm");
        assert_eq!(library_dedupe_key("org.ow2.asm:asm:9.6"), "org.ow2.asm:asm");
        // classifier 不同 → 不同键（natives jar 不能被当成主 jar 的重复）
        assert_eq!(
            library_dedupe_key("org.lwjgl:lwjgl-glfw:3.3.3"),
            "org.lwjgl:lwjgl-glfw"
        );
        assert_eq!(
            library_dedupe_key("org.lwjgl:lwjgl-glfw:3.3.3:natives-windows"),
            "org.lwjgl:lwjgl-glfw:natives-windows"
        );
        assert_eq!(
            library_dedupe_key("net.minecraftforge:mergetool:1.1.5:api"),
            "net.minecraftforge:mergetool:api"
        );
        // 认不出来的原样返回，绝不乱切
        assert_eq!(library_dedupe_key("broken"), "broken");
    }

    /// 原版 1.21.10 自带 asm 9.6、Fabric 自带 9.10.1：合并后只留子版本那份，
    /// 否则 Fabric 启动前的 classpath 自检会直接报 duplicate ASM classes。
    #[test]
    fn merge_drops_parent_library_with_same_group_artifact() {
        let parent = VersionDetail::parse(
            r#"{"id":"1.21.10","libraries":[
                {"name":"org.ow2.asm:asm:9.6","downloads":{"artifact":{"url":"https://x/asm96.jar"}}},
                {"name":"com.google.guava:guava:32.1.2-jre","downloads":{"artifact":{"url":"https://x/guava.jar"}}}
            ]}"#,
        )
        .unwrap();
        let child = VersionDetail::parse(
            r#"{"id":"fabric-loader-0.20.1-1.21.10","inheritsFrom":"1.21.10","libraries":[
                {"name":"org.ow2.asm:asm:9.10.1","url":"https://maven.fabricmc.net/"}
            ]}"#,
        )
        .unwrap();

        let merged = merge_versions(child, parent);
        let names: Vec<&str> = merged.libraries.iter().map(|l| l.name.as_str()).collect();
        assert_eq!(
            names,
            vec!["org.ow2.asm:asm:9.10.1", "com.google.guava:guava:32.1.2-jre"]
        );
    }

    #[test]
    fn merged_json_drops_duplicate_parent_library() {
        // asm 是同一个 组:构件 → 原版那条丢掉；asm-tree 是不同构件 → 留着
        let child = r#"{"id":"x","libraries":[
            {"name":"org.ow2.asm:asm:9.10.1","url":"https://maven.fabricmc.net/"}
        ]}"#;
        let parent = r#"{"id":"1.21.10","libraries":[
            {"name":"org.ow2.asm:asm:9.6","downloads":{"artifact":{"url":"https://x/asm.jar"}}},
            {"name":"org.ow2.asm:asm-tree:9.10.1","downloads":{"artifact":{"url":"https://x/tree.jar"}}}
        ]}"#;

        let merged = merge_version_json(child, parent, "xx").unwrap();
        let json: serde_json::Value = serde_json::from_str(&merged).unwrap();
        let names: Vec<&str> = json["libraries"]
            .as_array()
            .unwrap()
            .iter()
            .map(|lib| lib["name"].as_str().unwrap())
            .collect();
        assert_eq!(
            names,
            vec!["org.ow2.asm:asm:9.10.1", "org.ow2.asm:asm-tree:9.10.1"]
        );
    }

}
