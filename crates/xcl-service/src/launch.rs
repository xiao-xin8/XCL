//! 启动游戏。
//!
//! 能离线做的都放前面：
//!
//! ```text
//! 读本地版本 JSON（含 inheritsFrom 合并，不联网）
//!   → 检查关键文件齐不齐
//!   → 挑 Java（探测 + 版本匹配）
//!   → 解 natives
//!   → 拼 classpath / 参数（xcl-core::launch）
//!   → 起进程
//!   → 日志逐行推给前端
//!   → 等退出，推终态
//! ```
//!
//! 「文件不齐」这一版不自动补下载，而是明确报错，得回去点安装。
//! 理由：启动流程里塞下载会让失败原因变得难以定位，不如两条路各走各的。

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use serde::Serialize;
use tokio_util::sync::CancellationToken;
use tracing::{debug, warn};
use xcl_core::config::MemoryMode;
use xcl_core::launch::{
    build_command, classpath_separator, java_requirement, LaunchPaths, LaunchRequest, WindowOptions,
};
use xcl_core::{detect_kind, maven_to_relative_path, AppError, CommandLine, Env, VersionDetail};
use xcl_launch::{java, natives, process};

use crate::download::ProgressSink;
use crate::{local, AppState};

/// 启动阶段。前端据此切换界面状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum LaunchPhase {
    /// 正在准备（读文件、找 Java、解 natives）
    Preparing,
    /// 进程已起来
    Running,
    /// 正常退出
    Exited,
    /// 异常退出
    Failed,
    /// 手动结束
    Cancelled,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LaunchStatus {
    pub session_id: String,
    pub version_id: String,
    pub phase: LaunchPhase,
    /// 一句话说明，已经是可以直接显示的中文
    pub message: String,
    pub exit_code: Option<i32>,
    /// 从进程启动到退出的毫秒数
    pub duration_ms: u64,
    /// 实际使用的 Java
    pub java_path: Option<String>,
    pub java_major: Option<u32>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LaunchLog {
    pub session_id: String,
    /// `stdout` / `stderr`
    pub stream: String,
    pub text: String,
}

/// 推给前端的事件。用内部标签区分，前端 `switch (e.kind)` 即可。
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum LaunchEvent {
    Status(LaunchStatus),
    Log(LaunchLog),
}

pub type LaunchSink = Arc<dyn Fn(LaunchEvent) + Send + Sync>;

// ---------- 会话表 ----------

/// 正在跑的游戏会话。用来防重复启动，以及提供「结束游戏」的入口。
#[derive(Default)]
pub struct LaunchSessions {
    entries: Mutex<HashMap<String, SessionEntry>>,
    /// 正在准备、还没进 `entries` 的版本。
    ///
    /// `prepare` 可能要走 Java 探测/下载，耗时从几百毫秒到几分钟。这段时间里
    /// 会话还没登记，连点两次「启动」会两个请求都通过 `running_for` 检查，
    /// 最后起两个游戏。这里用占位把窗口堵住，正常单次启动完全不受影响。
    launching: Arc<Mutex<HashSet<String>>>,
    seq: AtomicU64,
}

/// 启动占位的 RAII 凭据：离开作用域（成功、失败、取消、panic 展开）都会释放。
struct LaunchReservation {
    launching: Arc<Mutex<HashSet<String>>>,
    version_id: String,
}

impl Drop for LaunchReservation {
    fn drop(&mut self) {
        // 中毒也要把占位清掉，否则这个版本会一直被判成「正在启动」
        let mut set = self.launching.lock().unwrap_or_else(|e| e.into_inner());
        set.remove(&self.version_id);
    }
}

struct SessionEntry {
    version_id: String,
    cancel: CancellationToken,
    java_path: String,
    java_major: u32,
    started: Instant,
}

impl LaunchSessions {
    fn new_id(&self) -> String {
        let millis = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0);
        let seq = self.seq.fetch_add(1, Ordering::Relaxed);
        format!("game-{millis}-{seq}")
    }

    fn insert(&self, id: &str, entry: SessionEntry) {
        if let Ok(mut map) = self.entries.lock() {
            map.insert(id.to_string(), entry);
        }
    }

    fn remove(&self, id: &str) {
        if let Ok(mut map) = self.entries.lock() {
            map.remove(id);
        }
    }

    /// 同一版本已经在跑就直接返回它的会话 id，不再起第二个。
    fn running_for(&self, version_id: &str) -> Option<String> {
        let map = self.entries.lock().ok()?;
        map.iter()
            .find(|(_, entry)| entry.version_id == version_id)
            .map(|(id, _)| id.clone())
    }

    /// 有没有游戏正在跑（或正在准备）。
    ///
    /// 清理未使用资源前要问一句：游戏运行时删 `assets/` 里的东西，
    /// 它切维度、播音效时就可能读不到文件。
    pub fn any_active(&self) -> bool {
        let running = self
            .entries
            .lock()
            .map(|map| !map.is_empty())
            .unwrap_or(false);
        let launching = self
            .launching
            .lock()
            .map(|set| !set.is_empty())
            .unwrap_or(false);
        running || launching
    }

    /// 抢占「正在准备启动」的占位。已经有同版本在准备时返回 `None`。
    fn begin_launch(&self, version_id: &str) -> Option<LaunchReservation> {
        let mut set = self
            .launching
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if !set.insert(version_id.to_string()) {
            return None;
        }
        Some(LaunchReservation {
            launching: Arc::clone(&self.launching),
            version_id: version_id.to_string(),
        })
    }

    /// 结束某个会话（按会话 id 或版本 id 都行）。
    pub fn stop(&self, target: &str) -> Option<String> {
        let map = self.entries.lock().ok()?;
        let (id, entry) = map
            .iter()
            .find(|(id, entry)| id.as_str() == target || entry.version_id == target)?;
        entry.cancel.cancel();
        Some(id.clone())
    }

    /// 当前在跑的所有会话（给界面恢复状态用）。
    pub fn list(&self) -> Vec<LaunchStatus> {
        let Ok(map) = self.entries.lock() else {
            return Vec::new();
        };
        map.iter()
            .map(|(id, entry)| LaunchStatus {
                session_id: id.clone(),
                version_id: entry.version_id.clone(),
                phase: LaunchPhase::Running,
                message: "游戏运行中".to_string(),
                exit_code: None,
                duration_ms: entry.started.elapsed().as_millis() as u64,
                java_path: Some(entry.java_path.clone()),
                java_major: Some(entry.java_major),
            })
            .collect()
    }
}

// ---------- 启动 ----------

/// 准备阶段的产物。
struct Prepared {
    command: CommandLine,
    working_dir: PathBuf,
    java_path: PathBuf,
    java_major: u32,
}

impl AppState {
    /// 启动一个版本。立刻返回「准备中」状态，后续通过 `sink` 推事件。
    ///
    /// `downloads` 是下载任务表的通道：缺 Java 时会直接开下（不问），
    /// 那个任务得能在统一的下载抽屉里看到进度。
    pub async fn launch_start(
        self: &Arc<Self>,
        version_id: &str,
        sink: LaunchSink,
        downloads: ProgressSink,
    ) -> Result<LaunchStatus, AppError> {
        if let Some(running) = self.launches.running_for(version_id) {
            return Err(AppError::unsupported(
                "这个版本已经在运行了",
                format!("{version_id} 已经有一个游戏进程（{running}）"),
            )
            .with_hint("先关掉正在运行的游戏，或者从任务栏切回去"));
        }

        // 准备阶段（Java 探测/下载可能很慢）也要挡住第二次启动。
        // 凭据在函数返回时自动释放；成功后由下面的会话表接管。
        let _reservation = match self.launches.begin_launch(version_id) {
            Some(reservation) => reservation,
            None => {
                return Err(AppError::unsupported(
                    "这个版本正在启动",
                    format!("{version_id} 已经有一个启动流程在准备中"),
                )
                .with_hint("等它完成；如果界面一直没有反应，可以重启启动器"));
            }
        };

        let session_id = self.launches.new_id();
        let cancel = CancellationToken::new();

        // 准备阶段就一句话：正在启动。
        // 不报「第几步」—— 这六步在本机 200ms 内就过去了，报步数只会闪一下；
        // 真正要等的是进程起来到窗口出现，而那一段没有可信的进度可报。
        sink(LaunchEvent::Status(LaunchStatus {
            session_id: session_id.clone(),
            version_id: version_id.to_string(),
            phase: LaunchPhase::Preparing,
            message: "正在启动".to_string(),
            exit_code: None,
            duration_ms: 0,
            java_path: None,
            java_major: None,
        }));

        let prepared = match self.prepare(version_id, &downloads).await {
            Ok(prepared) => prepared,
            Err(err) => {
                // 准备失败也要让界面知道，否则会一直停在「准备中」
                sink(LaunchEvent::Status(LaunchStatus {
                    session_id,
                    version_id: version_id.to_string(),
                    phase: LaunchPhase::Failed,
                    message: err.message.clone(),
                    exit_code: None,
                    duration_ms: 0,
                    java_path: None,
                    java_major: None,
                }));
                return Err(err);
            }
        };

        self.launches.insert(
            &session_id,
            SessionEntry {
                version_id: version_id.to_string(),
                cancel: cancel.clone(),
                java_path: prepared.java_path.display().to_string(),
                java_major: prepared.java_major,
                started: Instant::now(),
            },
        );

        // 高性能显卡：把 java.exe 和启动器本身都标成「高性能」（只有 Windows 做实事）。
        // 启动前命令：启动前写，失败只警告、不拦启动。
        if self.config().launch.high_perf_gpu {
            let launcher = std::env::current_exe().ok();
            let mut exes: Vec<&Path> = vec![prepared.java_path.as_path()];
            if let Some(exe) = launcher.as_deref() {
                exes.push(exe);
            }
            if let Err(err) = xcl_launch::os::prefer_high_perf_gpu(&exes) {
                warn!("设置高性能显卡失败：{err}");
            }
        }

        let java_path_text = prepared.java_path.display().to_string();
        let game = process::spawn(&process::SpawnOptions {
            java: &prepared.java_path,
            command: &prepared.command,
            working_dir: &prepared.working_dir,
            envs: &[],
        });

        let game = match game {
            Ok(game) => game,
            Err(err) => {
                self.launches.remove(&session_id);
                sink(LaunchEvent::Status(LaunchStatus {
                    session_id,
                    version_id: version_id.to_string(),
                    phase: LaunchPhase::Failed,
                    message: err.message.clone(),
                    exit_code: None,
                    duration_ms: 0,
                    java_path: Some(java_path_text),
                    java_major: Some(prepared.java_major),
                }));
                return Err(err);
            }
        };

        // 进程优先级：拉起之后才设；失败只记日志，不影响启动
        if let Some(pid) = game.child.id() {
            if let Err(err) =
                xcl_launch::os::set_process_priority(pid, self.config().launch.priority)
            {
                warn!("设置进程优先级失败：{err}");
            }
        }

        // 进程起来了，但还不算「运行中」—— 窗口要等几秒才出来。
        // 这一步只回一个「正在启动」，真正的 Running 由 `run_session` 判定就绪后推。
        let initial = LaunchStatus {
            session_id: session_id.clone(),
            version_id: version_id.to_string(),
            phase: LaunchPhase::Preparing,
            message: "正在启动".to_string(),
            exit_code: None,
            duration_ms: 0,
            java_path: Some(java_path_text.clone()),
            java_major: Some(prepared.java_major),
        };

        // 记一笔「上次启动」，详情页显示「2 小时前」用
        self.mark_version_played(version_id);

        // 后台跑：读日志 + 判就绪 + 等退出 + 推终态
        let state = Arc::clone(self);
        let version = version_id.to_string();
        let session = session_id.clone();
        // GameProcess 的字段都是 pub，直接拆开即可
        let process::GameProcess { child, logs } = game;
        tokio::spawn(async move {
            run_session(
                state,
                session,
                version,
                java_path_text,
                prepared.java_major,
                child,
                logs,
                cancel,
                sink,
            )
            .await;
        });

        Ok(initial)
    }

    /// 结束正在运行的游戏。
    pub fn launch_stop(&self, target: &str) -> bool {
        self.launches.stop(target).is_some()
    }

    pub fn launch_sessions(&self) -> Vec<LaunchStatus> {
        self.launches.list()
    }

    // ---- 准备阶段 ----

    async fn prepare(
        self: &Arc<Self>,
        version_id: &str,
        // 缺 Java 时自动补下载，任务要通过这个通道出现在统一的下载抽屉里
        downloads: &ProgressSink,
    ) -> Result<Prepared, AppError> {
        // 配置快照：整个准备阶段用同一份，避免中途设置页改了配置导致前后不一致
        let config = self.config();
        // 这个版本的最终启动值：版本设置里的覆盖优先，没覆盖的才用全局。
        // 和弹窗里显示的「最终生效」是同一个函数算的，不会两边对不上。
        let launch_values =
            crate::version_settings::resolve_launch(&config, &self.layout, version_id);

        // 1. 本地解析（含 inheritsFrom），全程不联网
        let detail = self.resolve_local(version_id)?;

        // 2. 版本隔离：只换游戏目录，库和资源仍然共享
        let kind = detect_kind(
            version_id,
            detail.version_type.as_deref(),
            detail.main_class.as_deref(),
            &detail.libraries,
        );
        let isolation = crate::isolation::resolve(
            &self.layout,
            &config,
            version_id,
            kind,
            detail.version_type.as_deref(),
        );
        let game_dir = isolation.game_dir.clone();
        debug!(
            isolated = isolation.enabled,
            source = ?isolation.source,
            game_dir = %game_dir.display(),
            "版本隔离"
        );
        // 工作目录必须存在，否则起进程会直接失败；1.0 及更早的版本不会走
        // ensure_language，所以这里不能省。
        fs::create_dir_all(&game_dir).map_err(|e| AppError::io(&e, "创建游戏目录"))?;

        let env = Env::current()
            // 官方 JSON 里 --width/--height 靠这个 feature 开关
            .with_feature("has_custom_resolution", true)
            .with_feature("is_demo_user", false);

        // 3. classpath 与 natives，同时收集缺了哪些文件
        let (classpath, native_archives, missing) =
            self.collect_classpath(&detail, version_id, &env)?;
        if !missing.is_empty() {
            let sample = missing
                .iter()
                .take(3)
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>()
                .join("\n");
            return Err(AppError::not_found(
                "游戏文件不完整",
                format!(
                    "还缺 {} 个文件，例如：\n{sample}",
                    missing.len()
                ),
            )
            .with_hint("回到下载页对这个版本点「校验并修复」，补齐后再启动"));
        }

        // 4. 挑 Java（没有就自动下一个，见 `pick_java`）
        let requirement = java_requirement(
            version_id,
            detail.java_version.as_ref().map(|v| v.major_version),
        );
        let java = self
            .pick_java(requirement, launch_values.java_path.as_deref(), downloads)
            .await?;

        // 5. 解 natives
        //    解压是同步 zip + 文件 IO，放阻塞线程，别占 tokio worker。
        let natives_dir = self.layout.version_natives_dir(version_id);
        let extract_dir = natives_dir.clone();
        tokio::task::spawn_blocking(move || natives::extract(&native_archives, &extract_dir))
            .await
            .map_err(|err| AppError::parse("解压 natives 失败", err.to_string()))??;

        // 6. 拼参数
        //    注意：这里是合并后的值 —— 版本设置里的覆盖优先，没覆盖才用全局。
        //    快照在最开头就取好了，准备过程中改设置不会影响本次启动。
        // 账号决定 --uuid / --accessToken / --userType；第三方账号还会带来
        // 一串 -javaagent JVM 参数（authlib-injector）
        // 登录：第三方账号每次启动都先确认凭据还有效，
        // 但能刷新就不重登），离线账号这一步不联网、直接过。
        self.ensure_authlib_session().await?;
        let player = self.player_profile(version_id)?;
        let mut extra_jvm = self.authlib_jvm_args().await?;
        extra_jvm.extend(split_user_args(&launch_values.jvm_args));

        // 自动内存按「当前可用内存 + 这个实例的 mod 数」算（阶梯分配）；
        // 手动就用配置里的值。放到请求构造前，`build_command` 只负责把最终值写成 `-Xmx`。
        let memory_mb = match launch_values.memory_mode {
            MemoryMode::Auto => self.auto_memory_for(version_id),
            MemoryMode::Custom => launch_values.memory_mb.max(512),
        };
        debug!(
            memory_mb,
            mode = ?launch_values.memory_mode,
            "内存分配"
        );

        let request = LaunchRequest {
            version_id: version_id.to_string(),
            version_type: detail
                .version_type
                .clone()
                .unwrap_or_else(|| "release".to_string()),
            player,
            paths: LaunchPaths {
                // 隔离时是 versions/<id>，否则就是数据根
                game_dir: game_dir.clone(),
                assets_dir: self.layout.assets_dir(),
                asset_index: asset_index_name(&detail),
                natives_dir: natives_dir.clone(),
                libraries_dir: self.layout.libraries_dir(),
                classpath,
                client_jar: local::client_jar(&self.layout, version_id),
            },
            java_major: java.major,
            memory_mb,
            window: WindowOptions {
                width: launch_values.window_width,
                height: launch_values.window_height,
                fullscreen: launch_values.fullscreen,
            },
            extra_jvm,
            extra_game: split_user_args(&launch_values.game_args),
        };

        // 7. 语言：MC 没有 --lang 启动参数，启动前把默认简体中文写进
        //    options.txt（进游戏改过就保留原样）。写坏顶多丢键位设置，
        //   不值得为它拦住启动 —— 只记日志。
        //
        //    目录名可能只是 `fabric-loader-0.20.1-26.2`：先按目录名判断语言的
        //    新旧代际，认不出来再用安装时写进 instance.toml 的 `gameVersion`。
        let game_version = crate::isolation::settings_at(
            &self.layout.version_settings_file(version_id),
        )
        .game_version;
        let language_source = language_era_source(version_id, game_version.as_deref());
        if let Err(e) = ensure_language(&game_dir, &launch_values.language, &language_source) {
            warn!(error = %e, "写入游戏语言失败，沿用 options.txt 现状");
        }

        // 8. 启动前命令：工作目录是游戏目录，失败不拦启动
        if !launch_values.pre_launch_command.trim().is_empty() {
            if let Err(e) = run_pre_launch_command(
                &launch_values.pre_launch_command,
                &game_dir,
                launch_values.wait_command,
            )
            .await
            {
                warn!(error = %e, "启动前命令执行失败，继续启动");
            }
        }

        // 9. 内存优化：给游戏腾一点现成的内存。
        //    有管理员 token 就走系统级深度清理，没有就自动降级成逐进程清 ——
        //    不弹任何权限框，具体见 `memory::trim_memory`。
        //    这是附加动作，失败/跳过都不该拦住启动。
        if self.config().launch.trim_memory {
            match tokio::task::spawn_blocking(crate::memory::trim_memory).await {
                Ok(report) => debug!(
                    mode = report.mode.label(),
                    count = report.count,
                    freedMb = report.freed_mb,
                    "启动前内存优化完成"
                ),
                Err(err) => warn!(error = %err, "内存优化失败，继续启动"),
            }
        }

        let command = build_command(&detail, &request, &env)?;

        Ok(Prepared {
            command,
            working_dir: game_dir,
            java_path: java.path,
            java_major: java.major,
        })
    }

    /// 沿 `inheritsFrom` 一路向上合并，全部从本地读。
    fn resolve_local(&self, version_id: &str) -> Result<VersionDetail, AppError> {
        let mut merged = local::load(&self.layout, version_id)?;
        let mut next = merged.inherits_from.clone();
        let mut visited = vec![version_id.to_string()];

        // 正常最多两三层；超过 8 层说明数据有问题，直接停，避免死循环
        while let Some(parent_id) = next {
            if visited.contains(&parent_id) || visited.len() >= 8 {
                break;
            }
            visited.push(parent_id.clone());
            let parent = local::load(&self.layout, &parent_id)?;
            next = parent.inherits_from.clone();
            merged = xcl_core::merge_versions(merged, parent);
        }

        Ok(merged)
    }

    /// 收集 classpath、natives 包，以及缺失文件。
    fn collect_classpath(
        &self,
        detail: &VersionDetail,
        version_id: &str,
        env: &Env,
    ) -> Result<(Vec<PathBuf>, Vec<natives::NativeArchive>, Vec<PathBuf>), AppError> {
        let libraries_dir = self.layout.libraries_dir();
        let mut jars: Vec<PathBuf> = Vec::new();
        let mut archives: Vec<natives::NativeArchive> = Vec::new();
        let mut missing: Vec<PathBuf> = Vec::new();
        // 同一个「组:构件」只允许进一次 classpath（先声明的赢）。
        //
        // 兜住已经装好的旧实例：合并逻辑以前不过滤重复，原版 1.21.10 自带
        // `asm:9.6`、Fabric 自带 `asm:9.10.1`，两份都在 → Fabric 启动前自检
        // 直接报 `duplicate ASM classes`。这里按声明顺序保留第一份（子版本在前）。
        let mut seen_libraries: std::collections::HashSet<String> =
            std::collections::HashSet::new();

        for library in detail.libraries.iter().filter(|lib| lib.is_applicable(env)) {
            // natives 包：解压用，不进 classpath
            if library
                .natives
                .as_ref()
                .is_some_and(|map| map.contains_key(&env.os_name))
            {
                if let Some(native) = library.native_artifact(&env.os_name) {
                    let relative = native
                        .path
                        .clone()
                        .or_else(|| native_relative_path(library, env));
                    if let Some(relative) = relative {
                        let local = libraries_dir.join(relative);
                        if !local.is_file() {
                            missing.push(local.clone());
                        }
                        archives.push(natives::NativeArchive {
                            jar: local,
                            excludes: library.extract_excludes().to_vec(),
                        });
                    }
                }
            }

            // 主 jar：进 classpath，顺序按 JSON 声明顺序
            //
            // `resolved_artifact` 而不是 `artifact`：Fabric 的库只有 maven 坐标
            // 没有 `downloads`，只认后者会让它们一个都进不了 classpath。
            if let Some(artifact) = library.resolved_artifact() {
                // 判重键带 classifier：`…:natives-windows` 和主 jar 必须都留下
                if !seen_libraries.insert(xcl_core::library_dedupe_key(&library.name)) {
                    continue;
                }
                let relative = artifact
                    .path
                    .clone()
                    .or_else(|| maven_to_relative_path(&library.name));
                if let Some(relative) = relative {
                    let local = libraries_dir.join(relative);
                    if !local.is_file() {
                        missing.push(local.clone());
                    }
                    jars.push(local);
                }
            }
        }

        // 客户端 jar 必须在最后（classpath 顺序有语义）
        match local::client_jar(&self.layout, version_id).or_else(|| {
            detail
                .inherits_from
                .as_ref()
                .and_then(|parent| local::client_jar(&self.layout, parent))
        }) {
            Some(jar) => jars.push(jar),
            None => {
                return Err(AppError::not_found(
                    "找不到客户端 jar",
                    format!("{version_id} 的目录里没有 .jar 文件"),
                )
                .with_hint("重新安装这个版本"));
            }
        }

        // 资源索引文件必须存在，否则游戏启动即报错
        let assets_dir = self.layout.assets_dir();
        let index_name = asset_index_name(detail);
        let index_file = self.layout.assets_indexes_dir().join(format!("{index_name}.json"));
        if !index_file.is_file() {
            return Err(AppError::not_found(
                "缺少资源索引",
                format!(
                    "{assets_dir} 下找不到 indexes/{index_name}.json",
                    assets_dir = assets_dir.display()
                ),
            )
            .with_hint("在下载页对这个版本点「校验并修复」，会补下缺失的资源索引"));
        }

        Ok((jars, archives, missing))
    }

    /// 挑一个能用的 Java；本机没有合适的就直接开下官方运行时，下完接着启动。
    ///
    /// 不问：装 Java 是启动的必经步骤，问一遍只会多一次点击。
    /// 唯一会停下来报错的情况是「这个 Java 需求官方根本不发运行时」（比如 Java 11），
    /// 那种只能自己装。
    pub(crate) async fn pick_java(
        self: &Arc<Self>,
        requirement: xcl_core::JavaRequirement,
        // 版本设置里指定的路径（优先于全局的手动路径）
        version_path: Option<&str>,
        downloads: &ProgressSink,
    ) -> Result<java::JavaInstall, AppError> {
        // 手动指定优先，且不校验版本 —— 既然手填了，就按手填的来。
        // 顺序是「版本设置 > 全局设置」：版本那个是更具体的选择。
        let manual_path = version_path
            .map(std::path::PathBuf::from)
            .or_else(|| self.config().java.manual_path);
        if let Some(manual) = &manual_path {
            if let Some(found) = java::probe(manual).await {
                return Ok(found);
            }
            warn!(path = %manual.display(), "手动指定的 Java 不可用，回落到自动探测");
        }

        // 除了系统里的，还要扫我们自己下到 data/java/ 的运行时
        let installs = java::detect_all_with(&self.java_runtime_roots()).await;
        if let Some(found) = java::pick(&installs, requirement.min, requirement.max) {
            return Ok(found.clone());
        }

        // 没有可用的 → 自动补：官方有对应运行时就直接下
        let component = self.install_java_for(&requirement, downloads).await?;

        // 装完直接探这一个可执行文件，比再全盘扫一遍快得多
        let exe = self.layout.java_executable(component);
        match java::probe(&exe).await {
            Some(found) => Ok(found),
            None => Err(AppError::not_found(
                "刚下好的 Java 用不了",
                format!("{} 存在，但跑不起来", exe.display()),
            )
            .with_hint("到「下载 → Java 运行时」里删掉重下一遍")),
        }
    }

    /// 缺 Java 时按需求挑一个官方运行时并等它装完，返回组件名。
    async fn install_java_for(
        self: &Arc<Self>,
        requirement: &xcl_core::JavaRequirement,
        downloads: &ProgressSink,
    ) -> Result<&'static str, AppError> {
        let Some((component, _)) = xcl_core::java_runtime::choose_component(requirement) else {
            // 官方没发这个版本（Java 11 之类），只能自己找
            return Err(AppError::unsupported(
                "找不到合适的 Java",
                format!(
                    "这个版本{}，而官方没有对应的运行时可以自动下载。",
                    requirement.describe()
                ),
            )
            .with_hint("装一个对应版本的 Java；也可以在设置里手动指定 java.exe 路径"));
        };

        tracing::info!(component, "缺少可用 Java，自动下载官方运行时");
        self.java_install_and_wait(component, Arc::clone(downloads))
            .await?;
        Ok(component)
    }
}

/// 等不到「材质已加载」那行日志时的兜底时长。
///
/// 只是个安全网：超时也当它起来了，免得某些把日志吞掉的整合包永远停在
/// 「正在启动」。正常版本几十毫秒到几秒就会命中日志，用不到它。
const READY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

/// 这行日志说明游戏已经把资源加载完、主界面在了。
fn logs_ready(text: &str) -> bool {
    let has = |needle: &str| text.contains(needle);
    (has("Created") && has("textures") && has("-atlas")) || has("Found animation info")
}

/// 后台读取日志并等进程退出。
///
/// 进程一起来不算「运行中」：那会儿 JVM 还在加载资源，窗口得等几秒才出来。
/// 所以这里挂一个就绪判定，Ready 了才推 `Running`；在此之前界面一直显示
/// 「正在启动」。
///
/// 判据只用来决定状态，不产生任何进度数字：
/// - 日志里出现材质图集那行（`Created: ... textures ... -atlas` / `Found animation info`）
///   —— 1.7+ 都有效；
/// - 兜底：进程起来 [`READY_TIMEOUT`] 还没等到（有的整合包日志被 mod 吞了），
///   就当它起来了，别一直停在「正在启动」。
async fn run_session(
    state: Arc<AppState>,
    session_id: String,
    version_id: String,
    java_path: String,
    java_major: u32,
    mut child: tokio::process::Child,
    mut logs: tokio::sync::mpsc::UnboundedReceiver<process::LogLine>,
    cancel: CancellationToken,
    sink: LaunchSink,
) {
    let started = Instant::now();
    let mut logs_open = true;
    let mut killed = false;
    let mut ready = false;
    let ready_deadline = tokio::time::Instant::now() + READY_TIMEOUT;

    let status = loop {
        tokio::select! {
            biased;

            _ = cancel.cancelled(), if !killed => {
                killed = true;
                let _ = child.start_kill();
            }

            // 兜底：等不到材质日志也放行
            _ = tokio::time::sleep_until(ready_deadline), if !ready => {
                ready = true;
                sink(LaunchEvent::Status(LaunchStatus {
                    session_id: session_id.clone(),
                    version_id: version_id.clone(),
                    phase: LaunchPhase::Running,
                    message: "游戏运行中".to_string(),
                    exit_code: None,
                    duration_ms: started.elapsed().as_millis() as u64,
                    java_path: Some(java_path.clone()),
                    java_major: Some(java_major),
                }));
            }

            line = logs.recv(), if logs_open => {
                match line {
                    Some(line) => {
                        if !ready && logs_ready(&line.text) {
                            ready = true;
                            sink(LaunchEvent::Status(LaunchStatus {
                                session_id: session_id.clone(),
                                version_id: version_id.clone(),
                                phase: LaunchPhase::Running,
                                message: "游戏运行中".to_string(),
                                exit_code: None,
                                duration_ms: started.elapsed().as_millis() as u64,
                                java_path: Some(java_path.clone()),
                                java_major: Some(java_major),
                            }));
                        }
                        sink(LaunchEvent::Log(LaunchLog {
                            session_id: session_id.clone(),
                            stream: match line.stream {
                                process::LogStream::Stdout => "stdout".to_string(),
                                process::LogStream::Stderr => "stderr".to_string(),
                            },
                            text: line.text,
                        }));
                    }
                    None => logs_open = false,
                }
            }

            waited = child.wait() => break waited,
        }
    };

    let duration_ms = started.elapsed().as_millis() as u64;
    let exit_code = status.as_ref().ok().and_then(|s| s.code());
    let (phase, message) = match &status {
        Ok(status) if killed || cancel.is_cancelled() => {
            (LaunchPhase::Cancelled, "游戏已关闭".to_string())
        }
        Ok(status) => {
            let text = process::describe_exit(status);
            if status.success() {
                (LaunchPhase::Exited, text)
            } else {
                (LaunchPhase::Failed, text)
            }
        }
        Err(err) => (
            LaunchPhase::Failed,
            format!("等待游戏进程时出错：{err}"),
        ),
    };

    debug!(version = version_id, ?phase, duration_ms, "游戏会话结束");
    sink(LaunchEvent::Status(LaunchStatus {
        session_id: session_id.clone(),
        version_id,
        phase,
        message,
        exit_code,
        duration_ms,
        java_path: None,
        java_major: None,
    }));

    state.launches.remove(&session_id);
}

/// `--assetIndex` 用哪个名字，也就是 `assets/indexes/<name>.json` 的文件名。
///
/// 优先 `assets` 字段，其次索引对象自己的 id。
/// 安装侧必须调同一个函数决定落盘文件名 —— 两边不一致就会「装了但启动说找不到」
/// （例如 1.6.4：`assets` 是 `pre-1.6`，而 `assetIndex.id` 是 `legacy`）。
pub(crate) fn asset_index_name(detail: &VersionDetail) -> String {
    detail
        .assets
        .clone()
        .or_else(|| detail.asset_index.as_ref().map(|index| index.id.clone()))
        .unwrap_or_else(|| "legacy".to_string())
}

/// natives jar 的兜底路径：用 maven 坐标 + classifier 推导。
fn native_relative_path(library: &xcl_core::version::Library, env: &Env) -> Option<String> {
    let classifier = library.native_classifier(&env.os_name)?;
    maven_to_relative_path(&format!("{}:{}", library.name, classifier))
}

/// 手填的额外参数：按空白切，忽略空串。
fn split_user_args(raw: &str) -> Vec<String> {
    raw.split_whitespace().map(str::to_string).collect()
}

/// MC 按「语言选项怎么写」分三代：
/// - `Ancient`：1.0 及更早（含 Alpha / Beta / Infdev）—— options.txt 里
///   根本没有语言选项，写了也不读
/// - `Legacy`：1.1 ~ 1.10 —— 地区码必须大写（`zh_CN`）。小写在 1.6~1.10
///   会静默退回英文，在 1.1~1.5 直接 NPE 崩溃
/// - `Modern`：1.11 及更新（含周快照）—— 地区码必须小写（`zh_cn`），
///   大写会静默退回英文
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LangEra {
    Ancient,
    Legacy,
    Modern,
}

/// 从版本 id 推断语言代。id 形如 "1.12.2" / "1.7.10" / "24w14a" / "b1.7.3" / "1.0"，
/// 也可能是 loader 的目录名（"fabric-loader-0.15.11-1.20.1"）或新年份版本（"26.2"）。
fn lang_era(version_id: &str) -> LangEra {
    let id = version_id.trim();
    let lower = id.to_ascii_lowercase();
    // Alpha / Beta / Infdev（"b1.7.3" / "a1.2.5" / "rd-132211"）都在 1.0 之前。
    // 必须先判断：这些 id 里也含 "1.x"，按普通规则会被误判成现代版本。
    if lower.starts_with('b')
        || lower.starts_with('a')
        || lower.starts_with("rd-")
        || lower.starts_with("inf-")
    {
        return LangEra::Ancient;
    }
    // 周快照（"24w14a"：两位年份 + w）都在 1.11 之后
    if id.len() >= 3 && id.as_bytes()[2] == b'w' && id[..2].parse::<u32>().is_ok() {
        return LangEra::Modern;
    }
    // 版本 id 里任意位置出现的 "1.X"（loader 目录名会把原版号放在末尾）
    if let Some(major) = minecraft_major_in(id) {
        return match major {
            // "1.0" 及解析不出的老写法都按远古版本处理：不写比写错安全
            0 => LangEra::Ancient,
            1..=10 => LangEra::Legacy,
            _ => LangEra::Modern,
        };
    }
    // Mojang 的新年份版本号（"26.2"）没有 "1." 前缀；主版本 >= 11 按现代处理
    if let Some(major) = leading_major(id) {
        if major >= 11 {
            return LangEra::Modern;
        }
    }
    // 加载器目录名里的年份版本："fabric-loader-0.20.1-26.2"。
    // 只看末尾的 `XX.Y`，且门槛抬到 20，避免把 Forge 14.x / Fabric 0.20.x
    // 这类加载器版本误认成 MC 版本。
    if year_major_in(id).is_some() {
        return LangEra::Modern;
    }
    LangEra::Ancient
}

/// 找 id 里第一个独立的 "1.<数字>"，返回小数点后的主版本号。
///
/// "1.20.1" / "fabric-loader-0.15.11-1.20.1" / "1.12.2-forge1.12.2-..." 都能命中；
/// "21.1.72" 里 "21." 的 1 前面是数字，不算。
fn minecraft_major_in(id: &str) -> Option<u32> {
    let bytes = id.as_bytes();
    let mut i = 0;
    while i + 2 < bytes.len() {
        let independent = i == 0 || !(bytes[i - 1].is_ascii_digit() || bytes[i - 1] == b'.');
        if bytes[i] == b'1' && bytes[i + 1] == b'.' && independent {
            let start = i + 2;
            let mut end = start;
            while end < bytes.len() && bytes[end].is_ascii_digit() {
                end += 1;
            }
            if end > start {
                return id[start..end].parse().ok();
            }
        }
        i += 1;
    }
    None
}

/// id 开头就是数字的情况（"26.2"），返回大版本号。
fn leading_major(id: &str) -> Option<u32> {
    let digits: String = id.chars().take_while(|c| c.is_ascii_digit()).collect();
    if digits.is_empty() || !id[digits.len()..].starts_with('.') {
        return None;
    }
    digits.parse().ok()
}

/// 加载器目录名末尾的年份版本："fabric-loader-0.20.1-26.2" → 26。
///
/// 只认 `XX.Y`，且 `XX >= 20`：Mojang 的新命名从 26.x 开始；Forge 14.x /
/// Fabric 0.20.x 这类加载器版本不能被当成 MC 版本。
fn year_major_in(id: &str) -> Option<u32> {
    id.split(['-', '_', ' '])
        .rev()
        .find_map(|token| {
            let mut parts = token.split('.');
            let major = parts.next()?.parse::<u32>().ok()?;
            let _minor = parts.next()?.parse::<u32>().ok()?;
            (major >= 20).then_some(major)
        })
}

/// 版本 id 对应的 MC 大版本号："1.20.1" → 20，"26.2" → 26，周快照 → `None`。
///
/// 给「这个版本有没有某功能」一类判断用（如材质包 / 资源包的分界）。
pub(crate) fn minecraft_major(id: &str) -> Option<u32> {
    minecraft_major_in(id)
        .or_else(|| leading_major(id))
        .or_else(|| year_major_in(id))
}

/// 语言代际判断用哪个版本号：目录名认不出来时，用实例设置里的 `gameVersion`。
///
/// 这是 `fabric-loader-0.20.1-26.2` 这类目录名的兜底：即使以后命名再变，
/// 安装/改名时写进 `instance.toml` 的原版版本号仍然是准的。
fn language_era_source(version_id: &str, game_version: Option<&str>) -> String {
    if matches!(lang_era(version_id), LangEra::Ancient) {
        if let Some(game) = game_version.map(str::trim).filter(|v| !v.is_empty()) {
            return game.to_string();
        }
    }
    version_id.to_string()
}

/// 按 MC 版本把配置的语言值整理成 options.txt 里该写的样子；
/// `None` = 这个版本没有语言选项，别写。
fn lang_value_for_version(version_id: &str, language: &str) -> Option<String> {
    let lang = language.trim();
    let lang = if lang.is_empty() { "zh_cn" } else { lang };
    match lang_era(version_id) {
        LangEra::Ancient => None,
        LangEra::Legacy => Some(match lang.split_once('_') {
            Some((head, tail)) => format!("{head}_{}", tail.to_uppercase()),
            None => lang.to_string(),
        }),
        LangEra::Modern => Some(lang.to_string()),
    }
}

/// 确保游戏目录的 `options.txt` 用的是启动器里配置的语言。
///
/// MC 客户端没有 `--lang` 启动参数 —— 语言只认游戏目录下 `options.txt`
/// 的 `lang:` 行（启动前写这个文件）。规则：
/// - 文件不存在 → 新建，只写 lang 一行（CJK 语言一并打开 forceUnicodeFont，
///   否则旧版 MC 里中文容易渲染成方括号）
/// - 已有 lang 行 → 改写这一行的值，其余行原样保留。启动器里的语言设置
///   是准的：不然上次在游戏里切成英文、或者旧启动器写过 en_us，就会一直
///   卡在英文（有存档时保留旧值）
/// - 没有 lang 行 → 其余行原样保留，末尾追加
///
/// 只动 lang 这一行，键位 / 音量 / 资源包都不碰。
/// options.txt 写坏顶多丢键位设置，不值得为它拦住启动，所以调用方出错只记日志。
fn ensure_language(game_dir: &Path, language: &str, version_id: &str) -> Result<(), AppError> {
    let Some(lang) = lang_value_for_version(version_id, language) else {
        // 1.0 及更早的版本压根没有语言选项，options.txt 里写了也不读
        return Ok(());
    };

    let path = game_dir.join("options.txt");
    let existing = fs::read_to_string(&path).unwrap_or_default();
    let fresh = existing.is_empty();
    let canonical = format!("lang:{lang}");

    let mut text = String::with_capacity(existing.len() + 24);
    let mut found = false;
    for line in existing.split_inclusive('\n') {
        // 保留原换行样式：Windows 上 options.txt 常是 CRLF，别顺手全改掉
        let (content, ending) = if let Some(body) = line.strip_suffix("\r\n") {
            (body, "\r\n")
        } else if let Some(body) = line.strip_suffix('\n') {
            (body, "\n")
        } else {
            (line, "")
        };
        let is_lang = content
            .split_once(':')
            .is_some_and(|(key, _)| key.trim() == "lang");
        if !is_lang {
            text.push_str(line);
            continue;
        }
        found = true;
        if content == canonical {
            text.push_str(line);
        } else {
            text.push_str(&canonical);
            text.push_str(ending);
        }
    }
    if !found {
        if !text.is_empty() && !text.ends_with('\n') {
            text.push('\n');
        }
        text.push_str(&canonical);
        text.push('\n');
    }
    if text == existing {
        // 语言值已经是对的：不碰文件，避免每次启动都刷新 mtime
        return Ok(());
    }
    // 只在「整个文件都是我们新建的」时补 forceUnicodeFont —— 往已有文件里
    // 插设置是越界，万一他故意关掉呢（关掉换自定义字体是常见操作）
    if fresh && matches!(lang.get(..3), Some("zh_") | Some("ja_") | Some("ko_")) {
        text.push_str("forceUnicodeFont:true\n");
    }

    fs::create_dir_all(game_dir).map_err(|e| AppError::io(&e, "创建游戏目录"))?;
    // 同配置读写：先写临时文件再改名，避免写一半留下损坏文件
    let tmp = path.with_extension("txt.tmp");
    fs::write(&tmp, text).map_err(|e| AppError::io(&e, "写入 options.txt"))?;
    fs::rename(&tmp, &path).map_err(|e| AppError::io(&e, "替换 options.txt"))?;
    Ok(())
}

/// 给界面展示用：classpath 的分隔符（前端拼路径提示要用）。
/// 执行「启动前命令」。
///
/// 交给系统 shell 跑（Windows `cmd /C`、其他 `sh -c`），所以能写重定向、
/// `&&`、`start` 这些习惯的东西。工作目录是游戏目录 —— 写 `./mclaunch.exe`
/// 这类相对路径时才符合直觉。
///
/// `wait = false` 只负责把进程甩出去，不关心它活多久（适合起个后台程序）；
/// `wait = true` 会等它退出，退出码非零也只记日志 —— 它是个附加动作，
/// 不该把游戏挡在门外。
async fn run_pre_launch_command(command: &str, cwd: &Path, wait: bool) -> Result<(), AppError> {
    let mut cmd = if cfg!(windows) {
        let mut c = tokio::process::Command::new("cmd");
        c.arg("/C").arg(command);
        c
    } else {
        let mut c = tokio::process::Command::new("sh");
        c.arg("-c").arg(command);
        c
    };
    cmd.current_dir(cwd);
    // 输出不接管：让它直接进启动器自己的控制台/日志管道，别和游戏日志抢终端
    cmd.stdin(std::process::Stdio::null());

    if !wait {
        cmd.spawn()
            .map_err(|e| AppError::io(&e, "执行启动前命令"))?;
        debug!(command, "启动前命令已发出（不等待）");
        return Ok(());
    }

    let status = cmd
        .status()
        .await
        .map_err(|e| AppError::io(&e, "执行启动前命令"))?;
    if status.success() {
        debug!(command, "启动前命令执行完成");
    } else {
        warn!(command, code = ?status.code(), "启动前命令返回非零退出码");
    }
    Ok(())
}

pub fn cp_separator() -> &'static str {
    classpath_separator()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_ids_are_unique() {
        let sessions = LaunchSessions::default();
        let a = sessions.new_id();
        let b = sessions.new_id();
        assert_ne!(a, b);
    }

    /// 就绪判据是「窗口出来了」的近似信号，认错会导致状态块一直停在
    /// 「正在启动」，所以把两种日志格式都钉住。
    #[test]
    fn ready_markers_cover_both_atlas_log_styles() {
        // 1.13+ 的写法
        assert!(logs_ready(
            "[Render thread/INFO]: Created: 1024x512x4 minecraft:textures/atlas/blocks.png-atlas"
        ));
        // 1.12.2 及更早的写法
        assert!(logs_ready("[Client thread/INFO]: Created: 512x512 textures-atlas"));
        // 旧版 OptiFine 类 mod 会打这句
        assert!(logs_ready("[Client thread/INFO]: Found animation info, using default!"));
        // 加载中途和无关日志不能被误判
        assert!(!logs_ready("[Client thread/INFO]: Setting user: Steve"));
        assert!(!logs_ready("[Client thread/INFO]: LWJGL Version: 2.9.4"));
        assert!(!logs_ready(""));
    }

    #[test]
    fn running_for_finds_same_version_only() {
        let sessions = LaunchSessions::default();
        sessions.insert(
            "s1",
            SessionEntry {
                version_id: "1.20.1".into(),
                cancel: CancellationToken::new(),
                java_path: "/java".into(),
                java_major: 17,
                started: Instant::now(),
            },
        );
        assert_eq!(sessions.running_for("1.20.1"), Some("s1".to_string()));
        assert_eq!(sessions.running_for("1.19"), None);
    }

    #[test]
    fn reservation_blocks_a_second_preparing_launch() {
        let sessions = LaunchSessions::default();
        let first = sessions.begin_launch("1.20.1").expect("第一次应当抢到");
        assert!(
            sessions.begin_launch("1.20.1").is_none(),
            "准备中的同版本不能再起第二个"
        );
        // 不同版本互不影响
        assert!(sessions.begin_launch("1.19.4").is_some());
        drop(first);
        assert!(
            sessions.begin_launch("1.20.1").is_some(),
            "上一个准备流程结束后应能重新启动"
        );
    }

    #[test]
    fn stop_cancels_matching_session() {
        let sessions = LaunchSessions::default();
        let token = CancellationToken::new();
        sessions.insert(
            "s1",
            SessionEntry {
                version_id: "1.20.1".into(),
                cancel: token.clone(),
                java_path: "/java".into(),
                java_major: 17,
                started: Instant::now(),
            },
        );
        assert!(sessions.stop("1.20.1").is_some());
        assert!(token.is_cancelled());
    }

    #[test]
    fn stop_unknown_returns_none() {
        let sessions = LaunchSessions::default();
        assert!(sessions.stop("nothing").is_none());
    }

    #[test]
    fn asset_index_prefers_assets_field() {
        let mut detail = empty_detail();
        detail.assets = Some("5".into());
        assert_eq!(asset_index_name(&detail), "5");

        detail.assets = None;
        detail.asset_index = Some(xcl_core::version::AssetIndex {
            id: "legacy".into(),
            url: String::new(),
            sha1: None,
            size: None,
            total_size: None,
        });
        assert_eq!(asset_index_name(&detail), "legacy");

        detail.asset_index = None;
        assert_eq!(asset_index_name(&detail), "legacy", "全缺时回落 legacy");
    }

    #[test]
    fn splits_user_args_on_whitespace() {
        assert_eq!(
            split_user_args("  -XX:+UseG1GC   -Dfoo=1 "),
            vec!["-XX:+UseG1GC", "-Dfoo=1"]
        );
        assert!(split_user_args("   ").is_empty());
    }

    fn empty_detail() -> VersionDetail {
        VersionDetail {
            id: "x".into(),
            version_type: None,
            inherits_from: None,
            main_class: None,
            assets: None,
            asset_index: None,
            downloads: None,
            libraries: Vec::new(),
            arguments: None,
            minecraft_arguments: None,
            java_version: None,
        }
    }

    fn temp_game_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("xcl-lang-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn creates_options_when_missing() {
        let dir = temp_game_dir("fresh");
        ensure_language(&dir, "zh_cn", "1.12.2").unwrap();
        let text = fs::read_to_string(dir.join("options.txt")).unwrap();
        // CJK 首次创建时一并打开 Unicode 字体，否则旧版 MC 中文是方括号
        assert_eq!(text, "lang:zh_cn\nforceUnicodeFont:true\n");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn inserts_lang_when_absent() {
        let dir = temp_game_dir("insert");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("options.txt"), "fov:70\nrenderDistance:8\n").unwrap();

        ensure_language(&dir, "zh_cn", "1.12.2").unwrap();

        let text = fs::read_to_string(dir.join("options.txt")).unwrap();
        // 原有行一行不丢，lang 追加在末尾
        assert!(text.contains("fov:70"));
        assert!(text.contains("renderDistance:8"));
        assert!(text.ends_with("lang:zh_cn\n"));
        // 不是新建文件，不许顺手动已有的 forceUnicodeFont
        assert!(!text.contains("forceUnicodeFont"));
        // 临时文件不许残留
        assert!(!dir.join("options.txt.tmp").exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn rewrites_existing_lang() {
        let dir = temp_game_dir("rewrite");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("options.txt"), "lang:en_us\nfov:90\n").unwrap();

        ensure_language(&dir, "zh_cn", "1.12.2").unwrap();

        let text = fs::read_to_string(dir.join("options.txt")).unwrap();
        assert_eq!(
            text, "lang:zh_cn\nfov:90\n",
            "启动器配置的语言是准的，旧值必须改写，其余行一行不丢"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn preserves_crlf_when_rewriting() {
        let dir = temp_game_dir("crlf");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("options.txt"), "lang:en_US\r\nfov:90\r\n").unwrap();

        ensure_language(&dir, "zh_cn", "1.7.10").unwrap();

        let text = fs::read_to_string(dir.join("options.txt")).unwrap();
        assert_eq!(text, "lang:zh_CN\r\nfov:90\r\n", "换行样式不许顺手改掉");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn appends_after_unterminated_last_line() {
        // 最后一行没有换行符：先补换行再追加，不许和原有行拼成一个键
        let dir = temp_game_dir("no-newline");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("options.txt"), "fov:70").unwrap();

        ensure_language(&dir, "zh_cn", "1.12.2").unwrap();

        let text = fs::read_to_string(dir.join("options.txt")).unwrap();
        assert_eq!(text, "fov:70\nlang:zh_cn\n");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn empty_language_falls_back_to_chinese() {
        let dir = temp_game_dir("empty-lang");
        ensure_language(&dir, "   ", "1.12.2").unwrap();
        let text = fs::read_to_string(dir.join("options.txt")).unwrap();
        assert_eq!(text, "lang:zh_cn\nforceUnicodeFont:true\n");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn casing_follows_minecraft_version() {
        // 1.1~1.10 必须大写，否则小写在 1.6~1.10 退回英文、1.1~1.5 直接崩溃
        assert_eq!(
            lang_value_for_version("1.7.10", "zh_cn").as_deref(),
            Some("zh_CN")
        );
        assert_eq!(
            lang_value_for_version("1.10", "zh_cn").as_deref(),
            Some("zh_CN")
        );
        assert_eq!(
            lang_value_for_version("1.5.2", "en_us").as_deref(),
            Some("en_US")
        );
        // 1.11+ 必须小写，大写同样退回英文
        assert_eq!(
            lang_value_for_version("1.11", "zh_cn").as_deref(),
            Some("zh_cn")
        );
        assert_eq!(
            lang_value_for_version("1.12.2", "zh_cn").as_deref(),
            Some("zh_cn")
        );
        // 周快照都在 1.11 之后
        assert_eq!(
            lang_value_for_version("24w14a", "zh_cn").as_deref(),
            Some("zh_cn")
        );
    }

    #[test]
    fn ancient_versions_skip_language() {
        // 1.0 及更早没有语言选项，写了也不读 —— 干脆别创建文件
        let dir = temp_game_dir("ancient");
        ensure_language(&dir, "zh_cn", "1.0").unwrap();
        assert!(!dir.join("options.txt").exists());

        ensure_language(&dir, "zh_cn", "b1.7.3").unwrap();
        assert!(!dir.join("options.txt").exists());

        assert_eq!(lang_value_for_version("a1.2.5", "zh_cn"), None);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn era_detection_from_version_id() {
        assert_eq!(lang_era("1.0"), LangEra::Ancient);
        assert_eq!(lang_era("1.1"), LangEra::Legacy);
        assert_eq!(lang_era("1.10"), LangEra::Legacy);
        assert_eq!(lang_era("1.11"), LangEra::Modern);
        assert_eq!(lang_era("1.12.2"), LangEra::Modern);
        assert_eq!(lang_era("1.21.4"), LangEra::Modern);
        assert_eq!(lang_era("24w14a"), LangEra::Modern);
        assert_eq!(lang_era("b1.7.3"), LangEra::Ancient);
        assert_eq!(lang_era("rd-132211"), LangEra::Ancient);
        // Mojang 新年份版本号
        assert_eq!(lang_era("26.2"), LangEra::Modern);
        assert_eq!(lang_era("26.3"), LangEra::Modern);
        // 加载器目录名：原版号不一定在开头
        assert_eq!(
            lang_era("fabric-loader-0.15.11-1.20.1"),
            LangEra::Modern
        );
        assert_eq!(lang_era("1.12.2-forge14.23.5.2859"), LangEra::Modern);
        assert_eq!(lang_era("1.7.10-LiteLoader1.7.10"), LangEra::Legacy);
        // 新年份版本的加载器目录名：26.2 在末尾，前面是 Fabric 版本
        assert_eq!(lang_era("fabric-loader-0.20.1-26.2"), LangEra::Modern);
        assert_eq!(
            lang_value_for_version("fabric-loader-0.20.1-26.2", "zh_cn").as_deref(),
            Some("zh_cn")
        );
        assert_eq!(minecraft_major("fabric-loader-0.20.1-26.2"), Some(26));
        // 加载器版本 14.x 不能被当成 MC 26.x
        assert_eq!(lang_era("forge-14.23.5.2859"), LangEra::Ancient);
    }

    #[test]
    fn language_falls_back_to_instance_game_version() {
        // 目录名认不出来时，用 instance.toml 里记的原版版本号
        assert_eq!(
            language_era_source("MyPack", Some("1.20.1")),
            "1.20.1".to_string()
        );
        assert_eq!(
            lang_value_for_version("MyPack", "zh_cn"),
            None,
            "单看目录名确实认不出"
        );
        assert_eq!(
            lang_value_for_version(
                &language_era_source("MyPack", Some("1.20.1")),
                "zh_cn"
            )
            .as_deref(),
            Some("zh_cn")
        );
        // 1.7.10 的加载器版本号不能被 year_major_in 抢走
        assert_eq!(
            language_era_source("forge-14.23.5.2859", Some("1.7.10")),
            "1.7.10".to_string()
        );
        assert_eq!(
            lang_value_for_version("1.7.10", "zh_cn").as_deref(),
            Some("zh_CN")
        );
    }

    #[test]
    fn lang_key_matches_exactly() {
        let dir = temp_game_dir("key-match");
        fs::create_dir_all(&dir).unwrap();
        // 带 lang 子串但不是 lang 键的行不许被当成语言行改写
        fs::write(
            dir.join("options.txt"),
            "resourcePacks:vanilla\nsoundCategory_master:1\n",
        )
        .unwrap();

        ensure_language(&dir, "zh_cn", "1.12.2").unwrap();

        let text = fs::read_to_string(dir.join("options.txt")).unwrap();
        assert_eq!(
            text,
            "resourcePacks:vanilla\nsoundCategory_master:1\nlang:zh_cn\n"
        );
        let _ = fs::remove_dir_all(&dir);
    }
}
