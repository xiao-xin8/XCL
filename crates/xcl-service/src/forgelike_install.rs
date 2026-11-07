//! Forge / NeoForge 的安装流水线。
//!
//! 与 Fabric 的根本差别：这两个加载器只有跑官方安装器才能产出完整文件
//! （安装器里的 processors 会生成 `…-client.jar`、`…-srg.jar` 等），所以流程是：
//!
//! ```text
//! 下载安装器（缓存里留着，预览也用得到）
//!   ↓ 读安装器里的 install_profile.json + version.json，与原版 JSON 合并
//! 写自包含版本 JSON（和 Fabric 一样：不多出一条原版记录）
//!   ↓ 临时目录 staging：launcher_profiles.json + versions/<原版>/{json,jar}
//!   ↓ 用我们的下载引擎把安装期要用的库 + Mojang 映射表下到 staging/libraries
//!   ↓ java -jar installer.jar --installClient <staging>
//!   ↓ 把 staging/libraries 收编回数据根、复制原版 jar 成本实例的 jar
//!   ↓ 交给统一下载链路补资源文件（assets）
//! 删 staging；安装器文件也删（它是临时资源，下次要再下）
//! ```
//!
//! 实测依据见 `docs/LOADER-FORGELIKE.md` 第 9 节：
//! - 安装器的 `--mirror` 参数不生效，必须预下载库，否则国内直连会超时；
//! - `DOWNLOAD_MOJMAPS` 自己联网打 Mojang，必须提前把映射表放到目标路径；
//! - 安装器往系统临时目录里解 `client.lzma`，跑完要自己清（它不自己删）。
//!
//! 界面与 Fabric 完全一致：同一张任务卡、同一份清单结构、同一个按钮。

use std::collections::VecDeque;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio_util::sync::CancellationToken;
use xcl_core::{merge_version_json, AppError, VersionDetail};
use xcl_loader::{forgelike, LoaderKind};

use crate::download::{apply_snapshot, ProgressSink, TaskPhase, TaskView};
use crate::install::ResolvedVersion;
use crate::local;
use crate::loader::LoaderRef;
use crate::AppState;

/// 安装器会自己往系统临时目录里解 `client.lzma`；失败时保留最后这么多行日志。
const LOG_TAIL: usize = 20;

/// 安装器整体超时。正常安装几十秒到两三分钟，超过这个数就是真卡住了。
const INSTALLER_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10 * 60);

/// 安装器退出后，给读线程收尾的宽限时间（之后强制中止，理由见 `run_installer`）。
const READER_GRACE: std::time::Duration = std::time::Duration::from_millis(500);

/// 安装器只要大于这个尺寸就认为下载完整（真实安装器都 ≥ 2MB）。
const INSTALLER_MIN_BYTES: u64 = 64 * 1024;

/// 从安装器里读出来的东西。
struct InstallerFiles {
    profile_text: String,
    version_json_text: Option<String>,
}

/// 一次安装要用到的全部前置材料。
pub(crate) struct ForgelikePrepared {
    pub resolved: ResolvedVersion,
    /// 合并后的自包含 JSON；本地已经有 JSON 时为 `None`
    pub merged: Option<String>,
    /// 已装过的版本不需要这两样（预览要快，别再下 6MB 安装器）
    installer: Option<PathBuf>,
    profile: Option<forgelike::InstallProfile>,
    /// 原版 JSON 原文（staging 里要放一份）
    parent_text: Option<String>,
}

impl AppState {
    /// 解析 Forge / NeoForge 的安装信息（预览与安装共用）。
    pub(crate) async fn resolve_forgelike_loader(
        &self,
        game: &str,
        loader: &LoaderRef,
    ) -> Result<(ResolvedVersion, Option<String>), AppError> {
        let prepared = self.prepare_forgelike(game, loader).await?;
        Ok((prepared.resolved, prepared.merged))
    }

    async fn prepare_forgelike(
        &self,
        game: &str,
        loader: &LoaderRef,
    ) -> Result<ForgelikePrepared, AppError> {
        let kind = loader.kind()?;
        let id = forgelike::instance_id(kind, game, &loader.version);

        // 装过：本地那份已经是合并结果，直接用 —— 不联网、不下安装器
        // （和 Fabric 同一条语义；预览面板点开就是秒出）
        let json_path = self.layout.version_json(&id);
        if json_path.is_file() {
            let detail = local::read_detail(&json_path)?;
            let reuse_jar = local::client_jar(&self.layout, game).is_some()
                || local::client_jar(&self.layout, &id).is_some();
            return Ok(ForgelikePrepared {
                resolved: ResolvedVersion {
                    // 本地这份是合并后的全量 JSON。修复时要按「增量」算：
                    // 减掉原版已经声明过的库、资源归原版，否则会把四千多个文件
                    // 全部重算一遍（Fabric 那边的「转半天」就是这个坑）
                    own: own_part_only(&detail, &self.layout, game),
                    detail,
                    json_url: None,
                    parent: Some(game.to_string()),
                    reuse_jar,
                },
                merged: None,
                installer: None,
                profile: None,
                parent_text: None,
            });
        }

        let installer = self.ensure_installer(kind, game, &loader.version).await?;
        let files = read_installer_entries(&installer).await?;
        let profile = forgelike::parse_install_profile(&files.profile_text)?;
        if profile.is_unsupported() {
            return Err(AppError::unsupported(
                "这个 Forge 版本装不了",
                format!(
                    "{} 用的是 1.7.10 及更早的老式安装器，暂不支持",
                    kind.label()
                ),
            )
            .with_hint("换 1.8 及更新的原版再试"));
        }

        let parent_text = self.version_json_text(game).await?;

        let child_text = files.version_json_text.ok_or_else(|| {
            AppError::parse(
                "安装器里没有版本描述",
                format!("{} 的安装器缺少 version.json", kind.label()),
            )
        })?;
        let child = VersionDetail::parse(&child_text)?;
        if child.inherits_from.as_deref() != Some(game) {
            return Err(AppError::parse(
                "加载器文件不对",
                format!(
                    "安装器给的 inheritsFrom 是 {:?}，期望 {game}",
                    child.inherits_from
                ),
            ));
        }

        let merged = merge_version_json(&child_text, &parent_text, &id)?;
        let detail = VersionDetail::parse(&merged)?;
        let reuse_jar = local::client_jar(&self.layout, game).is_some();
        Ok(ForgelikePrepared {
            // `own` 只留加载器自己声明的库：原版装过时清单靠它算增量，
            // 把合并结果塞进来会让一千多个资源文件全部重算一遍
            resolved: ResolvedVersion {
                own: child,
                detail,
                json_url: None,
                parent: Some(game.to_string()),
                reuse_jar,
            },
            merged: Some(merged),
            installer: Some(installer),
            profile: Some(profile),
            parent_text: Some(parent_text),
        })
    }

    /// 安装器 jar：下载到缓存目录，下次预览 / 安装直接复用。
    async fn ensure_installer(
        &self,
        kind: LoaderKind,
        game: &str,
        loader: &str,
    ) -> Result<PathBuf, AppError> {
        let url = forgelike::installer_url(kind, game, loader);
        let dir = self.layout.cache_root().join("forgelike").join("installers");
        fs::create_dir_all(&dir).map_err(|e| AppError::io(&e, "创建安装器缓存目录"))?;
        let name = url.rsplit('/').next().unwrap_or("installer.jar");
        let path = dir.join(name);
        if path.is_file() {
            let size = fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
            if size > INSTALLER_MIN_BYTES {
                return Ok(path);
            }
        }

        let bytes = self
            .http()
            .get_bytes(xcl_source::UrlKind::Library, &url)
            .await?;
        if (bytes.len() as u64) < INSTALLER_MIN_BYTES {
            return Err(AppError::parse(
                "安装器下载不完整",
                format!("{} 只返回了 {} 字节", kind.label(), bytes.len()),
            )
            .with_hint("换个下载源后重试"));
        }
        let tmp = path.with_extension("jar.tmp");
        fs::write(&tmp, &bytes).map_err(|e| AppError::io(&e, "写入安装器"))?;
        fs::rename(&tmp, &path).map_err(|e| AppError::io(&e, "替换安装器"))?;
        Ok(path)
    }

    /// 开始 Forge / NeoForge 安装。立刻返回一张任务卡，后面全在后台跑。
    ///
    /// 任务 id 由这里定死并贯穿准备 / 下载两个阶段，界面上只有一张卡。
    pub async fn start_forgelike_install(
        self: &Arc<Self>,
        game: &str,
        loader: &LoaderRef,
        instance_id: Option<String>,
        extra_files: Vec<crate::install::PendingFile>,
        task_id: Option<String>,
        emit: ProgressSink,
    ) -> Result<TaskView, AppError> {
        let kind = loader.kind()?;
        Self::ensure_installable(kind)?;
        let id = instance_id.unwrap_or_else(|| forgelike::instance_id(kind, game, &loader.version));
        if task_id.is_none() {
            if let Some(running) = self.tasks().running_for(&id) {
                return Ok(running);
            }
        }

        let task_id = task_id.unwrap_or_else(|| format!("forgelike-{id}"));
        let cancel = CancellationToken::new();
        let initial = TaskView {
            parent_id: Some(game.to_string()),
            message: Some("正在准备安装器".to_string()),
            ..TaskView::new(&task_id, &id, 0, 0)
        };
        self.tasks()
            .insert(task_id.clone(), cancel.clone(), initial.clone());
        emit(initial.clone());

        let state = Arc::clone(self);
        let game = game.to_string();
        let loader = loader.clone();
        let task_for_run = task_id.clone();
        tokio::spawn(async move {
            if let Err(error) = state
                .run_forgelike_install(
                    &task_for_run,
                    &id,
                    &game,
                    &loader,
                    extra_files,
                    cancel,
                    &emit,
                )
                .await
            {
                fail_task(&state, &task_for_run, &error, &emit);
            }
        });

        Ok(initial)
    }

    async fn run_forgelike_install(
        self: &Arc<Self>,
        task_id: &str,
        id: &str,
        game: &str,
        loader: &LoaderRef,
        extra_files: Vec<crate::install::PendingFile>,
        cancel: CancellationToken,
        emit: &ProgressSink,
    ) -> Result<(), AppError> {
        let kind = loader.kind()?;
        let label = kind.label();
        // 记下「这个实例属于哪个原版」：改名后「下载 Mod」页仍能认出它
        let _ = crate::isolation::ensure_game_version(&self.layout, id, game);
        let prepared = self.prepare_forgelike(game, loader).await?;

        // 自包含 JSON 先落盘（和 Fabric 一致）：中途失败也能靠「校验并修复」补
        if let Some(text) = &prepared.merged {
            self.write_version_json(id, text)?;
        }

        // 已装过的版本走到这里时没有安装器：直接按清单补齐（等同 Fabric 的修复）
        if prepared.installer.is_some() {
            let staging = self
                .layout
                .cache_root()
                .join("forgelike")
                .join("staging")
                .join(id);
            let result = self
                .install_into_staging(&staging, task_id, id, game, kind, &prepared, &cancel, emit)
                .await;

            // 无论成败都清掉临时目录：安装器会往里面写几十 MB
            let _ = fs::remove_dir_all(&staging);
            if let Some(installer) = &prepared.installer {
                let _ = fs::remove_file(installer);
            }
            result?;
        }

        // 资源文件走统一下载链路；任务卡沿用它自己的 id，不换卡
        let mut plan = self.plan_from(id, &prepared.resolved).await?;
        plan.files.extend(extra_files);
        self.start_install_plan_as(plan, Some(game), task_id.to_string(), emit.clone())
            .await?;
        tracing::debug!(id, "{} 安装准备阶段完成", label);
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    async fn install_into_staging(
        self: &Arc<Self>,
        staging: &Path,
        task_id: &str,
        id: &str,
        game: &str,
        kind: LoaderKind,
        prepared: &ForgelikePrepared,
        cancel: &CancellationToken,
        emit: &ProgressSink,
    ) -> Result<(), AppError> {
        let label = kind.label();
        let installer = prepared
            .installer
            .as_ref()
            .ok_or_else(|| AppError::unsupported("缺少安装器", "内部状态错误：没有安装器文件"))?;
        let profile = prepared
            .profile
            .as_ref()
            .ok_or_else(|| AppError::unsupported("缺少安装器描述", "内部状态错误：没读到 install_profile"))?;
        let parent_text = prepared
            .parent_text
            .as_ref()
            .ok_or_else(|| AppError::unsupported("缺少原版描述", "内部状态错误：没读到原版 JSON"))?;
        if staging.exists() {
            fs::remove_dir_all(staging).map_err(|e| AppError::io(&e, "清理上次的临时目录"))?;
        }

        // 1. staging 里要有「一个 Minecraft 根目录」：launcher_profiles + 原版 json/jar
        let version_dir = staging.join("versions").join(game);
        fs::create_dir_all(&version_dir).map_err(|e| AppError::io(&e, "创建临时版本目录"))?;
        fs::write(
            staging.join("launcher_profiles.json"),
            br#"{"profiles":{}}"#,
        )
        .map_err(|e| AppError::io(&e, "写入 launcher_profiles.json"))?;
        fs::write(version_dir.join(format!("{game}.json")), parent_text)
            .map_err(|e| AppError::io(&e, "写入原版版本描述"))?;

        // 2. 原版 jar：装过就复制，没装就下载到 staging（不建原版记录）
        let staged_jar = version_dir.join(format!("{game}.jar"));
        let parent = VersionDetail::parse(parent_text)?;
        if let Some(existing) = local::client_jar(&self.layout, game) {
            fs::copy(&existing, &staged_jar).map_err(|e| AppError::io(&e, "复制原版 jar"))?;
        } else {
            let client = parent
                .downloads
                .as_ref()
                .and_then(|d| d.client.as_ref())
                .ok_or_else(|| {
                    AppError::parse("原版缺少客户端文件", format!("{game} 的清单里没有 client"))
                })?;
            let files = vec![FileSpec {
                dest: staged_jar.clone(),
                url: client.url.clone(),
                sha1: client.sha1.clone(),
                size: client.size.unwrap_or(0),
            }];
            self.prep_download(task_id, id, game, files, cancel, emit)
                .await?;
        }

        // 3. 预下载安装期要用的库 + Mojang 映射表（安装器自己联网会超时，见文档第 9 节）
        let mut files: Vec<FileSpec> = Vec::new();
        let manifest = forgelike::prefetch_manifest(&profile.libraries, &prepared.resolved.own.libraries);
        let libraries_dir = staging.join("libraries");
        for item in &manifest {
            let Some(url) = &item.url else { continue };
            files.push(FileSpec {
                dest: libraries_dir.join(&item.relative),
                url: url.clone(),
                sha1: item.sha1.clone(),
                size: item.size,
            });
        }
        if let Some(relative) = profile.mojmaps_relative_path() {
            if let Some(mappings) = parent
                .downloads
                .as_ref()
                .and_then(|d| d.client_mappings.as_ref())
            {
                files.push(FileSpec {
                    dest: libraries_dir.join(relative),
                    url: mappings.url.clone(),
                    sha1: mappings.sha1.clone(),
                    size: mappings.size.unwrap_or(0),
                });
            }
        }
        self.prep_download(task_id, id, game, files, cancel, emit)
            .await?;

        if cancel.is_cancelled() {
            return Err(AppError::canceled());
        }

        // 4. 跑安装器（老式安装器没有处理器，直接解包更快）
        if profile.is_legacy_extract() {
            let installer = installer.clone();
            let libraries = libraries_dir.clone();
            tokio::task::spawn_blocking(move || extract_embedded_maven(&installer, &libraries))
                .await
                .map_err(|e| AppError::unsupported("解包安装器失败", e.to_string()))??;
        } else {
            let requirement = xcl_core::launch::java_requirement(
                game,
                parent.java_version.as_ref().map(|j| j.major_version),
            );
            set_message(self, task_id, emit, "正在挑选 Java".to_string());
            let java = self.pick_java(requirement, None, emit).await?;
            set_message(self, task_id, emit, format!("正在安装 {label}"));
            run_installer(&java.path, installer, staging, cancel).await?;
        }

        // 5. 收编：库回数据根，原版 jar 复制成本实例的 jar
        set_message(self, task_id, emit, "正在收编安装结果".to_string());
        copy_tree(&libraries_dir, &self.layout.libraries_dir())?;
        self.reuse_parent_jar(game, id);

        // 6. 现代安装器必须产出处理器产物，缺了就等于没装成
        if !profile.is_legacy_extract() {
            let missing: Vec<&String> = manifest
                .iter()
                .filter(|item| item.url.is_none())
                .map(|item| &item.relative)
                .filter(|relative| !self.layout.libraries_dir().join(relative).is_file())
                .collect();
            if !missing.is_empty() {
                return Err(AppError::unsupported(
                    format!("{label} 安装器没有产出完整文件"),
                    format!("缺少：{}", missing[0]),
                )
                .with_hint("重试一次；还不行就把安装器日志发我看"));
            }
        }

        let _ = staged_jar;
        Ok(())
    }

    /// 准备阶段的一批下载：进度写进同一张任务卡，结束时任务仍是「进行中」。
    async fn prep_download(
        &self,
        task_id: &str,
        version_id: &str,
        parent: &str,
        files: Vec<FileSpec>,
        cancel: &CancellationToken,
        emit: &ProgressSink,
    ) -> Result<(), AppError> {
        if files.is_empty() {
            return Ok(());
        }
        let policy = self.download_policy();
        let mut items = Vec::with_capacity(files.len());
        for file in files {
            let urls = policy.candidates(xcl_source::UrlKind::Library, &file.url);
            if urls.is_empty() {
                continue;
            }
            let name = file
                .dest
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            let split_urls = crate::download::split_urls_for(&urls);
            items.push(xcl_download::DownloadItem {
                name,
                urls,
                split_urls,
                dest: file.dest,
                sha1: file.sha1,
                size: file.size,
            });
        }
        if items.is_empty() {
            return Ok(());
        }

        let total_files = items.len();
        let total_bytes: u64 = items.iter().map(|i| i.size).sum();
        let progress = xcl_download::Progress::new(total_bytes, total_files);
        let options = xcl_download::DownloadOptions {
            concurrency: self.effective_concurrency(),
            attempts_per_url: 2,
            split: self.split_config(),
        };
        let download = xcl_download::run(
            items,
            Arc::clone(self.fetcher()),
            Arc::clone(&progress),
            options,
            cancel.clone(),
        );
        tokio::pin!(download);

        let mut view = self
            .tasks()
            .get(task_id)
            .unwrap_or_else(|| TaskView::new(task_id, version_id, total_files, total_bytes));
        view.parent_id = Some(parent.to_string());
        view.total_files = total_files;
        view.total_bytes = total_bytes;

        // 速度和统一下载链路同一套算法（4 秒滑动窗口 + EMA）：
        // 直接拿两次采样差值会出现「0 → 1MB/s → 0」的抖动
        let mut meter = crate::speed::SpeedMeter::new(crate::speed::DEFAULT_WINDOW);
        let mut last_pushed: Option<(u64, u64)> = None;
        let mut ticker = tokio::time::interval(std::time::Duration::from_millis(
            crate::download::SAMPLE_MS,
        ));
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let summary = loop {
            tokio::select! {
                summary = &mut download => break summary,
                _ = ticker.tick() => {
                    let snapshot = progress.snapshot();
                    let speed = meter.update(snapshot.done_bytes, std::time::Instant::now());
                    apply_snapshot(&mut view, &snapshot);
                    view.speed_bps = speed;
                    // 速度为 0 时保留上一次 ETA：否则数字不动反而更难受
                    if speed > 0 {
                        view.eta_seconds = crate::download::eta(snapshot.total_bytes, snapshot.done_bytes, speed);
                    }
                    // 和统一下载链路同一条规则：数值没变就不推
                    if last_pushed != Some((snapshot.done_bytes, speed)) {
                        last_pushed = Some((snapshot.done_bytes, speed));
                        self.tasks().update(view.clone());
                        emit(view.clone());
                    }
                }
            }
        };

        apply_snapshot(&mut view, &progress.snapshot());
        view.speed_bps = 0;
        view.eta_seconds = None;
        self.tasks().update(view.clone());
        emit(view);

        if summary.cancelled {
            return Err(AppError::canceled());
        }
        if summary.failed > 0 {
            return Err(AppError::unsupported(
                "安装依赖下载失败",
                format!(
                    "{} 个文件没下下来：{}",
                    summary.failed,
                    summary
                        .failures
                        .first()
                        .cloned()
                        .unwrap_or_else(|| "未知文件".to_string())
                ),
            )
            .with_hint("检查网络后重试，已完成的部分会跳过"));
        }
        Ok(())
    }
}

/// 从合并后的详情里剥出「只属于加载器自己」的那部分（用于增量清单）。
fn own_part_only(
    merged: &VersionDetail,
    layout: &xcl_core::path::PathLayout,
    game: &str,
) -> VersionDetail {
    let mut own = merged.clone();
    if let Ok(parent) = local::load(layout, game) {
        let names: std::collections::HashSet<&str> =
            parent.libraries.iter().map(|l| l.name.as_str()).collect();
        own.libraries.retain(|l| !names.contains(l.name.as_str()));
        // 资源索引归原版：增量安装不该再逐个校验资源文件
        own.asset_index = None;
    }
    own
}

/// 一条待下载文件。
struct FileSpec {
    dest: PathBuf,
    url: String,
    sha1: Option<String>,
    size: u64,
}

/// 读安装器里的 `install_profile.json` 和 `version.json`（zip 是同步的，扔进阻塞线程）。
async fn read_installer_entries(installer: &Path) -> Result<InstallerFiles, AppError> {
    let installer = installer.to_path_buf();
    tokio::task::spawn_blocking(move || -> Result<InstallerFiles, AppError> {
        let file = fs::File::open(&installer).map_err(|e| AppError::io(&e, "打开安装器"))?;
        let mut archive =
            zip::ZipArchive::new(file).map_err(|e| AppError::parse("安装器不是有效 zip", e.to_string()))?;
        let profile_text = read_entry(&mut archive, "install_profile.json")?
            .ok_or_else(|| AppError::parse("安装器不完整", "缺少 install_profile.json"))?;
        let version_json_text = read_entry(&mut archive, "version.json")?;
        Ok(InstallerFiles {
            profile_text,
            version_json_text,
        })
    })
    .await
    .map_err(|e| AppError::unsupported("读取安装器失败", e.to_string()))?
}

fn read_entry(archive: &mut zip::ZipArchive<fs::File>, name: &str) -> Result<Option<String>, AppError> {
    use std::io::Read;
    let mut entry = match archive.by_name(name) {
        Ok(entry) => entry,
        Err(zip::result::ZipError::FileNotFound) => return Ok(None),
        Err(e) => return Err(AppError::parse("安装器条目读取失败", e.to_string())),
    };
    let mut text = String::new();
    entry
        .read_to_string(&mut text)
        .map_err(|e| AppError::parse("安装器条目不是文本", e.to_string()))?;
    Ok(Some(text))
}

/// 老式安装器（Forge ≤1.12.2）：把内嵌的 `maven/` 解到 `libraries/`。
fn extract_embedded_maven(installer: &Path, libraries: &Path) -> Result<(), AppError> {
    use std::io::Read;
    let file = fs::File::open(installer).map_err(|e| AppError::io(&e, "打开安装器"))?;
    let mut archive =
        zip::ZipArchive::new(file).map_err(|e| AppError::parse("安装器不是有效 zip", e.to_string()))?;
    for index in 0..archive.len() {
        let mut entry = archive
            .by_index(index)
            .map_err(|e| AppError::parse("安装器条目读取失败", e.to_string()))?;
        if entry.is_dir() {
            continue;
        }
        let Some(name) = entry.name().strip_prefix("maven/") else {
            continue;
        };
        let dest = libraries.join(name);
        if let Some(dir) = dest.parent() {
            fs::create_dir_all(dir).map_err(|e| AppError::io(&e, "创建库目录"))?;
        }
        let mut bytes = Vec::new();
        entry
            .read_to_end(&mut bytes)
            .map_err(|e| AppError::parse("解包安装器失败", e.to_string()))?;
        fs::write(&dest, bytes).map_err(|e| AppError::io(&e, "写入内嵌库"))?;
    }
    Ok(())
}

/// 跑官方安装器（`--installClient`）。取消时直接 kill。
async fn run_installer(
    java: &Path,
    installer: &Path,
    staging: &Path,
    cancel: &CancellationToken,
) -> Result<(), AppError> {
    // `--offline`：安装器启动时会自己去拉镜像列表 / 探测网络（`files.minecraftforge.net`），
    // 国内这一步经常挂住不返回，界面就是「进度 100% 但永远不结束」
    // （实测日志停在 Host 解析之后、连 "Extracting json" 都没打印）。
    // 我们事先已经把安装期要用的库和映射全下到 staged 目录里了，
    // 所以让它完全离线跑：既不会卡，也不会绕过选的下载源。
    let mut child = Command::new(java)
        .arg("-jar")
        .arg(installer)
        .arg("--installClient")
        .arg(staging)
        .arg("--offline")
        .current_dir(staging)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| AppError::io(&e, "启动 Forge 安装器"))?;

    let tail = Arc::new(Mutex::new(VecDeque::new()));
    let mut readers = Vec::new();
    if let Some(stdout) = child.stdout.take() {
        readers.push(spawn_reader(stdout, Arc::clone(&tail)));
    }
    if let Some(stderr) = child.stderr.take() {
        readers.push(spawn_reader(stderr, Arc::clone(&tail)));
    }

    let status = tokio::select! {
        status = child.wait() => status.map_err(|e| AppError::io(&e, "等待安装器结束"))?,
        _ = cancel.cancelled() => {
            let _ = child.kill().await;
            for reader in readers { reader.abort(); }
            return Err(AppError::canceled());
        }
        // 安装器自己卡死（等输入、网络挂住）时不能把界面永远吊在 100%
        _ = tokio::time::sleep(INSTALLER_TIMEOUT) => {
            let _ = child.kill().await;
            for reader in readers { reader.abort(); }
            return Err(AppError::unsupported(
                "加载器安装超时",
                format!("安装器超过 {} 分钟没有结束", INSTALLER_TIMEOUT.as_secs() / 60),
            )
            .with_hint("多半是安装器在联网下载时挂住了；检查网络后重试"));
        }
    };
    // 只给读线程一小段时间收尾，然后强制中止。
    //
    // 安装器会拉起处理器子进程（Java），子进程继承了我们给安装器的 stdout/stderr 管道：
    // 主进程退出后管道仍被它们持有，读线程等不到 EOF 就会一直挂住 ——
    // 界面上就是「进度 100% 但任务永远不结束」。我们只需要末尾几行日志，
    // 所以收完就掐掉，绝不等 EOF。
    for reader in readers {
        let _ = tokio::time::timeout(READER_GRACE, reader).await;
    }

    if !status.success() {
        let lines: Vec<String> = tail
            .lock()
            .map(|t| t.iter().cloned().collect())
            .unwrap_or_default();
        let tail_text = lines
            .iter()
            .rev()
            .take(LOG_TAIL)
            .rev()
            .cloned()
            .collect::<Vec<_>>()
            .join("\n");
        tracing::warn!(?status, "Forge 安装器失败：\n{tail_text}");
        return Err(AppError::unsupported(
            "加载器安装器执行失败",
            format!("安装器退出码 {status}，日志末尾：\n{tail_text}"),
        )
        .with_hint("检查网络后重试；日志里提到 Java 版本就换一个对应版本的 Java"));
    }
    Ok(())
}

fn spawn_reader<R>(stream: R, tail: Arc<Mutex<VecDeque<String>>>) -> tokio::task::JoinHandle<()>
where
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
{
    tokio::spawn(async move {
        let mut lines = BufReader::new(stream).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            tracing::debug!(target: "forgelike", "{line}");
            if let Ok(mut tail) = tail.lock() {
                tail.push_back(line);
                if tail.len() > LOG_TAIL {
                    tail.pop_front();
                }
            }
        }
    })
}

/// 递归复制目录，已存在且大小相同的文件跳过。
fn copy_tree(source: &Path, destination: &Path) -> Result<(), AppError> {
    if !source.is_dir() {
        return Ok(());
    }
    fs::create_dir_all(destination).map_err(|e| AppError::io(&e, "创建库目录"))?;
    for entry in fs::read_dir(source).map_err(|e| AppError::io(&e, "读取临时库目录"))? {
        let entry = entry.map_err(|e| AppError::io(&e, "读取临时库目录"))?;
        let from = entry.path();
        let to = destination.join(entry.file_name());
        let meta = entry.metadata().map_err(|e| AppError::io(&e, "读取文件信息"))?;
        if meta.is_dir() {
            copy_tree(&from, &to)?;
            continue;
        }
        if let Ok(existing) = fs::metadata(&to) {
            if existing.len() == meta.len() {
                continue;
            }
        }
        if let Some(dir) = to.parent() {
            fs::create_dir_all(dir).map_err(|e| AppError::io(&e, "创建库目录"))?;
        }
        fs::copy(&from, &to).map_err(|e| AppError::io(&e, "收编安装器产物"))?;
    }
    Ok(())
}

fn set_message(state: &AppState, task_id: &str, emit: &ProgressSink, message: String) {
    let Some(mut view) = state.tasks().get(task_id) else {
        return;
    };
    // 阶段切换后已经没有下载在进行：速度与剩余时间必须归零，
    // 否则界面上会一直挂着上一个阶段的“xx MB/s”
    view.speed_bps = 0;
    view.eta_seconds = None;
    view.message = Some(message);
    state.tasks().update(view.clone());
    emit(view);
}

/// 失败也要让任务卡收尾，否则界面会一直转圈。
///
/// 整合包安装也用这一份：它同样需要「准备阶段失败 → 卡转红」的收尾。
pub(crate) fn fail_task(state: &AppState, task_id: &str, error: &AppError, emit: &ProgressSink) {
    let Some(mut view) = state.tasks().get(task_id) else {
        return;
    };
    view.phase = if error.code == xcl_core::ErrorCode::Canceled {
        TaskPhase::Cancelled
    } else {
        TaskPhase::Failed
    };
    view.speed_bps = 0;
    view.eta_seconds = None;
    view.message = Some(if error.message.is_empty() {
        error.title.clone()
    } else {
        error.message.clone()
    });
    state.tasks().update(view.clone());
    emit(view);
}
