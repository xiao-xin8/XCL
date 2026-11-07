//! Modrinth 搜索。
//!
//! 接口：`GET /v2/search`，`facets` 由我们自己拼（前端不碰查询串）。
//! 只要一种 `project_type`（Mod 或资源包），由 [`ResourceKind`] 决定。

use serde::Deserialize;
use xcl_core::AppError;

use super::{
    deps::{self, FileDependency},
    percent_encode, FileFacts, ModItemView, ModSearchView, ModSort, ProjectImageView,
    ProjectLinkView, ResourceFileView, ResourceProjectView, ResourceKind, PAGE_SIZE,
};
use crate::AppState;

const SEARCH_API: &str = "https://api.modrinth.com/v2/search";
const VERSION_API: &str = "https://api.modrinth.com/v2/version";

/// Modrinth 把加载器混在 `categories` 里返回，没有单独的 `loaders` 字段。
const LOADERS: [&str; 4] = ["fabric", "forge", "neoforge", "quilt"];

#[derive(Debug, Deserialize)]
struct SearchResponse {
    #[serde(default)]
    hits: Vec<Hit>,
    #[serde(default)]
    total_hits: u64,
}

#[derive(Debug, Deserialize)]
struct Hit {
    project_id: String,
    #[serde(default)]
    slug: Option<String>,
    title: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    author: String,
    #[serde(default)]
    icon_url: Option<String>,
    #[serde(default)]
    downloads: u64,
    #[serde(default)]
    date_modified: Option<String>,
    #[serde(default)]
    categories: Vec<String>,
}

impl AppState {
    #[allow(clippy::too_many_arguments)]
    pub(super) async fn search_modrinth(
        &self,
        query: &str,
        offset: usize,
        sort: ModSort,
        game_version: Option<&str>,
        loader: Option<&str>,
        kind: ResourceKind,
    ) -> Result<ModSearchView, AppError> {
        let index = match sort {
            ModSort::Relevance => "relevance",
            ModSort::Downloads => "downloads",
            ModSort::Updated => "updated",
        };
        // facets：只查一种类型；版本 / 加载器是可选筛选（默认都不限，
        // 因为真正的目标版本是安装时才定的，这里只是缩小范围）
        let mut groups = vec![format!(r#"["project_type:{}"]"#, kind.modrinth_type())];
        if let Some(version) = game_version.filter(|v| !v.is_empty()) {
            // 同一组里的多个 facet 是「或」：`1.20` → 1.20 / 1.20.1 / … / 1.20.6
            let versions = self.expand_game_versions(version).await;
            let list = versions
                .iter()
                .map(|item| format!(r#""versions:{item}""#))
                .collect::<Vec<_>>()
                .join(",");
            groups.push(format!("[{list}]"));
        }
        if let Some(loader) = loader.filter(|v| !v.is_empty()) {
            // 加载器只对 Mod 有意义：资源包没有「跑在哪个加载器上」这回事，
            // 照搬 categories:fabric 会一个结果都搜不到。
            if kind == ResourceKind::Mod {
                groups.push(format!(r#"["categories:{loader}"]"#));
            }
        }
        let facets = percent_encode(&format!("[{}]", groups.join(",")));
        let mut url = format!(
            "{SEARCH_API}?limit={PAGE_SIZE}&offset={offset}&index={index}&facets={facets}"
        );
        if !query.is_empty() {
            url.push_str("&query=");
            url.push_str(&percent_encode(query));
        }

        let text = self.fetch_mod_text(&url, false).await?;
        let raw: SearchResponse = serde_json::from_str(&text).map_err(|e| {
            AppError::parse("Modrinth 搜索结果解析失败", e.to_string())
        })?;

        let total = raw.total_hits;
        let items: Vec<ModItemView> = raw
            .hits
            .into_iter()
            .map(|hit| self.map_hit(hit, kind))
            .collect();
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
    fn map_hit(&self, hit: Hit, kind: ResourceKind) -> ModItemView {
        ModItemView {
            source: "modrinth".to_string(),
            url: format!(
                "https://modrinth.com/{}/{}",
                kind.modrinth_segment(),
                hit.slug.clone().unwrap_or_else(|| hit.project_id.clone())
            ),
            id: hit.project_id,
            title: hit.title,
            description: hit.description.trim().to_string(),
            author: hit.author,
            icon_url: hit
                .icon_url
                .filter(|url| !url.is_empty())
                .map(|url| self.mod_asset_url(&url)),
            downloads: hit.downloads,
            updated: hit.date_modified.as_deref().map(normalize_iso),
            loaders: loaders_from_categories(&hit.categories),
        }
    }

    /// `GET /v2/project/{id}` 的一项（只取依赖面板要显示的字段）。
    fn map_project(&self, raw: ProjectEntry) -> ModItemView {
        let slug = raw.slug.clone().unwrap_or_else(|| raw.id.clone());
        // 路径段跟着项目自己走，不能写死 `/mod/`：这个函数只服务依赖面板，
        // 而依赖可能是资源包 / 光影（比如某个整合包依赖一个光影项目），
        // 写死会生成 404 链接。`project_type` 是接口自带的，比自己猜准。
        let segment = match raw.project_type.as_str() {
            "resourcepack" => "resourcepack",
            "shader" => "shader",
            "modpack" => "modpack",
            _ => "mod",
        };
        ModItemView {
            source: "modrinth".to_string(),
            url: format!("https://modrinth.com/{segment}/{slug}"),
            id: raw.id,
            title: raw.title,
            description: raw.description.trim().to_string(),
            // 项目接口没有单一作者字段，依赖面板里留空即可
            author: String::new(),
            icon_url: raw
                .icon_url
                .filter(|url| !url.is_empty())
                .map(|url| self.mod_asset_url(&url)),
            downloads: raw.downloads,
            updated: raw.date_modified.as_deref().map(normalize_iso),
            loaders: if raw.loaders.is_empty() {
                loaders_from_categories(&raw.categories)
            } else {
                raw.loaders
            },
        }
    }
}

/// `GET /v2/project/{id}` 的响应（字段比搜索命中的 `Hit` 多 `loaders`）。
#[derive(Debug, Deserialize)]
struct ProjectEntry {
    id: String,
    #[serde(default)]
    slug: Option<String>,
    title: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    icon_url: Option<String>,
    #[serde(default)]
    downloads: u64,
    #[serde(default)]
    followers: u64,
    #[serde(default)]
    date_modified: Option<String>,
    #[serde(default)]
    published: Option<String>,
    #[serde(default)]
    categories: Vec<String>,
    /// `mod` / `resourcepack` / `shader` / `modpack` —— 生成项目页地址要用
    #[serde(default)]
    project_type: String,
    #[serde(default)]
    loaders: Vec<String>,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    client_side: Option<String>,
    #[serde(default)]
    server_side: Option<String>,
    #[serde(default)]
    license: Option<LicenseEntry>,
    #[serde(default)]
    issues_url: Option<String>,
    #[serde(default)]
    source_url: Option<String>,
    #[serde(default)]
    wiki_url: Option<String>,
    #[serde(default)]
    discord_url: Option<String>,
    #[serde(default)]
    gallery: Vec<GalleryEntry>,
}

#[derive(Debug, Default, Deserialize)]
struct LicenseEntry {
    #[serde(default)]
    id: String,
}

#[derive(Debug, Default, Deserialize)]
struct GalleryEntry {
    #[serde(default)]
    url: String,
    #[serde(default)]
    raw_url: String,
    #[serde(default)]
    title: Option<String>,
}

// ---------- 项目文件版本 ----------

/// Modrinth 的字段可能给 `null` 而不是干脆缺省 —— 整合包的版本里就见过
/// `loaders: null` / `version_type: null`。`#[serde(default)]` 只在缺失时生效，
/// 值为 null 会直接报 `invalid type: null, expected a string`，整个列表全挂。
/// 这里把 null 也当成缺省。
pub(super) fn null_default<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: serde::Deserializer<'de>,
    T: serde::Deserialize<'de> + Default,
{
    Ok(Option::<T>::deserialize(deserializer)?.unwrap_or_default())
}

/// `GET /v2/project/{id}/version` 的一项。
#[derive(Debug, Deserialize)]
struct VersionEntry {
    /// 版本 id 是唯一的必填项，接口不会给 null。真给了也该报错。
    id: String,
    #[serde(default, deserialize_with = "null_default")]
    version_number: String,
    #[serde(default, deserialize_with = "null_default")]
    name: String,
    #[serde(default, deserialize_with = "null_default")]
    game_versions: Vec<String>,
    #[serde(default, deserialize_with = "null_default")]
    loaders: Vec<String>,
    #[serde(default, deserialize_with = "null_default")]
    date_published: Option<String>,
    /// `release` / `beta` / `alpha`
    #[serde(default, deserialize_with = "null_default")]
    version_type: String,
    #[serde(default, deserialize_with = "null_default")]
    files: Vec<VersionFile>,
    /// 依赖。`dependency_type` 见 [`deps::modrinth_relation`]。
    #[serde(default, deserialize_with = "null_default")]
    dependencies: Vec<VersionDependency>,
}

/// Modrinth 的一条依赖：`version_id` 可能为空（作者没钉具体版本），
/// `file_name` 实测基本不用（全是 null），所以这里不收。
#[derive(Debug, Default, Deserialize)]
struct VersionDependency {
    #[serde(default)]
    version_id: Option<String>,
    #[serde(default, deserialize_with = "null_default")]
    project_id: String,
    #[serde(default, deserialize_with = "null_default")]
    dependency_type: String,
}

#[derive(Debug, Deserialize)]
struct VersionFile {
    #[serde(default, deserialize_with = "null_default")]
    hashes: Hashes,
    #[serde(default, deserialize_with = "null_default")]
    url: String,
    #[serde(default, deserialize_with = "null_default")]
    filename: String,
    #[serde(default, deserialize_with = "null_default")]
    size: u64,
    /// 一个版本可能挂多个文件，主文件才是要下的那个
    #[serde(default, deserialize_with = "null_default")]
    primary: bool,
}

#[derive(Debug, Default, Deserialize)]
struct Hashes {
    #[serde(default, deserialize_with = "null_default")]
    sha1: Option<String>,
}

impl AppState {
    /// 某个 Mod 的文件版本。
    ///
    /// `game_version` / `loader` 都是服务端筛选（Modrinth 支持
    /// `?game_versions=["1.20.1"]&loaders=["fabric"]`），所以自动模式下
    /// 拿回来的就已经是"这个本地版本能用的"那批，前端不用再比。
    pub(super) async fn modrinth_files(
        &self,
        id: &str,
        game_version: Option<&str>,
        loader: Option<&str>,
        limit: Option<usize>,
    ) -> Result<Vec<ResourceFileView>, AppError> {
        let mut url = format!(
            "https://api.modrinth.com/v2/project/{}/version",
            percent_encode(id)
        );
        let mut query: Vec<String> = Vec::new();
        if let Some(version) = game_version.filter(|v| !v.is_empty()) {
            let json = format!(r#"["{version}"]"#);
            query.push(format!("game_versions={}", percent_encode(&json)));
        }
        if let Some(loader) = loader.filter(|v| !v.is_empty()) {
            let json = format!(r#"["{loader}"]"#);
            query.push(format!("loaders={}", percent_encode(&json)));
        }
        if !query.is_empty() {
            url.push('?');
            url.push_str(&query.join("&"));
        }

        let text = self.fetch_mod_text(&url, false).await?;
        let raw: Vec<VersionEntry> = serde_json::from_str(&text).map_err(|e| {
            AppError::parse("Modrinth 版本列表解析失败", e.to_string())
        })?;

        let mut items: Vec<ResourceFileView> =
            raw.into_iter().filter_map(map_version).collect();
        // 接口本来就按新到旧返回，这里再排一次是为了不怕它改行为
        items.sort_by(|a, b| b.published.cmp(&a.published));
        if let Some(limit) = limit {
            items.truncate(limit);
        }
        Ok(items)
    }

    /// 按 versionId 取单个文件（下载要用文件名 / 大小 / sha1），
    /// 同时把这一版本的依赖一起带出来。
    pub(super) async fn modrinth_file_facts(
        &self,
        file_id: &str,
    ) -> Result<FileFacts, AppError> {
        let url = format!("{VERSION_API}/{}", percent_encode(file_id));
        let text = self.fetch_mod_text(&url, false).await?;
        let raw: VersionEntry = serde_json::from_str(&text).map_err(|e| {
            AppError::parse("Modrinth 文件信息解析失败", e.to_string())
        })?;
        let file = map_version(raw).ok_or_else(|| {
            AppError::not_found("这个文件没有可下载的内容", file_id.to_string())
        })?;
        let dependencies = file.dependencies.clone();
        Ok(FileFacts { file, dependencies })
    }

    /// 依赖项目的展示信息（标题 / 图标 / 简介）。
    pub(super) async fn modrinth_project(&self, id: &str) -> Result<ModItemView, AppError> {
        let url = format!("https://api.modrinth.com/v2/project/{}", percent_encode(id));
        let text = self.fetch_mod_text(&url, false).await?;
        let raw: ProjectEntry = serde_json::from_str(&text).map_err(|e| {
            AppError::parse("Modrinth 项目信息解析失败", e.to_string())
        })?;
        Ok(self.map_project(raw))
    }

    /// 项目详情（详情卡）：正文是 Markdown，图片走镜像改写。
    pub(super) async fn modrinth_project_detail(
        &self,
        id: &str,
        kind: ResourceKind,
    ) -> Result<ResourceProjectView, AppError> {
        let url = format!("https://api.modrinth.com/v2/project/{}", percent_encode(id));
        let text = self.fetch_mod_text(&url, false).await?;
        let raw: ProjectEntry = serde_json::from_str(&text).map_err(|e| {
            AppError::parse("Modrinth 项目详情解析失败", e.to_string())
        })?;

        let slug = raw.slug.clone().unwrap_or_else(|| raw.id.clone());
        let mut links: Vec<ProjectLinkView> = Vec::new();
        let mut push_link = |label: &str, value: Option<String>| {
            if let Some(value) = value.filter(|value| !value.trim().is_empty()) {
                links.push(ProjectLinkView {
                    label: label.to_string(),
                    url: value,
                });
            }
        };
        push_link("问题反馈", raw.issues_url);
        push_link("源码", raw.source_url);
        push_link("Wiki", raw.wiki_url);
        push_link("Discord", raw.discord_url);

        let gallery: Vec<ProjectImageView> = raw
            .gallery
            .into_iter()
            .filter_map(|image| {
                let url = if image.url.is_empty() {
                    image.raw_url
                } else {
                    image.url
                };
                if url.is_empty() {
                    return None;
                }
                Some(ProjectImageView {
                    url: self.mod_asset_url(&url),
                    title: image.title.filter(|title| !title.trim().is_empty()),
                })
            })
            .collect();

        let loaders = if raw.loaders.is_empty() {
            loaders_from_categories(&raw.categories)
        } else {
            raw.loaders
        };

        Ok(ResourceProjectView {
            source: "modrinth",
            id: raw.id,
            url: format!("https://modrinth.com/{}/{slug}", kind.modrinth_segment()),
            title: raw.title,
            description: raw.description.trim().to_string(),
            body: raw.body.filter(|body| !body.trim().is_empty()),
            icon_url: raw
                .icon_url
                .filter(|url| !url.is_empty())
                .map(|url| self.mod_asset_url(&url)),
            // 项目接口没有单一作者字段，留空
            author: String::new(),
            downloads: raw.downloads,
            followers: raw.followers,
            published: raw.published.as_deref().map(normalize_iso),
            updated: raw.date_modified.as_deref().map(normalize_iso),
            categories: raw.categories,
            loaders,
            client_side: raw.client_side.filter(|value| !value.is_empty()),
            server_side: raw.server_side.filter(|value| !value.is_empty()),
            license: raw
                .license
                .map(|license| license.id)
                .filter(|id| !id.is_empty()),
            gallery,
            links,
        })
    }
}

/// 原始依赖 → 内部依赖（内嵌库 / 未知类型直接丢掉）。
fn map_dependencies(entries: &[VersionDependency]) -> Vec<FileDependency> {
    entries
        .iter()
        .filter_map(|entry| {
            if entry.project_id.is_empty() {
                return None;
            }
            Some(FileDependency {
                project_id: entry.project_id.clone(),
                version_id: entry.version_id.clone(),
                relation: deps::modrinth_relation(&entry.dependency_type)?,
            })
        })
        .collect()
}

/// 一个 Modrinth 版本 → 统一视图。主文件优先，没标主文件就用第一个。
fn map_version(raw: VersionEntry) -> Option<ResourceFileView> {
    let file = raw
        .files
        .iter()
        .find(|file| file.primary)
        .or_else(|| raw.files.first())?;
    if file.url.is_empty() || file.filename.is_empty() {
        return None;
    }
    let dependencies = map_dependencies(&raw.dependencies);
    Some(ResourceFileView {
        dependency_count: dependencies.len(),
        dependencies,
        file_id: raw.id,
        version_number: if raw.version_number.is_empty() {
            raw.name
        } else {
            raw.version_number
        },
        filename: file.filename.clone(),
        size: file.size,
        sha1: file.hashes.sha1.clone(),
        url: file.url.clone(),
        published: raw
            .date_published
            .as_deref()
            .map(normalize_iso),
        release_type: if raw.version_type.is_empty() {
            "release".to_string()
        } else {
            raw.version_type
        },
        game_versions: raw.game_versions,
        loaders: raw.loaders,
    })
}

fn loaders_from_categories(categories: &[String]) -> Vec<String> {    LOADERS
        .iter()
        .filter(|loader| {
            categories
                .iter()
                .any(|category| category.eq_ignore_ascii_case(loader))
        })
        .map(|loader| (*loader).to_string())
        .collect()
}

/// 把 `2026-10-15T17:51:36.063140+00:00` 截成毫秒精度。
///
/// JS 的 `new Date()` 能吃微秒，但各家实现对超长小数位不一致；统一成 3 位最稳。
fn normalize_iso(raw: &str) -> String {
    let Some(dot) = raw.find('.') else {
        return raw.to_string();
    };
    let (head, tail) = raw.split_at(dot + 1);
    let frac_len = tail
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(tail.len());
    if frac_len <= 3 {
        return raw.to_string();
    }
    format!("{head}{}{}", &tail[..3], &tail[frac_len..])
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 整合包的版本里见过 `loaders: null` / `version_type: null`。
    /// `#[serde(default)]` 只兜缺失、不兜 null —— 少一个都会让整张版本列表挂掉，
    /// 所以这条必须钉住（真数据：Fabulously Optimized）。
    #[test]
    fn tolerates_null_fields_in_a_version() {
        let json = r#"[{
            "id": "NzL3x5Ce",
            "version_number": null,
            "name": null,
            "game_versions": null,
            "loaders": null,
            "version_type": null,
            "date_published": null,
            "files": null,
            "dependencies": null
        }]"#;
        let items: Vec<VersionEntry> = serde_json::from_str(json).expect("null 不能把整条打挂");
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].id, "NzL3x5Ce");
        assert!(items[0].game_versions.is_empty());
        assert!(items[0].loaders.is_empty());
    }

    #[test]
    fn loaders_are_picked_out_of_categories() {
        let cats = vec![
            "fabric".to_string(),
            "neoforge".to_string(),
            "optimization".to_string(),
        ];
        assert_eq!(loaders_from_categories(&cats), vec!["fabric", "neoforge"]);
    }

    #[test]
    fn iso_fraction_is_trimmed_to_millis() {
        assert_eq!(
            normalize_iso("2026-10-15T17:51:36.063140+00:00"),
            "2026-10-15T17:51:36.063+00:00"
        );
        assert_eq!(
            normalize_iso("2026-10-15T17:51:36+00:00"),
            "2026-10-15T17:51:36+00:00"
        );
        assert_eq!(
            normalize_iso("2026-10-15T17:51:36.12Z"),
            "2026-10-15T17:51:36.12Z"
        );
    }
}
