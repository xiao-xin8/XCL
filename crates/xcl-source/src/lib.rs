//! 下载源策略。
//!
//! 硬约束：纯函数、无 IO、无 async。所有改写规则必须可单测。
//!
//! 规则来源：常见启动器的下载源策略，
//! （行为规格参考，非代码移植）。
//!
//! 核心事实：
//! - Assets / Libraries / Meta 三类走「官方域名 → 镜像域名」的前缀替换
//! - Forge / Fabric / NeoForge 的 maven 不追加官方源 —— 官方地址对这些基本不可用
//! - Mod 源（Modrinth / CurseForge）走独立域名映射表，与镜像 base 无关
//! - 顺序（官方优先 / 镜像优先 / 自动）由配置 + 探测结论共同决定

/// BMCLAPI 镜像根地址。
pub const BMCLAPI_BASE: &str = "https://bmclapi2.bangbang93.com";

/// 「官方源够快」的判定阈值（毫秒）。
///
/// 拉官方版本清单耗时低于此值就认为官方可用，优先官方。
/// 用完整请求耗时而不是 ping 延迟，是因为它同时包含了 DNS、TLS 握手和正文传输。
pub const OFFICIAL_FAST_MS: u64 = 4000;

/// 官方 / 镜像的优先策略。
///
/// 三档设置（下载文件和拉版本列表各自一套，默认都是中间档）。
///
/// 注意三档只影响顺序，不减少候选 —— 即使「尽量官方」，镜像仍排在末尾兜底。
/// 「根本不使用镜像」是另一件事，由 [`Mirror::Official`] 表达。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Strategy {
    /// 总是镜像优先，官方排末尾兜底
    MirrorFirst,
    /// 按探测结论决定，没结论时按镜像优先
    #[default]
    Auto,
    /// 总是官方优先，镜像排末尾兜底
    OfficialFirst,
}

/// 官方域名，替换时逐个匹配前缀。
const OFFICIAL_HOSTS: &[&str] = &[
    "https://piston-data.mojang.com",
    "https://piston-meta.mojang.com",
    "https://launcher.mojang.com",
    "https://launchermeta.mojang.com",
    "https://libraries.minecraft.net",
    "https://resources.download.minecraft.net",
];

/// Mod 平台域名 → 镜像域名。
const MOD_HOST_MAP: &[(&str, &str)] = &[
    ("api.modrinth.com", "mod.mcimirror.top/modrinth"),
    ("staging-api.modrinth.com", "mod.mcimirror.top/modrinth"),
    ("cdn.modrinth.com", "mod.mcimirror.top"),
    ("api.curseforge.com", "mod.mcimirror.top/curseforge"),
    ("edge.forgecdn.net", "mod.mcimirror.top"),
    ("mediafilez.forgecdn.net", "mod.mcimirror.top"),
    ("media.forgecdn.net", "mod.mcimirror.top"),
];

/// 需要走 maven 路径的加载器官网关键词。
const LOADER_KEYWORDS: &[&str] = &["minecraftforge", "fabricmc", "neoforged"];

/// 加载器 maven 的官方域名，以及各自在镜像里不存在的那一段根路径。
///
/// 换域名时必须把这段一起剥掉，否则拼出来就是 404。实测（2026-10-27）：
/// - `bmclapi…/maven/releases/net/neoforged/…` → 404，`bmclapi…/maven/net/neoforged/…` → 200
/// - `files.minecraftforge.net` 的地址自带 `/maven`，镜像里没有这一层
const LOADER_MAVEN_HOSTS: &[(&str, &str)] = &[
    ("https://maven.neoforged.net", "/releases"),
    ("https://files.minecraftforge.net", "/maven"),
    ("https://maven.minecraftforge.net", ""),
    ("https://maven.fabricmc.net", ""),
];

/// 加载器 maven 地址的镜像改写；不是加载器域名就返回 `None`。
///
/// 元数据（`maven-metadata.xml`）和库文件走同一套规则：它们在镜像里都是
/// `{镜像}/maven/<maven 路径>`，区别只在我们把它当哪种 [`UrlKind`]。
fn loader_mirror_urls(original: &str, base: &str) -> Option<Vec<String>> {
    // 老 JSON 里混着 `http://`，按 https 形态匹配（地址本身不改写，只用来认域名）
    let matched = match original.strip_prefix("http://") {
        Some(rest) => format!("https://{rest}"),
        None => original.to_string(),
    };
    for (host, strip) in LOADER_MAVEN_HOSTS {
        let Some(path) = matched.strip_prefix(host) else {
            continue;
        };
        let path = path.strip_prefix(strip).unwrap_or(path);
        return Some(vec![
            format!("{base}/maven{path}"),
            format!("{base}/libraries{path}"),
        ]);
    }
    None
}

/// Fabric 元数据域名。
///
/// 它不在 [`OFFICIAL_HOSTS`] 里：镜像不是「换根域名」而是「换到 `/fabric-meta`
/// 前缀下」（`https://bmclapi2.bangbang93.com/fabric-meta/v2/...`）。
/// 漏了这条，Fabric 版本列表和 profile JSON 会直连官方，国内基本拉不动。
const FABRIC_META_HOST: &str = "https://meta.fabricmc.net";
const FABRIC_META_PREFIX: &str = "/fabric-meta";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mirror {
    Official,
    Bmclapi,
    /// 自定义镜像。约定：需兼容 BMCLAPI 的路径结构（`/assets`、`/maven`、`/libraries`）。
    Custom(String),
}

impl Mirror {
    /// 这个源是否适合单文件多连接（Range 分片）下载。
    ///
    /// 依据：
    /// - BMCLAPI 明确不适合。它被列入「不分片黑名单」，
    ///   并且每开一个下载线程都要 `Thread.Sleep(100)` 给它降频；我们对它 64 并发
    ///   的实测也退化到串行水平。多开连接只会更容易吃到 403/429。
    /// - 官方 CDN（`piston-data` / `piston-meta`）支持 Range，也没有按连接限速，
    ///   可以放心分片。
    /// - 自定义镜像没有证据，先按「可以」处理；真不行的时候 `xcl-download`
    ///   会检测到非 206 响应并整体退回单连接，不会把文件搞坏。
    pub fn allow_split(&self) -> bool {
        match self {
            Mirror::Official => true,
            Mirror::Bmclapi => false,
            Mirror::Custom(_) => true,
        }
    }
}

/// 这些主机不适合单文件多连接。判定看实际要请求的 URL，不是全局下载源设置：
/// 同一批文件里可能有的走官方 CDN、有的走镜像，不能用一把开关全关掉。
///
/// 不分片黑名单，唯一例外是 `github.com` ——
/// 我们的更新包靠「并发 1 + 分片」拿带宽，GitHub release asset 支持 Range，
/// 不能跟着一起禁掉。
const SPLIT_DENY_MARKERS: &[&str] = &[
    "bmclapi",
    "pcl2-server",
    "meloong.com",
    "optifine.net",
    "momot.rs",
];

pub fn url_allows_split(url: &str) -> bool {
    let lower = url.to_ascii_lowercase();
    !SPLIT_DENY_MARKERS
        .iter()
        .any(|marker| lower.contains(marker))
}

impl Mirror {
    /// 镜像根地址；官方源没有镜像根，返回 `None`。
    pub fn base(&self) -> Option<&str> {
        match self {
            Mirror::Official => None,
            Mirror::Bmclapi => Some(BMCLAPI_BASE),
            Mirror::Custom(url) => {
                let trimmed = url.trim().trim_end_matches('/');
                if trimmed.is_empty() {
                    None
                } else {
                    Some(trimmed)
                }
            }
        }
    }
}

/// URL 类型决定镜像的替换方式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UrlKind {
    /// 资源文件：镜像路径 `/assets`
    Asset,
    /// maven 库：镜像路径 `/maven`、`/libraries`
    Library,
    /// 版本描述 / 元数据：镜像根直接替换
    Meta,
    /// Mod 平台资源：走独立域名表
    Mod,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourcePolicy {
    pub mirror: Mirror,
    /// 官方 / 镜像的优先策略
    pub strategy: Strategy,
    /// `Auto` 档的探测结论：官方源是否够快。
    ///
    /// 由上层在拉版本清单时实测后填入（见 `xcl-service::versions`）。
    /// 非 `Auto` 档忽略此字段。
    pub auto_official_fast: bool,
}

impl Default for SourcePolicy {
    fn default() -> Self {
        Self {
            mirror: Mirror::Bmclapi,
            strategy: Strategy::Auto,
            auto_official_fast: false,
        }
    }
}

impl SourcePolicy {
    pub fn new(mirror: Mirror, strategy: Strategy) -> Self {
        Self {
            mirror,
            strategy,
            auto_official_fast: false,
        }
    }

    /// 带上探测结论。仅对 [`Strategy::Auto`] 有意义。
    pub fn with_probe(mut self, official_fast: bool) -> Self {
        self.auto_official_fast = official_fast;
        self
    }

    /// 最终是否把官方源排在最前。
    ///
    /// `Auto` 档没测过（`auto_official_fast == false`）时退回镜像优先 ——
    /// 保守选择：镜像挂了还有官方兜底，而官方挂了在国内可能长时间无响应。
    pub fn official_first(&self) -> bool {
        match self.strategy {
            Strategy::MirrorFirst => false,
            Strategy::OfficialFirst => true,
            Strategy::Auto => self.auto_official_fast,
        }
    }

    /// 给出某个 URL 的候选下载地址，按尝试顺序排列，已去重。
    ///
    /// 永不为空（除非传入空地址）：至少包含官方原始地址或镜像地址之一。
    pub fn candidates(&self, kind: UrlKind, official: &str) -> Vec<String> {
        let trimmed = official.trim();
        if trimmed.is_empty() {
            return Vec::new();
        }
        // 统一在这里升级协议，保证后续所有分支（含官方候选）拿到的是 https
        let official = upgrade_scheme(trimmed);
        let official = official.as_str();

        // Mod 源与 base 无关，走独立映射
        if kind == UrlKind::Mod {
            return dedupe(self.mod_candidates(official));
        }

        // 纯官方模式
        let Some(base) = self.mirror.base() else {
            return vec![official.to_string()];
        };

        let mirrors = self.mirror_urls(kind, base, official);
        let usable_mirrors: Vec<String> = mirrors.into_iter().filter(|m| m != official).collect();

        // 加载器的官方 maven 基本不可达，不追加官方源
        if kind == UrlKind::Library && is_loader_url(official) {
            if usable_mirrors.is_empty() {
                return vec![official.to_string()];
            }
            return dedupe(usable_mirrors);
        }

        let mut out = Vec::with_capacity(usable_mirrors.len() + 1);
        if self.official_first() || usable_mirrors.is_empty() {
            out.push(official.to_string());
            out.extend(usable_mirrors);
        } else {
            out.extend(usable_mirrors);
            out.push(official.to_string());
        }
        dedupe(out)
    }

    /// 只要官方地址。给「双源竞速」用：竞速需要两个地址各自独立存在。
    ///
    /// 传入空地址返回 `None`。
    pub fn official_only(&self, official: &str) -> Option<String> {
        let trimmed = official.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(upgrade_scheme(trimmed))
        }
    }

    /// 只要首选镜像地址。给「双源竞速」用。
    ///
    /// 返回 `None` 的三种情况：不配镜像、Mod 源（走独立域名表，不参与竞速）、
    /// 该地址改写后与官方相同（说明这个 host 镜像不认识）。
    pub fn mirror_only(&self, kind: UrlKind, official: &str) -> Option<String> {
        let trimmed = official.trim();
        if trimmed.is_empty() || kind == UrlKind::Mod {
            return None;
        }
        let official = upgrade_scheme(trimmed);
        let base = self.mirror.base()?;
        self.mirror_urls(kind, base, &official)
            .into_iter()
            .find(|url| *url != official)
    }

    fn mirror_urls(&self, kind: UrlKind, base: &str, original: &str) -> Vec<String> {
        match kind {
            UrlKind::Asset => vec![rewrite_prefix(original, base, "/assets")],
            UrlKind::Meta => {
                // Fabric 元数据要换成 `/fabric-meta` 前缀，不能只换根域名
                if let Some(rest) = original.strip_prefix(FABRIC_META_HOST) {
                    vec![format!("{base}{FABRIC_META_PREFIX}{rest}")]
                } else if let Some(loader) = loader_mirror_urls(original, base) {
                    // 加载器的 maven 元数据（`maven-metadata.xml`）也在 `/maven` 之下
                    loader
                } else {
                    vec![rewrite_prefix(original, base, "")]
                }
            }
            UrlKind::Library => {
                if let Some(loader) = loader_mirror_urls(original, base) {
                    loader
                } else {
                    vec![
                        rewrite_prefix(original, base, "/maven"),
                        rewrite_prefix(original, base, "/libraries"),
                    ]
                }
            }
            UrlKind::Mod => Vec::new(),
        }
    }

    fn mod_candidates(&self, official: &str) -> Vec<String> {
        let mirrored = rewrite_mod_host(official);

        // CurseForge 的官方 API 必须带 API Key（我们不内置），平时能搜全靠镜像代持。
        // 所以这里不看下载源设置：无论选官方 / 镜像 / 自定义，CF 都只走镜像 ——
        // 官方地址连候选都不放，放进去只会白吃一次 403。
        if official.contains("api.curseforge.com") {
            if let Some(ref url) = mirrored {
                return vec![url.clone()];
            }
        }

        if self.mirror == Mirror::Official {
            return vec![official.to_string()];
        }
        match mirrored {
            Some(mirrored) if !self.official_first() => vec![mirrored, official.to_string()],
            Some(mirrored) => vec![official.to_string(), mirrored],
            None => vec![official.to_string()],
        }
    }
}

/// 把官方域名前缀替换成 `{base}{suffix}`。
fn rewrite_prefix(original: &str, base: &str, suffix: &str) -> String {
    for host in OFFICIAL_HOSTS {
        if let Some(rest) = original.strip_prefix(host) {
            return format!("{base}{suffix}{rest}");
        }
    }
    original.to_string()
}

/// 老版本 JSON 里会混用 `http://`，统一升到 `https://`。
fn upgrade_scheme(url: &str) -> String {
    let Some(rest) = url.strip_prefix("http://") else {
        return url.to_string();
    };
    let upgraded = format!("https://{rest}");
    if OFFICIAL_HOSTS.iter().any(|host| upgraded.starts_with(host))
        || upgraded.contains("minecraft.net")
        || upgraded.contains("mojang.com")
    {
        upgraded
    } else {
        url.to_string()
    }
}

fn is_loader_url(url: &str) -> bool {
    LOADER_KEYWORDS.iter().any(|k| url.contains(k))
}

fn rewrite_mod_host(original: &str) -> Option<String> {
    for (from, to) in MOD_HOST_MAP {
        if let Some(pos) = original.find(from) {
            let mut out = String::with_capacity(original.len() + 8);
            out.push_str(&original[..pos]);
            out.push_str(to);
            out.push_str(&original[pos + from.len()..]);
            return Some(out);
        }
    }
    None
}

fn dedupe(items: Vec<String>) -> Vec<String> {
    let mut out: Vec<String> = Vec::with_capacity(items.len());
    for item in items {
        if !out.contains(&item) {
            out.push(item);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bmclapi() -> SourcePolicy {
        SourcePolicy::default()
    }

    fn official_first() -> SourcePolicy {
        SourcePolicy::new(Mirror::Bmclapi, Strategy::OfficialFirst)
    }

    #[test]
    fn asset_url_rewritten_to_assets_path() {
        let url = "https://resources.download.minecraft.net/ab/abcdef";
        let got = bmclapi().candidates(UrlKind::Asset, url);
        assert_eq!(got[0], "https://bmclapi2.bangbang93.com/assets/ab/abcdef");
        assert_eq!(got.last().unwrap(), url);
    }

    #[test]
    fn http_asset_is_upgraded_to_https() {
        let url = "http://resources.download.minecraft.net/ab/abcdef";
        let got = official_first().candidates(UrlKind::Asset, url);
        assert_eq!(got[0], "https://resources.download.minecraft.net/ab/abcdef");
    }

    #[test]
    fn normal_library_has_two_mirrors_plus_official() {
        let url = "https://libraries.minecraft.net/com/google/gson/2.10.1/gson-2.10.1.jar";
        let got = bmclapi().candidates(UrlKind::Library, url);
        assert_eq!(got.len(), 3);
        assert_eq!(
            got[0],
            "https://bmclapi2.bangbang93.com/maven/com/google/gson/2.10.1/gson-2.10.1.jar"
        );
        assert_eq!(
            got[1],
            "https://bmclapi2.bangbang93.com/libraries/com/google/gson/2.10.1/gson-2.10.1.jar"
        );
        assert_eq!(got[2], url);
    }

    #[test]
    fn loader_library_never_includes_official_source() {
        let cases = [
            (
                "https://maven.minecraftforge.net/net/minecraftforge/forge/1.20.1-47.2.0/forge-1.20.1-47.2.0-universal.jar",
                "https://bmclapi2.bangbang93.com/maven/net/minecraftforge/forge/1.20.1-47.2.0/forge-1.20.1-47.2.0-universal.jar",
            ),
            (
                "https://maven.fabricmc.net/net/fabricmc/fabric-loader/0.15.0/fabric-loader-0.15.0.jar",
                "https://bmclapi2.bangbang93.com/maven/net/fabricmc/fabric-loader/0.15.0/fabric-loader-0.15.0.jar",
            ),
            (
                "https://maven.neoforged.net/releases/net/neoforged/neoforge/20.4.80/neoforge-20.4.80-universal.jar",
                "https://bmclapi2.bangbang93.com/maven/net/neoforged/neoforge/20.4.80/neoforge-20.4.80-universal.jar",
            ),
        ];
        for (url, expect_first) in cases {
            let got = bmclapi().candidates(UrlKind::Library, url);
            assert_eq!(got[0], expect_first);
            assert!(
                !got.iter().any(|u| u == url),
                "加载器库不应包含官方源：{url} -> {got:?}"
            );
        }
    }

    #[test]
    fn loader_maven_root_segments_are_stripped() {
        // 实测 2026-10-27：镜像里没有 `/releases` 和 `/maven` 这一段，
        // 不剥掉拼出来就是 404 —— 装 Forge 时整批库都会失败。
        let neo = "https://maven.neoforged.net/releases/net/neoforged/neoforge/21.1.265/neoforge-21.1.265-universal.jar";
        let got = bmclapi().candidates(UrlKind::Library, neo);
        assert_eq!(
            got[0],
            "https://bmclapi2.bangbang93.com/maven/net/neoforged/neoforge/21.1.265/neoforge-21.1.265-universal.jar"
        );
        assert!(
            !got.iter().any(|u| u.contains("/maven/releases/")),
            "不该留下镜像没有的 /releases：{got:?}"
        );

        let forge = "https://files.minecraftforge.net/maven/net/minecraftforge/forge/1.20.1-47.4.5/forge-1.20.1-47.4.5-installer.jar";
        let got = bmclapi().candidates(UrlKind::Library, forge);
        assert_eq!(
            got[0],
            "https://bmclapi2.bangbang93.com/maven/net/minecraftforge/forge/1.20.1-47.4.5/forge-1.20.1-47.4.5-installer.jar"
        );
        assert!(
            !got.iter().any(|u| u.contains("/maven/maven/")),
            "不该留下重复的 /maven：{got:?}"
        );
    }

    #[test]
    fn loader_metadata_is_mirrored_but_keeps_official_fallback() {
        // 版本列表（maven-metadata.xml）在镜像里同样是 `/maven` 之下；
        // 和库文件不同，这里要保留官方兜底 —— 个别老构建镜像没同步。
        let url = "https://maven.neoforged.net/releases/net/neoforged/neoforge/maven-metadata.xml";
        let got = bmclapi().candidates(UrlKind::Meta, url);
        assert_eq!(
            got[0],
            "https://bmclapi2.bangbang93.com/maven/net/neoforged/neoforge/maven-metadata.xml"
        );
        assert_eq!(got.last().map(String::as_str), Some(url));

        let forge = "https://maven.minecraftforge.net/net/minecraftforge/forge/maven-metadata.xml";
        let got = bmclapi().candidates(UrlKind::Meta, forge);
        assert_eq!(
            got[0],
            "https://bmclapi2.bangbang93.com/maven/net/minecraftforge/forge/maven-metadata.xml"
        );
    }

    #[test]
    fn official_mode_leaves_loader_urls_alone() {
        let url = "https://maven.neoforged.net/releases/net/neoforged/neoforge/21.1.265/neoforge-21.1.265-installer.jar";
        let policy = SourcePolicy::new(Mirror::Official, Strategy::MirrorFirst);
        assert_eq!(policy.candidates(UrlKind::Library, url), vec![url.to_string()]);
    }

    #[test]
    fn meta_url_uses_bare_mirror_root() {
        let url = "https://piston-meta.mojang.com/mc/game/version_manifest_v2.json";
        let got = bmclapi().candidates(UrlKind::Meta, url);
        assert_eq!(
            got[0],
            "https://bmclapi2.bangbang93.com/mc/game/version_manifest_v2.json"
        );
    }

    #[test]
    fn fabric_meta_url_uses_fabric_meta_prefix() {
        // Fabric 的镜像不是换根域名，是换到 /fabric-meta 之下
        let url = "https://meta.fabricmc.net/v2/versions/loader/1.20.1";
        let got = bmclapi().candidates(UrlKind::Meta, url);
        assert_eq!(
            got[0],
            "https://bmclapi2.bangbang93.com/fabric-meta/v2/versions/loader/1.20.1"
        );
        assert_eq!(
            got.last().map(String::as_str),
            Some(url),
            "镜像之外仍要保留官方兜底"
        );
    }

    #[test]
    fn fabric_profile_url_is_mirrored() {
        let url = "https://meta.fabricmc.net/v2/versions/loader/1.20.1/0.16.5/profile/json";
        let got = bmclapi().candidates(UrlKind::Meta, url);
        assert_eq!(
            got[0],
            "https://bmclapi2.bangbang93.com/fabric-meta/v2/versions/loader/1.20.1/0.16.5/profile/json"
        );
    }

    #[test]
    fn official_mode_keeps_fabric_url_untouched() {
        let url = "https://meta.fabricmc.net/v2/versions/loader";
        let policy = SourcePolicy::new(Mirror::Official, Strategy::MirrorFirst);
        assert_eq!(policy.candidates(UrlKind::Meta, url), vec![url.to_string()]);
    }

    #[test]
    fn bmclapi_refuses_split_download() {
        // 不分片黑名单：bmclapi 多连接会 403/429
        assert!(!Mirror::Bmclapi.allow_split());
        assert!(Mirror::Official.allow_split());
        assert!(Mirror::Custom("https://mirror.example.com".into()).allow_split());
    }

    #[test]
    fn split_is_decided_by_actual_url() {
        assert!(!url_allows_split(
            "https://bmclapi2.bangbang93.com/maven/foo/bar.jar"
        ));
        assert!(!url_allows_split(
            "https://optifine.net/downloads/OptiFine_1.20.1_HD_U_I6.jar"
        ));
        assert!(url_allows_split(
            "https://piston-data.mojang.com/v1/objects/abc/client.jar"
        ));
        assert!(url_allows_split(
            "https://cdn.modrinth.com/data/AANobbMI/versions/abc/sodium.jar"
        ));
        assert!(url_allows_split(
            "https://mod.mcimirror.top/curseforge/mods/238222/files/9334752/jei.jar"
        ));
        // 更新包靠分片拿带宽，GitHub release 不跟着禁
        assert!(url_allows_split(
            "https://github.com/x/y/releases/download/v1/xcl.zip"
        ));
    }

    #[test]
    fn official_only_mode_returns_single_candidate() {
        let policy = SourcePolicy::new(Mirror::Official, Strategy::MirrorFirst);
        let url = "https://piston-meta.mojang.com/x.json";
        assert_eq!(policy.candidates(UrlKind::Meta, url), vec![url.to_string()]);
    }

    #[test]
    fn curseforge_api_always_goes_through_mirror() {
        // CF 官方 API 必须带 key（我们不内置），直连必然 403 ——
        // 所以无论下载源选什么，候选里都只能有镜像地址。
        let url = "https://api.curseforge.com/v1/mods/search?gameId=432";
        let expected =
            "https://mod.mcimirror.top/curseforge/v1/mods/search?gameId=432".to_string();

        for mirror in [Mirror::Official, Mirror::Bmclapi] {
            let policy = SourcePolicy::new(mirror.clone(), Strategy::Auto);
            assert_eq!(
                policy.candidates(UrlKind::Mod, url),
                vec![expected.clone()],
                "mirror = {mirror:?} 时 CF 也必须走镜像"
            );
        }
    }

    #[test]
    fn modrinth_still_honours_official_mode() {
        // Modrinth 官方能直连，选「官方源」时不该被强行改写到镜像
        let url = "https://api.modrinth.com/v2/search";
        let policy = SourcePolicy::new(Mirror::Official, Strategy::Auto);
        assert_eq!(policy.candidates(UrlKind::Mod, url), vec![url.to_string()]);
    }

    #[test]
    fn official_first_puts_official_first() {
        let url = "https://piston-meta.mojang.com/x.json";
        let got = official_first().candidates(UrlKind::Meta, url);
        assert_eq!(got[0], url);
        assert_eq!(got.len(), 2);
    }

    #[test]
    fn custom_mirror_trailing_slash_is_trimmed() {
        let policy = SourcePolicy::new(
            Mirror::Custom("https://mirror.example.com/".into()),
            Strategy::MirrorFirst,
        );
        let url = "https://piston-meta.mojang.com/x.json";
        let got = policy.candidates(UrlKind::Meta, url);
        assert_eq!(got[0], "https://mirror.example.com/x.json");
    }

    #[test]
    fn mod_url_mapped_to_mod_mirror() {
        let url = "https://api.modrinth.com/v2/project/sodium";
        let got = bmclapi().candidates(UrlKind::Mod, url);
        assert_eq!(got[0], "https://mod.mcimirror.top/modrinth/v2/project/sodium");
        assert_eq!(got[1], url);
    }

    #[test]
    fn unknown_host_does_not_duplicate() {
        let url = "https://example.com/some/file.jar";
        let got = bmclapi().candidates(UrlKind::Library, url);
        assert_eq!(got, vec![url.to_string()]);
    }

    #[test]
    fn empty_url_yields_no_candidate() {
        assert!(bmclapi().candidates(UrlKind::Asset, "   ").is_empty());
    }

    // ---------- 三档策略 ----------

    #[test]
    fn auto_without_probe_falls_back_to_mirror_first() {
        // Auto 档没测过时必须保守地走镜像优先，否则国内网络会遇到官方长时间无响应
        let policy = bmclapi();
        assert_eq!(policy.strategy, Strategy::Auto);
        assert!(!policy.official_first());
        let url = "https://piston-meta.mojang.com/x.json";
        assert!(policy.candidates(UrlKind::Meta, url)[0].contains("bmclapi"));
    }

    #[test]
    fn auto_uses_probe_result_to_flip_order() {
        let url = "https://piston-meta.mojang.com/x.json";
        let fast = bmclapi().with_probe(true);
        assert!(fast.official_first());
        assert_eq!(fast.candidates(UrlKind::Meta, url)[0], url);

        let slow = bmclapi().with_probe(false);
        assert!(!slow.official_first());
        assert!(slow.candidates(UrlKind::Meta, url)[0].contains("bmclapi"));
    }

    #[test]
    fn explicit_strategy_ignores_probe_result() {
        let url = "https://piston-meta.mojang.com/x.json";
        // 即使探测说官方慢，显式选了官方优先也必须听配置的
        let policy = SourcePolicy::new(Mirror::Bmclapi, Strategy::OfficialFirst).with_probe(false);
        assert_eq!(policy.candidates(UrlKind::Meta, url)[0], url);

        let policy = SourcePolicy::new(Mirror::Bmclapi, Strategy::MirrorFirst).with_probe(true);
        assert!(policy.candidates(UrlKind::Meta, url)[0].contains("bmclapi"));
    }

    // ---------- 竞速取址 ----------

    #[test]
    fn race_getters_return_both_sides_independently() {
        let url = "https://piston-meta.mojang.com/mc/game/version_manifest_v2.json";
        let policy = bmclapi();
        assert_eq!(policy.official_only(url).as_deref(), Some(url));
        assert_eq!(
            policy.mirror_only(UrlKind::Meta, url).as_deref(),
            Some("https://bmclapi2.bangbang93.com/mc/game/version_manifest_v2.json")
        );
    }

    #[test]
    fn race_getters_degrade_to_none_when_unavailable() {
        let url = "https://piston-meta.mojang.com/x.json";
        let no_mirror = SourcePolicy::new(Mirror::Official, Strategy::Auto);
        assert!(no_mirror.mirror_only(UrlKind::Meta, url).is_none());
        assert!(no_mirror.official_only(url).is_some());

        assert!(bmclapi().official_only("   ").is_none());
        // Mod 源走独立域名表，不参与竞速
        assert!(
            bmclapi()
                .mirror_only(UrlKind::Mod, "https://api.modrinth.com/v2/project/sodium")
                .is_none()
        );
    }
}
