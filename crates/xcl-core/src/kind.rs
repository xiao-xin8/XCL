//! 版本分类：只用于选图标，不参与启动逻辑。
//!
//! 下载清单里的条目只知道 Mojang 的 `type`，本地已安装的版本则能从版本 JSON
//! 的 `libraries` / `mainClass` 看出装了什么加载器。这里把两者的判断收在
//! 一处，保证同一个版本在下载页和启动页显示同一个图标。

use serde::{Deserialize, Serialize};

use crate::version::Library;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum VersionKind {
    Vanilla,
    Snapshot,
    OldAlpha,
    OldBeta,
    Forge,
    NeoForge,
    Fabric,
    OptiFine,
    LiteLoader,
    #[default]
    Unknown,
}

impl VersionKind {
    /// 是否装了模组加载器。用来判断「这个版本能不能装 Mod」，
    /// 版本隔离的全局策略会用到。
    pub fn is_modable(self) -> bool {
        matches!(
            self,
            Self::Forge | Self::NeoForge | Self::Fabric | Self::OptiFine | Self::LiteLoader
        )
    }

    /// Mojang 清单里的 `type` → 分类。清单只有原版，所以这里不会出现加载器。
    pub fn from_manifest_type(version_type: &str) -> Self {
        match version_type {
            "release" => Self::Vanilla,
            "snapshot" => Self::Snapshot,
            "old_alpha" => Self::OldAlpha,
            "old_beta" => Self::OldBeta,
            _ => Self::Unknown,
        }
    }
}

/// 判断一个版本该用哪个图标。
///
/// 加载器优先于原版类型：一个 Fabric 快照版本应当显示 Fabric 图标，而不是
/// 快照图标。多个加载器同时出现时按 [`Flags`] 的固定顺序取一个
/// （Forge/NeoForge/Fabric 覆写 OptiFine、LiteLoader）。
pub fn detect_kind(
    version_id: &str,
    version_type: Option<&str>,
    main_class: Option<&str>,
    libraries: &[Library],
) -> VersionKind {
    let mut flags = Flags::default();
    for library in libraries {
        flags.mark(&library.name);
    }
    // id 只在规范命名时参与判断（`fabric-loader-1.2.3-1.20.1` 这种安装器自己生成的名字）。
    // 不做 contains —— 把原版改名成「1.20.1-准备装forge」，那还是原版，
    // 却会被当成 Forge 塞进「下载 Mod」页的候选里，装进去直接崩。
    if let Some(kind) = kind_from_id(version_id) {
        flags.set(kind);
    }
    if let Some(main_class) = main_class {
        flags.mark(main_class);
    }

    // NeoForge 的库名和 id 里都含 forge 字样，必须先判
    if flags.neoforge {
        VersionKind::NeoForge
    } else if flags.fabric {
        VersionKind::Fabric
    } else if flags.forge {
        VersionKind::Forge
    } else if flags.optifine {
        VersionKind::OptiFine
    } else if flags.liteloader {
        VersionKind::LiteLoader
    } else {
        VersionKind::from_manifest_type(version_type.unwrap_or(""))
    }
}

/// 从规范命名的版本 id 认出加载器。
///
/// 只认两种能确定的形状，别的一概不猜（版本名可能被改成任意名字）：
///
/// 1. 安装器生成的固定前缀 + 紧跟数字版本号：`fabric-loader-0.20.1-26.2`、
///    `forge-47.2.0-1.20.1`、`neoforge-21.1.265-1.21.1`；
/// 2. 老式「原版号 + 加载器名 + 版本」：`1.12.2-forge-14.23.5.2860`、
///    `1.12.2-OptiFine_HD_U_G5`、`1.7.10-LiteLoader1.7.10_04`。
///
/// `neoforge-todo`、`1.20.1-准备装forge` 这类自起的名字不在其中 ——
/// 它们仍然是原版，否则会被塞进「下载 Mod」的候选里，装进去直接崩。
pub fn kind_from_id(version_id: &str) -> Option<VersionKind> {
    let lower = version_id.trim().to_ascii_lowercase();
    if let Some(rest) = lower.strip_prefix("neoforge-")
        && starts_with_digit(rest)
    {
        return Some(VersionKind::NeoForge);
    }
    if let Some(rest) = lower.strip_prefix("fabric-loader-")
        && starts_with_digit(rest)
    {
        return Some(VersionKind::Fabric);
    }
    if let Some(rest) = lower.strip_prefix("forge-")
        && starts_with_digit(rest)
    {
        return Some(VersionKind::Forge);
    }
    legacy_kind_from_id(&lower)
}

/// `1.12.2-forge-14.23.5.2860` 这一代命名：加载器名夹在两个版本号中间的。
fn legacy_kind_from_id(lower: &str) -> Option<VersionKind> {
    let parts: Vec<&str> = lower.split('-').collect();
    for index in 1..parts.len() {
        let previous = parts[index - 1];
        let current = parts[index];
        if !looks_like_mc_version(previous) {
            continue;
        }
        match current {
            "forge" if parts.get(index + 1).is_some_and(|next| starts_with_digit(next)) => {
                return Some(VersionKind::Forge);
            }
            "neoforge" if parts.get(index + 1).is_some_and(|next| starts_with_digit(next)) => {
                return Some(VersionKind::NeoForge);
            }
            _ if current.starts_with("optifine") => return Some(VersionKind::OptiFine),
            _ if current.starts_with("liteloader") => return Some(VersionKind::LiteLoader),
            _ => {}
        }
    }
    None
}

fn starts_with_digit(text: &str) -> bool {
    text.chars().next().is_some_and(|c| c.is_ascii_digit())
}

/// `1.12.2` / `1.20` 这种形状（只用于认出老式 id 里的原版段）。
fn looks_like_mc_version(text: &str) -> bool {
    let parts: Vec<&str> = text.split('.').collect();
    (2..=3).contains(&parts.len())
        && parts
            .iter()
            .all(|part| !part.is_empty() && part.len() <= 3 && part.chars().all(|c| c.is_ascii_digit()))
}

/// 从版本 JSON 的 `libraries` 反推「装了哪个加载器、版本号多少」。
///
/// 这是跟目录名无关的权威来源：实例改成任意名字，库坐标还在。
/// 同一个版本里出现多条加载器库时按 NeoForge > Fabric > Forge > OptiFine > LiteLoader
/// 取一个，与 [`detect_kind`] 的优先级保持一致。
///
/// 返回的版本号原样给出（Forge 系常带原版前缀，如 `1.21.11-61.2.1`），
/// 比对前请用调用方的「剥前缀」逻辑归一。
pub fn loader_stamp(libraries: &[Library]) -> Option<(VersionKind, String)> {
    let mut best: Option<(u8, VersionKind, String)> = None;
    for library in libraries {
        let Some((kind, version)) = stamp_of(&library.name) else {
            continue;
        };
        let rank = stamp_rank(kind);
        if best.as_ref().is_none_or(|(current, _, _)| rank > *current) {
            best = Some((rank, kind, version));
        }
    }
    best.map(|(_, kind, version)| (kind, version))
}

/// 单条 maven 坐标 → 加载器类型 + 版本号。
fn stamp_of(coordinates: &str) -> Option<(VersionKind, String)> {
    let mut parts = coordinates.split(':');
    let group = parts.next()?.trim();
    let artifact = parts.next()?.trim();
    let version = parts.next()?.trim();
    if version.is_empty() {
        return None;
    }
    let kind = match (group, artifact) {
        ("net.fabricmc", "fabric-loader") => VersionKind::Fabric,
        // 1.20.1 那一支的 NeoForge 仍叫 `net.neoforged:forge`
        ("net.neoforged", "neoforge") | ("net.neoforged", "forge") => VersionKind::NeoForge,
        ("net.minecraftforge", "forge") | ("net.minecraftforge", "fmlloader") => VersionKind::Forge,
        ("optifine", _) => VersionKind::OptiFine,
        (_, "optifine") | (_, "OptiFine") => VersionKind::OptiFine,
        (group, _) if group.starts_with("com.mumfrey") => VersionKind::LiteLoader,
        _ => return None,
    };
    Some((kind, version.to_string()))
}

fn stamp_rank(kind: VersionKind) -> u8 {
    match kind {
        VersionKind::NeoForge => 4,
        VersionKind::Fabric => 3,
        VersionKind::Forge => 2,
        VersionKind::OptiFine => 1,
        VersionKind::LiteLoader => 0,
        _ => 0,
    }
}

#[derive(Debug, Default)]
struct Flags {
    forge: bool,
    neoforge: bool,
    fabric: bool,
    optifine: bool,
    liteloader: bool,
}

impl Flags {
    fn set(&mut self, kind: VersionKind) {
        match kind {
            VersionKind::NeoForge => self.neoforge = true,
            VersionKind::Fabric => self.fabric = true,
            VersionKind::Forge => self.forge = true,
            VersionKind::OptiFine => self.optifine = true,
            VersionKind::LiteLoader => self.liteloader = true,
            _ => {}
        }
    }

    fn mark(&mut self, text: &str) {
        let text = text.to_ascii_lowercase();
        if text.contains("neoforge") || text.contains("net.neoforged") {
            self.neoforge = true;
        }
        if text.contains("fabric") {
            self.fabric = true;
        }
        if text.contains("forge") {
            self.forge = true;
        }
        if text.contains("optifine") {
            self.optifine = true;
        }
        if text.contains("liteloader") || text.contains("mumfrey") {
            self.liteloader = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lib(name: &str) -> Library {
        Library {
            name: name.to_string(),
            downloads: None,
            natives: None,
            rules: None,
            extract: None,
            url: None,
            sha1: None,
            size: None,
        }
    }

    #[test]
    fn loader_kinds_are_modable() {
        for kind in [
            VersionKind::Forge,
            VersionKind::NeoForge,
            VersionKind::Fabric,
            VersionKind::OptiFine,
            VersionKind::LiteLoader,
        ] {
            assert!(kind.is_modable(), "{kind:?} 应该算可装 Mod");
        }
        for kind in [
            VersionKind::Vanilla,
            VersionKind::Snapshot,
            VersionKind::OldAlpha,
            VersionKind::OldBeta,
            VersionKind::Unknown,
        ] {
            assert!(!kind.is_modable(), "{kind:?} 不该算可装 Mod");
        }
    }

    #[test]
    fn manifest_types_map_to_icons() {
        assert_eq!(
            VersionKind::from_manifest_type("release"),
            VersionKind::Vanilla
        );
        assert_eq!(
            VersionKind::from_manifest_type("snapshot"),
            VersionKind::Snapshot
        );
        assert_eq!(
            VersionKind::from_manifest_type("old_alpha"),
            VersionKind::OldAlpha
        );
        assert_eq!(
            VersionKind::from_manifest_type("old_beta"),
            VersionKind::OldBeta
        );
        assert_eq!(
            VersionKind::from_manifest_type("weird"),
            VersionKind::Unknown
        );
    }

    #[test]
    fn loader_wins_over_manifest_type() {
        let libraries = [lib("net.fabricmc:fabric-loader:0.15.11")];
        assert_eq!(
            detect_kind("23w14a", Some("snapshot"), None, &libraries),
            VersionKind::Fabric
        );
    }

    #[test]
    fn neoforge_is_not_mistaken_for_forge() {
        let libraries = [lib("net.neoforged:neoforge:21.1.0")];
        assert_eq!(
            detect_kind("neoforge-21.1.0", Some("release"), None, &libraries),
            VersionKind::NeoForge
        );
    }

    #[test]
    fn forge_library_and_id_prefix() {
        let libraries = [lib("net.minecraftforge:forge:1.20.1-47.2.0")];
        assert_eq!(
            detect_kind("1.20.1-forge", Some("release"), None, &libraries),
            VersionKind::Forge
        );
        // JSON 里没写库时，id 里的 forge 也能认出来
        assert_eq!(
            detect_kind("1.12.2-forge-14.23.5.2860", None, None, &[]),
            VersionKind::Forge
        );
    }

    #[test]
    fn fabric_main_class_is_a_fallback() {
        assert_eq!(
            detect_kind(
                "custom-name",
                Some("release"),
                Some("net.fabricmc.loader.impl.launch.knot.KnotClient"),
                &[],
            ),
            VersionKind::Fabric
        );
    }

    #[test]
    fn optifine_and_liteloader() {
        assert_eq!(
            detect_kind(
                "1.12.2-OptiFine_HD_U_G5",
                Some("release"),
                None,
                &[],
            ),
            VersionKind::OptiFine
        );
        assert_eq!(
            detect_kind(
                "1.7.10-LiteLoader",
                None,
                None,
                &[lib("com.mumfrey:liteloader:1.7.10_04")],
            ),
            VersionKind::LiteLoader
        );
    }

    #[test]
    fn loader_beats_optifine_when_both_are_installed() {
        let libraries = [
            lib("optifine:OptiFine:1.20.1_HD_U_I6"),
            lib("net.minecraftforge:forge:1.20.1-47.2.0"),
        ];
        assert_eq!(
            detect_kind("1.20.1-forge-optifine", Some("release"), None, &libraries),
            VersionKind::Forge
        );
    }

    #[test]
    fn plain_versions_fall_back_to_manifest_type() {
        assert_eq!(
            detect_kind("1.21.1", Some("release"), None, &[]),
            VersionKind::Vanilla
        );
        assert_eq!(
            detect_kind("24w33a", Some("snapshot"), None, &[]),
            VersionKind::Snapshot
        );
        assert_eq!(
            detect_kind("a1.2.6", Some("old_alpha"), None, &[]),
            VersionKind::OldAlpha
        );
    }

    /// 改名不能改身份：把原版改叫「1.20.1-准备装forge」，它还是原版。
    /// （以前 id 走的是 `contains`，这种名字会被判成 Forge，进而出现在「下载 Mod」的候选里。）
    #[test]
    fn renamed_vanilla_is_still_vanilla() {
        for id in [
            "1.20.1-准备装forge",
            "fabric 版？还没装",
            "neoforge-todo",
            "我的整合包",
        ] {
            assert_eq!(
                detect_kind(id, Some("release"), None, &[]),
                VersionKind::Vanilla,
                "「{id}」是原版，不该被名字带偏"
            );
        }
    }

    /// 规范命名的 id 仍然要能兜底（老版本 JSON 没有库信息时全靠它）。
    #[test]
    fn canonical_ids_still_fall_back_to_loader() {
        assert_eq!(
            detect_kind("fabric-loader-0.16.5-1.20.1", None, None, &[]),
            VersionKind::Fabric
        );
        assert_eq!(
            detect_kind("forge-47.2.0-1.20.1", None, None, &[]),
            VersionKind::Forge
        );
        assert_eq!(
            detect_kind("neoforge-21.1.265-1.21.1", None, None, &[]),
            VersionKind::NeoForge
        );
    }

    /// 加载器身份从库坐标反推 —— 这是「改目录名也不丢」的那份权威信息。
    #[test]
    fn loader_stamp_comes_from_libraries() {
        assert_eq!(
            loader_stamp(&[lib("net.fabricmc:fabric-loader:0.20.1")]),
            Some((VersionKind::Fabric, "0.20.1".to_string()))
        );
        // 现代 Forge 的版本号带原版前缀，原样返回，由调用方归一
        assert_eq!(
            loader_stamp(&[lib("net.minecraftforge:fmlloader:1.21.11-61.2.1")]),
            Some((VersionKind::Forge, "1.21.11-61.2.1".to_string()))
        );
        // 旧版 Forge 用的是 `net.minecraftforge:forge`
        assert_eq!(
            loader_stamp(&[lib("net.minecraftforge:forge:1.12.2-14.23.5.2860")]),
            Some((VersionKind::Forge, "1.12.2-14.23.5.2860".to_string()))
        );
        assert_eq!(
            loader_stamp(&[lib("net.neoforged:neoforge:21.10.64")]),
            Some((VersionKind::NeoForge, "21.10.64".to_string()))
        );
        // 1.20.1 那一支的 NeoForge 借用了 forge 这个 artifact
        assert_eq!(
            loader_stamp(&[lib("net.neoforged:forge:1.20.1-47.1.106")]),
            Some((VersionKind::NeoForge, "1.20.1-47.1.106".to_string()))
        );
        assert_eq!(loader_stamp(&[lib("com.google.code.gson:gson:2.10.1")]), None);
    }

    /// 一个版本里混着两条加载器库时，取优先级高的那条（和 `detect_kind` 一致）。
    #[test]
    fn loader_stamp_prefers_neoforge() {
        let libraries = [
            lib("net.minecraftforge:forge:1.20.1-47.2.0"),
            lib("net.neoforged:neoforge:20.4.237"),
        ];
        assert_eq!(
            loader_stamp(&libraries),
            Some((VersionKind::NeoForge, "20.4.237".to_string()))
        );
    }
}
