//! CurseForge 搜索。
//!
//! 接口：`GET /v1/mods/search?gameId=432&classId=6&...`
//!
//! 两个要点：
//! 1. 官方地址需要 API Key（实测无 key 直接 403），key 由 MCIM 镜像代持，
//!    所以这里只走镜像（`fetch_mod_text(.., true)`）。
//! 2. 排序 / 加载器 / 分类全是数字枚举，不认识的数字一律忽略，别硬猜。

use serde::Deserialize;
use xcl_core::AppError;

use super::{
    deps::{self, FileDependency},
    percent_encode, FileFacts, ModItemView, ModSearchView, ModSort, ProjectImageView,
    ProjectLinkView, ResourceFileView, ResourceProjectView, ResourceKind, MANUAL_LIMIT, PAGE_SIZE,
};
use crate::AppState;

const SEARCH_API: &str = "https://api.curseforge.com/v1/mods/search";
/// 文件列表 / 单个文件都挂在 `/v1/mods/{id}/files` 下。
const MOD_API: &str = "https://api.curseforge.com/v1/mods";

/// Minecraft 的 gameId，写死（CF 全站就这一个值）。
const GAME_ID: u32 = 432;
/// classId = 6 是 Mod（整合包 4471 / 资源包 12 / 光影 6552）。
const CLASS_MOD: u32 = 6;

/// `classId` → 项目页路径段（没有 `websiteUrl` 时拼兜底链接用）。
///
/// 只列认识的三种，其余一律按 Mod 处理 —— CF 的项目基本都带 websiteUrl，
/// 这条只是兜底，没必要为它维护全表。
fn cf_slug_segment(class_id: Option<u32>) -> &'static str {
    match class_id {
        Some(12) => "texture-packs",
        Some(4471) => "modpacks",
        Some(6552) => "shaders",
        _ => "mc-mods",
    }
}

#[derive(Debug, Deserialize)]
struct SearchResponse {
    #[serde(default)]
    data: Vec<Entry>,
    #[serde(default)]
    pagination: Pagination,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Pagination {
    #[serde(default)]
    total_count: u64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Entry {
    id: u64,
    name: String,
    #[serde(default)]
    slug: Option<String>,
    #[serde(default)]
    summary: String,
    /// 6 = Mod。依赖可能指向别的类型（资源包等），这种不作为依赖展示。
    #[serde(default)]
    class_id: Option<u32>,
    #[serde(default)]
    download_count: u64,
    #[serde(default)]
    date_modified: Option<String>,
    #[serde(default)]
    date_created: Option<String>,
    #[serde(default)]
    logo: Option<Logo>,
    #[serde(default)]
    links: Option<Links>,
    #[serde(default)]
    authors: Vec<Author>,
    #[serde(default)]
    latest_files_indexes: Vec<FileIndex>,
    #[serde(default)]
    screenshots: Vec<Screenshot>,
    #[serde(default)]
    categories: Vec<Category>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Screenshot {
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    thumbnail_url: Option<String>,
    #[serde(default)]
    url: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Category {
    #[serde(default)]
    name: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Logo {
    #[serde(default)]
    thumbnail_url: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Links {
    #[serde(default)]
    website_url: Option<String>,
    #[serde(default)]
    wiki_url: Option<String>,
    #[serde(default)]
    issues_url: Option<String>,
    #[serde(default)]
    source_url: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Author {
    #[serde(default)]
    name: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FileIndex {
    // 筛选阶段用不到，安装时按 MC 版本挑文件要用
    #[allow(dead_code)]
    #[serde(default)]
    game_version: Option<String>,
    /// 1 = Forge，4 = Fabric，5 = Quilt，6 = NeoForge
    #[serde(default)]
    mod_loader: Option<u32>,
}

impl AppState {
    #[allow(clippy::too_many_arguments)]
    pub(super) async fn search_curseforge(
        &self,
        query: &str,
        offset: usize,
        sort: ModSort,
        game_version: Option<&str>,
        loader: Option<&str>,
        kind: ResourceKind,
    ) -> Result<ModSearchView, AppError> {
        // 有搜索词时 CF 自己按相关度排；没词就按热门
        let sort_field = match sort {
            ModSort::Relevance => 2, // Popularity
            ModSort::Downloads => 6, // TotalDownloads
            ModSort::Updated => 3,   // LastUpdated
        };
        let mut url = format!(
            "{SEARCH_API}?gameId={GAME_ID}&classId={}&pageSize={PAGE_SIZE}\
             &index={offset}&sortField={sort_field}&sortOrder=desc",
            kind.curseforge_class()
        );
        if !query.is_empty() {
            url.push_str("&searchFilter=");
            url.push_str(&percent_encode(query));
        }
        // 可选筛选：MC 版本 / 加载器（CF 这边是数字枚举）
        //
        // CF 的 `gameVersion` 只有一个值、不能给一组，所以大版本线（`1.20`）
        // 没办法像 Modrinth 那样展开成 OR —— 原样传，CF 自己按族匹配。
        if let Some(version) = game_version.filter(|v| !v.is_empty()) {
            url.push_str("&gameVersion=");
            url.push_str(&percent_encode(version));
        }
        if let Some(loader_id) = loader.and_then(loader_id_of) {
            // 加载器只对 Mod 有意义：资源包传 `modLoaderType` 会一个结果都搜不到
            if kind == ResourceKind::Mod {
                url.push_str("&modLoaderType=");
                url.push_str(&loader_id.to_string());
            }
        }

        let text = self.fetch_mod_text(&url, true).await?;
        let raw: SearchResponse = serde_json::from_str(&text).map_err(|e| {
            AppError::parse("CurseForge 搜索结果解析失败", e.to_string())
        })?;

        let total = raw.pagination.total_count;
        let items: Vec<ModItemView> = raw.data.into_iter().map(|entry| self.map_entry(entry)).collect();
        let has_more = !items.is_empty()
            && items.len() == PAGE_SIZE
            && (offset as u64 + items.len() as u64) < total;

        Ok(ModSearchView {
            items,
            total,
            offset,
            has_more,
        })
    }
}

impl AppState {
    fn map_entry(&self, entry: Entry) -> ModItemView {
        // 项目页路径段跟着类型走：资源包在 `/minecraft/texture-packs/` 下，
        // 拼成 `mc-mods` 会得到一个 404 链接（只在没有 websiteUrl 时才用到）
        let segment = cf_slug_segment(entry.class_id);
        let url = entry
            .links
            .and_then(|links| links.website_url)
            .or_else(|| {
                entry
                    .slug
                    .as_ref()
                    .map(|slug| format!("https://www.curseforge.com/minecraft/{segment}/{slug}"))
            })
            .unwrap_or_default();

        ModItemView {
            source: "curseforge".to_string(),
            url,
            id: entry.id.to_string(),
            title: entry.name,
            description: entry.summary.trim().to_string(),
            author: entry
                .authors
                .first()
                .map(|author| author.name.clone())
                .unwrap_or_default(),
            icon_url: entry
                .logo
                .and_then(|logo| logo.thumbnail_url)
                .filter(|url| !url.is_empty())
                .map(|url| self.mod_asset_url(&url)),
            downloads: entry.download_count,
            updated: entry.date_modified,
            loaders: loaders_from_indexes(&entry.latest_files_indexes),
        }
    }
}

/// 从 `latestFilesIndexes` 里抽加载器（数字 → 名字），去重保序。
// ---------- 项目文件版本 ----------

/// `GET /v1/mods/{id}/files` 的一项。
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CfFile {
    id: u64,
    #[serde(default)]
    display_name: String,
    #[serde(default)]
    file_name: String,
    /// 1 = release，2 = beta，3 = alpha（数字，不是字符串）
    #[serde(default)]
    release_type: u32,
    #[serde(default)]
    file_date: Option<String>,
    #[serde(default)]
    file_length: u64,
    /// 作者关掉第三方分发时是 null
    #[serde(default)]
    download_url: Option<String>,
    /// 混装：`["Client","Forge","Server","1.12.2"]`
    #[serde(default)]
    game_versions: Vec<String>,
    #[serde(default)]
    hashes: Vec<CfHash>,
    /// 依赖挂文件上：只给 `modId` + `relationType`（数字）
    #[serde(default)]
    dependencies: Vec<CfDependency>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CfDependency {
    #[serde(default)]
    mod_id: u64,
    /// 1 内嵌库 / 2 可选 / 3 必需 / 4 工具 / 5 不兼容 / 6 Include
    #[serde(default)]
    relation_type: u32,
}

#[derive(Debug, Deserialize)]
struct CfHash {
    #[serde(default)]
    value: String,
    /// 1 = sha1
    #[serde(default)]
    algo: u32,
}

#[derive(Debug, Deserialize)]
struct FilesResponse {
    #[serde(default)]
    data: Vec<CfFile>,
    #[serde(default)]
    pagination: Pagination,
}

#[derive(Debug, Deserialize)]
struct SingleFile {
    data: CfFile,
}

#[derive(Debug, Deserialize)]
struct SingleMod {
    data: Entry,
}

impl AppState {
    /// 某个 Mod 的文件列表。
    ///
    /// CF 的 `gameVersion` 只收单个值（不像 Modrinth 能给一组），
    /// 所以自动模式下拿到的就是精确那个 MC 版本的文件；
    /// `modLoaderType` 是数字枚举。
    pub(super) async fn curseforge_files(
        &self,
        id: &str,
        game_version: Option<&str>,
        loader: Option<&str>,
        limit: Option<usize>,
    ) -> Result<Vec<ResourceFileView>, AppError> {
        // CF 的 pageSize 上限就是 50
        let size = limit.unwrap_or(MANUAL_LIMIT).clamp(1, 50);
        let mut url = format!(
            "{MOD_API}/{}/files?pageSize={size}&index=0",
            percent_encode(id)
        );
        if let Some(version) = game_version.filter(|v| !v.is_empty()) {
            url.push_str("&gameVersion=");
            url.push_str(&percent_encode(version));
        }
        if let Some(loader_id) = loader.and_then(loader_id_of) {
            url.push_str("&modLoaderType=");
            url.push_str(&loader_id.to_string());
        }

        let text = self.fetch_mod_text(&url, true).await?;
        let raw: FilesResponse = serde_json::from_str(&text).map_err(|e| {
            AppError::parse("CurseForge 版本列表解析失败", e.to_string())
        })?;

        let mut items: Vec<ResourceFileView> =
            raw.data.into_iter().filter_map(map_file).collect();
        // 不指望接口顺序：自己按日期排一次，默认项才真的是"最新稳定版"
        items.sort_by(|a, b| b.published.cmp(&a.published));
        Ok(items)
    }

    /// 一页文件（手动模式要翻页，所以把分页拆出来）。
    async fn curseforge_files_page(
        &self,
        id: &str,
        index: usize,
        size: usize,
    ) -> Result<(Vec<ResourceFileView>, u64), AppError> {
        let url = format!(
            "{MOD_API}/{}/files?pageSize={size}&index={index}",
            percent_encode(id)
        );
        let text = self.fetch_mod_text(&url, true).await?;
        let raw: FilesResponse = serde_json::from_str(&text).map_err(|e| {
            AppError::parse("CurseForge 版本列表解析失败", e.to_string())
        })?;
        let items: Vec<ResourceFileView> =
            raw.data.into_iter().filter_map(map_file).collect();
        Ok((items, raw.pagination.total_count))
    }

    /// 全量文件（手动模式用）。
    ///
    /// CF 单页最多 50 条，老 Mod 动辄几千个文件 —— 必须翻完，
    /// 不然看到的永远是"最近 50 个"，想装老版本就没得选。
    /// 每 4 页并发，翻页上限 200 页（10000 条，CF 自己的分页上限也是这个）。
    pub(super) async fn curseforge_all_files(
        &self,
        id: &str,
    ) -> Result<Vec<ResourceFileView>, AppError> {
        use futures::future::join_all;

        const PAGE: usize = 50;
        const MAX_PAGES: usize = 200;
        const BATCH: usize = 4;

        let (mut items, total) = self.curseforge_files_page(id, 0, PAGE).await?;
        let pages = ((total as usize).div_ceil(PAGE)).min(MAX_PAGES);
        if pages > 1 {
            for chunk in (1..pages).collect::<Vec<_>>().chunks(BATCH) {
                let batch = join_all(
                    chunk
                        .iter()
                        .map(|&index| self.curseforge_files_page(id, index, PAGE)),
                )
                .await;
                for result in batch {
                    let (page_items, _) = result?;
                    items.extend(page_items);
                }
            }
        }
        items.sort_by(|a, b| b.published.cmp(&a.published));
        Ok(items)
    }

    /// 按 fileId 取单个文件，同时把依赖一起带出来。
    pub(super) async fn curseforge_file_facts(
        &self,
        mod_id: &str,
        file_id: &str,
    ) -> Result<FileFacts, AppError> {
        let url = format!(
            "{MOD_API}/{}/files/{}",
            percent_encode(mod_id),
            percent_encode(file_id)
        );
        let text = self.fetch_mod_text(&url, true).await?;
        let raw: SingleFile = serde_json::from_str(&text).map_err(|e| {
            AppError::parse("CurseForge 文件信息解析失败", e.to_string())
        })?;
        let file = map_file(raw.data).ok_or_else(|| {
            AppError::not_found("这个文件没有可下载的内容", file_id.to_string())
        })?;
        let dependencies = file.dependencies.clone();
        Ok(FileFacts { file, dependencies })
    }

    /// 依赖项目的展示信息。依赖只给 `modId`，标题 / 图标都得再查一次。
    ///
    /// 不是 Mod（`classId != 6`，比如指向资源包）的返回 `None`：
    /// 这种项目没有 `/files?modLoaderType=` 这一套，不能当依赖展示。
    pub(super) async fn curseforge_mod(
        &self,
        mod_id: &str,
    ) -> Result<Option<ModItemView>, AppError> {
        let url = format!("{MOD_API}/{}", percent_encode(mod_id));
        let text = self.fetch_mod_text(&url, true).await?;
        let raw: SingleMod = serde_json::from_str(&text).map_err(|e| {
            AppError::parse("CurseForge 项目信息解析失败", e.to_string())
        })?;
        if raw.data.class_id.is_some_and(|class_id| class_id != CLASS_MOD) {
            return Ok(None);
        }
        Ok(Some(self.map_entry(raw.data)))
    }

    /// 项目详情（详情卡）。
    ///
    /// 公开接口没有正文（实测 JEI/Create/Jade/Fabric API 的 `description` 都是 null），
    /// 所以 `body` 固定为空，只给摘要 / 截图 / 元数据 / 外链。
    pub(super) async fn curseforge_project_detail(
        &self,
        mod_id: &str,
    ) -> Result<ResourceProjectView, AppError> {
        let url = format!("{MOD_API}/{}", percent_encode(mod_id));
        let text = self.fetch_mod_text(&url, true).await?;
        let raw: SingleMod = serde_json::from_str(&text).map_err(|e| {
            AppError::parse("CurseForge 项目详情解析失败", e.to_string())
        })?;
        let entry = raw.data;

        let links = entry.links.unwrap_or_default();
        let mut out_links: Vec<ProjectLinkView> = Vec::new();
        let mut push_link = |label: &str, value: Option<String>| {
            if let Some(url) = value.filter(|value| !value.trim().is_empty()) {
                out_links.push(ProjectLinkView {
                    label: label.to_string(),
                    url,
                });
            }
        };
        push_link("Wiki", links.wiki_url.clone());
        push_link("问题反馈", links.issues_url.clone());
        push_link("源码", links.source_url.clone());

        let page_url = links
            .website_url
            .clone()
            .filter(|url| !url.is_empty())
            .or_else(|| {
                entry.slug.as_ref().map(|slug| {
                    format!(
                        "https://www.curseforge.com/minecraft/{}/{slug}",
                        cf_slug_segment(entry.class_id)
                    )
                })
            })
            .unwrap_or_default();

        let gallery: Vec<ProjectImageView> = entry
            .screenshots
            .iter()
            .filter_map(|image| {
                let url = image
                    .url
                    .clone()
                    .filter(|url| !url.is_empty())
                    .or_else(|| image.thumbnail_url.clone().filter(|url| !url.is_empty()))?;
                Some(ProjectImageView {
                    url: self.mod_asset_url(&url),
                    title: image.title.clone().filter(|title| !title.trim().is_empty()),
                })
            })
            .collect();

        Ok(ResourceProjectView {
            source: "curseforge",
            id: entry.id.to_string(),
            url: page_url,
            title: entry.name,
            description: entry.summary.trim().to_string(),
            body: None,
            icon_url: entry
                .logo
                .and_then(|logo| logo.thumbnail_url)
                .filter(|url| !url.is_empty())
                .map(|url| self.mod_asset_url(&url)),
            author: entry
                .authors
                .first()
                .map(|author| author.name.clone())
                .unwrap_or_default(),
            downloads: entry.download_count,
            followers: 0,
            published: entry.date_created,
            updated: entry.date_modified,
            categories: entry
                .categories
                .iter()
                .map(|category| category.name.clone())
                .filter(|name| !name.is_empty())
                .collect(),
            loaders: loaders_from_indexes(&entry.latest_files_indexes),
            client_side: None,
            server_side: None,
            license: None,
            gallery,
            links: out_links,
        })
    }
}

/// 原始依赖 → 内部依赖（内嵌库 / 工具 / Include 直接丢掉）。
fn map_dependencies(entries: &[CfDependency]) -> Vec<FileDependency> {
    entries
        .iter()
        .filter_map(|entry| {
            if entry.mod_id == 0 {
                return None;
            }
            Some(FileDependency {
                project_id: entry.mod_id.to_string(),
                version_id: None,
                relation: deps::curseforge_relation(entry.relation_type)?,
            })
        })
        .collect()
}

fn map_file(raw: CfFile) -> Option<ResourceFileView> {
    if raw.file_name.is_empty() {
        return None;
    }
    let (game_versions, loaders) = split_game_versions(&raw.game_versions);
    let url = raw
        .download_url
        .filter(|url| !url.is_empty())
        .unwrap_or_else(|| cdn_url(raw.id, &raw.file_name));
    let dependencies = map_dependencies(&raw.dependencies);

    Some(ResourceFileView {
        dependency_count: dependencies.len(),
        dependencies,
        file_id: raw.id.to_string(),
        version_number: if raw.display_name.is_empty() {
            raw.file_name.clone()
        } else {
            raw.display_name.clone()
        },
        filename: raw.file_name,
        size: raw.file_length,
        sha1: raw
            .hashes
            .iter()
            .find(|hash| hash.algo == 1)
            .map(|hash| hash.value.clone()),
        url,
        published: raw.file_date,
        release_type: match raw.release_type {
            2 => "beta",
            3 => "alpha",
            _ => "release",
        }
        .to_string(),
        game_versions,
        loaders,
    })
}

/// 作者关掉第三方分发时 `downloadUrl` 是 null，按 fileId 反推 CDN 地址。
///
/// 这是 CF 自己的规则：`/{id/1000}/{id%1000}/{fileName}`。
fn cdn_url(id: u64, filename: &str) -> String {
    format!(
        "https://edge.forgecdn.net/files/{}/{}/{}",
        id / 1000,
        id % 1000,
        percent_encode(filename)
    )
}

/// CF 把加载器和 `Client`/`Server` 混进 `gameVersions`，必须拆开。
///
/// 实测 JEI：`["Client","Forge","Server","1.12.2"]`。不拆的话，
/// 拿它跟本地 MC 版本比会一条都对不上。
pub(crate) fn split_game_versions(raw: &[String]) -> (Vec<String>, Vec<String>) {
    const LOADERS: [&str; 4] = ["forge", "fabric", "neoforge", "quilt"];
    const NOISE: [&str; 2] = ["client", "server"];

    let mut versions: Vec<String> = Vec::new();
    let mut loaders: Vec<String> = Vec::new();
    for value in raw {
        let lower = value.to_ascii_lowercase();
        if LOADERS.contains(&lower.as_str()) {
            if !loaders.iter().any(|existing| existing == &lower) {
                loaders.push(lower);
            }
        } else if NOISE.contains(&lower.as_str()) || value.is_empty() {
            continue;
        } else {
            versions.push(value.clone());
        }
    }
    (versions, loaders)
}

fn loaders_from_indexes(indexes: &[FileIndex]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for index in indexes {
        let Some(name) = index.mod_loader.and_then(loader_name) else {
            continue;
        };
        if !out.iter().any(|existing| existing == name) {
            out.push(name.to_string());
        }
    }
    out
}

/// 名字 → CF 的数字枚举（拼请求用）。
fn loader_id_of(name: &str) -> Option<u32> {
    match name {
        "forge" => Some(1),
        "fabric" => Some(4),
        "quilt" => Some(5),
        "neoforge" => Some(6),
        _ => None,
    }
}

fn loader_name(id: u32) -> Option<&'static str> {
    match id {
        1 => Some("forge"),
        4 => Some("fabric"),
        5 => Some("quilt"),
        6 => Some("neoforge"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(items: &[&str]) -> Vec<String> {
        items.iter().map(|item| item.to_string()).collect()
    }

    #[test]
    fn game_versions_are_split_from_loaders_and_noise() {
        // 实测 JEI 的真实返回
        let raw = strings(&["Client", "Forge", "Server", "1.12.2"]);
        let (versions, loaders) = split_game_versions(&raw);
        assert_eq!(versions, strings(&["1.12.2"]));
        assert_eq!(loaders, strings(&["forge"]));

        let raw = strings(&["NeoForge", "1.21.1", "1.21"]);
        let (versions, loaders) = split_game_versions(&raw);
        assert_eq!(versions, strings(&["1.21.1", "1.21"]));
        assert_eq!(loaders, strings(&["neoforge"]));
    }

    #[test]
    fn duplicate_loaders_are_collapsed() {
        let raw = strings(&["Fabric", "fabric", "1.20.1"]);
        let (_, loaders) = split_game_versions(&raw);
        assert_eq!(loaders, strings(&["fabric"]));
    }

    #[test]
    fn cdn_url_follows_curseforge_rule() {
        assert_eq!(
            cdn_url(9334752, "jei_1.12.2-4.24.0.1040.jar"),
            "https://edge.forgecdn.net/files/9334/752/jei_1.12.2-4.24.0.1040.jar"
        );
    }

    #[test]
    fn numeric_loaders_are_mapped_and_deduped() {
        let indexes = vec![
            FileIndex {
                game_version: Some("1.20.1".into()),
                mod_loader: Some(4),
            },
            FileIndex {
                game_version: Some("1.20.1".into()),
                mod_loader: Some(4),
            },
            FileIndex {
                game_version: Some("1.20.1".into()),
                mod_loader: Some(6),
            },
            FileIndex {
                game_version: Some("1.20.1".into()),
                mod_loader: Some(99),
            },
        ];
        assert_eq!(loaders_from_indexes(&indexes), vec!["fabric", "neoforge"]);
    }
}
