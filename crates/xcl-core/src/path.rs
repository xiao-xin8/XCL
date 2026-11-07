//! 目录布局。
//!
//! 采用「共享 + 隔离」混合方案，并严格区分数据与缓存：
//!
//! ```text
//! <root>/                            数据：跟着程序走，可整体拷走
//!   config.toml
//!   accounts.toml
//!   versions/<id>/<id>.json          版本描述
//!   versions/<id>/<id>.jar           客户端 jar
//!   versions/<id>/natives/           natives 解压目录
//!   versions/<id>/XCL/instance.toml  该版本的启动器设置（隔离等）
//!   versions/<id>/{mods,saves,...}   版本隔离后的游戏数据
//!   libraries/                       maven 库，共享（不共享会磁盘爆炸）
//!   assets/indexes/                  资源索引
//!   assets/objects/                  资源对象（按 sha1 前两位分桶）
//!
//! <cache>/                           缓存：可再生，不进便携目录
//!   http/                            HTTP 缓存
//!   temp/                            下载临时文件（未完成的 .part）
//!   skin/                            皮肤（地址索引 + 位图，见 `xcl-service::skin`）
//! ```
//!
//! 本模块只做纯路径计算，不碰文件系统。目录是否真的存在由上层负责。

use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathLayout {
    root: PathBuf,
    cache_root: PathBuf,
}

impl PathLayout {
    /// `root` 是可移动的数据目录，`cache_root` 是系统缓存目录（不进便携包）。
    pub fn new(root: impl Into<PathBuf>, cache_root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            cache_root: cache_root.into(),
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// 缓存根目录。不属于数据，可随时删除。
    pub fn cache_root(&self) -> &Path {
        &self.cache_root
    }

    pub fn http_cache_dir(&self) -> PathBuf {
        self.cache_root.join("http")
    }

    pub fn temp_dir(&self) -> PathBuf {
        self.cache_root.join("temp")
    }

    // ---- 皮肤缓存 ----

    /// 皮肤缓存根目录（地址索引 + PNG 位图）。可再生，随时可删。
    pub fn skin_cache_dir(&self) -> PathBuf {
        self.cache_root.join("skin")
    }

    /// 皮肤地址索引目录：`{uuid}.json` 记着这个 UUID 的皮肤 / 披风 URL。
    pub fn skin_index_dir(&self) -> PathBuf {
        self.skin_cache_dir().join("index")
    }

    pub fn skin_index_file(&self, uuid: &str) -> PathBuf {
        self.skin_index_dir().join(format!("{uuid}.json"))
    }

    /// 皮肤位图目录：文件名是 URL 的 sha1（由调用方算）。
    pub fn skin_blob_dir(&self) -> PathBuf {
        self.skin_cache_dir().join("blob")
    }

    pub fn config_file(&self) -> PathBuf {
        self.root.join("config.toml")
    }

    /// 账号表。跟 config.toml 分开：它含 accessToken，是「账号数据」
    /// 而不是「偏好设置」。
    pub fn authlib_accounts_file(&self) -> PathBuf {
        self.root.join("accounts.toml")
    }

    /// 资源收藏（Mod / 资源包 / 光影 / 整合包）。
    ///
    /// 单独一个文件，不塞进 `config.toml`：收藏会一直增长，且可能想
    /// 手动备份或分享这一份；混进设置里会跟着「恢复默认设置」一起没掉。
    pub fn favorites_file(&self) -> PathBuf {
        self.root.join("favorites.toml")
    }

    // ---- 版本（共享） ----

    pub fn versions_dir(&self) -> PathBuf {
        self.root.join("versions")
    }

    pub fn version_dir(&self, id: &str) -> PathBuf {
        self.versions_dir().join(id)
    }

    pub fn version_json(&self, id: &str) -> PathBuf {
        self.version_dir(id).join(format!("{id}.json"))
    }

    pub fn version_jar(&self, id: &str) -> PathBuf {
        self.version_dir(id).join(format!("{id}.jar"))
    }

    /// 该版本的 natives 解压目录。
    ///
    /// 放在版本目录下而不是全局共享：不同版本依赖的 natives
    /// 版本不同，混在一起会互相覆盖。
    pub fn version_natives_dir(&self, id: &str) -> PathBuf {
        self.version_dir(id).join("natives")
    }

    // ---- 库与资源（共享） ----

    pub fn libraries_dir(&self) -> PathBuf {
        self.root.join("libraries")
    }

    pub fn assets_dir(&self) -> PathBuf {
        self.root.join("assets")
    }

    pub fn assets_indexes_dir(&self) -> PathBuf {
        self.assets_dir().join("indexes")
    }

    pub fn assets_objects_dir(&self) -> PathBuf {
        self.assets_dir().join("objects")
    }

    /// 资源对象路径：按 sha1 前两位分桶，这是 Mojang 的固定规则。
    pub fn asset_object(&self, hash: &str) -> PathBuf {
        let bucket = hash.get(0..2).unwrap_or("__");
        self.assets_objects_dir().join(bucket).join(hash)
    }

    /// 老版本（pre-1.6）的虚拟资源目录。
    pub fn assets_virtual_legacy(&self) -> PathBuf {
        self.assets_dir().join("virtual").join("legacy")
    }

    // ---- Java 运行时（共享） ----

    /// 下载来的 Java 运行时根目录。
    ///
    /// 放在数据根下而不是系统用户目录：整个 `data/` 拷到别的机器就能直接玩，
    /// 这是「便携分发」的前提。143MB 一次下载，删了要重下，所以不算缓存。
    pub fn java_dir(&self) -> PathBuf {
        self.root.join("java")
    }

    /// 某个 Java 组件的安装目录，如 `java/jre-legacy`。
    pub fn java_component_dir(&self, component: &str) -> PathBuf {
        self.java_dir().join(component)
    }

    /// 组件的 java 可执行文件。
    pub fn java_executable(&self, component: &str) -> PathBuf {
        let bin = if cfg!(windows) { "java.exe" } else { "java" };
        self.java_component_dir(component).join("bin").join(bin)
    }

    // ---- 第三方工具 ----

    /// 第三方工具根目录（联机组件之类）。
    ///
    /// 和 `java/` 同理放在数据根下：这些是「装好就能用」的东西，不是随手可弃的临时文件。
    /// 整个 `data/` 拷到别的机器仍然直接可用。
    pub fn tools_dir(&self) -> PathBuf {
        self.root.join("tools")
    }

    /// 联机组件根目录，版本号在下一层：`tools/easytier/<version>/`。
    ///
    /// 按版本分目录是为了升级时能并存：新版装好了再删旧的；装失败也不会把能用的那份搞坏。
    pub fn connect_tools_dir(&self) -> PathBuf {
        self.tools_dir().join("easytier")
    }

    /// 某个版本的联机组件目录。
    pub fn connect_module_dir(&self, version: &str) -> PathBuf {
        self.connect_tools_dir().join(version)
    }

    // ---- 版本级设置 ----

    /// 该版本自己的启动器数据。
    ///
    /// 游戏数据直接放在版本目录下，
    /// 启动器自己产生的东西收进 `XCL/`，不和游戏文件混在一起。
    pub fn version_settings_dir(&self, id: &str) -> PathBuf {
        self.version_dir(id).join("XCL")
    }

    pub fn version_settings_file(&self, id: &str) -> PathBuf {
        self.version_settings_dir(id).join("instance.toml")
    }

    // ---- maven ----

    /// 把 maven 坐标转成本地库文件路径。坐标非法时返回 `None`。
    pub fn library_file(&self, maven_name: &str) -> Option<PathBuf> {
        maven_to_relative_path(maven_name).map(|rel| self.libraries_dir().join(rel))
    }

    // ---- 账号（authlib-injector） ----

    /// authlib-injector.jar 在数据根下的位置。
    ///
    /// 放在数据根而不是安装目录：整个 `data/` 拷到别的机器就能直接登录，
    /// 这是「便携分发」的一部分。jar 不再随安装包分发：启动时缺失就静默下载，
    /// 已有就不管；第三方账号启动前再保证一次（官方优先、BMCLAPI 兜底，SHA-256 校验）。
    pub fn authlib_jar(&self) -> PathBuf {
        self.root.join("authlib-injector.jar")
    }
}

/// maven 坐标 → 相对路径。
///
/// 形如 `group:artifact:version[:classifier][@ext]`，例如：
/// - `com.google.code.gson:gson:2.10.1`
///   → `com/google/code/gson/gson/2.10.1/gson-2.10.1.jar`
/// - `org.lwjgl:lwjgl:3.3.1:natives-windows`
///   → `org/lwjgl/lwjgl/3.3.1/lwjgl-3.3.1-natives-windows.jar`
/// - `net.minecraftforge:forge:1.20.1-47.2.0:universal@zip`
///   → `.../forge-1.20.1-47.2.0-universal.zip`
pub fn maven_to_relative_path(coord: &str) -> Option<String> {
    let coord = coord.trim();
    if coord.is_empty() {
        return None;
    }

    // 先剥离 @ext
    let (body, ext) = match coord.rsplit_once('@') {
        Some((body, ext)) if !body.is_empty() && !ext.is_empty() => (body, ext),
        Some(_) => return None,
        None => (coord, "jar"),
    };

    let parts: Vec<&str> = body.split(':').collect();
    if parts.len() != 3 && parts.len() != 4 {
        return None;
    }
    if parts.iter().any(|p| p.is_empty()) {
        return None;
    }

    let group = parts[0];
    let artifact = parts[1];
    let version = parts[2];
    let classifier = parts.get(3).copied();

    // 防止坐标里混入路径分隔符导致路径穿越
    if [group, artifact, version, ext].iter().any(|p| {
        p.contains('/') || p.contains('\\') || p.contains("..")
    }) || classifier.is_some_and(|c| c.contains('/') || c.contains('\\') || c.contains(".."))
    {
        return None;
    }

    let group_path = group.replace('.', "/");
    let file_name = match classifier {
        Some(c) => format!("{artifact}-{version}-{c}.{ext}"),
        None => format!("{artifact}-{version}.{ext}"),
    };

    Some(format!("{group_path}/{artifact}/{version}/{file_name}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_coordinate() {
        assert_eq!(
            maven_to_relative_path("com.google.code.gson:gson:2.10.1").unwrap(),
            "com/google/code/gson/gson/2.10.1/gson-2.10.1.jar"
        );
    }

    #[test]
    fn with_classifier() {
        assert_eq!(
            maven_to_relative_path("org.lwjgl:lwjgl:3.3.1:natives-windows").unwrap(),
            "org/lwjgl/lwjgl/3.3.1/lwjgl-3.3.1-natives-windows.jar"
        );
    }

    #[test]
    fn with_extension() {
        assert_eq!(
            maven_to_relative_path("net.minecraftforge:forge:1.20.1-47.2.0:universal@zip")
                .unwrap(),
            "net/minecraftforge/forge/1.20.1-47.2.0/forge-1.20.1-47.2.0-universal.zip"
        );
    }

    #[test]
    fn rejects_malformed() {
        assert!(maven_to_relative_path("").is_none());
        assert!(maven_to_relative_path("only:two").is_none());
        assert!(maven_to_relative_path("a:b:c:d:e").is_none());
        assert!(maven_to_relative_path("a::c").is_none());
        assert!(maven_to_relative_path("a:b:c@").is_none());
    }

    #[test]
    fn rejects_path_traversal() {
        assert!(maven_to_relative_path("../../etc:passwd:1.0").is_none());
        assert!(maven_to_relative_path("a:b:c:../../x").is_none());
    }

    #[test]
    fn asset_object_is_bucketed_by_hash_prefix() {
        let layout = PathLayout::new("C:/mc", "C:/cache");
        let path = layout.asset_object("abcdef1234567890");
        assert!(path.ends_with("assets/objects/ab/abcdef1234567890"));
    }

    #[test]
    fn cache_is_separate_from_data() {
        let layout = PathLayout::new("C:/mc", "C:/cache");
        assert_eq!(layout.temp_dir(), PathBuf::from("C:/cache/temp"));
        assert!(!layout.temp_dir().starts_with("C:/mc"));
    }

    #[test]
    fn skin_cache_lives_under_cache_root() {
        let layout = PathLayout::new("C:/mc", "C:/cache");
        assert_eq!(layout.skin_cache_dir(), PathBuf::from("C:/cache/skin"));
        assert_eq!(
            layout.skin_index_file("abc123"),
            PathBuf::from("C:/cache/skin/index/abc123.json")
        );
        assert_eq!(layout.skin_blob_dir(), PathBuf::from("C:/cache/skin/blob"));
        // 皮肤是缓存，不是数据
        assert!(!layout.skin_cache_dir().starts_with("C:/mc"));
    }

    #[test]
    fn shared_and_isolated_dirs() {
        let layout = PathLayout::new("C:/mc", "C:/cache");
        assert_eq!(layout.libraries_dir(), PathBuf::from("C:/mc/libraries"));
        assert_eq!(
            layout.version_settings_file("整合包A"),
            PathBuf::from("C:/mc/versions/整合包A/XCL/instance.toml")
        );
        assert_eq!(
            layout.version_json("1.20.1"),
            PathBuf::from("C:/mc/versions/1.20.1/1.20.1.json")
        );
    }
}
