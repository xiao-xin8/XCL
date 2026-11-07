//! 配置数据模型。
//!
//! 落盘位置由上层决定（XCL 默认是程序目录下的 `data/config.toml`）。
//! 所有字段都带默认值：配置文件缺字段、多字段都不该导致启动失败。

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::kind::VersionKind;

/// 打包时由 `build.rs` 从仓库根的 `bake.toml` 生成的部署地址。
///
/// 地址本身不进源码：仓库里只有模板 `bake.example.toml`，自己的 `bake.toml`
/// 不入 Git。别人 clone 下来自己编译、没有这个文件时，这些常量全是空串，
/// 对应功能就按「这个构建没有该服务」处理。
mod baked {
    include!(concat!(env!("OUT_DIR"), "/baked.rs"));
}

/// 这个构建内置的服务地址。
///
/// 它们是**程序自己的服务**，不是用户选项：打包时由 `bake.toml` 注入，
/// 运行时没有开关、界面上也不给填。自己 clone 编译、没提供 `bake.toml` 时
/// 全部为空串，对应功能就按「这个构建没有该服务」处理。
pub const RELAY_ENDPOINT: &str = baked::RELAY;
pub const CONNECT_MANIFEST_ENDPOINT: &str = baked::CONNECT_MANIFEST;
pub const ACCOUNT_SERVER_ENDPOINT: &str = baked::ACCOUNT_SERVER;
pub const UPDATE_MANIFEST_ENDPOINT: &str = baked::UPDATE_MANIFEST;

/// 当前配置结构版本。将来做迁移时靠它判断。
pub const CURRENT_SCHEMA: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Config {
    pub schema_version: u32,
    pub mirror: MirrorConfig,
    pub download: DownloadConfig,
    pub java: JavaConfig,
    pub launch: LaunchConfig,
    pub ui: UiConfig,
    pub connect: ConnectConfig,
    pub log: LogConfig,
    pub update: UpdateConfig,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            schema_version: CURRENT_SCHEMA,
            mirror: MirrorConfig::default(),
            download: DownloadConfig::default(),
            java: JavaConfig::default(),
            launch: LaunchConfig::default(),
            ui: UiConfig::default(),
            connect: ConnectConfig::default(),
            log: LogConfig::default(),
            update: UpdateConfig::default(),
        }
    }
}

/// 联机设置。
///
/// 房间身份只在这里：房间名与密码是「这个房间叫什么」，与游戏端口无关 ——
/// 端口是每次开游戏都可能变的运行时信息，不进房间身份，也不进邀请码。
/// 记下来是为了「下次开房还是同一个房间」，好友存过的邀请码继续有效。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ConnectConfig {
    /// 房间名。可改；空表示还没起过房，第一次创建时随机生成后记下。
    pub room: String,
    /// 房间密码。知道它才能进同一个虚拟网；同理首次随机生成后固定。
    pub secret: String,
    /// 是否已经看过「离线账号进不去」那一次提示。
    ///
    /// 只在第一次创建房间时弹，弹过就记下 —— 这是产品引导，不是状态：
    /// 不参与任何联机判定，删掉它最多让提示再弹一次。
    pub hint_seen: bool,
}

impl Default for ConnectConfig {
    fn default() -> Self {
        Self {
            room: String::new(),
            secret: String::new(),
            hint_seen: false,
        }
    }
}

/// 自更新设置。
///
/// 全部字段可缺省：老配置文件里没有 `[update]` 段时按默认值补全，
/// 不改 `schema_version`。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct UpdateConfig {
    /// 发布频道。稳定版不把预发布当成更新。
    pub channel: UpdateChannel,
    /// 启动后自动检查。默认开：云端发了新版就该看到。
    pub auto_check: bool,
    /// 发现新版本后自动下载。v1 恒为 false，字段先留着。
    pub auto_download: bool,
    /// 选了「跳过这个版本」时记下来。
    pub skipped_version: String,
    /// 上次检查时间（unix 秒），给「24 小时内只查一次」用。
    pub last_check_at: u64,
    /// 已经展示过更新说明的版本，避免每次启动都弹。
    pub last_seen_version: String,
    /// 强制更新的持久化状态。
    ///
    /// 云端清单里 `force: true` 时会立刻写进配置。之后每次启动都按这里的
    /// 版本号拦住升级，直到当前版本追上 —— 故意不依赖下次联网，
    /// 否则断网（或临时把清单改回去）就能绕过强制更新。
    pub force_version: String,
    pub force_title: String,
    pub force_notes: String,
    pub force_changelog: Vec<String>,
}

impl Default for UpdateConfig {
    fn default() -> Self {
        Self {
            channel: UpdateChannel::Stable,
            auto_check: true,
            auto_download: false,
            skipped_version: String::new(),
            last_check_at: 0,
            last_seen_version: String::new(),
            force_version: String::new(),
            force_title: String::new(),
            force_notes: String::new(),
            force_changelog: Vec::new(),
        }
    }
}

/// 发布频道。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum UpdateChannel {
    /// 稳定版：忽略预发布版本
    #[default]
    Stable,
    /// 预览版：能收到 `0.2.0-beta.1`
    Preview,
}

impl UpdateChannel {
    pub fn as_str(self) -> &'static str {
        match self {
            UpdateChannel::Stable => "stable",
            UpdateChannel::Preview => "preview",
        }
    }

    /// 这个频道接不接受预发布版本。
    pub fn accepts_prerelease(self) -> bool {
        matches!(self, UpdateChannel::Preview)
    }
}

/// 日志相关设置。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct LogConfig {
    /// 日志级别。改完立刻生效，不用重启。
    pub level: LogLevel,
}

impl Default for LogConfig {
    fn default() -> Self {
        // 默认只输出警告：正常流程的 info/debug 会把控制台刷满
        Self {
            level: LogLevel::Warn,
        }
    }
}

/// 日志级别。对应 `tracing` 的五档。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum LogLevel {
    Error,
    #[default]
    Warn,
    Info,
    Debug,
    Trace,
}

impl LogLevel {
    /// 给 `EnvFilter` 用的小写名字。
    pub fn as_filter(self) -> &'static str {
        match self {
            LogLevel::Error => "error",
            LogLevel::Warn => "warn",
            LogLevel::Info => "info",
            LogLevel::Debug => "debug",
            LogLevel::Trace => "trace",
        }
    }
}

/// 下载源。
///
/// 「用哪个镜像」和「官方与镜像谁优先」是两件独立的事，所以分开存：
/// `provider` 决定镜像候选，两个 `*_strategy` 决定排序。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct MirrorConfig {
    /// 镜像渠道
    pub provider: MirrorProvider,
    /// 下载文件时的优先策略
    ///
    /// 和版本列表分开的原因：文件要的是带宽，列表要的是新鲜度 ——
    /// 镜像的版本清单可能比官方滞后。
    pub download_strategy: SourceStrategy,
    /// 拉版本列表时的优先策略
    pub version_strategy: SourceStrategy,
}

impl Default for MirrorConfig {
    fn default() -> Self {
        Self {
            provider: MirrorProvider::Bmclapi {},
            download_strategy: SourceStrategy::Auto,
            version_strategy: SourceStrategy::Auto,
        }
    }
}

/// 官方 / 镜像的优先策略。三档语义对应常见的两个下载源设置（默认都是中间档）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SourceStrategy {
    /// 尽量使用镜像源，官方只兜底
    MirrorFirst,
    /// 优先官方，慢则换镜像。判据是官方版本清单的实测耗时（见 `xcl_source::OFFICIAL_FAST_MS`）
    #[default]
    Auto,
    /// 尽量使用官方源，镜像只兜底
    OfficialFirst,
}

/// 下载源。
///
/// 用内部标记（`kind` 字段）而不是 `tag + content`：设置页保存的是局部 JSON，
/// 后端递归合并。从「自定义」切回 BMCLAPI 时，旧对象里的 `value` 会残留下来；
/// adjacently tagged 的 unit 变体会把它当成内容去解析，报
/// `invalid type: string "", expected unit variant` —— 也就是「切回默认源却存不上」。
/// 内部标记下多余字段会被忽略，换源就只是一次普通覆盖。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum MirrorProvider {
    Official {},
    Bmclapi {},
    /// 自定义镜像根地址，例如 `https://mirror.example.com`
    Custom {
        value: String,
    },
}

impl Default for MirrorProvider {
    fn default() -> Self {
        Self::Bmclapi {}
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct DownloadConfig {
    /// 并发下载数。
    ///
    /// `0` 表示自动：由最终选中的源决定 —— 官方源扛得住高并发，用 64；
    /// 镜像在高并发下会退化，用 16。见 [`auto_concurrency`]。
    ///
    /// 非 0 时以手填的为准（上限 [`MAX_CONCURRENCY`]）。
    ///
    /// 注意这是并发任务数，不是操作系统线程数 —— tokio 里 64 个下载任务
    /// 可能只跑在几个线程上，成本远低于 64 个阻塞线程。
    pub concurrency: u32,
    /// 限速（KB/s），0 表示不限速
    pub speed_limit_kbps: u32,
    /// 是否允许单文件多连接（Range 分片）。
    ///
    /// 注意它和 `concurrency` 是两件事：`concurrency` 是「同时下几个文件」，
    /// 这里是「一个文件内部开几条连接」。默认开，但最终还要看这个文件的实际
    /// URL（`xcl_source::url_allows_split`，BMCLAPI 等主机会被关掉）。
    pub split_enabled: bool,
    /// 单个文件最多几条连接。
    ///
    /// 取 16 的依据是 2026-10-25 在 HTTP/1.1 下的实测（每段固定 512KB，只改连接数）：
    /// 4 → 604 KB/s、8 → 943、16 → 1363、32 → 1424 但 32 个请求里有 12 个被拒
    /// （服务器返回非 206）。
    ///
    /// 注意：这个结论只在 HTTP/1.1 下成立。早期用 h2 测出「8 比 16 好」是假象 ——
    /// h2 会把所有流多路复用进一条 TCP 连接，连接数根本没起作用。
    pub split_connections: u32,
    /// 全局分片连接总数上限（所有文件加起来）。
    ///
    /// 16 × 2 = 32：让两个大文件全速分片。不再往上调 —— 实测 32 条连接时
    /// 服务器已经开始拒绝部分 Range 请求。
    pub split_total_connections: u32,
    /// 单个地址失败后的重试次数（不含第一次尝试）。
    ///
    /// 引擎里是 `attempts_per_url = retries + 1`。默认 1 = 最多试两次：
    /// 多试几次能盖住偶发的 403/连接重置，再多只是让坏地址拖时间。
    pub retries: u32,
}

/// 重试次数上限。再高对坏地址只是白等。
pub const MAX_RETRIES: u32 = 10;

/// 单文件分片连接数默认值，见 `DownloadConfig::split_connections` 的注释。
pub const SPLIT_CONNECTIONS: u32 = 16;
/// 全局分片连接数默认值。
pub const SPLIT_TOTAL_CONNECTIONS: u32 = 32;

/// 并发数上限。再高对磁盘和镜像都不友好。
pub const MAX_CONCURRENCY: u32 = 128;
/// 并发数「自动」的标记值。
pub const CONCURRENCY_AUTO: u32 = 0;

/// 官方源优先时的并发数。64 是实测出来的上限
/// —— 12 个库文件从串行 34.7s 降到 5.1s。
pub const CONCURRENCY_OFFICIAL: u32 = 64;

/// 镜像源优先时的并发数。
///
/// 保守一点：实测 BMCLAPI 在「多连接同时建立」的突发场景会 403/429，
/// 同一批文件并发 64 耗时 27.6s（约等于串行），并发 8~16 反而最好。
pub const CONCURRENCY_MIRROR: u32 = 16;

/// 自动并发数：官方优先给 64，镜像优先给 16。
pub fn auto_concurrency(official_first: bool) -> u32 {
    if official_first {
        CONCURRENCY_OFFICIAL
    } else {
        CONCURRENCY_MIRROR
    }
}

impl Default for DownloadConfig {
    fn default() -> Self {
        Self {
            concurrency: CONCURRENCY_AUTO,
            speed_limit_kbps: 0,
            split_enabled: true,
            split_connections: SPLIT_CONNECTIONS,
            split_total_connections: SPLIT_TOTAL_CONNECTIONS,
            retries: 1,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct JavaConfig {
    /// 手动指定的 java 可执行文件；为空则自动探测
    pub manual_path: Option<PathBuf>,
    /// 找不到合适 Java 时是否允许自动下载
    pub auto_download: bool,
}

impl Default for JavaConfig {
    fn default() -> Self {
        Self {
            manual_path: None,
            auto_download: true,
        }
    }
}

/// 内存分配方式。
///
/// - `Auto`：按总物理内存的四分之一估算（最高 8GB）；
/// - `Custom`：用 `memory_mb` 里手填的值。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum MemoryMode {
    #[default]
    Auto,
    Custom,
}

/// 自动内存的下限、上限和取整粒度。
pub const AUTO_MEMORY_MIN_MB: u32 = 512;
/// 上限 16GB。
///
/// 不封顶，但 MC 的堆不是越大越好：堆越大 GC 停顿越长，超过 16G 之后
/// 通常只会变得更卡。封在这里是为了不把 64G 机器的整合包推到 20G+。
pub const AUTO_MEMORY_MAX_MB: u32 = 16_384;
pub const AUTO_MEMORY_STEP_MB: u32 = 256;

/// 自动内存：按可用物理内存 + 实例的 mod 数量算（阶梯分配）
/// （`PageInstanceSetup.GetRam`）。
///
/// 两步：
///
/// 1. 按 mod 数定四个目标档位（单位 GB）——
///    可装 mod 的版本用 mod 数递增，纯原版用固定值；
/// 2. 按可用内存分段给，边际递减——第一段 100%，之后 70% / 40% / 15%，
///    所以内存越多给得越多，但越来越吝啬，不会把机器榨干。
///
/// 为什么用可用内存而不是总量：大包 OOM 的真实原因是「机器当时没有那么多内存」，
/// 用总量会给出一个系统根本给不出的值。代价是这个值会随系统负载浮动。
///
/// 参数：可用物理内存（MB）、`mods/` 里的 mod 数、这个实例有没有加载器。
pub fn auto_memory_mb(available_mb: u64, mod_count: usize, modable: bool) -> u32 {
    let mods = mod_count as f64;
    // 四个档位（GB）
    let (minimum, target1, target2, target3) = if modable {
        (
            0.5 + mods / 150.0,
            1.5 + mods / 90.0,
            2.7 + mods / 50.0,
            4.5 + mods / 25.0,
        )
    } else {
        (0.5, 1.5, 2.5, 4.0)
    };

    let mut available = available_mb as f64 / 1024.0;
    let mut give = 0.0;

    // 阶段一：0 ~ T1，全给
    give += available.min(target1);
    available -= target1;
    // 阶段二：T1 ~ T2，给 70%
    if available > 0.1 {
        let delta = target2 - target1;
        give += (available * 0.7).min(delta);
        available -= delta / 0.7;
    }
    // 阶段三：T2 ~ T3，给 40%
    if available > 0.1 {
        let delta = target3 - target2;
        give += (available * 0.4).min(delta);
        available -= delta / 0.4;
    }
    // 阶段四：T3 ~ 2×T3，给 15%
    if available > 0.1 {
        let delta = target3;
        give += (available * 0.15).min(delta);
    }

    // 无论如何不低于「最低」
    let give = give.max(minimum);
    let step = AUTO_MEMORY_STEP_MB as f64;
    let mb = (give * 1024.0 / step).round() * step;
    (mb as u32).clamp(AUTO_MEMORY_MIN_MB, AUTO_MEMORY_MAX_MB)
}

/// 启动相关设置。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct LaunchConfig {    /// 离线模式的玩家名。
    ///
    /// 已废弃，不再参与启动：账号体系落地后，身份一律来自 `accounts.toml`；
    /// 一个账号都没有时启动会直接报错（约定），不再拿这里的名字兜底。
    /// 字段保留是为了兼容旧配置文件，设置页也不展示它。
    pub player_name: String,
    /// 内存分配方式。默认自动。
    pub memory_mode: MemoryMode,
    /// 手动模式下的最大内存（MB）。自动模式下这个值不参与分配。
    pub memory_mb: u32,
    pub window_width: u32,
    pub window_height: u32,
    pub fullscreen: bool,
    /// 自定义 JVM 参数，空格分隔
    pub extra_jvm_args: String,
    /// 自定义游戏参数，空格分隔
    pub extra_game_args: String,
    /// 游戏语言，即 options.txt 里 `lang:` 的值。默认简体中文。
    ///
    /// MC 客户端没有 `--lang` 启动参数，语言只在启动前写进游戏目录的
    /// options.txt；进游戏改过就保留原样，这里只是「没有 lang 行时」
    /// 补的默认值。
    pub language: String,
    /// 启动游戏前执行的命令。空表示不执行。
    ///
    /// 整行交给系统执行（Windows 走 `cmd /C`），所以能写重定向、`&&` 之类的东西。
    /// 失败只记日志、不拦启动 —— 它是个附加动作，不该把游戏挡在门外。
    pub pre_launch_command: String,
    /// 是否等 `pre_launch_command` 执行完再继续启动。
    ///
    /// 关掉就是「甩出去不管」，适合启个后台程序；开着的话命令不结束就不会启动游戏。
    pub wait_command: bool,
    /// 默认的版本隔离策略。单个版本还能在实例设置里覆盖。
    ///
    /// 隔离只换游戏目录（存档 / mods / config / 日志），versions、
    /// libraries、assets、Java 仍然共享。
    pub isolation: IsolationPolicy,
    /// 游戏进程优先级。启动后设置，只对 Windows 生效。
    pub priority: ProcessPriority,
    /// 是否把 java.exe 和本启动器标记为「使用高性能显卡」。
    ///
    /// 这不是 JVM 参数：Windows 的显卡偏好存在注册表
    /// `HKCU\Software\Microsoft\DirectX\UserGpuPreferences` 里（`GpuPreference=2;`），
    /// 写注册表的 `GpuPreference`。只对 Windows 生效。
    pub high_perf_gpu: bool,
    /// 启动游戏前清理一次内存：把各进程不活跃的工作集换出到备用列表。
    ///
    /// 为什么默认关：它不是没有代价的 —— 清完之后切回浏览器/IDE 会重新缺页
    /// 加载一下，机械硬盘上更明显。收益只对「内存吃紧 + 大型整合包」成立。
    ///
    /// 只做不需要管理员的那一层（逐进程 `EmptyWorkingSet`）。需要提权的
    /// 系统级清理（清备用列表 / 文件缓存）不做：那层会把还能直接复用的页
    /// 扔成空闲，游戏要的时候反而得重新读盘。
    pub trim_memory: bool,
}

/// 版本隔离策略。
///
/// 默认值是 4（隔离所有版本）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum IsolationPolicy {
    /// 不隔离，所有版本共用数据根作游戏目录
    Off,
    /// 只隔离装了模组加载器的版本
    Modable,
    /// 只隔离非正式版（快照 / 远古版 / 愚人节版）
    NonRelease,
    /// 非正式版或装了加载器的版本
    NonReleaseOrModable,
    /// 所有版本都隔离（默认）
    #[default]
    All,
}

impl IsolationPolicy {
    /// 这个版本在没有显式设置、也没有自动检测到旧数据时是否隔离。
    pub fn isolates(self, kind: VersionKind, version_type: Option<&str>) -> bool {
        let modable = kind.is_modable();
        let release = version_type == Some("release");
        match self {
            Self::Off => false,
            Self::Modable => modable,
            Self::NonRelease => !release,
            Self::NonReleaseOrModable => !release || modable,
            Self::All => true,
        }
    }
}

/// 内存下限。低于这个值连原版都难跑起来。
pub const MIN_MEMORY_MB: u32 = 512;
/// 内存上限。超过它多半是把虚拟内存当物理内存填了。
pub const MAX_MEMORY_MB: u32 = 65_536;

impl Default for LaunchConfig {
    fn default() -> Self {
        Self {
            player_name: "Steve".to_string(),
            memory_mode: MemoryMode::Auto,
            memory_mb: 4096,
            window_width: 854,
            window_height: 480,
            fullscreen: false,
            extra_jvm_args: String::new(),
            extra_game_args: String::new(),
            language: "zh_cn".to_string(),
            pre_launch_command: String::new(),
            wait_command: true,
            isolation: IsolationPolicy::All,
            priority: ProcessPriority::Normal,
            high_perf_gpu: false,
            trim_memory: false,
        }
    }
}

/// 关闭主窗口时的行为。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CloseAction {
    /// 关窗口就结束程序
    Quit,
    /// 关窗口后继续在托盘待命，随时点图标打开（默认）
    #[default]
    Background,
}

/// 游戏进程优先级。
///
/// 三档：低 / 普通 / 高（`BelowNormal` / 默认 / `AboveNormal`），
/// 没有「实时」—— 那会把系统输入线程饿死。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ProcessPriority {
    Low,
    #[default]
    Normal,
    High,
}

// 手写 normalized 以处理越界值
impl Config {
    pub fn normalized(mut self) -> Self {
        // 0 是「自动」，不是非法值，所以只夹上限
        self.download.concurrency = self.download.concurrency.min(MAX_CONCURRENCY);
        self.download.retries = self.download.retries.min(MAX_RETRIES);
        self.launch.memory_mb = self
            .launch
            .memory_mb
            .clamp(MIN_MEMORY_MB, MAX_MEMORY_MB);
        // 0 尺寸会让游戏窗口不可见，回落默认
        if self.launch.window_width < 200 {
            self.launch.window_width = 854;
        }
        if self.launch.window_height < 200 {
            self.launch.window_height = 480;
        }
        if self.launch.player_name.trim().is_empty() {
            self.launch.player_name = "Steve".to_string();
        }
        // 空语言等于没配，回落简体中文
        if self.launch.language.trim().is_empty() {
            self.launch.language = "zh_cn".to_string();
        }
        // 空页签等于没记录，回落启动页
        if self.ui.last_tab.trim().is_empty() || self.ui.last_tab == "instance" {
            // 「实例」页已经删掉（版本管理全在启动页），旧配置落到启动页
            self.ui.last_tab = "launch".to_string();
        }
        // 起始页同理：老配置里可能钉着已经删掉的实例页
        if self.ui.start_page == StartPage::Instance {
            self.ui.start_page = StartPage::Launch;
        }
        // 透明度不设下限（要全透也随他）；模糊超过 20px 没有意义
        self.ui.surface_opacity = self.ui.surface_opacity.min(100);
        self.ui.surface_blur = self.ui.surface_blur.min(20);
        self.ui.background_overlay = self.ui.background_overlay.min(100);
        self.ui.background_blur = self.ui.background_blur.min(20);
        // 自定义色只认 `#rrggbb`：不合法的直接回到预设，别让主题拿到脏值
        self.ui.accent_custom = self
            .ui
            .accent_custom
            .take()
            .filter(|color| is_hex_color(color))
            .map(|color| color.to_ascii_lowercase());
        // 背景图名只认纯文件名；不合法就当作没设置（文件读不到时前端也会自己回落）
        self.ui.background_image = self
            .ui
            .background_image
            .take()
            .filter(|name| is_background_name(name));
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct UiConfig {
    pub remember_window: bool,
    /// 关闭主窗口时的行为：直接退出，还是留在右下角继续跑。
    pub close_action: CloseAction,
    /// 上次停留的页面（真实 tab key）。`start_page = last` 时用它。
    pub last_tab: String,
    /// 启动时打开哪一页。
    pub start_page: StartPage,
    /// 主题模式。
    pub theme_mode: ThemeMode,
    /// 强调色。
    pub accent: Accent,
    /// 动画强度。
    pub animation: AnimationMode,
    /// 自定义强调色（`#rrggbb`）。`None` = 用 `accent` 的预设色。
    pub accent_custom: Option<String>,
    /// 界面不透明度（0–100，100 = 完全不透明）。
    ///
    /// 统一控制所有「面」：卡片、侧栏/分栏面板、弹出层。不设下限 ——
    /// 看不清是自己的选择。
    pub surface_opacity: u32,
    /// 毛玻璃模糊半径（0–20 px，0 = 关闭）。
    pub surface_blur: u32,
    /// 自定义背景图在 `data/background/` 里的文件名（不含路径）。
    ///
    /// 选的图会被复制进我们自己的目录，原文件挪走或删掉都不影响；
    /// 目录里只留这一张。
    pub background_image: Option<String>,
    /// 背景图上的蒙版不透明度（0–100）：0 = 原图，100 = 完全被主题底色盖住。
    pub background_overlay: u32,
    /// 背景图自身的模糊半径（0–20 px）。
    pub background_blur: u32,
    pub window_width: u32,
    pub window_height: u32,
    /// 是否在顶部显示「联机」页签。
    ///
    /// 有人只拿它当离线启动器，多一个页签就是多一份噪音。关掉只是不显示，
    /// 已经装好的联机组件不会被删，重新打开还在。
    pub show_connect_tab: bool,
    /// 首页最后选中的版本 id（空串 = 还没选过）。
    ///
    /// 只解决「切回首页还停在上次那个」，不是启动记录 —— 别拿它当
    /// "上次玩了什么"，那个语义在 `version_stats` 里。恢复时如果这个版本
    /// 已经不在了，界面回退到列表第一个。
    pub selected_version: String,
}

/// 背景图只收「纯文件名 + 白名单扩展名」：挡住分隔符和 `..`，
/// 免得配置被手工改成能读到目录外的路径。
fn is_background_name(name: &str) -> bool {
    if name.is_empty() || name.contains(['/', '\\']) || name == ".." {
        return false;
    }
    let Some(ext) = name.rsplit_once('.').map(|(_, ext)| ext.to_ascii_lowercase()) else {
        return false;
    };
    matches!(ext.as_str(), "png" | "jpg" | "jpeg" | "webp" | "bmp" | "gif")
}

/// 颜色只收 `#rrggbb`：带 alpha / 简写都不认，避免前端推导配色时踩坑。
fn is_hex_color(text: &str) -> bool {
    text.len() == 7
        && text.starts_with('#')
        && text[1..].chars().all(|c| c.is_ascii_hexdigit())
}

/// 启动时打开的页面。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum StartPage {
    /// 记住上次停在哪（默认）
    #[default]
    Last,
    Launch,
    Download,
    /// 已废弃：实例页删掉了。留着这个变体只为读得懂旧 `config.toml`，
    /// `Config::normalized()` 会把它改回 `Launch`。
    Instance,
    Setting,
}

/// 界面主题。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ThemeMode {
    /// 跟随操作系统
    #[default]
    System,
    Light,
    Dark,
}

/// 强调色。淡黄是原厂配色，其余是预设。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Accent {
    #[default]
    Yellow,
    Sky,
    Mint,
    Sakura,
    Graphite,
}

/// 动画强度。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AnimationMode {
    /// 跟随系统的「减少动态效果」
    #[default]
    System,
    /// 强制开启（系统关了也播）
    On,
    /// 强制关闭
    Off,
}

impl Default for UiConfig {
    fn default() -> Self {
        Self {
            remember_window: true,
            // 默认关窗口后继续在右下角待命：游戏正在跑的时候关界面
            // 不应该顺手把后台一起带走
            close_action: CloseAction::Background,
            last_tab: "launch".to_string(),
            start_page: StartPage::Last,
            theme_mode: ThemeMode::System,
            accent: Accent::Yellow,
            animation: AnimationMode::System,
            accent_custom: None,
            surface_opacity: 100,
            surface_blur: 0,
            background_image: None,
            // 默认压一层 60% 的蒙版：不加的话文字直接压在图上，多数图都看不清
            background_overlay: 60,
            background_blur: 0,
            window_width: 1180,
            window_height: 760,
            show_connect_tab: true,
            // 空串 = 还没选过，首页回退到列表第一个
            selected_version: String::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_sane() {
        let cfg = Config::default();
        assert_eq!(cfg.schema_version, CURRENT_SCHEMA);
        assert_eq!(cfg.mirror.provider, MirrorProvider::Bmclapi {});
        // 两套策略都默认「自动」
        assert_eq!(cfg.mirror.download_strategy, SourceStrategy::Auto);
        assert_eq!(cfg.mirror.version_strategy, SourceStrategy::Auto);
        // 并发默认交给自动：官方 64、镜像 16
        assert_eq!(cfg.download.concurrency, CONCURRENCY_AUTO);
        assert_eq!(cfg.download.speed_limit_kbps, 0);
        assert_eq!(cfg.ui.window_width, 1180);
        // 重试 1 次 = 最多试两次，和之前的写死值一致
        assert_eq!(cfg.download.retries, 1);
        assert_eq!(cfg.launch.priority, ProcessPriority::Normal);
        assert!(!cfg.launch.high_perf_gpu);
        assert_eq!(cfg.log.level, LogLevel::Warn);
    }

    #[test]
    fn defaults_to_background_on_close() {
        // 默认关窗口后继续在右下角待命
        assert_eq!(
            Config::default().ui.close_action,
            CloseAction::Background
        );
    }

    #[test]
    fn stale_value_key_does_not_break_provider_switch() {
        // 设置页改配置是「局部 JSON 递归合并」：从自定义源切回 BMCLAPI 时，
        // 旧对象里的 `value` 会留在合并结果里，不能让它把整次保存弄失败。
        let provider: MirrorProvider =
            serde_json::from_value(serde_json::json!({ "kind": "bmclapi", "value": "" }))
                .unwrap();
        assert_eq!(provider, MirrorProvider::Bmclapi {});
    }

    #[test]
    fn retries_are_clamped() {
        let mut cfg = Config::default();
        cfg.download.retries = 99;
        assert_eq!(cfg.normalized().download.retries, MAX_RETRIES);
    }

    #[test]
    fn personalization_fields_are_sane() {
        let cfg = Config::default();
        assert_eq!(cfg.ui.accent_custom, None);
        assert_eq!(cfg.ui.surface_opacity, 100);
        assert_eq!(cfg.ui.surface_blur, 0);
        assert_eq!(cfg.ui.background_overlay, 60);
        assert_eq!(cfg.ui.background_blur, 0);

        let mut cfg = Config::default();
        cfg.ui.accent_custom = Some("#A8CDF0".to_string());
        cfg.ui.surface_opacity = 10;
        cfg.ui.surface_blur = 99;
        cfg.ui.background_overlay = 150;
        cfg.ui.background_blur = 99;
        let cfg = cfg.normalized();

        // 大小写统一；透明度不设下限，模糊夹到 20px
        assert_eq!(cfg.ui.accent_custom.as_deref(), Some("#a8cdf0"));
        assert_eq!(cfg.ui.surface_opacity, 10);
        assert_eq!(cfg.ui.surface_blur, 20);
        assert_eq!(cfg.ui.background_overlay, 100);
        assert_eq!(cfg.ui.background_blur, 20);
    }

    #[test]
    fn background_image_name_is_sanitized() {
        assert_eq!(Config::default().ui.background_image, None);

        let mut cfg = Config::default();
        cfg.ui.background_image = Some("../x.png".to_string());
        assert_eq!(cfg.normalized().ui.background_image, None);

        let mut cfg = Config::default();
        cfg.ui.background_image = Some("background.txt".to_string());
        assert_eq!(cfg.normalized().ui.background_image, None);

        let mut cfg = Config::default();
        cfg.ui.background_image = Some("background.png".to_string());
        assert_eq!(
            cfg.normalized().ui.background_image.as_deref(),
            Some("background.png")
        );
    }

    #[test]
    fn invalid_custom_accent_falls_back_to_preset() {
        let mut cfg = Config::default();
        cfg.ui.accent_custom = Some("红色".to_string());
        assert_eq!(cfg.normalized().ui.accent_custom, None);
    }

    #[test]
    fn new_settings_roundtrip() {
        let mut cfg = Config::default();
        cfg.download.retries = 4;
        cfg.launch.priority = ProcessPriority::High;
        cfg.launch.high_perf_gpu = true;
        cfg.log.level = LogLevel::Debug;

        let text = toml::to_string(&cfg).unwrap();
        let back: Config = toml::from_str(&text).unwrap();

        assert_eq!(back.download.retries, 4);
        assert_eq!(back.launch.priority, ProcessPriority::High);
        assert!(back.launch.high_perf_gpu);
        assert_eq!(back.log.level, LogLevel::Debug);
    }

    #[test]
    fn update_defaults_check_on_startup() {
        // 云端发了新版就该看到，所以自动检查默认开；自动下载仍然关着
        let cfg = Config::default();
        assert_eq!(cfg.update.channel, UpdateChannel::Stable);
        assert!(cfg.update.auto_check);
        assert!(!cfg.update.auto_download);
        assert_eq!(cfg.update.last_check_at, 0);
        assert!(cfg.update.force_version.is_empty());
    }

    #[test]
    fn update_section_is_optional_in_config_file() {
        // 老配置文件里没有 [update]，不该导致解析失败，也不该升 schema
        let cfg: Config = toml::from_str("[launch]\nmemoryMb = 2048").unwrap();
        assert_eq!(cfg.schema_version, CURRENT_SCHEMA);
        assert_eq!(cfg.update.channel, UpdateChannel::Stable);

        let cfg: Config =
            toml::from_str("[update]\nchannel = \"preview\"\n").unwrap();
        assert_eq!(cfg.update.channel, UpdateChannel::Preview);
        assert!(cfg.update.channel.accepts_prerelease());

        // 老配置里残留的 manifestUrl 要能正常解析（默认值来自打包注入）
        let cfg: Config =
            toml::from_str("[update]\nchannel = \"stable\"\nmanifestUrl = \"https://a.example/u.json\"\n")
                .unwrap();
        assert_eq!(cfg.update.channel, UpdateChannel::Stable);
    }

    #[test]
    fn forced_update_state_survives_roundtrip() {
        let mut cfg = Config::default();
        cfg.update.force_version = "0.2.0".into();
        cfg.update.force_title = "重要更新".into();
        cfg.update.force_notes = "修了个安全问题".into();
        cfg.update.force_changelog = vec!["修复 A".into(), "修复 B".into()];

        let text = toml::to_string(&cfg).unwrap();
        let back: Config = toml::from_str(&text).unwrap();
        assert_eq!(back.update.force_version, "0.2.0");
        assert_eq!(back.update.force_changelog.len(), 2);
        assert!(text.contains("forceVersion"));
    }

    #[test]
    fn update_roundtrips_through_toml() {
        let mut cfg = Config::default();
        cfg.update.auto_check = true;
        cfg.update.skipped_version = "0.2.0".into();
        cfg.update.last_seen_version = "0.2.0".into();
        let text = toml::to_string(&cfg).unwrap();
        let back: Config = toml::from_str(&text).unwrap();
        assert!(back.update.auto_check);
        assert_eq!(back.update.skipped_version, "0.2.0");
        // 字段名必须是 camelCase，和项目其它配置一致
        assert!(text.contains("lastSeenVersion"));
        // 服务地址不落配置：它是构建期注入的内置地址，配置文件里不该出现
        assert!(!text.contains("manifestUrl"));
    }

    #[test]
    fn auto_concurrency_follows_source() {
        assert_eq!(auto_concurrency(true), CONCURRENCY_OFFICIAL);
        assert_eq!(auto_concurrency(false), CONCURRENCY_MIRROR);
        // 官方给得比镜像多 —— 这是实测结论，不是拍脑袋
        assert!(CONCURRENCY_OFFICIAL > CONCURRENCY_MIRROR);
    }

    #[test]
    fn partial_config_uses_defaults() {
        // 只写一个字段，其余应全部取默认值
        let cfg: Config = toml::from_str("schemaVersion = 1").unwrap();
        assert_eq!(cfg.download.concurrency, CONCURRENCY_AUTO);
        assert_eq!(cfg.mirror.provider, MirrorProvider::Bmclapi {});
        assert_eq!(cfg.mirror.download_strategy, SourceStrategy::Auto);
    }

    #[test]
    fn old_version_list_flag_is_gone_without_breaking_parse() {
        // 旧配置里的 versionListOfficial 已删除，多出来的字段不该让解析失败
        let cfg: Config = toml::from_str(
            "schemaVersion = 1\n[mirror]\nprovider = { kind = \"bmclapi\" }\nversionListOfficial = true\n",
        )
        .unwrap();
        assert_eq!(cfg.mirror.version_strategy, SourceStrategy::Auto);
    }

    #[test]
    fn custom_mirror_roundtrip() {
        let mut cfg = Config::default();
        cfg.mirror.provider = MirrorProvider::Custom {
            value: "https://mirror.example.com".into(),
        };
        cfg.mirror.download_strategy = SourceStrategy::OfficialFirst;
        let text = toml::to_string(&cfg).unwrap();
        let back: Config = toml::from_str(&text).unwrap();
        assert_eq!(back.mirror.provider, cfg.mirror.provider);
        assert_eq!(back.mirror.download_strategy, SourceStrategy::OfficialFirst);
    }

    #[test]
    fn removed_instance_tab_falls_back_to_launch() {
        // 「实例」页已经删掉：旧配置里钉着它不能变成打不开的页签
        let old: Config = toml::from_str(
            "[ui]\nstartPage = \"instance\"\nlastTab = \"instance\"\n",
        )
        .unwrap();
        let cfg = old.normalized();
        assert_eq!(cfg.ui.start_page, StartPage::Launch);
        assert_eq!(cfg.ui.last_tab, "launch");
    }

    #[test]
    fn concurrency_is_clamped() {
        let mut cfg = Config::default();
        cfg.download.concurrency = 999;
        assert_eq!(cfg.normalized().download.concurrency, MAX_CONCURRENCY);
    }

    #[test]
    fn auto_concurrency_marker_survives_normalization() {
        // 0 是「自动」的意思，不能被当成非法值改掉
        let mut cfg = Config::default();
        cfg.download.concurrency = CONCURRENCY_AUTO;
        assert_eq!(cfg.normalized().download.concurrency, CONCURRENCY_AUTO);
    }

    #[test]
    fn isolation_defaults_to_all_like_pcl() {
        let cfg = Config::default();
        assert_eq!(cfg.launch.isolation, IsolationPolicy::All);
        // 只写老字段时也要拿到默认策略
        let old: Config = toml::from_str("[launch]\nmemoryMb = 2048").unwrap();
        assert_eq!(old.launch.isolation, IsolationPolicy::All);
    }

    #[test]
    fn auto_memory_follows_pcl_curve() {
        // 纯原版、4G 可用 → 约 2.9G
        assert_eq!(auto_memory_mb(4096, 0, false), 3072);

        // 200 个 mod、10G 可用 → 7G 上下（旧算法「总量 ÷4」在 16G 机器上只给 4G）
        let big = auto_memory_mb(10 * 1024, 200, true);
        assert!(
            (7168..=8192).contains(&big),
            "200 mod 应当给 7G 上下，实际 {big}"
        );

        // 可用内存为 0 时兜到下限
        assert_eq!(auto_memory_mb(0, 0, false), 512);

        // 上限 16G：64G 机器 + 2000 mod 也不越过
        assert_eq!(auto_memory_mb(256 * 1024, 2000, true), 16_384);

        // 同样的可用内存，mod 越多给得越多
        assert!(auto_memory_mb(10 * 1024, 200, true) > auto_memory_mb(10 * 1024, 0, true));

        // 取整到 256MB
        assert_eq!(big % 256, 0);

        assert_eq!(Config::default().launch.memory_mode, MemoryMode::Auto);
    }

    #[test]
    fn isolation_policy_matches_pcl_semantics() {
        let vanilla = (VersionKind::Vanilla, Some("release"));
        let snapshot = (VersionKind::Snapshot, Some("snapshot"));
        let modded = (VersionKind::Forge, Some("release"));

        assert!(!IsolationPolicy::Off.isolates(vanilla.0, vanilla.1));
        assert!(!IsolationPolicy::Off.isolates(modded.0, modded.1));

        assert!(!IsolationPolicy::Modable.isolates(vanilla.0, vanilla.1));
        assert!(IsolationPolicy::Modable.isolates(modded.0, modded.1));

        assert!(!IsolationPolicy::NonRelease.isolates(vanilla.0, vanilla.1));
        assert!(IsolationPolicy::NonRelease.isolates(snapshot.0, snapshot.1));

        assert!(!IsolationPolicy::NonReleaseOrModable.isolates(vanilla.0, vanilla.1));
        assert!(IsolationPolicy::NonReleaseOrModable.isolates(snapshot.0, snapshot.1));
        assert!(IsolationPolicy::NonReleaseOrModable.isolates(modded.0, modded.1));

        assert!(IsolationPolicy::All.isolates(vanilla.0, vanilla.1));
        // 没写 type 的旧 JSON 当作非正式版处理
        assert!(IsolationPolicy::NonRelease.isolates(vanilla.0, None));
    }

    /// 前端 `apps/desktop/src/lib/types.ts` 按这个形状读 `MirrorProvider`。
    /// 改序列化方式必须同步改那边，否则设置页的下载源会读不出来。
    #[test]
    fn provider_json_shape_matches_frontend() {
        let cfg = Config::default();
        let value = serde_json::to_value(&cfg).unwrap();
        assert_eq!(
            value["mirror"]["provider"],
            serde_json::json!({ "kind": "bmclapi" })
        );

        let mut cfg = Config::default();
        cfg.mirror.provider = MirrorProvider::Custom {
            value: "https://mirror.example.com".into(),
        };
        let value = serde_json::to_value(&cfg).unwrap();
        assert_eq!(
            value["mirror"]["provider"],
            serde_json::json!({ "kind": "custom", "value": "https://mirror.example.com" })
        );

        // config_patch 走的就是 from_value 这条路，形状必须能回来
        let back: Config = serde_json::from_value(value).unwrap();
        assert_eq!(back.mirror.provider, cfg.mirror.provider);
    }

    /// 局部 JSON（只有一层字段）能被补全成完整配置 —— 这是 `config_patch` 的前提。
    #[test]
    fn partial_json_patch_uses_defaults() {
        let cfg: Config =
            serde_json::from_value(serde_json::json!({ "launch": { "memoryMb": 2048 } })).unwrap();
        assert_eq!(cfg.launch.memory_mb, 2048);
        assert_eq!(cfg.launch.memory_mode, MemoryMode::Auto);
        assert_eq!(cfg.launch.isolation, IsolationPolicy::All);
        assert_eq!(cfg.download.concurrency, CONCURRENCY_AUTO);
        // 新加的字段没写也要有默认值
        assert!(cfg.launch.pre_launch_command.is_empty());
        assert!(cfg.launch.wait_command);
    }
}
