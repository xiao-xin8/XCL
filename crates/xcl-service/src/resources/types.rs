//! 资源平台的视图类型 + 平台映射。
//!
//! 这里只有数据形状和纯映射：不碰网络、不碰文件系统、不读 `AppState`。
//! 拆出来是为了让 `resources/mod.rs` 只剩「查什么 + 怎么装」，
//! 新增一种资源类型时改的是这个文件里的三张表，而不是在两千行里翻。
//!
//! 对外路径不变（`resources/mod.rs` 用 `pub use types::*` 重导出），
//! 所以 `curseforge.rs` / `modrinth.rs` 里的 `super::ModItemView` 照旧。

use serde::{Deserialize, Serialize};

use super::deps;
use crate::install::FileKind;

/// 单页条数。Modrinth 最大 100，CF 最大 50，取 20 刚好一屏多。
pub const PAGE_SIZE: usize = 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ModSource {
    Modrinth,
    #[serde(rename = "curseforge")]
    CurseForge,
}

impl ModSource {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "modrinth" => Some(Self::Modrinth),
            "curseforge" => Some(Self::CurseForge),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Modrinth => "modrinth",
            Self::CurseForge => "curseforge",
        }
    }
}

/// 资源类型。搜索、项目详情、文件列表三件事完全同构，只有「类型」这一个
/// 维度不同：平台侧的类型值、项目页路径段、本地落点目录。
///
/// 三者都走同一条单文件下载链路，差异全部收在下面这几个映射里 —— 别处不许再
/// 拼 `resourcepacks` / `shaderpacks` 这类字符串。
///
/// 光影比另外两种多一层：必须靠 Iris（或 OptiFine）才能加载，见
/// [`Self::prerequisite_project`]。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResourceKind {
    Mod,
    ResourcePack,
    Shader,
    /// 整合包。不往任何本地版本里落文件 —— 它装出来的是一个新实例，
    /// 走 `modpack::modpack_install` 那条独立的链路，不走 `resource_install`。
    /// 所以 `subdir()` 给它一个空串，实体落点由整合包自己的清单决定。
    Modpack,
}

impl ResourceKind {
    /// 没传 / 传空串时按 Mod 处理 —— 老客户端不带这个字段，行为不能变。
    pub fn parse(value: Option<&str>) -> Option<Self> {
        match value.map(str::trim) {
            None | Some("") | Some("mod") => Some(Self::Mod),
            Some("resourcepack") => Some(Self::ResourcePack),
            Some("shader") => Some(Self::Shader),
            Some("modpack") => Some(Self::Modpack),
            _ => None,
        }
    }

    /// Modrinth 的 `project_type` facet 取值（单数，`shader` 不是 `shaders`）。
    pub fn modrinth_type(self) -> &'static str {
        match self {
            Self::Mod => "mod",
            Self::ResourcePack => "resourcepack",
            Self::Shader => "shader",
            Self::Modpack => "modpack",
        }
    }

    /// Modrinth 项目页的路径段：`modrinth.com/<这里>/<slug>`。
    pub fn modrinth_segment(self) -> &'static str {
        match self {
            Self::Mod => "mod",
            Self::ResourcePack => "resourcepack",
            Self::Shader => "shader",
            Self::Modpack => "modpack",
        }
    }

    /// CurseForge 的 `classId`（6 = Mod，12 = 资源包，6552 = 光影，4471 = 整合包）。
    pub fn curseforge_class(self) -> u32 {
        match self {
            Self::Mod => 6,
            Self::ResourcePack => 12,
            Self::Shader => 6552,
            Self::Modpack => 4471,
        }
    }

    /// 本地落点目录名。游戏只认这几个名字，写错等于没装 —— 所以放在这里，
    /// 不散落在各处拼字符串。
    ///
    /// 整合包没有落点（它建实例，不往别人的目录里塞文件），返回空串；
    /// 调用方在动手前会先拒绝这个类型。
    pub fn subdir(self) -> &'static str {
        match self {
            Self::Mod => "mods",
            Self::ResourcePack => "resourcepacks",
            Self::Shader => "shaderpacks",
            Self::Modpack => "",
        }
    }

    /// 面向看的名字（错误文案用）。
    pub fn label(self) -> &'static str {
        match self {
            Self::Mod => "Mod",
            Self::ResourcePack => "资源包",
            Self::Shader => "光影",
            Self::Modpack => "整合包",
        }
    }

    /// 下载引擎里的文件类型：决定任务卡片的文案和分组，也决定走哪张域名表。
    pub fn file_kind(self) -> FileKind {
        match self {
            Self::Mod => FileKind::Mod,
            Self::ResourcePack => FileKind::ResourcePack,
            Self::Shader => FileKind::Shader,
            Self::Modpack => FileKind::ModpackFile,
        }
    }

    /// 类型级前置依赖（Modrinth 的项目 id）。
    ///
    /// 光影没有 Iris 就是静默无效：文件装进去了、游戏里也能选，但画面一点不变。
    /// 而上游数据指望不上 —— 实测 Fresh Animations 从 1.11 起就不再声明依赖了，
    /// 所以这条只能由我们固定写死，不能等文件自己带。
    ///
    /// 返回 `None` 的类型没有前置。
    pub fn prerequisite_project(self) -> Option<&'static str> {
        match self {
            Self::Mod | Self::ResourcePack | Self::Modpack => None,
            Self::Shader => Some(IRIS_PROJECT_ID),
        }
    }
}

/// Iris Shaders 在 Modrinth 的项目 id（`modrinth.com/mod/iris`，实测
/// `project_type: mod`）。CurseForge 那边没有对应的已知 id，所以前置只在
/// Modrinth 源下注入，见 [`super::attach_prerequisites`]。
pub const IRIS_PROJECT_ID: &str = "YL57xq9U";

/// 排序方式。两个平台的枚举值完全不同，在这里统一成一套名字。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ModSort {
    /// 相关度（有搜索词时最有用）
    Relevance,
    /// 总下载量
    Downloads,
    /// 最近更新
    Updated,
}

impl ModSort {
    pub fn parse(value: &str) -> Self {
        match value {
            "downloads" => Self::Downloads,
            "updated" => Self::Updated,
            _ => Self::Relevance,
        }
    }
}

/// 列表里的一条 Mod。字段都是要显示的：图标、名字、作者、简介、下载量、
/// 更新时间、支持的加载器。不塞 category id、project_type 这类没用信息。
///
/// 之所以能 `Deserialize`：收藏要在本地存一份卡片快照（`favorites.toml`），
/// 这样搜自己收藏的名字、离线打开收藏页都不用回平台查。快照只用于画卡片，
/// 点进去仍然是实时的 `resource_project`。
///
/// 这里没有"是否已收藏"：那是纯展示判断（这个 id 在不在本地收藏集合里），
/// 前端拿一次收藏列表就能自己判。放进搜索结果会逼着每次收藏都去失效搜索缓存，
/// 为了一个星标浪费一次网络请求不划算。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModItemView {
    /// `"modrinth"` / `"curseforge"`
    ///
    /// 用 `String` 而不是 `&'static str`：收藏要反序列化回来，借用的字符串做不到。
    pub source: String,
    /// 平台内的项目 id（Modrinth 是 base62 串，CF 是数字串）
    pub id: String,
    /// 项目主页（以后"在浏览器打开"用；也是手动下载的兜底地址）
    pub url: String,
    pub title: String,
    pub description: String,
    pub author: String,
    pub icon_url: Option<String>,
    pub downloads: u64,
    /// ISO 8601；前端用现成的 `formatDateTime` 渲染
    pub updated: Option<String>,
    /// 支持的加载器（小写、去重）：`fabric` / `forge` / `neoforge` / `quilt`
    pub loaders: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModSearchView {
    pub items: Vec<ModItemView>,
    /// 命中总数（分页用）
    pub total: u64,
    /// 这次返回的起始偏移
    pub offset: usize,
    /// 还有没有下一页
    pub has_more: bool,
}

/// 手动模式一次拉多少个文件版本。
///
/// CF 的 `pageSize` 上限就是 50；老 Mod 动辄几千个文件，全拉下来既慢又没法看，
/// 所以两个源都按这个数收口，并在视图里说明「只显示最近 N 个」。
pub const MANUAL_LIMIT: usize = 50;

/// 一个可下载的文件版本。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourceFileView {
    pub file_id: String,
    /// 版本号（CF 没有独立版本号字段，用显示名兜底）
    pub version_number: String,
    pub filename: String,
    pub size: u64,
    pub sha1: Option<String>,
    /// 官方下载地址；真正下载时再按源策略展开候选
    pub url: String,
    pub published: Option<String>,
    /// `release` / `beta` / `alpha`
    pub release_type: String,
    /// 支持的 MC 版本（已去掉 CF 混进来的加载器名和 Client/Server）
    pub game_versions: Vec<String>,
    /// 支持的加载器（小写）
    pub loaders: Vec<String>,
    /// 需要展示的依赖数量（内嵌库 / 工具不算）。
    /// 界面靠它一开始就知道有没有依赖，不用等依赖解析回来才决定出不出加载动画。
    pub dependency_count: usize,
    /// 原始依赖（随文件列表一起序列化给前端：手动模式不用选安装位置
    /// 就能显示依赖项目；具体文件版本等选了目标版本再解析）。
    pub(crate) dependencies: Vec<deps::FileDependency>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourceFilesView {
    pub items: Vec<ResourceFileView>,
    /// 默认选中的文件（只有"自动模式"给；手动模式必须是 `None`，不替选）
    pub default_file_id: Option<String>,
    /// 自动模式：默认文件解析好的依赖，和文件列表一起回来。
    /// 手动模式为空（没有默认文件，依赖等选完文件 + 落点再查）。
    pub dependencies: Vec<ResourceDependencyView>,
    /// 是否因为数量上限被截断（界面据此显示「只显示最近 N 个」）
    pub truncated: bool,
}

/// 三级里的最内层：一个具体 MC 版本下的全部文件（比如 `1.20.1`）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourceFileGroupView {
    /// 具体版本号（`1.20.1`）
    pub label: String,
    pub items: Vec<ResourceFileView>,
}

/// 三级里的第一层：一条版本线（`1.20`），下面是具体的 MC 版本。
///
/// 结构和「下载 → 游戏」的版本树一样是三层（版本线 → 具体版本 → 行）：
/// 平铺一层的话，一个老 Mod 上万个文件条目全挂在一层，进去就卡死。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourceFileLineView {
    /// 版本线（`1.20`）
    pub label: String,
    /// 这条线下的文件条目数（同一个文件在不同版本里各算一次）
    pub count: usize,
    pub versions: Vec<ResourceFileGroupView>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourceGroupsView {
    pub lines: Vec<ResourceFileLineView>,
    /// 文件总数（去重前，一个文件可能属于多个 MC 版本）
    pub total: usize,
}

/// 详情页左侧的一行：一个本地版本。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalTargetView {
    pub version_id: String,
    pub game_version: String,
    pub loader: String,
    /// 最近游玩（目录修改时间）近似的毫秒时间戳，列表倒序用
    pub last_played_ms: u64,
    /// 目标 `mods/` 里已经有这个 Mod 的文件了
    pub installed: bool,
}

/// 手动模式右栏的一行：本地能装 Mod 的版本。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallTargetView {
    pub version_id: String,
    pub game_version: String,
    pub loader: String,
    pub last_played_ms: u64,
}

/// 右栏依赖区块的一行。
///
/// `project` 带完整的列表视图字段：点「安装依赖」时直接拿它构造依赖 Mod
/// 的详情页，不用前端再发一次项目请求。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourceDependencyView {
    pub project: ModItemView,
    /// `required` / `optional` / `incompatible`
    pub relation: &'static str,
    /// 对当前本地版本解析出来的文件；`None` 表示没有兼容文件或本来就不该装
    pub file: Option<ResourceFileView>,
    /// 目标 `mods/` 里已经有这个文件
    pub installed: bool,
    /// 解析不出来时给界面看的说明
    pub reason: Option<String>,
}

/// 项目详情里的一张截图。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectImageView {
    pub url: String,
    pub title: Option<String>,
}

/// 项目详情里的一个外链。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectLinkView {
    pub label: String,
    pub url: String,
}

/// 项目详情（详情卡用）。两个源能拿到的字段不一样，
/// 拿不到的保持 `None` / 空数组，界面按有无渲染。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourceProjectView {
    pub source: &'static str,
    pub id: String,
    /// 项目主页
    pub url: String,
    pub title: String,
    /// 列表里的短摘要
    pub description: String,
    /// 正文（Markdown）。CurseForge 的公开接口拿不到正文，固定 `None`
    pub body: Option<String>,
    pub icon_url: Option<String>,
    pub author: String,
    pub downloads: u64,
    pub followers: u64,
    pub published: Option<String>,
    pub updated: Option<String>,
    pub categories: Vec<String>,
    /// fabric / forge / neoforge / quilt
    pub loaders: Vec<String>,
    pub client_side: Option<String>,
    pub server_side: Option<String>,
    pub license: Option<String>,
    pub gallery: Vec<ProjectImageView>,
    pub links: Vec<ProjectLinkView>,
}
