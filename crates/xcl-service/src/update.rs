//! 启动器自更新：检查 → 下载 → 校验 → 交给 helper 替换。
//!
//! 分工：
//! - 本模块只管在线上的那一段（拉清单、下载更新包），持有状态机与视图
//! - 真正的换文件在 `xcl-update` 里，`apply` 只是把它拉起来
//! - 前端只订阅 `UpdateView`，不做任何秒数/百分比的换算
//!
//! 几条设计约束：
//!
//! - 更新包直接下到安装目录，不用 `%TEMP%`：`ReplaceFileW` 要求同卷，
//!   而且「从临时目录跑未知 exe」正是杀软的典型特征。
//! - 下载与普通资源下载不共用任务表：更新是一次性的、全局唯一的，
//!   塞进右下角任务列表会和「装了三个 mod」混在一起。
//! - 更新包不参与镜像改写。它是我们自己的地址，`SourcePolicy` 那套
//!   Mojang 域名重写对它没有意义。

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::Serialize;
use tokio_util::sync::CancellationToken;
use xcl_core::{AppError, ErrorCode};
use xcl_download::{DownloadItem, DownloadOptions, Progress};
use xcl_update::{CheckState, Manifest, SignatureCheck};

use crate::AppState;
use crate::download::SAMPLE_MS;

/// 进度回调。Tauri 层用它推 `update://state`。
pub type UpdateSink = Arc<dyn Fn(UpdateView) + Send + Sync>;

/// 更新流程的阶段。前端按它决定显示什么。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum UpdatePhase {
    /// 还没查过
    #[default]
    Idle,
    Checking,
    /// 已是最新（或判断不了，按最新处理）
    UpToDate,
    /// 有新版本，等决定
    Available,
    Downloading,
    /// 下载完了，正在算哈希
    Verifying,
    /// 校验通过，可以重启更新
    Ready,
    /// 已经通知 helper，主程序即将退出
    Applying,
    Failed,
}

/// 一次更新尝试的完整视图。字段都是可直接渲染的成品。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateView {
    pub phase: UpdatePhase,
    pub current_version: String,
    pub latest_version: Option<String>,
    pub notes: Option<String>,
    /// 逐条更新内容，界面按行渲染
    pub changelog: Vec<String>,
    /// 版本标题（清单里的 `title`），没有就用版本号
    pub title: Option<String>,
    pub notes_url: Option<String>,
    /// 清单标了 critical：文案上建议尽快更新（但不强制）
    pub critical: bool,
    /// 当前版本低于清单的 minSupported
    pub below_min_supported: bool,
    /// 强制更新。为真时前端只允许操作更新，其余界面全部禁用。
    ///
    /// 来源是持久化在配置里的版本号，不是这一次的联网结果 —— 断网、
    /// 或者云端临时撤掉清单，都不该被绕过去。
    pub forced: bool,
    /// 强制更新的说明（来自持久化副本）
    pub force_title: Option<String>,
    pub force_notes: Option<String>,
    pub force_changelog: Vec<String>,
    /// 此前选择跳过的版本
    pub skipped_version: Option<String>,
    pub total_bytes: u64,
    pub done_bytes: u64,
    /// 0.0 ~ 1.0
    pub fraction: f32,
    pub speed_bps: u64,
    pub eta_seconds: Option<u64>,
    pub error: Option<AppError>,
    /// 安装目录能不能写。false 时前端应禁用「下载更新」
    pub can_auto_update: bool,
    pub install_dir: String,
}

impl Default for UpdateView {
    fn default() -> Self {
        Self {
            phase: UpdatePhase::Idle,
            current_version: current_version(),
            latest_version: None,
            notes: None,
            changelog: Vec::new(),
            title: None,
            notes_url: None,
            critical: false,
            below_min_supported: false,
            forced: false,
            force_title: None,
            force_notes: None,
            force_changelog: Vec::new(),
            skipped_version: None,
            total_bytes: 0,
            done_bytes: 0,
            fraction: 0.0,
            speed_bps: 0,
            eta_seconds: None,
            error: None,
            can_auto_update: true,
            install_dir: String::new(),
        }
    }
}

/// 当前版本。全项目唯一来源：workspace `Cargo.toml` 的 `package.version`。
pub fn current_version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

/// 更新状态机的内部状态。
#[derive(Default)]
pub struct UpdateState {
    inner: Mutex<Inner>,
}

#[derive(Default)]
struct Inner {
    view: UpdateView,
    /// 检查时拿到的清单，下载和应用都要用
    manifest: Option<Manifest>,
    /// 已校验通过的更新包路径
    new_file: Option<PathBuf>,
    cancel: Option<CancellationToken>,
    /// 从配置里恢复的强制更新状态
    force: ForceInfo,
}

/// 强制更新的持久化副本。存在配置里，指向一个「必须装到」的版本。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ForceInfo {
    pub version: String,
    pub title: String,
    pub notes: String,
    pub changelog: Vec<String>,
}

impl ForceInfo {
    /// 还生效吗：当前版本低于它才继续拦，追上了就自动解除。
    fn is_active(&self) -> bool {
        if self.version.trim().is_empty() {
            return false;
        }
        let Some(forced) = xcl_update::parse_version(&self.version) else {
            // 配置被手工改坏了：宁可不拦，也不能把人锁死在更新页
            return false;
        };
        let Some(current) = xcl_update::parse_version(&current_version()) else {
            return false;
        };
        forced > current
    }
}

impl UpdateState {
    /// 从配置恢复强制状态。启动时就要拦人，不能等联网。
    pub fn new(config: &xcl_core::UpdateConfig) -> Self {
        let state = Self::default();
        {
            let mut inner = state.lock();
            inner.force = ForceInfo {
                version: config.force_version.clone(),
                title: config.force_title.clone(),
                notes: config.force_notes.clone(),
                changelog: config.force_changelog.clone(),
            };
            inner.view.skipped_version = Some(config.skipped_version.clone())
                .filter(|s| !s.is_empty());
        }
        state
    }

    fn set_force(&self, force: ForceInfo) {
        self.lock().force = force;
    }
}

impl UpdateState {
    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        // 锁里没有会 panic 的代码
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn snapshot(&self) -> UpdateView {
        let inner = self.lock();
        let mut view = inner.view.clone();
        // 强制状态以持久化副本为准：只要当前版本还低于它，就一直拦。
        // 不看这一轮有没有联网 —— 那正是「断网就能绕过」的来源。
        if inner.force.is_active() {
            view.forced = true;
            view.force_title = Some(inner.force.title.clone()).filter(|t| !t.is_empty());
            view.force_notes = Some(inner.force.notes.clone()).filter(|n| !n.is_empty());
            view.force_changelog = inner.force.changelog.clone();
            if view.latest_version.is_none() {
                view.latest_version = Some(inner.force.version.clone());
            }
        } else {
            view.forced = false;
            view.force_title = None;
            view.force_notes = None;
            view.force_changelog.clear();
        }
        view
    }

    /// 起一个新阶段，顺手清掉上一次的错误。
    fn set_phase(&self, phase: UpdatePhase) {
        let mut inner = self.lock();
        inner.view.phase = phase;
        if !matches!(phase, UpdatePhase::Failed) {
            inner.view.error = None;
        }
    }

    fn fail(&self, err: &AppError) {
        let mut inner = self.lock();
        inner.view.phase = UpdatePhase::Failed;
        inner.view.error = Some(err.clone());
        inner.view.speed_bps = 0;
        inner.view.eta_seconds = None;
        inner.cancel = None;
    }

    fn set_available(
        &self,
        manifest: Manifest,
        check: &xcl_update::CheckResult,
        can_auto_update: bool,
        install_dir: &std::path::Path,
    ) {
        let mut inner = self.lock();
        inner.view.phase = UpdatePhase::Available;
        inner.view.latest_version = Some(check.latest.clone());
        inner.view.notes = Some(check.notes.clone());
        inner.view.changelog = check.changelog.clone();
        inner.view.title = Some(check.title.clone()).filter(|t| !t.is_empty());
        inner.view.notes_url = check.notes_url.clone();
        inner.view.critical = matches!(manifest.severity, xcl_update::Severity::Critical);
        inner.view.below_min_supported = check.below_min_supported;
        inner.view.total_bytes = manifest.exe.size;
        inner.view.done_bytes = 0;
        inner.view.fraction = 0.0;
        inner.view.speed_bps = 0;
        inner.view.eta_seconds = None;
        inner.view.error = None;
        inner.view.can_auto_update = can_auto_update;
        inner.view.install_dir = install_dir.display().to_string();
        inner.manifest = Some(manifest);
        inner.new_file = None;
        inner.cancel = None;
    }

    fn set_up_to_date(
        &self,
        latest: &str,
        can_auto_update: bool,
        install_dir: &std::path::Path,
    ) {
        let mut inner = self.lock();
        inner.view.phase = UpdatePhase::UpToDate;
        // 「已是最新」也要把服务器上的版本号带上，便于排查「为什么没提示」
        inner.view.latest_version = Some(latest.to_string());
        inner.view.error = None;
        inner.view.speed_bps = 0;
        inner.view.eta_seconds = None;
        inner.view.can_auto_update = can_auto_update;
        inner.view.install_dir = install_dir.display().to_string();
        inner.manifest = None;
        inner.new_file = None;
        inner.cancel = None;
    }

    fn begin_download(&self, cancel: CancellationToken, size: u64) {
        let mut inner = self.lock();
        inner.view.phase = UpdatePhase::Downloading;
        inner.view.done_bytes = 0;
        inner.view.total_bytes = size;
        inner.view.fraction = 0.0;
        inner.view.speed_bps = 0;
        inner.view.eta_seconds = None;
        inner.view.error = None;
        inner.cancel = Some(cancel);
    }

    fn progress(&self, done: u64, total: u64, speed: u64, eta: Option<u64>) {
        let mut inner = self.lock();
        inner.view.done_bytes = done;
        inner.view.total_bytes = total.max(inner.view.total_bytes);
        inner.view.fraction = if total == 0 {
            0.0
        } else {
            (done as f32 / total as f32).clamp(0.0, 1.0)
        };
        inner.view.speed_bps = speed;
        // 速度为 0 时保留上一次的 ETA，免得数字闪成空
        if speed > 0 {
            inner.view.eta_seconds = eta;
        }
    }

    fn finish_download(&self, ok: bool) {
        let mut inner = self.lock();
        inner.view.speed_bps = 0;
        inner.view.eta_seconds = None;
        inner.cancel = None;
        if ok {
            inner.view.phase = UpdatePhase::Ready;
            inner.view.fraction = 1.0;
            inner.view.done_bytes = inner.view.total_bytes;
        } else {
            inner.view.phase = UpdatePhase::Available;
        }
    }

    fn set_skipped(&self, version: Option<String>) {
        self.lock().view.skipped_version = version;
    }

    /// 回到「什么都没做」—— 点了「稍后」或「忽略此版本」之后。
    fn reset_to_idle(&self) {
        let mut inner = self.lock();
        inner.view.phase = UpdatePhase::Idle;
        inner.view.error = None;
        inner.view.speed_bps = 0;
        inner.view.eta_seconds = None;
        inner.cancel = None;
    }

    fn cancel_running(&self) -> bool {
        let inner = self.lock();
        match (&inner.cancel, inner.view.phase) {
            (Some(token), UpdatePhase::Downloading | UpdatePhase::Verifying) => {
                token.cancel();
                true
            }
            _ => false,
        }
    }

    fn download_plan(&self) -> Option<(Manifest, PathBuf)> {
        let inner = self.lock();
        let manifest = inner.manifest.clone()?;
        let dir = PathBuf::from(&inner.view.install_dir);
        Some((manifest, dir))
    }

    fn apply_plan(&self) -> Option<(PathBuf, PathBuf, String)> {
        let inner = self.lock();
        let new_file = inner.new_file.clone()?;
        let manifest = inner.manifest.clone()?;
        // 要替换的就是「当前正在运行的这个 exe」—— 名字从系统取，不猜。
        // 写死 `XCL.exe` 之类的名字，在包名改过一次之后就永远替换失败。
        let target = std::env::current_exe().ok()?;
        Some((target, new_file, manifest.exe.sha256.clone()))
    }
}

// ---------- 对外接口 ----------

impl AppState {
    pub fn update_view(&self) -> UpdateView {
        self.update.snapshot()
    }

    /// 拉清单、验签、比版本。不改任何文件（除了把强制更新状态落盘）。
    ///
    /// `auto` 表示这是启动时的自动检查：跳过过的版本不再打扰。
    /// 手动点「检查更新」时一律照实显示。
    pub async fn update_check(
        self: &Arc<Self>,
        auto: bool,
        emit: UpdateSink,
    ) -> Result<UpdateView, AppError> {
        // 强制状态下也别联网：已经有结论了，查了只会闪一下
        if self.update.snapshot().forced {
            return Ok(self.update.snapshot());
        }

        let config = self.config();
        // 地址来自 `config.toml` 的 `update.manifestUrl`：部署信息不进代码。
        // 空着说明这个构建没有更新服务（自己编译的、别人的 fork），
        // 自动检查安静跳过，手动点检查才报错 —— 每次启动弹一条红字没意义。
        let url = config.update.manifest_url.trim().to_string();
        if url.is_empty() {
            let err = AppError::unsupported("没有配置更新服务", "这个构建没有内置更新地址")
                .with_hint("在config.toml的update.manifestUrl里填上清单地址");
            if auto {
                return Ok(self.update.snapshot());
            }
            self.update.fail(&err);
            emit(self.update.snapshot());
            return Err(err);
        }

        self.update.set_phase(UpdatePhase::Checking);
        emit(self.update.snapshot());

        let result = self.check_inner(&url, config.update.channel, auto).await;
        match result {
            Ok(()) => {
                let view = self.update.snapshot();
                emit(view.clone());
                Ok(view)
            }
            Err(err) => {
                self.update.fail(&err);
                emit(self.update.snapshot());
                Err(err)
            }
        }
    }

    async fn check_inner(
        &self,
        url: &str,
        channel: xcl_core::config::UpdateChannel,
        auto: bool,
    ) -> Result<(), AppError> {
        let (text, _source) = self.fetch_meta_text(url).await?;

        // 清单签名：配了公钥就必须有，没配（开发期）能缺
        match self.fetch_meta_text(&format!("{url}.sig")).await {
            Ok((signature, _)) => {
                let checked = xcl_update::verify_manifest(text.as_bytes(), signature.trim())?;
                if checked == SignatureCheck::Skipped {
                    tracing::warn!("更新清单未验签（还没配置内置公钥），仅开发期可接受");
                }
            }
            Err(err) => {
                if xcl_update::requires_signature() {
                    return Err(AppError::parse(
                        "更新清单缺少签名",
                        format!("无法获取清单签名：{err}"),
                    )
                    .with_hint("本次更新已中止，请稍后重试"));
                }
                tracing::warn!("清单签名缺失，且未配置公钥，跳过验签");
            }
        }

        let manifest = Manifest::parse(&text)?;
        let check = manifest.check(&current_version(), channel);

        let install_dir = install_dir()?;
        let can_auto_update = xcl_update::dir_is_writable(&install_dir);

        // 跳过过这个版本，而且这次不是强制：自动检查不必再提
        let skipped = self.config().update.skipped_version;
        if auto
            && !check.force
            && check.state == CheckState::Available
            && !skipped.is_empty()
            && check.latest == skipped
        {
            self.update
                .set_up_to_date(&check.latest, can_auto_update, &install_dir);
            return Ok(());
        }

        if check.state == CheckState::Available {
            self.update
                .set_available(manifest, &check, can_auto_update, &install_dir);
            if check.force {
                self.persist_force(&check)?;
            }
        } else {
            self.update
                .set_up_to_date(&check.latest, can_auto_update, &install_dir);
            // 版本追上了：把过期的强制状态清掉，别让下一次启动还拦着
            self.clear_force_if_stale()?;
        }
        Ok(())
    }

    /// 把「必须更新到某个版本」写进配置。
    ///
    /// 这是强制更新能跨启动生效的唯一原因 —— 只要写进去了，断网、换 DNS、
    /// 云端把清单撤掉，都拦不住它继续生效。
    fn persist_force(&self, check: &xcl_update::CheckResult) -> Result<(), AppError> {
        let force = ForceInfo {
            version: check.latest.clone(),
            title: check.title.clone(),
            notes: check.notes.clone(),
            changelog: check.changelog.clone(),
        };
        if !force.is_active() {
            return Ok(());
        }

        let mut config = self.config();
        // 已经记过同一个版本就不重复落盘
        if config.update.force_version != force.version
            || config.update.force_changelog != force.changelog
        {
            config.update.force_version = force.version.clone();
            config.update.force_title = force.title.clone();
            config.update.force_notes = force.notes.clone();
            config.update.force_changelog = force.changelog.clone();
            config.update.last_check_at = now_secs();
            self.update_config(config)?;
        }
        self.update.set_force(force);
        Ok(())
    }

    /// 当前版本已经追上强制版本时，清掉配置里的残留。
    fn clear_force_if_stale(&self) -> Result<(), AppError> {
        let mut config = self.config();
        if config.update.force_version.is_empty() {
            return Ok(());
        }
        let probe = ForceInfo {
            version: config.update.force_version.clone(),
            ..ForceInfo::default()
        };
        if probe.is_active() {
            return Ok(());
        }
        config.update.force_version.clear();
        config.update.force_title.clear();
        config.update.force_notes.clear();
        config.update.force_changelog.clear();
        self.update_config(config)?;
        self.update.set_force(ForceInfo::default());
        tracing::info!("已追上强制更新版本，解除限制");
        Ok(())
    }

    /// 忽略某个版本：写进配置，之后自动检查不再提它。
    ///
    /// 强制更新不受影响 —— 那种情况下前端不会给「忽略」的入口。
    pub fn update_skip(&self, version: &str) -> Result<UpdateView, AppError> {
        let version = version.trim().to_string();
        if version.is_empty() {
            return Err(AppError::validation("没有指定版本", "要跳过的版本号是空的"));
        }
        let mut config = self.config();
        config.update.skipped_version = version.clone();
        self.update_config(config)?;
        self.update.set_skipped(Some(version));
        self.update.reset_to_idle();
        Ok(self.update.snapshot())
    }

    /// 下载更新包到安装目录，进度用 `emit` 推。
    pub async fn update_download(self: &Arc<Self>, emit: UpdateSink) -> Result<UpdateView, AppError> {
        let Some((manifest, install_dir)) = self.update.download_plan() else {
            return Err(AppError::validation("还没有可用的更新", "请先检查更新"));
        };
        if !xcl_update::dir_is_writable(&install_dir) {
            let err = AppError::new(
                ErrorCode::Io,
                "这个文件夹不能写入",
                format!("{} 不可写，无法自动更新", install_dir.display()),
            )
            .with_hint("把启动器放到普通文件夹（比如桌面）再试，或手动下载新版覆盖");
            self.update.fail(&err);
            emit(self.update.snapshot());
            return Err(err);
        }

        let work_dir = update_work_dir()?;
        let stem = current_binary_stem()?;
        let exe_name = manifest.update_file_name(&stem)?;
        let dest = work_dir.join(&exe_name);
        let archive_name = manifest.archive_file_name(&stem)?;
        let archive_path = work_dir.join(&archive_name);

        // 上次已经解压好、校验通过：不用再下一遍
        if dest.is_file()
            && xcl_update::verify_artifact(&dest, manifest.exe.size, &manifest.exe.sha256).is_ok()
        {
            self.update.set_phase(UpdatePhase::Verifying);
            self.update.finish_download(true);
            self.update.set_new_file(dest.clone());
            remember_pending(&manifest, &dest, &work_dir);
            emit(self.update.snapshot());
            return Ok(self.update.snapshot());
        }

        let cancel = CancellationToken::new();
        self.update.begin_download(cancel.clone(), manifest.archive.size);
        emit(self.update.snapshot());

        // 压缩包是一整个文件：并发给 1，靠分片拿带宽
        let options = DownloadOptions {
            concurrency: 1,
            attempts_per_url: self.config().download.retries.saturating_add(1),
            split: self.split_config_with(true),
        };
        let urls = vec![manifest.archive.url.clone()];
        let split_urls = crate::download::split_urls_for(&urls);
        let item = DownloadItem {
            name: archive_name,
            urls,
            split_urls,
            dest: archive_path.clone(),
            // 引擎目前只认 SHA-1；SHA-256 在下完之后由我们自己算（见下）
            sha1: None,
            size: manifest.archive.size,
        };
        let progress = Progress::new(manifest.archive.size, 1);

        // 限速跟全局下载设置走。0 就是不限速（默认），设了才会真的掐带宽。
        let limit_bps = self.config().download.speed_limit_kbps as u64 * 1024;
        self.fetcher().set_speed_limit(limit_bps);

        let download = xcl_download::run(
            vec![item],
            Arc::clone(self.fetcher()),
            Arc::clone(&progress),
            options,
            cancel.clone(),
        );
        tokio::pin!(download);

        let mut ticker = tokio::time::interval(Duration::from_millis(SAMPLE_MS));
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let mut meter = crate::speed::SpeedMeter::new(crate::speed::DEFAULT_WINDOW);
        let mut last_pushed: Option<(u64, u64)> = None;

        let summary = loop {
            tokio::select! {
                summary = &mut download => break summary,
                _ = ticker.tick() => {
                    let snapshot = progress.snapshot();
                    let speed = meter.update(snapshot.done_bytes, Instant::now());
                    let eta = crate::download::eta(snapshot.total_bytes, snapshot.done_bytes, speed);
                    self.update.progress(snapshot.done_bytes, snapshot.total_bytes, speed, eta);
                    if last_pushed != Some((snapshot.done_bytes, speed)) {
                        last_pushed = Some((snapshot.done_bytes, speed));
                        emit(self.update.snapshot());
                    }
                }
            }
        };

        if summary.cancelled {
            self.update.finish_download(false);
            emit(self.update.snapshot());
            return Ok(self.update.snapshot());
        }
        if summary.failed > 0 {
            let reason = summary
                .failures
                .first()
                .cloned()
                .unwrap_or_else(|| "下载失败".to_string());
            let err = AppError::network(
                "更新包下载失败",
                format!("{} 没有下载完整：{reason}", manifest.archive.url),
            )
            .with_hint("检查网络后重试；已下载的部分会续传");
            self.update.fail(&err);
            emit(self.update.snapshot());
            return Err(err);
        }

        // 第一道：压缩包本身完整吗。放在解压之前，坏包不必白解一遍。
        self.update.set_phase(UpdatePhase::Verifying);
        emit(self.update.snapshot());
        if let Err(err) = xcl_update::verify_artifact(
            &archive_path,
            manifest.archive.size,
            &manifest.archive.sha256,
        ) {
            std::fs::remove_file(&archive_path).ok();
            self.update.fail(&err);
            emit(self.update.snapshot());
            return Err(err);
        }

        // 第二道：解出来的启动器是不是清单说的那一个。
        // 解压成功之前不删压缩包 —— 万一中途失败，留着还能重试。
        if let Err(err) = xcl_update::unpack::extract_exe(
            &archive_path,
            &dest,
            &manifest.exe.name,
            manifest.exe.size,
            &manifest.exe.sha256,
        ) {
            std::fs::remove_file(&archive_path).ok();
            self.update.fail(&err);
            emit(self.update.snapshot());
            return Err(err);
        }
        // 两份都留着白占磁盘，装好就清掉
        std::fs::remove_file(&archive_path).ok();

        self.update.finish_download(true);
        self.update.set_new_file(dest.clone());
        remember_pending(&manifest, &dest, &work_dir);
        emit(self.update.snapshot());
        Ok(self.update.snapshot())
    }

    pub fn update_cancel(&self) -> bool {
        self.update.cancel_running()
    }

    /// 启动 helper 换文件。
    ///
    /// 返回 `Ok` 时 helper 已经握手完成，调用方应当尽快退出主进程 ——
    /// 再等下去 helper 就会超时放弃。
    pub async fn update_apply(self: &Arc<Self>) -> Result<(), AppError> {
        let Some((target, new_file, sha256)) = self.update.apply_plan() else {
            return Err(AppError::validation("更新还没准备好", "请先下载更新包"));
        };
        self.update.set_phase(UpdatePhase::Applying);

        // `launch_helper` 是同步阻塞的（复制 exe + 等 5 秒握手），
        // 不能直接跑在 async 上下文里把整个 runtime 卡住
        let joined = tokio::task::spawn_blocking(move || {
            xcl_update::launch_helper(&target, &new_file, &sha256, true)
        })
        .await
        .map_err(|err| AppError::new(ErrorCode::Unknown, "更新线程异常", err.to_string()))?;

        match joined {
            Ok(()) => Ok(()),
            Err(err) => {
                self.update.fail(&err);
                Err(err)
            }
        }
    }
}

impl UpdateState {
    /// 记下校验通过的更新包（下载完成后调用）。
    fn set_new_file(&self, path: PathBuf) {
        self.lock().new_file = Some(path);
    }
}

/// 把「下好、待应用」这件事记到盘上。
///
/// 这是自动更新能接上的唯一原因：随时可能直接关掉程序，
/// 内存里的状态一关就没。记录留下之后，下次 `main()` 一进来就知道手上有活。
///
/// 写失败只记日志，不影响本次下载 —— 大不了这次不自动装。
fn remember_pending(manifest: &Manifest, dest: &std::path::Path, work_dir: &std::path::Path) {
    let pending = xcl_update::PendingUpdate {
        version: manifest.version.clone(),
        file: dest.to_path_buf(),
        size: manifest.exe.size,
        sha256: manifest.exe.sha256.clone(),
        title: manifest.title.clone(),
    };
    if let Err(err) = xcl_update::pending::save(work_dir, &pending) {
        tracing::warn!(error = %err, "写入待应用更新记录失败，下次启动不会自动更新");
    }
}

/// 安装目录：当前 exe 所在目录。
fn install_dir() -> Result<PathBuf, AppError> {
    let exe = std::env::current_exe()
        .map_err(|err| AppError::io(&err, "定位当前程序"))?;
    exe.parent()
        .map(|p| p.to_path_buf())
        .ok_or_else(|| AppError::new(ErrorCode::Io, "无法确定安装目录", exe.display().to_string()))
}

/// 更新工作目录：`<程序目录>/data/update`。
///
/// 更新包、helper、`.old` 备份全在这里，程序目录只留一个 exe。
/// 挂在程序目录下是为了保证同卷 —— `ReplaceFileW` 要求源和目标同卷。
fn update_work_dir() -> Result<PathBuf, AppError> {
    let dir = xcl_update::work_dir(&install_dir()?);
    std::fs::create_dir_all(&dir).map_err(|err| AppError::io(&err, "创建更新目录"))?;
    Ok(dir)
}

/// 程序文件名（不含扩展名）。更新包与 helper 的临时文件名都从它派生，
/// 这样包名从内部名换成正式全名时，链路自动跟上。
fn current_binary_stem() -> Result<String, AppError> {
    let exe = std::env::current_exe()
        .map_err(|err| AppError::io(&err, "定位当前程序"))?;
    exe.file_stem()
        .and_then(|stem| stem.to_str())
        .filter(|stem| !stem.trim().is_empty())
        .map(str::to_string)
        .ok_or_else(|| {
            AppError::new(
                ErrorCode::Io,
                "无法确定程序文件名",
                exe.display().to_string(),
            )
        })
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fresh_state_is_idle() {
        let state = UpdateState::default();
        let view = state.snapshot();
        assert_eq!(view.phase, UpdatePhase::Idle);
        assert_eq!(view.current_version, current_version());
        assert!(view.error.is_none());
        // 0 字节也必须是 0，不能变成 None/横杠
        assert_eq!(view.done_bytes, 0);
        assert_eq!(view.fraction, 0.0);
    }

    #[test]
    fn phase_serializes_as_camel_case() {
        let json = serde_json::to_string(&UpdatePhase::UpToDate).unwrap();
        assert_eq!(json, "\"upToDate\"");
        let json = serde_json::to_string(&UpdatePhase::Available).unwrap();
        assert_eq!(json, "\"available\"");
    }

    #[test]
    fn failure_clears_speed_and_keeps_error() {
        let state = UpdateState::default();
        state.begin_download(CancellationToken::new(), 1000);
        state.progress(500, 1000, 2048, Some(1));
        assert_eq!(state.snapshot().speed_bps, 2048);

        let err = AppError::network("下载失败", "断网了");
        state.fail(&err);
        let view = state.snapshot();
        assert_eq!(view.phase, UpdatePhase::Failed);
        assert_eq!(view.error.unwrap().title, "下载失败");
        // 失败后不该还显示速度
        assert_eq!(view.speed_bps, 0);
    }

    #[test]
    fn progress_keeps_eta_when_speed_drops_to_zero() {
        let state = UpdateState::default();
        state.begin_download(CancellationToken::new(), 1000);
        state.progress(500, 1000, 1000, Some(5));
        assert_eq!(state.snapshot().eta_seconds, Some(5));
        // 速度归零（短暂断流）：ETA 保持上一次
        state.progress(500, 1000, 0, None);
        let view = state.snapshot();
        assert_eq!(view.speed_bps, 0);
        assert_eq!(view.eta_seconds, Some(5));
    }

    #[test]
    fn fraction_is_clamped() {
        let state = UpdateState::default();
        state.begin_download(CancellationToken::new(), 100);
        state.progress(150, 100, 10, None);
        assert!((state.snapshot().fraction - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn cancel_only_works_while_downloading() {
        let state = UpdateState::default();
        assert!(!state.cancel_running());
        state.set_phase(UpdatePhase::Available);
        assert!(!state.cancel_running());

        let token = CancellationToken::new();
        state.begin_download(token.clone(), 100);
        assert!(state.cancel_running());
        assert!(token.is_cancelled());
    }

    #[test]
    fn finishing_cancelled_download_returns_to_available() {
        let state = UpdateState::default();
        state.begin_download(CancellationToken::new(), 100);
        state.finish_download(false);
        assert_eq!(state.snapshot().phase, UpdatePhase::Available);
    }

    #[test]
    fn finishing_successful_download_is_ready_and_full() {
        let state = UpdateState::default();
        state.begin_download(CancellationToken::new(), 100);
        state.progress(40, 100, 10, None);
        state.finish_download(true);
        let view = state.snapshot();
        assert_eq!(view.phase, UpdatePhase::Ready);
        assert_eq!(view.done_bytes, 100);
        assert!((view.fraction - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn apply_plan_is_empty_without_download() {
        let state = UpdateState::default();
        assert!(state.apply_plan().is_none());
    }

    #[test]
    fn install_dir_is_exe_parent() {
        let dir = install_dir().unwrap();
        let exe = std::env::current_exe().unwrap();
        assert_eq!(Some(dir), exe.parent().map(|p| p.to_path_buf()));
    }
}
