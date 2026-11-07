//! Tauri 命令。
//!
//! 约定：
//! - 命令一律是一层转发，不含业务逻辑。
//! - 命名 `<域>_<动作>`，例如 `version_list`、`download_start`。
//! - 返回 `Result<T, AppError>`，`AppError` 是可直接展示的成品文案。

use std::sync::Arc;

use serde::Serialize;
use tauri::{AppHandle, Emitter, State};
use xcl_core::AppError;
use xcl_service::connect::ConnectModuleStatus;
use xcl_service::connect_account::AccountView;
use xcl_service::connect_session::{ConnectStateView, StateSink as ConnectStateSink};
use xcl_service::download::{ProgressSink, TaskView};
use xcl_service::favorites::FavoriteEntry;
use xcl_service::install::InstallPreview;
use xcl_service::java_runtime::JavaComponent as JavaRuntimeComponent;
use xcl_service::launch::{LaunchEvent, LaunchStatus};
use xcl_service::loader::{LoaderListView, LoaderRef};
use xcl_service::memory::{MemoryStats, TrimReport};
use xcl_service::storage::UnusedReport;
use xcl_service::resources::{
    InstallTargetView, LocalTargetView, ModItemView, ModSearchView, ResourceDependencyView,
    ResourceFilesView, ResourceGroupsView, ResourceProjectView,
};
use xcl_service::local::LocalVersion;
use xcl_service::update::{UpdateSink, UpdateView};
use xcl_service::versions::{SourceTestView, VersionListView};
use xcl_service::AppState;

/// 下载进度事件名。前端只监听这一个，不需要知道内部任务结构。
const DOWNLOAD_EVENT: &str = "download://progress";

/// 联机组件状态变化（装好了 / 装失败了）。前端据此切换「下载」与「创建房间」两种界面。
const CONNECT_MODULE_EVENT: &str = "connect://module";

/// 联机会话状态变化（连接中 / 已连接 / 失败）。
const CONNECT_STATE_EVENT: &str = "connect://state";

/// 游戏事件名（日志 + 状态混在一条流里，用 `kind` 区分）。
const LAUNCH_EVENT: &str = "launch://event";

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppInfo {
    pub name: &'static str,
    pub version: &'static str,
    pub target_os: &'static str,
    pub target_arch: &'static str,
    pub debug: bool,
}

/// 应用基础信息。
#[tauri::command]
pub fn app_info() -> AppInfo {
    AppInfo {
        name: "Xin Craft Launcher",
        version: env!("CARGO_PKG_VERSION"),
        target_os: std::env::consts::OS,
        target_arch: std::env::consts::ARCH,
        debug: cfg!(debug_assertions),
    }
}

/// 数据目录布局，供设置页展示与排错。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PathsInfo {
    pub root: String,
    pub versions: String,
    pub libraries: String,
    pub assets: String,
    pub config: String,
    /// 缓存根（`%LOCALAPPDATA%\XCL\cache`，不进便携包）
    pub cache: String,
}

#[tauri::command]
pub fn app_paths(state: State<'_, Arc<AppState>>) -> Result<PathsInfo, AppError> {
    let layout = &state.layout;
    Ok(PathsInfo {
        root: layout.root().display().to_string(),
        versions: layout.versions_dir().display().to_string(),
        libraries: layout.libraries_dir().display().to_string(),
        assets: layout.assets_dir().display().to_string(),
        config: layout.config_file().display().to_string(),
        cache: layout.cache_root().display().to_string(),
    })
}

// ---------- 设置 ----------

/// 读当前配置（含默认值补全后的完整结构）。
#[tauri::command]
pub fn config_get(state: State<'_, Arc<AppState>>) -> Result<xcl_core::Config, AppError> {
    Ok(state.config())
}

/// 局部改配置：只传改动过的字段，其余保持现状（后端递归合并 + 归一 + 落盘）。
///
/// 返回落盘后的完整配置，前端直接覆盖本地状态 —— 钳制后的值以后端为准。
#[tauri::command]
pub fn config_patch(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
    patch: serde_json::Value,
) -> Result<xcl_core::Config, AppError> {
    let next = state.patch_config(patch)?;
    // 日志级别改完立刻生效，不用重启
    crate::logging::set_level(next.log.level);
    // 托盘同理：开关一拨，图标立刻出现/消失
    crate::tray::sync(&app);
    Ok(next)
}

/// 恢复默认配置。只动 `config.toml`，账号、实例、缓存都不碰。
#[tauri::command]
pub fn config_reset(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
) -> Result<xcl_core::Config, AppError> {
    let next = state.reset_config()?;
    crate::logging::set_level(next.log.level);
    crate::tray::sync(&app);
    Ok(next)
}

/// 把设置导出成 TOML 文件。返回保存到的路径；取消返回 `None`。
#[tauri::command]
pub async fn config_export(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
) -> Result<Option<String>, AppError> {
    use tauri_plugin_dialog::DialogExt;

    // 先序列化再弹窗：序列化失败就别白选一次路径
    let text = xcl_service::config::to_toml(&state.config())?;

    let dialog = app
        .dialog()
        .file()
        .add_filter("XCL 设置", &["toml"])
        .set_file_name("xcl-config.toml");
    let picked = tauri::async_runtime::spawn_blocking(move || dialog.blocking_save_file())
        .await
        .map_err(|e| {
            AppError::new(
                xcl_core::ErrorCode::Unknown,
                "打开保存对话框失败",
                e.to_string(),
            )
        })?;

    let Some(path) = picked else {
        return Ok(None);
    };
    let path = path.to_string();
    std::fs::write(&path, text).map_err(|e| AppError::io(&e, "写入设置文件"))?;
    Ok(Some(path))
}

/// 从 TOML 文件导入设置，整份覆盖当前配置。返回导入后的配置；取消返回 `None`。
#[tauri::command]
pub async fn config_import(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
) -> Result<Option<xcl_core::Config>, AppError> {
    use tauri_plugin_dialog::DialogExt;

    let dialog = app.dialog().file().add_filter("XCL 设置", &["toml"]);
    let picked = tauri::async_runtime::spawn_blocking(move || dialog.blocking_pick_file())
        .await
        .map_err(|e| {
            AppError::new(
                xcl_core::ErrorCode::Unknown,
                "打开文件选择器失败",
                e.to_string(),
            )
        })?;

    let Some(path) = picked else {
        return Ok(None);
    };
    let text = std::fs::read_to_string(path.to_string())
        .map_err(|e| AppError::io(&e, "读取设置文件"))?;
    let next = state.update_config(xcl_service::config::from_toml(&text)?)?;
    crate::logging::set_level(next.log.level);
    Ok(Some(next))
}

/// 下载源连通性测试：真拉一次版本清单并解析，返回耗时与命中的源。
#[tauri::command]
pub async fn download_source_test(
    state: State<'_, Arc<AppState>>,
) -> Result<SourceTestView, AppError> {
    state.test_source().await
}

/// 选一张图片当背景：复制进 `data/background/`（只留一张），返回 data URL。
#[tauri::command]
pub async fn background_pick(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
) -> Result<Option<String>, AppError> {
    use tauri_plugin_dialog::DialogExt;

    let dialog = app
        .dialog()
        .file()
        .add_filter("图片", &["png", "jpg", "jpeg", "webp", "bmp", "gif"]);
    let picked = tauri::async_runtime::spawn_blocking(move || dialog.blocking_pick_file())
        .await
        .map_err(|e| {
            AppError::new(
                xcl_core::ErrorCode::Unknown,
                "打开图片选择器失败",
                e.to_string(),
            )
        })?;

    let Some(path) = picked else {
        return Ok(None);
    };
    // 复制 + 清目录是同步 IO，放到阻塞线程里做
    let state = Arc::clone(state.inner());
    let source = std::path::PathBuf::from(path.to_string());
    let url = tauri::async_runtime::spawn_blocking(move || state.set_background(&source))
        .await
        .map_err(|e| {
            AppError::new(
                xcl_core::ErrorCode::Unknown,
                "保存背景图失败",
                e.to_string(),
            )
        })??;
    Ok(Some(url))
}

/// 当前背景图的 data URL（没设置 / 文件丢了都返回 null）。
#[tauri::command]
pub fn background_image(state: State<'_, Arc<AppState>>) -> Result<Option<String>, AppError> {
    state.background_data_url()
}

/// 移除背景图：清空目录并清掉配置。
#[tauri::command]
pub fn background_clear(state: State<'_, Arc<AppState>>) -> Result<(), AppError> {
    state.clear_background()
}

/// 用系统文件管理器打开启动器自己的目录（数据根 / 缓存根）。
#[tauri::command]
pub fn app_open_dir(
    state: State<'_, Arc<AppState>>,
    dir: xcl_service::folders::AppDir,
) -> Result<(), AppError> {
    state.open_app_dir(dir)
}

// ---------- 缓存 ----------

/// 各类缓存的体积。前端只负责格式化。
#[tauri::command]
pub fn storage_info(
    state: State<'_, Arc<AppState>>,
) -> Result<Vec<xcl_service::storage::CacheEntry>, AppError> {
    Ok(state.cache_entries())
}

/// 清理指定缓存。缓存不属于数据，删了只会让下次重新下载。
#[tauri::command]
pub fn storage_clear(
    state: State<'_, Arc<AppState>>,
    kind: xcl_service::storage::CacheKind,
) -> Result<(), AppError> {
    state.cache_clear(kind)
}

/// 版本清单：Rust 侧已完成拉取、解析、分组与排序，并带 TTL 缓存。
/// `force = true` 时绕过缓存（对应界面上的"刷新"）。
#[tauri::command]
pub async fn version_list(
    state: State<'_, Arc<AppState>>,
    force: Option<bool>,
) -> Result<VersionListView, AppError> {
    state.version_list(force.unwrap_or(false)).await
}

/// 安装清单摘要：文件数量与体积，供详情面板展示。
///
/// `loader` 有值时算的是「这个原版 + 这个加载器」的清单（只有加载器自己的库）。
#[tauri::command]
pub async fn install_preview(
    state: State<'_, Arc<AppState>>,
    version_id: String,
    loader: Option<LoaderRef>,
) -> Result<InstallPreview, AppError> {
    state.install_preview(&version_id, loader.as_ref()).await
}

/// 搜索社区资源（Modrinth / CurseForge 互斥，一次只查一个源）。
///
/// `kind` 是资源类型（`mod` / `resourcepack`），只影响平台侧的枚举值；
/// 不传按 Mod 处理。
#[tauri::command]
pub async fn mod_search(
    state: State<'_, Arc<AppState>>,
    source: String,
    query: String,
    offset: usize,
    sort: String,
    game_version: Option<String>,
    loader: Option<String>,
    kind: Option<String>,
) -> Result<ModSearchView, AppError> {
    state
        .mod_search(
            &source,
            &query,
            offset,
            &sort,
            game_version.as_deref(),
            loader.as_deref(),
            kind.as_deref(),
        )
        .await
}

/// 下载页 Mod 的「版本筛选」选项：大版本线，新的在前。
#[tauri::command]
pub async fn mod_version_lines(state: State<'_, Arc<AppState>>) -> Result<Vec<String>, AppError> {
    state.mod_version_lines().await
}

/// 设置页「内存分配」的实时数据：系统已用 / 总量 + 游戏将分配多少。
///
/// 自动模式按「这个版本的 mod 数」算：
/// - 全局设置页不传 `versionId`，用上次选中的版本；
/// - 版本设置页传自己那个版本。
#[tauri::command]
pub async fn memory_stats(
    state: State<'_, Arc<AppState>>,
    version_id: Option<String>,
) -> Result<MemoryStats, AppError> {
    Ok(state.memory_stats(version_id.as_deref()))
}

/// 扫描「没有任何版本引用」的库与资源。只统计，不动文件。
#[tauri::command]
pub async fn storage_unused_scan(
    state: State<'_, Arc<AppState>>,
) -> Result<UnusedReport, AppError> {
    Ok(state.storage_unused_scan_async().await)
}

/// 删掉未被引用的库与资源，返回实际删掉的量。
#[tauri::command]
pub async fn storage_unused_clean(
    state: State<'_, Arc<AppState>>,
) -> Result<UnusedReport, AppError> {
    state.storage_unused_clean_async().await
}

/// 设置 → 高级：手动清一次内存。
///
/// 只清不需要管理员的那一层（各进程不活跃的工作集换进备用列表）。
/// 返回清到了多少个进程、可用内存前后差了多少。
#[tauri::command]
pub async fn memory_trim(state: State<'_, Arc<AppState>>) -> Result<TrimReport, AppError> {
    state.trim_memory().await
}

/// 收藏页的数据：纯本地过滤，不发任何网络请求。
///
/// `kind` 为空表示不筛类型；`query` 同时匹配标题和作者。
#[tauri::command]
pub async fn favorites_list(
    state: State<'_, Arc<AppState>>,
    kind: Option<String>,
    query: Option<String>,
) -> Result<Vec<FavoriteEntry>, AppError> {
    Ok(state.favorites_list(kind.as_deref(), query.as_deref()))
}

/// 收藏 / 取消收藏。返回操作后「是否已收藏」。
///
/// `info` 是卡片快照：收藏时必须带（没有它收藏页画不出东西），
/// 取消时可以不传。
#[tauri::command]
pub async fn favorite_set(
    state: State<'_, Arc<AppState>>,
    source: String,
    kind: String,
    id: String,
    info: Option<ModItemView>,
    on: bool,
) -> Result<bool, AppError> {
    state.favorite_set(&source, &kind, &id, info, on)
}

/// 资源详情页左侧的「项目详情」卡。
#[tauri::command]
pub async fn resource_project(
    state: State<'_, Arc<AppState>>,
    source: String,
    id: String,
    kind: Option<String>,
) -> Result<ResourceProjectView, AppError> {
    state.resource_project(&source, &id, kind.as_deref()).await
}

/// 资源详情页的左侧列表：哪些本地版本能装这个资源。
///
/// 一行 = 一个本地版本（不是远程文件）。空数组表示一个都装不上。
#[tauri::command]
pub async fn resource_targets(
    state: State<'_, Arc<AppState>>,
    source: String,
    id: String,
    kind: Option<String>,
) -> Result<Vec<LocalTargetView>, AppError> {
    state.resource_targets(&source, &id, kind.as_deref()).await
}

/// 侧栏的文件列表。
///
/// - `manual = false`：给 `versionId`，返回那个本地版本能用的文件（新到旧，带默认项）；
/// - `manual = true`：忽略 `versionId`，返回全量（截断到 50 条，`truncated` 会说明）。
#[tauri::command]
pub async fn resource_files(
    state: State<'_, Arc<AppState>>,
    source: String,
    id: String,
    version_id: Option<String>,
    manual: Option<bool>,
    kind: Option<String>,
) -> Result<ResourceFilesView, AppError> {
    state
        .resource_files(
            &source,
            &id,
            version_id.as_deref(),
            manual.unwrap_or(false),
            kind.as_deref(),
        )
        .await
}

/// 手动模式左侧：全量文件，按 MC 版本分组（新的在前）。
#[tauri::command]
pub async fn resource_file_groups(
    state: State<'_, Arc<AppState>>,
    source: String,
    id: String,
) -> Result<ResourceGroupsView, AppError> {
    state.resource_file_groups(&source, &id).await
}

/// 手动模式右栏：本地所有能装的版本（口径跟着 `kind` 走）。
#[tauri::command]
pub async fn resource_install_targets(
    state: State<'_, Arc<AppState>>,
    kind: Option<String>,
) -> Result<Vec<InstallTargetView>, AppError> {
    state.resource_install_targets(kind.as_deref())
}

/// 右栏依赖区块：选中的文件声明了哪些依赖。
///
/// `targetVersionId` 可空：手动模式还没选安装位置时只返回依赖项目/关系，
/// 选了本地版本后再传，才会补具体文件与已安装状态。资源包恒为空。
#[tauri::command]
pub async fn resource_dependencies(
    state: State<'_, Arc<AppState>>,
    source: String,
    id: String,
    file_id: String,
    target_version_id: Option<String>,
    kind: Option<String>,
) -> Result<Vec<ResourceDependencyView>, AppError> {
    state
        .resource_dependencies(
            &source,
            &id,
            &file_id,
            target_version_id.as_deref(),
            kind.as_deref(),
        )
        .await
}

/// 下载一个文件（Mod）。
///
/// `targetVersionId` 与 `targetDir` 二选一：前者落到该版本的游戏目录 `/mods/`，
/// 后者直接用（不再套子目录）。目标已有同名文件时直接报错，不重复下载。
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn resource_install(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
    source: String,
    id: String,
    file_id: String,
    target_version_id: Option<String>,
    target_dir: Option<String>,
    kind: Option<String>,
) -> Result<TaskView, AppError> {
    let state = Arc::clone(state.inner());
    let sink: ProgressSink = Arc::new(move |view| {
        let _ = app.emit(DOWNLOAD_EVENT, &view);
    });
    state
        .resource_install(
            &source,
            &id,
            &file_id,
            target_version_id.as_deref(),
            target_dir.as_deref(),
            kind.as_deref(),
            sink,
        )
        .await
}

/// 装一个整合包。
///
/// 装出来的是一个新实例（`versions/<包名>/`），不需要选目标版本，
/// 所以入参只有「哪个源、哪个项目、哪个文件版本」三样。
#[tauri::command]
pub async fn modpack_install(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
    source: String,
    id: String,
    version_id: String,
) -> Result<TaskView, AppError> {
    let state = Arc::clone(state.inner());
    let sink: ProgressSink = Arc::new(move |view| {
        let _ = app.emit(DOWNLOAD_EVENT, &view);
    });
    state.modpack_install(&source, &id, &version_id, sink).await
}

/// 可选的加载器类型（只返回后端真的能装的）。
#[tauri::command]
pub fn loader_kinds(state: State<'_, Arc<AppState>>) -> Vec<xcl_service::loader::LoaderKindView> {
    state.loader_kinds()
}

/// 加载器可选版本（已按 minor 分组，带「已安装」标记）。
///
/// `game` 传了就顺便算出「这个原版 + 这个加载器」的实例是否存在。
#[tauri::command]
pub async fn loader_versions(
    state: State<'_, Arc<AppState>>,
    kind: String,
    game: Option<String>,
) -> Result<LoaderListView, AppError> {
    state.loader_versions(&kind, game.as_deref()).await
}

/// 开始安装。立刻返回初始任务（总文件数/总体积），进度通过事件推送。
///
/// `loader` 有值时先写加载器 JSON 再走同一条下载链路，返回的任务和普通安装没有区别。
#[tauri::command]
pub async fn install_start(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
    version_id: String,
    loader: Option<LoaderRef>,
) -> Result<TaskView, AppError> {
    let state = Arc::clone(state.inner());
    let sink: ProgressSink = Arc::new(move |view| {
        // 前端可能已经关掉面板，事件发不出去也无所谓，不当作错误
        let _ = app.emit(DOWNLOAD_EVENT, &view);
    });
    match loader {
        Some(loader) => {
            state
                .install_loader(
                    xcl_service::loader::LoaderInstall {
                        game: version_id,
                        loader,
                        instance_id: None,
                        extra_files: Vec::new(),
                        task_id: None,
                    },
                    sink,
                )
                .await
        }
        None => state.start_install(&version_id, None, sink).await,
    }
}

/// 取消下载任务。
#[tauri::command]
pub fn install_cancel(state: State<'_, Arc<AppState>>, task_id: String) -> bool {
    state.cancel_install(&task_id)
}

/// 当前所有下载任务。界面重新挂载时用它恢复进度条。
#[tauri::command]
pub fn install_tasks(state: State<'_, Arc<AppState>>) -> Vec<TaskView> {
    state.install_tasks()
}

/// 忘掉一条已结束的任务（队列里的「删除」）。
#[tauri::command]
pub fn install_forget(state: State<'_, Arc<AppState>>, task_id: String) -> bool {
    state.forget_task(&task_id)
}

// ---------- 本地版本与启动 ----------

/// 本机已安装的版本（纯本地扫描，不联网）。
#[tauri::command]
pub fn installed_versions(state: State<'_, Arc<AppState>>) -> Vec<LocalVersion> {
    xcl_service::local::scan(&state.layout)
}

/// 删掉一个本机版本（只删 `versions/<id>`，共享库与资源不动）。
#[tauri::command]
pub fn version_delete(
    state: State<'_, Arc<AppState>>,
    version_id: String,
) -> Result<(), AppError> {
    state.delete_version(&version_id)
}

/// 改版本名（目录名 + JSON id + 子版本 inheritsFrom 一起改），返回新名字。
#[tauri::command]
pub fn version_rename(
    state: State<'_, Arc<AppState>>,
    version_id: String,
    new_id: String,
) -> Result<String, AppError> {
    state.rename_version(&version_id, &new_id)
}

/// 写版本描述，空串等于清掉。
#[tauri::command]
pub fn version_set_description(
    state: State<'_, Arc<AppState>>,
    version_id: String,
    description: String,
) -> Result<(), AppError> {
    state.set_version_description(&version_id, &description)
}

/// 收藏 / 取消收藏。
#[tauri::command]
pub fn version_set_favorite(
    state: State<'_, Arc<AppState>>,
    version_id: String,
    favorite: bool,
) -> Result<(), AppError> {
    state.set_version_favorite(&version_id, favorite)
}

/// 单个版本的启动设置：覆盖项、最终生效值、全局值。
#[tauri::command]
pub fn version_settings_get(
    state: State<'_, Arc<AppState>>,
    version_id: String,
) -> Result<xcl_service::version_settings::VersionSettingsView, AppError> {
    state.version_settings(&version_id)
}

/// 改单个版本的设置：只传要改的字段，`null` = 恢复跟随全局。
#[tauri::command]
pub fn version_settings_patch(
    state: State<'_, Arc<AppState>>,
    version_id: String,
    patch: serde_json::Value,
) -> Result<xcl_service::version_settings::VersionSettingsView, AppError> {
    state.patch_version_settings(&version_id, &patch)
}

/// 版本占用与内容统计（目录大小、Mod / 存档 / 截图数量、上次启动）。
#[tauri::command]
pub fn version_stats(
    state: State<'_, Arc<AppState>>,
    version_id: String,
) -> Result<xcl_service::version_settings::VersionStats, AppError> {
    state.version_stats(&version_id)
}

/// 存自定义图标（PNG data URL，前端已缩到 128×128）。
#[tauri::command]
pub fn version_set_icon(
    state: State<'_, Arc<AppState>>,
    version_id: String,
    data_url: String,
) -> Result<(), AppError> {
    state.set_version_icon(&version_id, &data_url)
}

/// 恢复默认图标。
#[tauri::command]
pub fn version_clear_icon(
    state: State<'_, Arc<AppState>>,
    version_id: String,
) -> Result<(), AppError> {
    state.clear_version_icon(&version_id)
}

/// 某个版本的隔离状态、原因与游戏目录。
#[tauri::command]
pub fn version_isolation(
    state: State<'_, Arc<AppState>>,
    version_id: String,
) -> Result<xcl_service::isolation::IsolationView, AppError> {
    state.version_isolation(&version_id)
}

/// 手动设置隔离；`isolate = null` 表示恢复跟随全局策略。
#[tauri::command]
pub fn version_set_isolation(
    state: State<'_, Arc<AppState>>,
    version_id: String,
    isolate: Option<bool>,
) -> Result<xcl_service::isolation::IsolationView, AppError> {
    state.set_version_isolation(&version_id, isolate)
}

/// 用系统文件管理器打开版本相关的文件夹。
#[tauri::command]
pub fn version_open_folder(
    state: State<'_, Arc<AppState>>,
    version_id: String,
    folder: xcl_service::folders::VersionFolder,
) -> Result<(), AppError> {
    state.open_version_folder(&version_id, folder)
}

/// 本机可用的 Java 列表。设置页展示 + 排错用。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JavaInfo {
    pub path: String,
    pub version: String,
    pub major: u32,
}

#[tauri::command]
pub async fn java_list(
    state: State<'_, Arc<AppState>>,
) -> Result<Vec<JavaInfo>, AppError> {
    // 系统里的 + 我们下载到 data/java/ 的，都要列出来
    let roots = state.java_runtime_roots();
    Ok(xcl_launch::detect_all_with(&roots)
        .await
        .into_iter()
        .map(|java| JavaInfo {
            path: java.path.display().to_string(),
            version: java.version,
            major: java.major,
        })
        .collect())
}

/// 本机探测到的所有 Java（含 `data/java/` 下我们自己下的）。
///
/// 与 `java_list` 的区别：这里带 `managed` 标记，前端能区分
/// 「自己装的」和「XCL 下到 data/java 的」。
#[tauri::command]
pub async fn java_detected(
    state: State<'_, Arc<AppState>>,
) -> Result<Vec<xcl_service::java_runtime::SystemJava>, AppError> {
    Ok(state.java_detected().await)
}

/// 可下载的 Java 运行时列表（Mojang 官方 java-runtime）。
#[tauri::command]
pub async fn java_components(
    state: State<'_, Arc<AppState>>,
) -> Result<Vec<JavaRuntimeComponent>, AppError> {
    state.java_components().await
}

/// 下载并安装一个 Java 运行时。进度复用 `download://progress`。
#[tauri::command]
pub async fn java_install(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
    component: String,
) -> Result<TaskView, AppError> {
    let state = Arc::clone(state.inner());
    let sink: ProgressSink = Arc::new(move |view| {
        let _ = app.emit(DOWNLOAD_EVENT, &view);
    });
    state.java_install(&component, sink).await
}

/// 删除一个已安装的 Java 运行时。
#[tauri::command]
pub fn java_remove(state: State<'_, Arc<AppState>>, component: String) -> Result<(), AppError> {
    state.java_remove(&component)
}

/// 这个版本缺 Java 时，推荐一个可下载的组件；不缺就返回 null。
#[tauri::command]
pub async fn java_suggest(
    state: State<'_, Arc<AppState>>,
    version_id: String,
) -> Result<Option<JavaRuntimeComponent>, AppError> {
    Ok(state.java_suggest(&version_id).await)
}

// ---------- 账号 ----------

/// 所有账号（当前账号排最前）。脱敏视图，不含密码与 token。
#[tauri::command]
pub fn account_list(state: State<'_, Arc<AppState>>) -> Vec<xcl_service::auth::AccountView> {
    state.accounts()
}

/// 添加离线账号并切为当前。
#[tauri::command]
pub fn account_add_offline(
    state: State<'_, Arc<AppState>>,
    name: String,
) -> Result<xcl_service::auth::AccountView, AppError> {
    state.add_offline_account(&name)
}

/// 登录第三方账号（Yggdrasil / authlib-injector）。
///
/// 多角色账号且没传 `profileId` 时返回 `NeedProfile`，前端选一个后
/// 带着 `profileId` 再调一次。
#[tauri::command]
pub async fn account_add_authlib(
    state: State<'_, Arc<AppState>>,
    server_url: String,
    email: String,
    password: String,
    profile_id: Option<String>,
    remember_password: Option<bool>,
) -> Result<xcl_service::auth::AuthlibLoginOutcome, AppError> {
    let outcome = state
        .add_authlib_account(
            &server_url,
            &email,
            &password,
            profile_id.as_deref(),
            remember_password.unwrap_or(false),
        )
        .await?;
    // 账号已入库；登录组件在后台预取。失败只记日志，启动前会再保证一次。
    if matches!(
        &outcome,
        xcl_service::auth::AuthlibLoginOutcome::LoggedIn(_)
    ) {
        let state = Arc::clone(state.inner());
        let _ = tauri::async_runtime::spawn(async move {
            if let Err(err) = state.prefetch_authlib_jar().await {
                tracing::warn!(error = %err, "预下载 authlib-injector 失败，启动时会重试");
            }
        });
    }
    Ok(outcome)
}

/// 设置离线账号的默认皮肤模型（Steve / Alex）。
#[tauri::command]
pub fn account_set_skin_model(
    state: State<'_, Arc<AppState>>,
    id: String,
    model: xcl_service::skin::SkinModel,
) -> Result<xcl_service::auth::AccountView, AppError> {
    state.set_offline_skin_model(&id, model)
}

/// 切换当前账号。
#[tauri::command]
pub fn account_set_active(
    state: State<'_, Arc<AppState>>,
    id: String,
) -> Result<xcl_service::auth::AccountView, AppError> {
    state.set_active_account(&id)
}

/// 刷新第三方账号的 token（需要登录时选了「记住密码」）。
#[tauri::command]
pub async fn account_refresh(
    state: State<'_, Arc<AppState>>,
    id: String,
) -> Result<xcl_service::auth::AccountView, AppError> {
    state.refresh_authlib_account(&id).await
}

/// 删除账号（会先在服务器侧登出，失败也删）。
#[tauri::command]
pub async fn account_remove(state: State<'_, Arc<AppState>>, id: String) -> Result<(), AppError> {
    state.remove_account(&id).await
}

// ---------- 皮肤 ----------

/// 当前（或指定）账号的皮肤渲染数据，供前端 3D 渲染。
///
/// 位图是 base64（PNG），前端直接拼 dataURL。拉取失败会自动回落到内置
/// 默认皮肤，所以这个命令不会因为网络问题失败。
#[tauri::command]
pub async fn skin_current(
    state: State<'_, Arc<AppState>>,
    account_id: Option<String>,
) -> Result<xcl_service::skin::SkinRender, AppError> {
    Ok(state.skin_render(account_id.as_deref()).await)
}

/// 启动游戏。立刻返回 `running` 快照，之后日志与终态走 `launch://event`。
#[tauri::command]
pub async fn launch_start(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
    version_id: String,
) -> Result<LaunchStatus, AppError> {
    let state = Arc::clone(state.inner());
    // 缺 Java 时启动链路会自己开下载，那个任务也要推给统一的任务列表
    let app_for_downloads = app.clone();
    let sink: xcl_service::launch::LaunchSink = Arc::new(move |event: LaunchEvent| {
        let _ = app.emit(LAUNCH_EVENT, &event);
    });
    let downloads: ProgressSink = Arc::new(move |view| {
        let _ = app_for_downloads.emit(DOWNLOAD_EVENT, &view);
    });
    state.launch_start(&version_id, sink, downloads).await
}

/// 结束正在运行的游戏。参数可以是会话 id，也可以是版本 id。
#[tauri::command]
pub fn launch_stop(state: State<'_, Arc<AppState>>, target: String) -> bool {
    state.launch_stop(&target)
}

/// 当前在跑的会话。界面重新挂载时用它恢复状态。
#[tauri::command]
pub fn launch_sessions(state: State<'_, Arc<AppState>>) -> Vec<LaunchStatus> {
    state.launch_sessions()
}

/// 选一个文件夹（手动安装的「下载到自定义文件夹」）。
///
/// 用 `tauri-plugin-dialog` 的阻塞接口 + `spawn_blocking`：系统对话框本来
/// 就是阻塞的，放到阻塞线程里等不会卡住 Tauri 的异步运行时。
/// 只在 Rust 侧用它，所以 capabilities 不用加 `dialog:*` 权限。
#[tauri::command]
pub async fn pick_folder(app: AppHandle) -> Result<Option<String>, AppError> {
    use tauri_plugin_dialog::DialogExt;

    let dialog = app.dialog().file();
    let picked = tauri::async_runtime::spawn_blocking(move || dialog.blocking_pick_folder())
        .await
        .map_err(|e| {
            AppError::new(
                xcl_core::ErrorCode::Unknown,
                "打开文件夹选择器失败",
                e.to_string(),
            )
        })?;

    let Some(path) = picked else {
        return Ok(None);
    };
    Ok(Some(path.to_string()))
}

/// 回显，用于确认命令调用链路正常。
#[tauri::command]
pub fn ping(message: String) -> String {
    format!("pong: {message}")
}

// ---------- 自更新 ----------

/// 更新状态事件名。前端只监听这一条，整份视图一次推过去。
const UPDATE_EVENT: &str = "update://state";

/// 当前更新状态。界面重新挂载时用它恢复对话框。
#[tauri::command]
pub fn update_status(state: State<'_, Arc<AppState>>) -> UpdateView {
    state.update_view()
}

/// 拉清单、验签、比版本。不写任何文件（强制更新状态除外，它必须落盘）。
///
/// `auto` = 启动时的自动检查：跳过过的版本不再打扰。
#[tauri::command]
pub async fn update_check(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
    auto: Option<bool>,
) -> Result<UpdateView, AppError> {
    let state = Arc::clone(state.inner());
    state
        .update_check(auto.unwrap_or(false), update_sink(app))
        .await
}

/// 忽略某个版本。之后自动检查不再提它（强制更新不受影响）。
#[tauri::command]
pub fn update_skip(
    state: State<'_, Arc<AppState>>,
    version: String,
) -> Result<UpdateView, AppError> {
    state.update_skip(&version)
}

/// 下载更新包到安装目录。后台跑，进度走 `update://state`。
#[tauri::command]
pub async fn update_download(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
) -> Result<UpdateView, AppError> {
    let state = Arc::clone(state.inner());
    state.update_download(update_sink(app)).await
}

/// 取消下载。只有正在下载/校验时才有效。
#[tauri::command]
pub fn update_cancel(state: State<'_, Arc<AppState>>) -> bool {
    state.update_cancel()
}

/// 重启并更新。
///
/// helper 起来之后必须尽快退出主进程 —— 它在等这个 PID。所以这里
/// 先返回给前端（让"正在重启"的提示能画出来），再延迟一点退出。
/// 不能用 `std::process::exit`：那会跳过窗口状态保存和配置落盘。
#[tauri::command]
pub async fn update_apply(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
) -> Result<(), AppError> {
    let state = Arc::clone(state.inner());
    state.update_apply().await?;

    let handle = app.clone();
    std::thread::spawn(move || {
        // 300ms 够 IPC 把返回值送到前端；再长 helper 那边就开始等得不耐烦了
        std::thread::sleep(std::time::Duration::from_millis(300));
        handle.exit(0);
    });
    Ok(())
}

fn update_sink(app: AppHandle) -> UpdateSink {
    Arc::new(move |view| {
        // 前端可能已经关掉对话框，事件发不出去也无所谓
        let _ = app.emit(UPDATE_EVENT, &view);
    })
}

// ---------------------------------------------------------------------------
// 联机组件
// ---------------------------------------------------------------------------

/// 联机组件是否就绪。前端据此决定「只显示下载」还是「显示创建/加入房间」。
#[tauri::command]
pub fn connect_module_status(state: State<'_, Arc<AppState>>) -> ConnectModuleStatus {
    state.connect_module_status()
}

/// 扫本机 Minecraft 正在监听的端口。
#[tauri::command]
pub fn connect_scan_port() -> Vec<u16> {
    xcl_service::connect::scan_minecraft_ports()
}

/// 起房主。房间名与密码必填；`port` 是房主游戏的端口，会编进邀请码（0 = 还没扫到）。
#[tauri::command]
pub async fn connect_start_host(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
    room: String,
    secret: String,
    port: u16,
) -> Result<ConnectStateView, AppError> {
    let state = Arc::clone(state.inner());
    let sink = connect_state_sink(app);
    xcl_service::connect_session::start_host(&state, room, secret, port, sink).await
}

/// 起加入方。端口在邀请码里，不需要额外参数。
#[tauri::command]
pub async fn connect_start_guest(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
    code: String,
) -> Result<ConnectStateView, AppError> {
    let state = Arc::clone(state.inner());
    let sink = connect_state_sink(app);
    xcl_service::connect_session::start_guest(&state, &code, sink).await
}

/// 房主扫到 / 手填了游戏端口：重编邀请码并推给界面。
#[tauri::command]
pub async fn connect_set_port(port: u16) -> Result<ConnectStateView, AppError> {
    xcl_service::connect_session::set_host_port(port).await
}

fn connect_state_sink(app: AppHandle) -> ConnectStateSink {
    Arc::new(move |view| {
        let _ = app.emit(CONNECT_STATE_EVENT, &view);
    })
}

/// 停掉联机会话。
#[tauri::command]
pub async fn connect_stop(app: AppHandle) -> ConnectStateView {
    let view = xcl_service::connect_session::stop().await;
    let _ = app.emit(CONNECT_STATE_EVENT, &view);
    view
}

/// 取当前状态（切页回来时补一次）。
#[tauri::command]
pub async fn connect_state() -> ConnectStateView {
    xcl_service::connect_session::current_state().await
}

/// 联机模块的日志（尾部 256 KB）。排障材料：前端弹窗展示 + 一键复制。
#[tauri::command]
pub fn connect_log(state: State<'_, Arc<AppState>>) -> Result<String, AppError> {
    xcl_service::connect_session::read_log_tail(state.inner().as_ref(), 256 * 1024)
}

/// 下载并安装联机组件。
///
/// 进度走下载球那套事件，装完/失败再补一条状态事件 —— 前端只认 `ready`。
#[tauri::command]
pub async fn connect_module_install(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
) -> Result<TaskView, AppError> {
    let state = Arc::clone(state.inner());

    let sink: ProgressSink = {
        let app = app.clone();
        Arc::new(move |view| {
            let _ = app.emit(DOWNLOAD_EVENT, &view);
        })
    };
    let status_sink: Arc<dyn Fn(ConnectModuleStatus) + Send + Sync> = Arc::new(move |status| {
        let _ = app.emit(CONNECT_MODULE_EVENT, &status);
    });

    state.connect_module_install(sink, status_sink).await
}

// ---------- 联机账号（进节点的账号，不是 Minecraft 账号） ----------

/// 当前登录状态。联机页打开时先问一次，决定显示登录卡还是创建/加入。
#[tauri::command]
pub fn connect_account_state(state: State<'_, Arc<AppState>>) -> AccountView {
    xcl_service::connect_account::state(state.inner())
}

/// 登录。成功即落盘，前端拿到的是脱敏后的视图。
#[tauri::command]
pub async fn connect_account_login(
    state: State<'_, Arc<AppState>>,
    username: String,
    password: String,
) -> Result<AccountView, AppError> {
    let state = Arc::clone(state.inner());
    xcl_service::connect_account::login(&state, &username, &password).await
}

/// 注册。内部会领一道人机验证题、算完再提交，耗时约 0.3~1 秒。
#[tauri::command]
pub async fn connect_account_register(
    state: State<'_, Arc<AppState>>,
    username: String,
    password: String,
) -> Result<AccountView, AppError> {
    let state = Arc::clone(state.inner());
    xcl_service::connect_account::register(&state, &username, &password).await
}

/// 登出。本地一定清干净，服务端那一步失败也不影响。
#[tauri::command]
pub async fn connect_account_logout(state: State<'_, Arc<AppState>>) -> Result<(), AppError> {
    let state = Arc::clone(state.inner());
    xcl_service::connect_account::logout(&state).await
}

/// 向服务端确认登录态是否还有效，顺便刷新过期时间。
#[tauri::command]
pub async fn connect_account_refresh(
    state: State<'_, Arc<AppState>>,
) -> Result<AccountView, AppError> {
    let state = Arc::clone(state.inner());
    xcl_service::connect_account::refresh(&state).await
}

/// 向服务端确认登录态（只读，不延长有效期）。
///
/// 桥接对未授权的连接一个字节都不回，所以「我的登录还在不在」只能主动问。
/// 进联机页时调一次：401 就把本地清掉，界面自然回到登录卡。
#[tauri::command]
pub async fn connect_account_check(
    state: State<'_, Arc<AppState>>,
) -> Result<AccountView, AppError> {
    let state = Arc::clone(state.inner());
    xcl_service::connect_account::check(&state).await
}
