//! 模组加载器安装。
//!
//! - `fabric`：已接通。纯逻辑（版本列表 / profile JSON 地址 / 实例命名），
//!   不做 IO —— 请求由 `xcl-service` 用 `xcl-net` 发，镜像改写由 `xcl-source` 负责。
//! - `forgelike`：Forge / NeoForge 的纯逻辑（版本列表 / 安装器地址 / `install_profile`
//!   解析 / 预下载清单并集）。这两个加载器必须跑官方安装器，
//!   IO 与子进程在 `xcl-service`，方案与实测见 `docs/LOADER-FORGELIKE.md`。
//! - Quilt / OptiFine：待做。

pub mod fabric;
pub mod forgelike;

/// 当前支持的加载器类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoaderKind {
    Fabric,
    Forge,
    NeoForge,
}

impl LoaderKind {
    /// 从 IPC 传进来的字符串解析。
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "fabric" => Some(Self::Fabric),
            "forge" => Some(Self::Forge),
            "neoforge" => Some(Self::NeoForge),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Fabric => "fabric",
            Self::Forge => "forge",
            Self::NeoForge => "neoforge",
        }
    }

    /// 展示名。
    pub fn label(self) -> &'static str {
        match self {
            Self::Fabric => "Fabric",
            Self::Forge => "Forge",
            Self::NeoForge => "NeoForge",
        }
    }

    /// 全部类型，界面按这个顺序摆下拉项。
    pub fn all() -> [Self; 3] {
        [Self::Fabric, Self::Forge, Self::NeoForge]
    }

    /// 安装流水线是否接通。
    ///
    /// 界面不读它做特殊分支（三个加载器同一条渲染路径）；它只让后端在
    /// 预览 / 安装那一步明确拒绝还没接通的类型，而不是在前端藏半块界面。
    pub fn is_installable(self) -> bool {
        matches!(self, Self::Fabric | Self::Forge | Self::NeoForge)
    }
}

/// 已装实例 id → 加载器展示名；认不出来返回 `None`。
///
/// 三种命名都以 `-<原版 id>` 结尾，前缀各不相同，所以只按前缀认：
/// `fabric-loader-0.16.5-1.20.1` / `forge-47.4.5-1.20.1` / `neoforge-21.1.265-1.21.1`。
///
/// 这份对应关系只在这里写一次：版本列表的「已安装 Forge」标签和
/// 「这个原版装过哪些加载器」的判定都读它，免得两边各数一遍字符串。
pub fn loader_label_of(id: &str) -> Option<&'static str> {
    LoaderKind::all()
        .into_iter()
        .find(|kind| id.starts_with(&format!("{}-", kind.as_str())))
        .map(|kind| kind.label())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_every_loader_id_shape() {
        assert_eq!(loader_label_of("fabric-loader-0.16.5-1.20.1"), Some("Fabric"));
        assert_eq!(loader_label_of("forge-47.4.5-1.20.1"), Some("Forge"));
        assert_eq!(loader_label_of("neoforge-21.1.265-1.21.1"), Some("NeoForge"));
    }

    #[test]
    fn neoforge_is_not_mistaken_for_forge() {
        // `neoforge-` 也以 `neoforge` 开头，但不能被 `forge-` 抢先认走
        assert_eq!(loader_label_of("neoforge-47.1.106-1.20.1"), Some("NeoForge"));
    }

    #[test]
    fn vanilla_and_unknown_ids_are_ignored() {
        assert_eq!(loader_label_of("1.20.1"), None);
        assert_eq!(loader_label_of("optifine-1.20.1"), None);
    }

    #[test]
    fn every_loader_is_installable_now() {
        // 这条盯的是后端的能力判定：Forge / NeoForge 的流水线接好后改这一处，
        // 预览与安装就放开，界面一行都不用动
        assert_eq!(LoaderKind::all().len(), 3);
        assert!(LoaderKind::Fabric.is_installable());
        assert!(LoaderKind::Forge.is_installable());
        assert!(LoaderKind::NeoForge.is_installable());
    }
}
