//! 本地内容管理：扫描版本的 Mod / 资源包 / 光影 / 材质包目录、读包内元数据、
//! 启用/禁用、删除、从本地文件安装。
//!
//! 这里只处理本机文件，不联网。来源索引和线上更新检查是另一层的事，
//! 没有索引的文件只能显示它自己带的信息 —— 这正是大部分外部内容的真实状态。
//!
//! 文件状态沿用常见约定：启用 = `xxx.zip`，禁用 = `xxx.zip.disabled`。
//! `count_mods` 只认 Mod 的三种扩展名，所以禁用项天然不计入内存估算。

use std::collections::HashMap;
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use xcl_core::AppError;
use zip::ZipArchive;

use crate::AppState;

/// 来源索引格式版本。
const ORIGIN_SCHEMA: u32 = 1;

/// 元数据条目上限。正常的 mod 元数据只有几 KB，超过就是异常文件。
const MAX_META_BYTES: u64 = 512 * 1024;
/// 图标上限。列表一次画几十个图标，太大只会拖慢 IPC；
/// 常见的 128×128 图标远小于这个值。
const MAX_ICON_BYTES: u64 = 128 * 1024;
/// 简介最多保留的字符数。太长的简介在列表里没有意义。
const MAX_DESCRIPTION_CHARS: usize = 2_000;
/// 作者最多显示几个。
const MAX_AUTHORS: usize = 8;

/// 一个版本里可以管理的本地内容类型。
///
/// `dir_name` 同时是游戏认的目录名、回收站子目录和来源索引名的一部分，
/// 改名要同步 `VersionFolder` 与前端入口 key。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ContentKind {
    Mod,
    Resourcepack,
    Shader,
    Texturepack,
}

impl ContentKind {
    pub fn dir_name(self) -> &'static str {
        match self {
            Self::Mod => "mods",
            Self::Resourcepack => "resourcepacks",
            Self::Shader => "shaderpacks",
            Self::Texturepack => "texturepacks",
        }
    }

    /// 面向看的名字，用在按钮、错误文案和安装提示里。
    pub fn label(self) -> &'static str {
        match self {
            Self::Mod => "Mod",
            Self::Resourcepack => "资源包",
            Self::Shader => "光影",
            Self::Texturepack => "材质包",
        }
    }

    /// 能出现在这个目录里的文件扩展名（小写，不带点）。
    pub fn extensions(self) -> &'static [&'static str] {
        match self {
            Self::Mod => &["jar", "zip", "litemod"],
            Self::Resourcepack | Self::Shader | Self::Texturepack => &["zip"],
        }
    }

    /// 来源索引文件名。Mod 沿用 `mods.json`，已有安装记录不受影响。
    pub fn index_file(self) -> &'static str {
        match self {
            Self::Mod => "mods.json",
            Self::Resourcepack => "resourcepacks.json",
            Self::Shader => "shaderpacks.json",
            Self::Texturepack => "texturepacks.json",
        }
    }

    /// 只有 Mod 才有可判重的 ID；资源包 / 光影没有稳定的项目 ID。
    fn has_content_id(self) -> bool {
        self == Self::Mod
    }
}

/// 由启动器安装时记下的来源。外部拷进来的内容没有这个。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContentOriginView {
    pub source: String,
    pub project_id: String,
    pub file_id: String,
}

/// 列表里的一条本地内容。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalContentView {
    /// 真实文件名，启停 / 删除都靠它定位。
    pub file_name: String,
    pub name: String,
    /// Mod 的元数据 ID；资源包 / 光影没有，固定为 `None`。
    pub content_id: Option<String>,
    /// Mod 的版本号；资源包类内容没有。
    pub version: Option<String>,
    /// 资源包 / 材质包 / 光影的格式版本（`pack.mcmeta` 里读得到才有）。
    pub pack_format: Option<u32>,
    pub authors: Vec<String>,
    pub description: Option<String>,
    /// `data:image/...;base64,...`；包内没有图标时为 `None`。
    pub icon: Option<String>,
    pub enabled: bool,
    pub size: u64,
    /// 同一目录里有多个相同 Mod ID 时为 `true`；其它类型恒为 `false`。
    pub duplicate: bool,
    /// 由启动器安装时才有；外部拷入的内容为 `None`。
    pub origin: Option<ContentOriginView>,
}

/// 一个版本的内容列表快照。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContentListView {
    pub version_id: String,
    /// 实际扫描的目录。非隔离版本会指向共享根目录。
    pub game_dir: String,
    pub isolated: bool,
    /// 这个版本正在准备 / 运行。运行时不允许改文件。
    pub running: bool,
    pub items: Vec<LocalContentView>,
    pub enabled_count: u32,
    pub disabled_count: u32,
    pub duplicate_count: u32,
    /// 目录里还剩几个子文件夹。首版不管理文件夹形式的包，界面用这个数提示。
    pub folder_count: u32,
}

/// 启停一条内容后的结果：只回变化的这一条，前端原地替换，不整表刷新。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContentToggleView {
    pub item: LocalContentView,
    pub enabled_count: u32,
    pub disabled_count: u32,
    pub duplicate_count: u32,
}

/// 每个类型一份来源索引：`versions/<id>/XCL/<目录名>.json`。
/// Mod 沿用 `mods.json`，已有记录继续可读。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct ContentOriginIndex {
    schema: u32,
    entries: Vec<ContentOriginEntry>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct ContentOriginEntry {
    /// 不含 `.disabled` 后缀的文件名；启停不会让索引失配。
    file_name: String,
    /// 清单里记录的大小；文件被替换后大小对不上就不再认这条来源。
    size: u64,
    sha1: Option<String>,
    source: String,
    project_id: String,
    file_id: String,
    installed_at: u64,
}

impl ContentOriginEntry {
    fn view(&self) -> ContentOriginView {
        ContentOriginView {
            source: self.source.clone(),
            project_id: self.project_id.clone(),
            file_id: self.file_id.clone(),
        }
    }
}

impl AppState {
    /// 列出某个版本的一类本地内容。只读，不改任何文件。
    pub fn list_contents(
        &self,
        version_id: &str,
        kind: ContentKind,
    ) -> Result<ContentListView, AppError> {
        self.require_version_dir(version_id)?;
        let config = self.config();
        let resolution = crate::isolation::resolution_for(&self.layout, &config, version_id)?;
        let content_dir = resolution.game_dir.join(kind.dir_name());
        let running = self.launches.active_for(version_id);
        let origins = self.read_origins(version_id, kind);

        let mut items = Vec::new();
        let mut folder_count = 0u32;
        if let Ok(entries) = fs::read_dir(&content_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    // 首版只管理文件；文件夹形式的包在界面上单独提示。
                    folder_count += 1;
                    continue;
                }
                if !path.is_file() {
                    continue;
                }
                let Some(file_name) = path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .map(str::to_string)
                else {
                    continue;
                };
                let Some(enabled) = enabled_state(kind, &file_name) else {
                    continue;
                };
                let size = entry.metadata().map(|meta| meta.len()).unwrap_or(0);
                let origin = origins
                    .entries
                    .iter()
                    .rev()
                    .find(|entry| {
                        entry.file_name == origin_name(&file_name)
                            && (entry.size == 0 || entry.size == size)
                    })
                    .map(ContentOriginEntry::view);
                let meta = read_metadata(kind, &path).unwrap_or_default();
                let fallback = display_fallback(&file_name);
                let name = meta
                    .name
                    .filter(|text| !text.trim().is_empty())
                    .unwrap_or(fallback);
                items.push(LocalContentView {
                    file_name,
                    name,
                    content_id: meta.id,
                    version: meta.version,
                    pack_format: meta.pack_format,
                    authors: meta.authors,
                    description: meta.description,
                    icon: meta.icon,
                    enabled,
                    size,
                    duplicate: false,
                    origin,
                });
            }
        }

        if kind.has_content_id() {
            mark_duplicates(&mut items);
        }
        // 不按启用状态排序：启停只是临时调试，禁用项留在原位置，
        // 不能因为关一下就跳到列表最后。
        items.sort_by(|a, b| {
            a.name
                .to_lowercase()
                .cmp(&b.name.to_lowercase())
                .then_with(|| a.file_name.cmp(&b.file_name))
        });

        let enabled_count = items.iter().filter(|item| item.enabled).count() as u32;
        let disabled_count = items.len() as u32 - enabled_count;
        let duplicate_count = items.iter().filter(|item| item.duplicate).count() as u32;

        Ok(ContentListView {
            version_id: version_id.to_string(),
            game_dir: resolution.game_dir.display().to_string(),
            isolated: resolution.enabled,
            running,
            items,
            enabled_count,
            disabled_count,
            duplicate_count,
            folder_count,
        })
    }

    /// 启用 / 禁用：在名字后面加减 `.disabled`。只改名，不动内容。
    pub fn toggle_content(
        &self,
        version_id: &str,
        kind: ContentKind,
        file_name: &str,
    ) -> Result<ContentToggleView, AppError> {
        self.ensure_contents_mutable(version_id, kind)?;
        let (content_dir, _) = self.content_dir(version_id, kind)?;
        let path = safe_child(&content_dir, file_name)?;
        if !path.is_file() {
            return Err(AppError::not_found(
                format!("{}文件不存在", kind.label()),
                format!("找不到 {file_name}"),
            ));
        }
        let Some(enabled) = enabled_state(kind, file_name) else {
            return Err(AppError::validation(
                format!("不是可管理的{}文件", kind.label()),
                format!("{file_name} 不是 {}", extension_list(kind)),
            ));
        };

        let target_name = if enabled {
            format!("{file_name}.disabled")
        } else {
            file_name
                .strip_suffix(".disabled")
                .unwrap_or(file_name)
                .to_string()
        };
        let target = content_dir.join(&target_name);
        if target.exists() {
            return Err(AppError::validation(
                "同名文件已经存在",
                format!("切换状态时需要 {target_name}，但这个文件已经存在"),
            ));
        }

        fs::rename(&path, &target)
            .map_err(|err| AppError::io(&err, &format!("切换{}状态", kind.label())))?;

        // 只返回变化的那一条；列表顺序由前端按原 key 原地替换，不重排。
        let refreshed = self.list_contents(version_id, kind)?;
        let key = origin_name(&target_name);
        let Some(item) = refreshed
            .items
            .iter()
            .find(|item| origin_name(&item.file_name) == key)
            .cloned()
        else {
            return Err(AppError::not_found(
                format!("找不到切换后的{}", kind.label()),
                target.display().to_string(),
            ));
        };
        Ok(ContentToggleView {
            item,
            enabled_count: refreshed.enabled_count,
            disabled_count: refreshed.disabled_count,
            duplicate_count: refreshed.duplicate_count,
        })
    }

    /// 删除 = 移到版本目录的 `XCL/trash/<目录名>/`，不直接永久删除。
    pub fn delete_content(
        &self,
        version_id: &str,
        kind: ContentKind,
        file_name: &str,
    ) -> Result<ContentListView, AppError> {
        self.ensure_contents_mutable(version_id, kind)?;
        let (content_dir, _) = self.content_dir(version_id, kind)?;
        let path = safe_child(&content_dir, file_name)?;
        if !path.is_file() {
            return Err(AppError::not_found(
                format!("{}文件不存在", kind.label()),
                format!("找不到 {file_name}"),
            ));
        }

        let trash = self
            .layout
            .version_dir(version_id)
            .join("XCL")
            .join("trash")
            .join(kind.dir_name());
        fs::create_dir_all(&trash)
            .map_err(|err| AppError::io(&err, &format!("创建{}回收站", kind.label())))?;
        let target = unique_child(&trash, format!("{}-{file_name}", now_ms()));
        fs::rename(&path, &target)
            .map_err(|err| AppError::io(&err, &format!("移动{}到回收站", kind.label())))?;
        self.forget_origin(version_id, kind, file_name);
        self.list_contents(version_id, kind)
    }

    /// 从本地文件安装。目标已有同名文件时自动加序号，不覆盖。
    pub fn install_content_file(
        &self,
        version_id: &str,
        kind: ContentKind,
        source: &Path,
    ) -> Result<ContentListView, AppError> {
        self.ensure_contents_mutable(version_id, kind)?;
        let Some(file_name) = source
            .file_name()
            .and_then(|name| name.to_str())
            .map(str::to_string)
        else {
            return Err(AppError::validation(
                "文件名不合法",
                "选中的文件没有可用的文件名",
            ));
        };
        if enabled_state(kind, &file_name) != Some(true) {
            return Err(AppError::validation(
                format!("这个文件不能直接当{}安装", kind.label()),
                format!("只支持 {}", extension_list(kind)),
            ));
        }
        if !source.is_file() {
            return Err(AppError::not_found(
                "找不到选中的文件",
                source.display().to_string(),
            ));
        }

        let (content_dir, _) = self.content_dir(version_id, kind)?;
        fs::create_dir_all(&content_dir)
            .map_err(|err| AppError::io(&err, &format!("创建 {} 目录", kind.dir_name())))?;
        let target = unique_child(&content_dir, file_name);
        fs::copy(source, &target)
            .map_err(|err| AppError::io(&err, &format!("安装{}", kind.label())))?;
        self.list_contents(version_id, kind)
    }

    fn content_dir(
        &self,
        version_id: &str,
        kind: ContentKind,
    ) -> Result<(PathBuf, bool), AppError> {
        self.require_version_dir(version_id)?;
        let config = self.config();
        let resolution = crate::isolation::resolution_for(&self.layout, &config, version_id)?;
        Ok((
            resolution.game_dir.join(kind.dir_name()),
            resolution.enabled,
        ))
    }

    /// 记录一次由启动器完成的安装。下一阶段用它识别线上项目和检查更新。
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn record_content_origin(
        &self,
        kind: ContentKind,
        version_id: &str,
        file_name: &str,
        size: u64,
        sha1: Option<String>,
        source: &str,
        project_id: &str,
        file_id: &str,
    ) -> Result<(), AppError> {
        self.require_version_dir(version_id)?;
        let _guard = origin_lock().lock().unwrap_or_else(|err| err.into_inner());
        let file_name = origin_name(file_name).to_string();
        let mut index = self.read_origins(version_id, kind);
        index.schema = ORIGIN_SCHEMA;
        index.entries.retain(|entry| entry.file_name != file_name);
        index.entries.push(ContentOriginEntry {
            file_name,
            size,
            sha1,
            source: source.to_string(),
            project_id: project_id.to_string(),
            file_id: file_id.to_string(),
            installed_at: now_ms() as u64,
        });
        self.write_origins(version_id, kind, &index)
    }

    fn forget_origin(&self, version_id: &str, kind: ContentKind, file_name: &str) {
        let _guard = origin_lock().lock().unwrap_or_else(|err| err.into_inner());
        let mut index = self.read_origins(version_id, kind);
        let file_name = origin_name(file_name);
        let before = index.entries.len();
        index.entries.retain(|entry| entry.file_name != file_name);
        if index.entries.len() != before
            && let Err(err) = self.write_origins(version_id, kind, &index)
        {
            tracing::warn!(error = %err, kind = ?kind, "清理来源索引失败");
        }
    }

    fn origin_path(&self, version_id: &str, kind: ContentKind) -> PathBuf {
        self.layout
            .version_dir(version_id)
            .join("XCL")
            .join(kind.index_file())
    }

    fn read_origins(&self, version_id: &str, kind: ContentKind) -> ContentOriginIndex {
        let path = self.origin_path(version_id, kind);
        let Ok(text) = fs::read_to_string(path) else {
            return ContentOriginIndex::default();
        };
        let Ok(index) = serde_json::from_str::<ContentOriginIndex>(&text) else {
            tracing::warn!(path = %self.origin_path(version_id, kind).display(), "来源索引损坏");
            return ContentOriginIndex::default();
        };
        if index.schema != ORIGIN_SCHEMA {
            return ContentOriginIndex::default();
        }
        index
    }

    fn write_origins(
        &self,
        version_id: &str,
        kind: ContentKind,
        index: &ContentOriginIndex,
    ) -> Result<(), AppError> {
        let path = self.origin_path(version_id, kind);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|err| AppError::io(&err, "创建来源目录"))?;
        }
        let text = serde_json::to_string_pretty(index)
            .map_err(|err| AppError::parse("写入来源索引失败", err.to_string()))?;
        let tmp = path.with_extension("json.tmp");
        fs::write(&tmp, text).map_err(|err| AppError::io(&err, "写入来源索引"))?;
        fs::rename(&tmp, &path).map_err(|err| AppError::io(&err, "更新来源索引"))?;
        Ok(())
    }

    fn ensure_contents_mutable(&self, version_id: &str, kind: ContentKind) -> Result<(), AppError> {
        if self.launches.active_for(version_id) {
            return Err(AppError::validation(
                "游戏正在运行或准备中",
                format!("先等启动结束并关闭游戏，再修改{}", kind.label()),
            )
            .with_hint(format!(
                "运行中的游戏会锁住 {} 目录里的文件",
                kind.dir_name()
            )));
        }
        Ok(())
    }
}

#[derive(Debug, Default)]
struct ContentMeta {
    name: Option<String>,
    id: Option<String>,
    version: Option<String>,
    pack_format: Option<u32>,
    authors: Vec<String>,
    description: Option<String>,
    icon: Option<String>,
    icon_path: Option<String>,
}

/// 按类型读包内元数据；解析失败返回 `None`，调用方用文件名兜底。
fn read_metadata(kind: ContentKind, path: &Path) -> Option<ContentMeta> {
    match kind {
        ContentKind::Mod => read_mod_metadata(path),
        ContentKind::Resourcepack | ContentKind::Texturepack => read_pack_metadata(path, kind),
        ContentKind::Shader => read_shader_metadata(path),
    }
}

/// 读 jar 里的 Mod 元数据。
fn read_mod_metadata(path: &Path) -> Option<ContentMeta> {
    let file = File::open(path).ok()?;
    let mut archive = ZipArchive::new(file).ok()?;

    let mut meta = None;
    if let Some(bytes) = read_entry(&mut archive, "fabric.mod.json", MAX_META_BYTES) {
        meta = parse_fabric(&bytes);
    }
    if meta.is_none()
        && let Some(bytes) = read_entry(&mut archive, "quilt.mod.json", MAX_META_BYTES)
    {
        meta = parse_quilt(&bytes);
    }
    if meta.is_none()
        && let Some(bytes) = read_entry(&mut archive, "META-INF/neoforge.mods.toml", MAX_META_BYTES)
    {
        meta = parse_forge(&bytes);
    }
    if meta.is_none()
        && let Some(bytes) = read_entry(&mut archive, "META-INF/mods.toml", MAX_META_BYTES)
    {
        meta = parse_forge(&bytes);
    }
    if meta.is_none()
        && let Some(bytes) = read_entry(&mut archive, "mcmod.info", MAX_META_BYTES)
    {
        meta = parse_mcmod_info(&bytes);
    }
    if meta.is_none()
        && let Some(bytes) = read_entry(&mut archive, "META-INF/mcmod.info", MAX_META_BYTES)
    {
        meta = parse_mcmod_info(&bytes);
    }

    let mut meta = meta?;
    if let Some(icon_path) = meta.icon_path.take() {
        meta.icon = load_icon(&mut archive, &icon_path);
    }
    Some(meta)
}

/// 资源包 / 材质包：`pack.mcmeta` 提供格式与简介，`pack.png` 提供图标；
/// 1.6 以前的材质包没有 mcmeta，简介写在 `pack.txt`。
fn read_pack_metadata(path: &Path, kind: ContentKind) -> Option<ContentMeta> {
    let file = File::open(path).ok()?;
    let mut archive = ZipArchive::new(file).ok()?;
    let mut meta = ContentMeta::default();

    if let Some(bytes) = read_entry(&mut archive, "pack.mcmeta", MAX_META_BYTES)
        && let Some((description, pack_format)) = parse_pack_mcmeta(&bytes)
    {
        meta.description = description;
        meta.pack_format = pack_format;
    }
    if kind == ContentKind::Texturepack
        && meta.description.is_none()
        && let Some(bytes) = read_entry(&mut archive, "pack.txt", MAX_META_BYTES)
    {
        let text = String::from_utf8_lossy(&bytes).replace("\r\n", "\n");
        let text = text.trim();
        if !text.is_empty() {
            meta.description = Some(clean_description(text.to_string()));
        }
    }
    meta.icon = load_icon(&mut archive, "pack.png");
    Some(meta)
}

/// 光影没有统一的名称 / 简介规范，只有包里碰巧带了这两个文件时才能用上。
fn read_shader_metadata(path: &Path) -> Option<ContentMeta> {
    let file = File::open(path).ok()?;
    let mut archive = ZipArchive::new(file).ok()?;
    let mut meta = ContentMeta::default();
    if let Some(bytes) = read_entry(&mut archive, "pack.mcmeta", MAX_META_BYTES)
        && let Some((description, pack_format)) = parse_pack_mcmeta(&bytes)
    {
        meta.description = description;
        meta.pack_format = pack_format;
    }
    meta.icon = load_icon(&mut archive, "pack.png");
    Some(meta)
}

/// `pack.mcmeta` 的 `description` 既可能是纯字符串，也可能是文本组件对象。
fn parse_pack_mcmeta(bytes: &[u8]) -> Option<(Option<String>, Option<u32>)> {
    let value: Value = serde_json::from_slice(bytes).ok()?;
    let pack = value.get("pack")?;
    let pack_format = pack
        .get("pack_format")
        .and_then(Value::as_u64)
        .and_then(|value| u32::try_from(value).ok());
    let description = match pack.get("description") {
        Some(Value::String(text)) => {
            let text = text.trim();
            (!text.is_empty()).then(|| clean_description(text.to_string()))
        }
        Some(Value::Object(object)) => object
            .get("text")
            .or_else(|| object.get("translate"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .map(|text| clean_description(text.to_string())),
        _ => None,
    };
    Some((description, pack_format))
}

fn read_entry(archive: &mut ZipArchive<File>, name: &str, limit: u64) -> Option<Vec<u8>> {
    let entry = archive.by_name(name).ok()?;
    if entry.size() > limit {
        return None;
    }
    let mut bytes = Vec::new();
    entry.take(limit + 1).read_to_end(&mut bytes).ok()?;
    (bytes.len() as u64 <= limit).then_some(bytes)
}

fn load_icon(archive: &mut ZipArchive<File>, path: &str) -> Option<String> {
    let name = path.trim_start_matches(['/', '\\']).replace('\\', "/");
    if name.is_empty() {
        return None;
    }
    let bytes = read_entry(archive, &name, MAX_ICON_BYTES)?;
    let mime = crate::local::image_mime(&bytes)?;
    Some(format!(
        "data:{mime};base64,{}",
        base64::engine::general_purpose::STANDARD.encode(bytes)
    ))
}

fn parse_fabric(bytes: &[u8]) -> Option<ContentMeta> {
    let value: Value = serde_json::from_slice(bytes).ok()?;
    Some(ContentMeta {
        name: text(value.get("name")),
        id: text(value.get("id")),
        version: text(value.get("version")),
        authors: authors(value.get("authors")),
        description: description(value.get("description")),
        icon_path: icon_path(value.get("icon")),
        ..ContentMeta::default()
    })
}

fn parse_quilt(bytes: &[u8]) -> Option<ContentMeta> {
    let value: Value = serde_json::from_slice(bytes).ok()?;
    let loader = value.get("quilt_loader");
    let metadata = loader.and_then(|value| value.get("metadata"));
    Some(ContentMeta {
        name: text(metadata.and_then(|value| value.get("name"))),
        id: text(loader.and_then(|value| value.get("id"))),
        version: text(loader.and_then(|value| value.get("version"))),
        authors: contributors(metadata.and_then(|value| value.get("contributors"))),
        description: description(metadata.and_then(|value| value.get("description"))),
        icon_path: icon_path(metadata.and_then(|value| value.get("icon"))),
        ..ContentMeta::default()
    })
}

fn parse_forge(bytes: &[u8]) -> Option<ContentMeta> {
    let text = String::from_utf8_lossy(bytes);
    let value: toml::Value = toml::from_str(text.as_ref()).ok()?;
    let first = value.get("mods")?.as_array()?.first()?.as_table()?;
    let get = |key: &str| first.get(key).and_then(|value| value.as_str());
    let authors = get("authors").map(split_authors).unwrap_or_default();
    Some(ContentMeta {
        name: get("displayName")
            .map(|text| text.trim().to_string())
            .filter(|text| !text.is_empty()),
        id: get("modId")
            .map(|text| text.trim().to_string())
            .filter(|text| !text.is_empty()),
        version: get("version")
            .map(|text| text.trim().to_string())
            .filter(|text| !text.is_empty()),
        authors,
        description: get("description")
            .map(|text| clean_description(text.to_string()))
            .filter(|text| !text.is_empty()),
        icon_path: get("logoFile").map(str::to_string),
        ..ContentMeta::default()
    })
}

fn parse_mcmod_info(bytes: &[u8]) -> Option<ContentMeta> {
    let value: Value = serde_json::from_slice(bytes).ok()?;
    let first = if let Some(items) = value.as_array() {
        items.first()?
    } else {
        value.get("modList")?.as_array()?.first()?
    };
    Some(ContentMeta {
        name: text(first.get("name")),
        id: text(first.get("modid")).or_else(|| text(first.get("modId"))),
        version: text(first.get("version")),
        authors: authors(first.get("authorList"))
            .into_iter()
            .chain(authors(first.get("authors")))
            .collect(),
        description: description(first.get("description")),
        icon_path: text(first.get("logoFile")),
        ..ContentMeta::default()
    })
}

fn text(value: Option<&Value>) -> Option<String> {
    let raw = match value? {
        Value::String(text) => text.clone(),
        Value::Number(number) => number.to_string(),
        _ => return None,
    };
    let text = raw.trim();
    (!text.is_empty()).then(|| text.to_string())
}

fn description(value: Option<&Value>) -> Option<String> {
    text(value).map(clean_description)
}

fn clean_description(text: String) -> String {
    let text = text.replace("\r\n", "\n").trim().to_string();
    if text.chars().count() <= MAX_DESCRIPTION_CHARS {
        return text;
    }
    let mut cut: String = text.chars().take(MAX_DESCRIPTION_CHARS).collect();
    cut.push('…');
    cut
}

fn authors(value: Option<&Value>) -> Vec<String> {
    let mut result = match value {
        Some(Value::String(text)) => split_authors(text),
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(|item| {
                item.as_str()
                    .map(str::to_string)
                    .or_else(|| text(item.get("name")))
                    .or_else(|| text(item.get("author")))
            })
            .collect(),
        _ => Vec::new(),
    };
    result.truncate(MAX_AUTHORS);
    result
}

fn split_authors(text: &str) -> Vec<String> {
    text.split([',', ';'])
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .take(MAX_AUTHORS)
        .map(str::to_string)
        .collect()
}

fn contributors(value: Option<&Value>) -> Vec<String> {
    match value {
        Some(Value::Object(map)) => map.keys().take(MAX_AUTHORS).cloned().collect(),
        other => authors(other),
    }
}

fn icon_path(value: Option<&Value>) -> Option<String> {
    let value = value?;
    value
        .as_str()
        .map(str::to_string)
        .or_else(|| text(value.get("path")))
}

/// 启用态返回 `Some(true)`，禁用态返回 `Some(false)`，不是该类型的文件返回 `None`。
fn enabled_state(kind: ContentKind, file_name: &str) -> Option<bool> {
    if let Some(base) = file_name.strip_suffix(".disabled") {
        return has_extension(kind, base).then_some(false);
    }
    has_extension(kind, file_name).then_some(true)
}

fn has_extension(kind: ContentKind, name: &str) -> bool {
    let Some(ext) = Path::new(name).extension().and_then(|ext| ext.to_str()) else {
        return false;
    };
    let ext = ext.to_ascii_lowercase();
    kind.extensions().iter().any(|item| *item == ext.as_str())
}

fn extension_list(kind: ContentKind) -> String {
    kind.extensions()
        .iter()
        .map(|ext| format!(".{ext}"))
        .collect::<Vec<_>>()
        .join(" / ")
}

fn display_fallback(file_name: &str) -> String {
    let name = origin_name(file_name);
    Path::new(name)
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or(name)
        .to_string()
}

fn origin_name(file_name: &str) -> &str {
    file_name.strip_suffix(".disabled").unwrap_or(file_name)
}

fn mark_duplicates(items: &mut [LocalContentView]) {
    let mut counts: HashMap<String, u32> = HashMap::new();
    for item in items.iter() {
        if let Some(id) = &item.content_id {
            *counts.entry(id.to_ascii_lowercase()).or_default() += 1;
        }
    }
    for item in items.iter_mut() {
        item.duplicate = item
            .content_id
            .as_ref()
            .and_then(|id| counts.get(&id.to_ascii_lowercase()))
            .is_some_and(|count| *count > 1);
    }
}

fn safe_child(dir: &Path, file_name: &str) -> Result<PathBuf, AppError> {
    let invalid = file_name.is_empty()
        || file_name == "."
        || file_name == ".."
        || file_name.contains(['/', '\\', ':', '\0'])
        || file_name.chars().any(char::is_control);
    if invalid {
        return Err(AppError::validation(
            "文件名不合法",
            format!("{file_name:?} 不能作为内容文件名"),
        ));
    }
    let path = dir.join(file_name);
    if path.parent() != Some(dir) {
        return Err(AppError::validation(
            "文件名不合法",
            "文件必须直接位于内容目录下",
        ));
    }
    Ok(path)
}

fn unique_child(dir: &Path, name: String) -> PathBuf {
    let candidate = dir.join(&name);
    if !candidate.exists() {
        return candidate;
    }
    let path = Path::new(&name);
    let stem = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or(&name);
    let ext = path.extension().and_then(|ext| ext.to_str());
    for index in 2..1_000 {
        let next = match ext {
            Some(ext) => format!("{stem} ({index}).{ext}"),
            None => format!("{stem} ({index})"),
        };
        let candidate = dir.join(next);
        if !candidate.exists() {
            return candidate;
        }
    }
    dir.join(format!("{}-{}", now_ms(), name))
}

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0)
}

/// 来源索引是「读-改-写」，两个安装同时完成时不能各写各的。
fn origin_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enabled_state_recognizes_supported_extensions() {
        for file in ["a.jar", "a.zip", "a.litemod"] {
            assert_eq!(enabled_state(ContentKind::Mod, file), Some(true), "{file}");
        }
        assert_eq!(
            enabled_state(ContentKind::Mod, "a.jar.disabled"),
            Some(false)
        );
        assert_eq!(enabled_state(ContentKind::Mod, "a.txt"), None);

        for kind in [
            ContentKind::Resourcepack,
            ContentKind::Shader,
            ContentKind::Texturepack,
        ] {
            assert_eq!(enabled_state(kind, "a.zip"), Some(true));
            assert_eq!(enabled_state(kind, "a.zip.disabled"), Some(false));
            // 这几个目录里只有 zip 算包，jar 不能混进来
            assert_eq!(enabled_state(kind, "a.jar"), None);
        }
    }

    #[test]
    fn kind_deserializes_frontend_keys() {
        for (key, expected) in [
            ("mod", ContentKind::Mod),
            ("resourcepack", ContentKind::Resourcepack),
            ("shader", ContentKind::Shader),
            ("texturepack", ContentKind::Texturepack),
        ] {
            let parsed: ContentKind = serde_json::from_str(&format!("\"{key}\"")).unwrap();
            assert_eq!(parsed, expected);
        }
    }

    #[test]
    fn kind_maps_to_its_own_directory_and_index() {
        assert_eq!(ContentKind::Mod.dir_name(), "mods");
        assert_eq!(ContentKind::Mod.index_file(), "mods.json");
        assert_eq!(ContentKind::Resourcepack.dir_name(), "resourcepacks");
        assert_eq!(ContentKind::Resourcepack.index_file(), "resourcepacks.json");
        assert_eq!(ContentKind::Shader.dir_name(), "shaderpacks");
        assert_eq!(ContentKind::Shader.index_file(), "shaderpacks.json");
        assert_eq!(ContentKind::Texturepack.dir_name(), "texturepacks");
    }

    #[test]
    fn legacy_mods_json_still_parses() {
        let text = r#"{"schema":1,"entries":[{"fileName":"a.jar","size":12,
            "sha1":null,"source":"modrinth","projectId":"p","fileId":"f",
            "installedAt":1}]}"#;
        let index: ContentOriginIndex = serde_json::from_str(text).unwrap();
        assert_eq!(index.schema, ORIGIN_SCHEMA);
        assert_eq!(index.entries.len(), 1);
        assert_eq!(index.entries[0].file_name, "a.jar");
        assert_eq!(index.entries[0].source, "modrinth");
    }

    #[test]
    fn parses_pack_mcmeta_string_and_component() {
        let (description, format) =
            parse_pack_mcmeta(br#"{"pack":{"pack_format":15,"description":"hello"}}"#).unwrap();
        assert_eq!(description.as_deref(), Some("hello"));
        assert_eq!(format, Some(15));

        let meta = r#"{"pack":{"pack_format":9,"description":{"text":"component"}}}"#;
        let (description, _) = parse_pack_mcmeta(meta.as_bytes()).unwrap();
        assert_eq!(description.as_deref(), Some("component"));

        assert!(parse_pack_mcmeta(b"{}").is_none());
        assert!(parse_pack_mcmeta(b"not json").is_none());
    }

    #[test]
    fn fallback_name_strips_extension_and_disabled_suffix() {
        assert_eq!(
            display_fallback("example-mod-1.2.3.jar"),
            "example-mod-1.2.3"
        );
        assert_eq!(display_fallback("example-mod.jar.disabled"), "example-mod");
    }

    #[test]
    fn origin_name_ignores_disabled_suffix() {
        assert_eq!(origin_name("example.jar"), "example.jar");
        assert_eq!(origin_name("example.jar.disabled"), "example.jar");
    }

    #[test]
    fn parses_fabric_metadata() {
        let meta = parse_fabric(
            br#"{"id":"example","name":"Example Mod","version":"1.2.3",
                 "description":"hello","authors":["Alice",{"name":"Bob"}],
                 "icon":"icon.png"}"#,
        )
        .unwrap();
        assert_eq!(meta.id.as_deref(), Some("example"));
        assert_eq!(meta.name.as_deref(), Some("Example Mod"));
        assert_eq!(meta.version.as_deref(), Some("1.2.3"));
        assert_eq!(meta.authors, vec!["Alice", "Bob"]);
        assert_eq!(meta.icon_path.as_deref(), Some("icon.png"));
    }

    #[test]
    fn parses_forge_metadata() {
        let meta = parse_forge(
            br#"[[mods]]
modId = "example"
displayName = "Example Mod"
version = "1.2.3"
authors = "Alice, Bob"
logoFile = "logo.png"
"#,
        )
        .unwrap();
        assert_eq!(meta.id.as_deref(), Some("example"));
        assert_eq!(meta.authors, vec!["Alice", "Bob"]);
        assert_eq!(meta.icon_path.as_deref(), Some("logo.png"));
    }

    #[test]
    fn parses_legacy_mcmod_info() {
        let meta = parse_mcmod_info(
            br#"[{"modid":"example","name":"Example","version":"1.2.3",
                  "authorList":["Alice"]}]"#,
        )
        .unwrap();
        assert_eq!(meta.id.as_deref(), Some("example"));
        assert_eq!(meta.authors, vec!["Alice"]);
    }

    #[test]
    fn rejects_path_traversal() {
        let dir = Path::new("mods");
        assert!(safe_child(dir, "../evil.jar").is_err());
        assert!(safe_child(dir, "a/b.jar").is_err());
        assert!(safe_child(dir, "a\\b.jar").is_err());
        assert!(safe_child(dir, "ok.jar").is_ok());
    }

    #[test]
    fn unique_child_avoids_existing_files() {
        let dir = std::env::temp_dir().join(format!("xcl-mod-test-{}", now_ms()));
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("a.jar"), b"x").unwrap();
        let next = unique_child(&dir, "a.jar".to_string());
        assert_eq!(
            next.file_name().and_then(|name| name.to_str()),
            Some("a (2).jar")
        );
        let _ = fs::remove_dir_all(&dir);
    }
}
