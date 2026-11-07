//! 文件 ↔ 本地版本配对。纯逻辑，不碰网络也不碰文件系统。
//!
//! 详情页左侧列表的主体是「本地版本被这个 Mod 割过一刀」之后剩下的那些，
//! 这一层只回答一个问题：这个本地版本能不能用这个文件？
//!
//! 规则：
//! - MC 版本精确字符串相等，不做区间 / 大小比较（官方数据本来就是精确小版本）；
//! - 加载器名必须出现在文件的 `loaders` 里 —— Fabric 对 Fabric、Forge 对 Forge；
//! - 不猜兼容：NeoForge 不顶 Forge，Quilt 不参与（`VersionKind` 里也没有它）。
//! - 资源包两个维度都不硬筛：不看加载器，也不按 MC 版本把文件挡掉。
//!   资源包的 `game_versions` 只标作者测过的那几个版本，硬筛会把大部分包挡在外面
//!   （`ResourcePack` / `Shader` 也完全不比对 MC 版本）。
//!   精确匹配只用来决定默认选哪个文件，见 [`default_pack_file_id`]。
//! - 光影比资源包多一道门槛：实例必须有模组加载器（[`shader_candidate_of`]）。
//!   没有加载器就装不了 Iris，光影装进去也是死的（游戏里能选、画面不动），
//!   所以那种实例连候选列表都不进 —— 列出来只会让人白点一趟。
//!   注意这只是"能不能装"，文件仍然不按加载器筛（光影文件自己标的是
//!   `iris` / `optifine`，拿 `fabric` 去比会一个都匹配不上）。
//!
//! 已安装判定要看磁盘，放在 `resources::mod` 里做。

use xcl_core::VersionKind;

use super::{ResourceFileView, ResourceKind};
use crate::local::LocalVersion;

/// 一个「能装」的本地版本。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalCandidate {
    pub version_id: String,
    /// 这个版本对应的 MC 版本号
    pub game_version: String,
    /// 文件里用的加载器名（小写）。资源包是 `None` —— 没有这个维度。
    pub loader: Option<&'static str>,
    /// 这个本地版本自身的加载器（`vanilla` 没有就是 `None`）。
    ///
    /// 和 `loader` 的区别在于用途：`loader` 决定「这个包能不能装进来」（资源包恒为
    /// `None`），`mod_loader` 决定「它依赖的 Mod 该装哪个构建」。资源包的目标实例
    /// 装了 Fabric，它的依赖（比如 Fresh Animations 要的 EMF）就得挑 Fabric 版，
    /// 不能因为资源包自己不判加载器就全放行。
    pub mod_loader: Option<&'static str>,
    /// 纯展示用的标签：Mod 与 `loader` 相同；资源包填本地版本自身的类型
    /// （`vanilla` / `fabric` / `forge` / `neoforge`），好让列表行和版本列表长得一样。
    pub display_loader: &'static str,
    /// 最近游玩近似值（目录修改时间），列表倒序用
    pub last_played_ms: u64,
}

impl LocalCandidate {
    /// 拿它去筛文件时该传的加载器：资源包传 `None`（不判这一项）。
    pub fn file_loader(&self) -> Option<&str> {
        self.loader
    }

    /// 解析依赖（永远是 Mod）时该传的加载器：用本地版本自身的类型。
    pub fn dep_loader(&self) -> Option<&str> {
        self.mod_loader
    }
}

/// `VersionKind` → 文件里用的加载器名。
///
/// 只有真正能装 Mod 的三种才算；OptiFine / LiteLoader / 原版 / Unknown（含 Quilt）返回 `None`。
pub fn loader_name(kind: VersionKind) -> Option<&'static str> {
    match kind {
        VersionKind::Fabric => Some("fabric"),
        VersionKind::Forge => Some("forge"),
        VersionKind::NeoForge => Some("neoforge"),
        _ => None,
    }
}

/// 单个本地版本能不能进候选列表（Mod）。
pub fn candidate_of(version: &LocalVersion) -> Option<LocalCandidate> {
    if !version.valid || !version.modable {
        return None;
    }
    let loader = loader_name(version.kind)?;
    let game_version = version.game_version.clone()?;
    Some(LocalCandidate {
        version_id: version.id.clone(),
        game_version,
        loader: Some(loader),
        mod_loader: Some(loader),
        display_loader: loader,
        last_played_ms: version.modified_ms,
    })
}

/// 本地版本自身的类型标签（资源包列表行展示用）。
///
/// 只区分「有没有加载器、是哪个」，其余一律算原版 —— 资源包不关心这些细节，
/// 但行上的图标和文字要跟版本列表对得上。
fn kind_label(kind: VersionKind) -> &'static str {
    match kind {
        VersionKind::Fabric => "fabric",
        VersionKind::Forge => "forge",
        VersionKind::NeoForge => "neoforge",
        _ => "vanilla",
    }
}

/// 单个本地版本能不能装资源包 / 光影。
///
/// 不要求 `modable`、也不要求加载器：原版就能用资源包。光影虽然还要 Iris 才
/// 生效，但"能不能装进来"和它无关 —— 没有 Iris 也有 `shaderpacks/` 这个目录，
/// 所以照样算候选，界面上由依赖卡去提醒缺什么。
/// `valid` 还是要的 —— JSON 都坏了就别往里装东西了。
pub fn pack_candidate_of(version: &LocalVersion) -> Option<LocalCandidate> {
    if !version.valid {
        return None;
    }
    let game_version = version.game_version.clone()?;
    Some(LocalCandidate {
        version_id: version.id.clone(),
        game_version,
        loader: None,
        // 资源包自己不判加载器，但它依赖的 Mod 要按这个实例的类型挑构建
        mod_loader: loader_name(version.kind),
        display_loader: kind_label(version.kind),
        last_played_ms: version.modified_ms,
    })
}

/// 单个本地版本能不能装光影。
///
/// 比资源包多一条硬门槛：必须有模组加载器。
/// 原版没有加载器 → 装不了 Iris → 光影装进去也是死的（游戏里能选、画面一点不动）。
/// 列出来只会白点一趟，不如不列。
///
/// 但 `loader` 仍然留 `None`：光影文件自己的 `loaders` 是 `iris` / `optifine`，
/// 拿 `fabric` 去比会一个都匹配不上。能不能装只看 `mod_loader`。
pub fn shader_candidate_of(version: &LocalVersion) -> Option<LocalCandidate> {
    if !version.valid || !version.modable {
        return None;
    }
    let loader = loader_name(version.kind)?;
    let game_version = version.game_version.clone()?;
    Some(LocalCandidate {
        version_id: version.id.clone(),
        game_version,
        loader: None,
        mod_loader: Some(loader),
        display_loader: kind_label(version.kind),
        last_played_ms: version.modified_ms,
    })
}

/// 全部候选（按类型），最近玩过的在前。
///
/// 只排序不预选 —— 详情页进去不能替选好，这是明确的界面要求。
pub fn candidates(kind: ResourceKind, versions: &[LocalVersion]) -> Vec<LocalCandidate> {
    let mut out: Vec<LocalCandidate> = versions
        .iter()
        .filter_map(|version| match kind {
            ResourceKind::Mod => candidate_of(version),
            // 资源包与光影的区别就在这一行：资源包原版也能用，光影非要加载器不可
            ResourceKind::ResourcePack => pack_candidate_of(version),
            ResourceKind::Shader => shader_candidate_of(version),
            // 整合包不装进本地版本（它自己建实例），没有候选
            ResourceKind::Modpack => None,
        })
        .collect();
    out.sort_by(|a, b| {
        b.last_played_ms
            .cmp(&a.last_played_ms)
            .then_with(|| a.version_id.cmp(&b.version_id))
    });
    out
}

/// 这个文件能不能装到「这个 MC 版本 + 这个加载器」上。
///
/// `loader` 为 `None` 时不判加载器（资源包）：只比 MC 版本。
pub fn file_matches(
    file: &ResourceFileView,
    game_version: &str,
    loader: Option<&str>,
) -> bool {
    file.game_versions.iter().any(|value| value == game_version)
        && match loader {
            None => true,
            Some(loader) => file
                .loaders
                .iter()
                .any(|value| value.eq_ignore_ascii_case(loader)),
        }
}

/// 挑出能用的那些（保持原来的新→旧顺序）。
pub fn matching_files<'a>(
    files: &'a [ResourceFileView],
    game_version: &str,
    loader: Option<&str>,
) -> Vec<&'a ResourceFileView> {
    files
        .iter()
        .filter(|file| file_matches(file, game_version, loader))
        .collect()
}

/// 默认选哪个文件：最新 release → 没有才退 beta → alpha → 实在没有就第一个。
///
/// `files` 必须是新到旧排好的（两个平台的接口都这么返回）。
/// 只挑类型、不挑"能不能装"：调用方传进来的已经筛过了。
pub fn default_file_id(files: &[ResourceFileView]) -> Option<String> {
    for wanted in ["release", "beta", "alpha"] {
        if let Some(file) = files.iter().find(|file| file.release_type == wanted) {
            return Some(file.file_id.clone());
        }
    }
    files.first().map(|file| file.file_id.clone())
}

/// 资源包的默认文件：优先声明支持这个 MC 版本的，没有就退最新。
///
/// 之所以不用 [`default_file_id`]：资源包不按版本硬筛，列表里可能混着 1.8 和
/// 1.21 的文件，只按 release 挑会挑到别的版本上去。这里把「版本对得上」放在
/// 第一优先级，对不上时再退「最新的那个」—— 不会出现「装了但游戏里看不到」。
pub fn default_pack_file_id(files: &[ResourceFileView], game_version: &str) -> Option<String> {
    let exact = files
        .iter()
        .find(|file| file.game_versions.iter().any(|value| value == game_version));
    exact
        .or_else(|| files.first())
        .map(|file| file.file_id.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn version(
        id: &str,
        kind: VersionKind,
        modable: bool,
        game_version: Option<&str>,
        modified_ms: u64,
    ) -> LocalVersion {
        LocalVersion {
            id: id.to_string(),
            version_type: "release".into(),
            kind,
            modable,
            texture_packs: true,
            has_logs: true,
            inherits_from: None,
            game_version: game_version.map(str::to_string),
            loader_version: None,
            has_client_jar: true,
            launchable: true,
            java_major: Some(17),
            main_class: None,
            modified_ms,
            valid: true,
            error: None,
            description: None,
            favorite: false,
            icon: None,
        }
    }

    fn file(id: &str, release_type: &str, versions: &[&str], loaders: &[&str]) -> ResourceFileView {
        ResourceFileView {
            dependency_count: 0,
            dependencies: Vec::new(),
            file_id: id.into(),
            version_number: id.into(),
            filename: format!("{id}.jar"),
            size: 100,
            sha1: None,
            url: "https://example.invalid/a.jar".into(),
            published: None,
            release_type: release_type.into(),
            game_versions: versions.iter().map(|v| v.to_string()).collect(),
            loaders: loaders.iter().map(|l| l.to_string()).collect(),
        }
    }

    #[test]
    fn only_moddable_versions_with_game_version_are_candidates() {
        let list = vec![
            version("1.20.1", VersionKind::Vanilla, false, Some("1.20.1"), 1),
            version("1.20.1-fabric-0.15", VersionKind::Fabric, true, Some("1.20.1"), 2),
            version("1.20.1-forge", VersionKind::Forge, true, Some("1.20.1"), 3),
            version("1.20.1-optifine", VersionKind::OptiFine, true, Some("1.20.1"), 4),
            version("weird", VersionKind::Unknown, true, Some("1.20.1"), 5),
            // 没有 game_version 的（JSON 坏了）不能进
            version("broken", VersionKind::Fabric, true, None, 6),
        ];
        let got: Vec<_> = candidates(ResourceKind::Mod, &list)
            .into_iter()
            .map(|c| c.version_id)
            .collect();
        // 最近玩过的在前
        assert_eq!(got, vec!["1.20.1-forge", "1.20.1-fabric-0.15"]);
    }

    #[test]
    fn resource_packs_take_vanilla_and_ignore_loaders() {
        let list = vec![
            version("1.20.1", VersionKind::Vanilla, false, Some("1.20.1"), 1),
            version("1.20.1-fabric-0.15", VersionKind::Fabric, true, Some("1.20.1"), 2),
            // JSON 坏了不能进
            version("broken", VersionKind::Fabric, true, None, 3),
            version("dead", VersionKind::Vanilla, false, Some("1.20.1"), 4),
        ]
        .into_iter()
        .map(|mut item| {
            if item.id == "dead" {
                item.valid = false;
            }
            item
        })
        .collect::<Vec<_>>();

        let got: Vec<_> = candidates(ResourceKind::ResourcePack, &list)
            .into_iter()
            .map(|c| (c.version_id, c.loader))
            .collect();
        // 原版也能装资源包；`loader` 一律是 None
        assert_eq!(
            got,
            vec![
                ("1.20.1-fabric-0.15".to_string(), None),
                ("1.20.1".to_string(), None),
            ]
        );
    }

    #[test]
    fn shaders_require_a_mod_loader() {
        let list = vec![
            version("vanilla", VersionKind::Vanilla, false, Some("1.20.1"), 1),
            version("fabric", VersionKind::Fabric, true, Some("1.20.1"), 2),
            version("forge", VersionKind::Forge, true, Some("1.20.1"), 3),
            // 有加载器但读不出 MC 版本：没法判兼容，不进
            version("broken", VersionKind::Fabric, true, None, 4),
        ];

        let got: Vec<_> = candidates(ResourceKind::Shader, &list)
            .into_iter()
            .map(|c| (c.version_id, c.loader, c.mod_loader))
            .collect();

        // 原版没有加载器 → 装不了 Iris → 光影装进去也是死的，所以不列。
        // 但 `loader` 仍是 None：光影文件的 `loaders` 是 iris/optifine，
        // 拿 fabric 去比会一个都匹配不上。
        assert_eq!(
            got,
            vec![
                ("forge".to_string(), None, Some("forge")),
                ("fabric".to_string(), None, Some("fabric")),
            ]
        );
    }

    #[test]
    fn no_loader_means_any_loader() {
        let fabric_file = file("a", "release", &["1.20.1"], &["fabric"]);
        // 资源包：只看 MC 版本
        assert!(file_matches(&fabric_file, "1.20.1", None));
        assert!(!file_matches(&fabric_file, "1.20.2", None));
    }

    #[test]
    fn loaders_must_match_exactly() {
        let fabric_file = file("a", "release", &["1.20.1"], &["fabric"]);
        assert!(file_matches(&fabric_file, "1.20.1", Some("fabric")));
        assert!(!file_matches(&fabric_file, "1.20.1", Some("forge")));
        assert!(!file_matches(&fabric_file, "1.20.2", Some("fabric")));
    }

    #[test]
    fn neoforge_file_does_not_match_forge_and_case_is_ignored() {
        let neo = file("b", "release", &["1.20.1"], &["NeoForge"]);
        assert!(file_matches(&neo, "1.20.1", Some("neoforge")));
        assert!(!file_matches(&neo, "1.20.1", Some("forge")));
    }

    #[test]
    fn one_file_can_serve_several_mc_versions() {
        let multi = file("c", "release", &["1.20.1", "1.20.2"], &["fabric"]);
        assert!(file_matches(&multi, "1.20.1", Some("fabric")));
        assert!(file_matches(&multi, "1.20.2", Some("fabric")));
        assert!(!file_matches(&multi, "1.20.3", Some("fabric")));
    }

    #[test]
    fn matching_keeps_newest_first_order() {
        let files = vec![
            file("new-beta", "beta", &["1.20.1"], &["fabric"]),
            file("new-release", "release", &["1.20.1"], &["fabric"]),
            file("old", "release", &["1.20.1"], &["forge"]),
        ];
        let got: Vec<_> = matching_files(&files, "1.20.1", Some("fabric"))
            .into_iter()
            .map(|f| f.file_id.as_str())
            .collect();
        assert_eq!(got, vec!["new-beta", "new-release"]);
    }

    #[test]
    fn default_prefers_release_then_beta_then_alpha() {
        let mixed = vec![
            file("beta-1", "beta", &["1.20.1"], &["fabric"]),
            file("rel-1", "release", &["1.20.1"], &["fabric"]),
        ];
        assert_eq!(default_file_id(&mixed), Some("rel-1".to_string()));

        let no_release = vec![
            file("alpha-1", "alpha", &["1.20.1"], &["fabric"]),
            file("beta-1", "beta", &["1.20.1"], &["fabric"]),
        ];
        assert_eq!(default_file_id(&no_release), Some("beta-1".to_string()));

        assert_eq!(default_file_id(&[]), None);
    }
}
