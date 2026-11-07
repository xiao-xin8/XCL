//! 安装执行层：把 `InstallPlan` 变成真实的并发下载。
//!
//! 职责划分：
//! - `install.rs` 只算「要下什么」，不碰网络
//! - `xcl-download` 只管「怎么并发下」，不认识版本概念
//! - 本模块是两者的粘合层：算候选地址、建任务、采样进度、汇总结果
//!
//! 进度在这里被加工成可直接渲染的视图模型（速度、剩余时间、阶段、
//! 失败数量都已算好），前端不做任何计算。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::Serialize;
use tokio_util::sync::CancellationToken;
use xcl_core::AppError;
use xcl_download::{DownloadItem, DownloadOptions, Fetcher, Progress};
use xcl_source::UrlKind;

use crate::install::{FileKind, InstallPlan};
use crate::AppState;

/// 进度采样间隔。
///
/// 150ms（6.7 次/秒）时界面每帧都在重画，下载时动画明显掉帧；
/// 250ms（4 次/秒）看起来仍然连续，但把重绘压力砍掉近一半。
/// 再慢就会「一顿一顿」了。
pub(crate) const SAMPLE_MS: u64 = 250;

/// 失败项最多回传几条 —— 全量传过去没意义，界面也放不下。
const MAX_FAILURES: usize = 5;

/// 进度回调。Tauri 层用它推事件；测试里可以直接收集到 Vec。
pub type ProgressSink = Arc<dyn Fn(TaskView) + Send + Sync>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum TaskPhase {
    Running,
    Done,
    Failed,
    Cancelled,
}

impl TaskPhase {
    fn is_finished(self) -> bool {
        !matches!(self, TaskPhase::Running)
    }
}

/// 前端拿到的任务视图。所有派生量都已算好（速度、剩余时间、百分比）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskView {
    pub task_id: String,
    pub version_id: String,
    /// 界面上显示的名字。版本安装是 `None`（前端直接用 `version_id`），
    /// 单文件下载（Mod）给文件名 —— 否则抽屉里会显示成 `mod:xxx` 这种内部 id。
    pub title: Option<String>,
    /// 加载器安装时指回父版本（原版）的 id。
    ///
    /// 加载器任务的 `version_id` 是 `fabric-loader-x-y` 这种新实例 id，
    /// 而界面是从父版本详情里发起安装的 —— 没有这个字段，详情面板就找不到
    /// 自己刚发起的那个任务，进度条永远不出现。
    pub parent_id: Option<String>,
    /// 单文件下载（Mod）落到哪：给了目标版本就是实例 id，自定义文件夹时是目录。
    ///
    /// 版本安装是 `None`。同一个文件装到两个实例是两条独立任务，
    /// 界面靠「版本 id + 目标」这一对把它们的进度分开算。
    pub target: Option<String>,
    pub phase: TaskPhase,
    pub total_files: usize,
    pub finished_files: usize,
    pub total_bytes: u64,
    pub done_bytes: u64,
    /// 0.0 ~ 1.0，按字节算（按文件数会抖）
    pub fraction: f32,
    /// 字节 / 秒
    pub speed_bps: u64,
    /// 预计剩余秒数；速度未知时为 None
    pub eta_seconds: Option<u64>,
    pub downloaded: usize,
    pub skipped: usize,
    pub failed: usize,
    /// 失败项名称，最多 [`MAX_FAILURES`] 条
    pub failures: Vec<String>,
    /// 结束时的补充说明（失败原因 / 取消说明）
    pub message: Option<String>,
    pub started_at_ms: u64,
}

impl TaskView {
    pub(crate) fn new(task_id: &str, version_id: &str, total_files: usize, total_bytes: u64) -> Self {
        Self {
            task_id: task_id.to_string(),
            version_id: version_id.to_string(),
            title: None,
            parent_id: None,
            target: None,
            phase: TaskPhase::Running,
            total_files,
            finished_files: 0,
            total_bytes,
            done_bytes: 0,
            fraction: 0.0,
            speed_bps: 0,
            eta_seconds: None,
            downloaded: 0,
            skipped: 0,
            failed: 0,
            failures: Vec::new(),
            message: None,
            started_at_ms: now_ms(),
        }
    }
}

// ---------- 任务表 ----------

#[derive(Default)]
pub struct DownloadTasks {
    inner: Mutex<HashMap<String, TaskRecord>>,
}

struct TaskRecord {
    cancel: CancellationToken,
    view: TaskView,
}

impl DownloadTasks {
    pub(crate) fn insert(&self, task_id: String, cancel: CancellationToken, view: TaskView) {
        let mut map = self.lock();
        // 只保留最近若干条已完成记录，避免长时间运行后无限增长
        if map.len() >= 20 {
            let mut finished: Vec<(String, u64)> = map
                .iter()
                .filter(|(_, r)| r.view.phase.is_finished())
                .map(|(k, r)| (k.clone(), r.view.started_at_ms))
                .collect();
            finished.sort_by_key(|(_, at)| *at);
            for (key, _) in finished {
                map.remove(&key);
                if map.len() < 20 {
                    break;
                }
            }
        }
        map.insert(task_id, TaskRecord { cancel, view });
    }

    pub(crate) fn update(&self, view: TaskView) {
        if let Some(record) = self.lock().get_mut(&view.task_id) {
            record.view = view;
        }
    }

    /// 取一条任务的最新快照。给「等这个任务跑完」的场景用（启动时缺 Java）。
    pub(crate) fn get(&self, task_id: &str) -> Option<TaskView> {
        self.lock().get(task_id).map(|record| record.view.clone())
    }

    pub(crate) fn cancel(&self, task_id: &str) -> bool {
        match self.lock().get(task_id) {
            Some(record) if !record.view.phase.is_finished() => {
                record.cancel.cancel();
                true
            }
            _ => false,
        }
    }

    /// 从任务表里移除一条已结束的任务。还在跑的不让删，先取消。
    pub fn remove(&self, task_id: &str) -> bool {
        let mut map = self.lock();
        match map.get(task_id) {
            Some(record) if record.view.phase.is_finished() => {
                map.remove(task_id);
                true
            }
            _ => false,
        }
    }

    /// 同一版本已在下载中时直接复用，避免重复建任务。
    pub(crate) fn running_for(&self, version_id: &str) -> Option<TaskView> {
        self.lock()
            .values()
            .find(|r| r.view.version_id == version_id && !r.view.phase.is_finished())
            .map(|r| r.view.clone())
    }

    pub fn list(&self) -> Vec<TaskView> {
        let mut views: Vec<TaskView> = self.lock().values().map(|r| r.view.clone()).collect();
        views.sort_by_key(|v| std::cmp::Reverse(v.started_at_ms));
        views
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, TaskRecord>> {
        // 锁内不会 panic 的代码，中毒也没关系
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }
}

// ---------- 对外接口 ----------

impl AppState {
    /// 开始安装某个版本。
    ///
    /// 立刻返回初始任务视图（含总文件数与总体积），真正的下载在后台跑，
    /// 进度通过 `emit` 回调推出去。
    ///
    /// `parent` 只在装加载器时给：任务的 `version_id` 会是新实例的 id，
    /// 界面靠 `parent_id` 认出「这条任务是那个原版详情里发起的」。
    pub async fn start_install(
        self: &Arc<Self>,
        version_id: &str,
        parent: Option<&str>,
        emit: ProgressSink,
    ) -> Result<TaskView, AppError> {
        if let Some(running) = self.tasks().running_for(version_id) {
            return Ok(running);
        }
        let plan = self.build_plan(version_id).await?;
        // 原版版本：把「它属于哪个原版」也记一份（= 它自己）。
        // 猜不出来的名字（改过名）不写，避免把错误值覆盖上去。
        if let Some(game) = crate::local::guess_game_version(version_id, None) {
            let _ = crate::isolation::ensure_game_version(&self.layout, version_id, &game);
        }
        self.start_install_plan(plan, parent, emit).await
    }

    /// 用已经算好的清单开始安装。
    ///
    /// 给加载器安装用：那份清单是合并且按「原版是否已装」优化过的，
    /// 让引擎再 `build_plan` 一次会按自包含 JSON 重新解析，把一千多个
    /// 资源文件全算进来 —— 「秒装」会变成「转半天」。
    pub(crate) async fn start_install_plan(
        self: &Arc<Self>,
        plan: InstallPlan,
        parent: Option<&str>,
        emit: ProgressSink,
    ) -> Result<TaskView, AppError> {
        let task_id = new_task_id();
        self.start_install_plan_as(plan, parent, task_id, emit).await
    }

    /// 用调用方指定的任务 id 开始下载。
    ///
    /// 给 Forge / NeoForge 用：它们要先下载安装器、跑安装器，再走统一下载链路。
    /// 整段用同一个任务 id，界面上就是一张卡从头走到尾，而不是先冒一张"准备中"
    /// 再换一张"下载中"。
    ///
    /// 不做重复任务检查：调用方在准备阶段已经把自己插进任务表了，
    /// 这里再按版本号查一次会查到它自己。
    pub(crate) async fn start_install_plan_as(
        self: &Arc<Self>,
        plan: InstallPlan,
        parent: Option<&str>,
        task_id: String,
        emit: ProgressSink,
    ) -> Result<TaskView, AppError> {
        let version_id = plan.version_id.clone();

        // 下载文件走 download_strategy（与拉列表的策略分开）
        let policy = self.download_policy();

        // 清单里的 URL 是官方地址，这里按源策略展开成候选列表。
        // 展开放在下载前而不是清单里，是为了让"换源"立刻生效、不用重建清单。
        let mut items: Vec<DownloadItem> = Vec::with_capacity(plan.files.len());
        for file in &plan.files {
            let urls = policy.candidates(url_kind(file.kind), &file.url);
            if urls.is_empty() {
                continue;
            }
            let split_urls = split_urls_for(&urls);
            items.push(DownloadItem {
                name: display_name(&file.local),
                urls,
                split_urls,
                dest: file.local.clone(),
                sha1: file.sha1.clone(),
                size: file.size,
            });
        }

        self.spawn_task(items, &version_id, None, None, parent, Some(task_id), emit)
            .await
    }

    /// 把「已经算好的一批文件」变成真实任务。
    ///
    /// 从 `start_install_plan` 里抽出来的：社区资源（Mod）只是一个文件，
    /// 没必要为了它伪造一份版本安装清单。
    ///
    /// 这里不做 `running_for` 去重 —— 版本安装要（重复点「安装」不该起两个
    /// 一模一样的任务），单文件下载不要（同一个 mod 装到两个实例是两个独立任务）。
    pub(crate) async fn spawn_task(
        self: &Arc<Self>,
        items: Vec<DownloadItem>,
        version_id: &str,
        title: Option<String>,
        target: Option<String>,
        parent: Option<&str>,
        task_id: Option<String>,
        emit: ProgressSink,
    ) -> Result<TaskView, AppError> {
        if items.is_empty() {
            return Err(AppError::unsupported(
                "没有需要下载的文件",
                format!("{version_id} 的安装清单是空的"),
            )
            .with_hint("试试刷新版本清单后重试"));
        }

        let total_files = items.len();
        let total_bytes: u64 = items.iter().map(|i| i.size).sum();
        // 加载器安装会把自己的任务 id 传进来，好让「准备阶段」和「下载阶段」
        // 共用同一张卡；其余场景照旧现取一个。
        let task_id = task_id.unwrap_or_else(new_task_id);
        let cancel = CancellationToken::new();
        let progress = Progress::new(total_bytes, total_files);

        let initial = TaskView {
            title,
            target,
            parent_id: parent.map(|p| p.to_string()),
            ..TaskView::new(&task_id, version_id, total_files, total_bytes)
        };
        self.tasks().insert(task_id.clone(), cancel.clone(), initial.clone());
        emit(initial.clone());

        let state = Arc::clone(self);
        // 并发数跟着最终选中的源走：官方 64、镜像 16（见 `auto_concurrency`）
        let options = DownloadOptions {            concurrency: state.effective_concurrency(),
            // 重试次数是「失败后再试几次」，引擎要的是总数，所以 +1
            attempts_per_url: state.config().download.retries.saturating_add(1),
            // 单文件多连接也按源开关：BMCLAPI 会被压成单连接
            split: state.split_config(),
        };
        // 限速：每次任务开始时按当前配置设一次。0 = 不限速，等于这段没有。
        state
            .fetcher()
            .set_speed_limit(state.config().download.speed_limit_kbps as u64 * 1024);
        let fetcher = Arc::clone(state.fetcher());

        tokio::spawn(async move {
            run_task(
                state, task_id, items, fetcher, progress, options, cancel, emit,
            )
            .await;
        });

        Ok(initial)
    }

    /// 下载一个文件到指定路径（社区资源用）。
    ///
    /// `task_version` 是任务归属的标识（不进界面，抽屉显示的是 `title`）：
    /// 用 `mod:<fileId>` 这种形式，就不会和版本安装任务的 id 撞上，
    /// 也不会被版本详情的 `forVersion` 误认成自己发起的安装。
    ///
    /// `target` 是落点（目标版本 id 或自定义目录），和 `task_version` 一起
    /// 唯一确定一条「这个文件装到哪儿」的下载。
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn start_file_install(
        self: &Arc<Self>,
        kind: FileKind,
        dest: std::path::PathBuf,
        url: &str,
        sha1: Option<String>,
        size: u64,
        task_version: &str,
        title: String,
        target: String,
        emit: ProgressSink,
    ) -> Result<TaskView, AppError> {
        let policy = self.download_policy();
        let urls = policy.candidates(url_kind(kind), url);
        if urls.is_empty() {
            return Err(AppError::unsupported(
                "这个文件没有可用的下载地址",
                url.to_string(),
            ));
        }

        let split_urls = split_urls_for(&urls);
        let items = vec![DownloadItem {
            name: display_name(&dest),
            urls,
            split_urls,
            dest,
            sha1,
            size,
        }];

        self.spawn_task(
            items,
            task_version,
            Some(title),
            Some(target),
            None,
            None,
            emit,
        )
        .await
    }

    /// 取消任务。返回是否真的发出了取消（已完成的返回 false）。
    pub fn cancel_install(&self, task_id: &str) -> bool {
        self.tasks().cancel(task_id)
    }

    /// 当前所有任务，最近的在前。界面重新挂载时用它恢复进度条。
    pub fn install_tasks(&self) -> Vec<TaskView> {
        self.tasks().list()
    }

    /// 忘掉一条已结束的任务（队列里的「删除」）。返回是否真的删掉了。
    pub fn forget_task(&self, task_id: &str) -> bool {
        self.tasks().remove(task_id)
    }
}

// ---------- 内部执行 ----------

#[allow(clippy::too_many_arguments)]
pub(crate) async fn run_task(
    state: Arc<AppState>,
    task_id: String,
    items: Vec<DownloadItem>,
    fetcher: Arc<dyn Fetcher>,
    progress: Arc<Progress>,
    options: DownloadOptions,
    cancel: CancellationToken,
    emit: ProgressSink,
) {
    let download = xcl_download::run(
        items,
        fetcher,
        Arc::clone(&progress),
        options,
        cancel.clone(),
    );
    tokio::pin!(download);

    let mut ticker = tokio::time::interval(Duration::from_millis(SAMPLE_MS));
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    // 速度用 4 秒滑动窗口 + 指数平滑算。采样间隔的瞬时差值会把「数据块到达的
    // 抖动」原样放大成「800KB/s → 0 → 1.2MB/s」，界面看起来就一直在抖。
    let mut meter = crate::speed::SpeedMeter::new(crate::speed::DEFAULT_WINDOW);
    // 上一次真正推给界面的 `(已下字节, 速度)`：没变化就不推。
    // 卡住的连接、末尾几个小文件都会让这个值停住，此时每秒 4 次的空更新
    // 纯属白让界面重画。
    let mut last_pushed: Option<(u64, u64)> = None;
    let mut view = state
        .tasks()
        .list()
        .into_iter()
        .find(|v| v.task_id == task_id)
        .unwrap_or_else(|| TaskView::new(&task_id, "", 0, 0));

    let summary = loop {
        tokio::select! {
            summary = &mut download => break summary,
            _ = ticker.tick() => {
                let snapshot = progress.snapshot();
                let speed = meter.update(snapshot.done_bytes, Instant::now());

                apply_snapshot(&mut view, &snapshot);
                view.speed_bps = speed;
                // 速度为 0 时保留上一次的 ETA：否则 ETA 也会跟着闪成空值，
                // 看起来比数字不动更难受。
                if speed > 0 {
                    view.eta_seconds = eta(snapshot.total_bytes, snapshot.done_bytes, speed);
                }

                if last_pushed != Some((snapshot.done_bytes, speed)) {
                    last_pushed = Some((snapshot.done_bytes, speed));
                    state.tasks().update(view.clone());
                    emit(view.clone());
                }
            }
        }
    };

    // 收尾：把最终统计补上，阶段由 summary 决定
    let snapshot = progress.snapshot();
    apply_snapshot(&mut view, &snapshot);
    view.speed_bps = 0;
    view.eta_seconds = None;
    view.failures = summary.failures.iter().take(MAX_FAILURES).cloned().collect();

    view.phase = if summary.cancelled {
        TaskPhase::Cancelled
    } else if summary.failed > 0 {
        TaskPhase::Failed
    } else {
        TaskPhase::Done
    };

    view.message = match view.phase {
        // 成功不写统计：只关心「装好了」，下载了几个、跳过几个没人看
        TaskPhase::Done => None,
        TaskPhase::Failed => Some(format!(
            "{} 个文件下载失败，其余已完成；重试只会补下缺失部分",
            summary.failed
        )),
        TaskPhase::Cancelled => Some("已取消，已完成的文件保留".to_string()),
        TaskPhase::Running => None,
    };

    state.tasks().update(view.clone());
    emit(view);
}

pub(crate) fn apply_snapshot(view: &mut TaskView, snapshot: &xcl_download::Snapshot) {
    view.total_bytes = snapshot.total_bytes.max(view.total_bytes);
    view.done_bytes = snapshot.done_bytes;
    view.total_files = snapshot.total_files;
    view.finished_files = snapshot.finished_files;
    view.downloaded = snapshot.downloaded_files;
    view.skipped = snapshot.skipped_files;
    view.failed = snapshot.failed_files;
    view.fraction = snapshot.fraction();
}

pub(crate) fn eta(total: u64, done: u64, speed: u64) -> Option<u64> {
    if speed == 0 {
        return None;
    }
    let remaining = total.saturating_sub(done);
    if remaining == 0 {
        return Some(0);
    }
    Some(remaining / speed)
}

/// 从候选地址里挑出允许分片的子集。
///
/// 只看第一个候选：它会先被尝试，如果它本身就是不分片的主机（BMCLAPI 等），
/// 这个文件就该老老实实单连接，不能为了分片去切到别的源。
/// 第一个允许时才继续过滤，避免主源能用、备用源却把分片拖坏。
pub(crate) fn split_urls_for(urls: &[String]) -> Vec<String> {
    match urls.first() {
        Some(first) if xcl_source::url_allows_split(first) => urls
            .iter()
            .filter(|url| xcl_source::url_allows_split(url))
            .cloned()
            .collect(),
        _ => Vec::new(),
    }
}

fn url_kind(kind: FileKind) -> UrlKind {
    match kind {
        // 版本 JSON、客户端 jar、资源索引都在 piston-meta 上
        FileKind::VersionJson | FileKind::ClientJar | FileKind::AssetIndex => UrlKind::Meta,
        FileKind::Library | FileKind::Native => UrlKind::Library,
        FileKind::Asset => UrlKind::Asset,
        // 社区资源走自己的域名表（modrinth / curseforge / forgecdn 的镜像）。
        // Mod / 资源包 / 光影 / 整合包文件是同一个 CDN，别单独列一套。
        FileKind::Mod | FileKind::ResourcePack | FileKind::Shader | FileKind::ModpackFile => {
            UrlKind::Mod
        }
    }
}

fn display_name(path: &std::path::Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| path.display().to_string())
}

pub(crate) fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

pub(crate) fn new_task_id() -> String {
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let seq = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    format!("dl-{}-{seq}", now_ms())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view(phase: TaskPhase, id: &str) -> TaskView {
        let mut v = TaskView::new(id, "1.20.1", 10, 1000);
        v.phase = phase;
        v
    }

    #[test]
    fn running_task_is_reused_per_version() {
        let tasks = DownloadTasks::default();
        tasks.insert(
            "a".into(),
            CancellationToken::new(),
            view(TaskPhase::Running, "a"),
        );

        assert!(tasks.running_for("1.20.1").is_some());
        assert!(tasks.running_for("1.20.2").is_none());
    }

    #[test]
    fn finished_task_is_not_reused() {
        let tasks = DownloadTasks::default();
        tasks.insert(
            "a".into(),
            CancellationToken::new(),
            view(TaskPhase::Done, "a"),
        );
        assert!(tasks.running_for("1.20.1").is_none());
    }

    #[test]
    fn cancel_only_works_on_running() {
        let tasks = DownloadTasks::default();
        tasks.insert(
            "running".into(),
            CancellationToken::new(),
            view(TaskPhase::Running, "running"),
        );
        tasks.insert(
            "done".into(),
            CancellationToken::new(),
            view(TaskPhase::Done, "done"),
        );

        assert!(tasks.cancel("running"));
        assert!(!tasks.cancel("done"));
        assert!(!tasks.cancel("missing"));
    }

    #[test]
    fn eta_is_none_without_speed() {
        assert_eq!(eta(1000, 500, 0), None);
        assert_eq!(eta(1000, 1000, 100), Some(0));
        assert_eq!(eta(1000, 0, 100), Some(10));
    }

    #[test]
    fn list_is_sorted_by_start_desc() {
        let tasks = DownloadTasks::default();
        let mut older = view(TaskPhase::Running, "older");
        older.started_at_ms = 1;
        let mut newer = view(TaskPhase::Running, "newer");
        newer.started_at_ms = 2;

        tasks.insert("older".into(), CancellationToken::new(), older);
        tasks.insert("newer".into(), CancellationToken::new(), newer);

        let list = tasks.list();
        assert_eq!(list[0].task_id, "newer");
    }

    #[test]
    fn task_table_is_capped() {
        let tasks = DownloadTasks::default();
        for i in 0..30 {
            let mut v = view(TaskPhase::Done, &format!("t{i}"));
            v.started_at_ms = i as u64;
            tasks.insert(format!("t{i}"), CancellationToken::new(), v);
        }
        assert!(tasks.list().len() <= 20, "已完成的旧任务应被清掉");
    }
}
