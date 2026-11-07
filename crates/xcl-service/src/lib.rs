//! 应用服务层。
//!
//! 「后端胖、前端瘦」的落点：持有运行时状态，对外暴露 async 方法，
//! Tauri 命令只做一层转发。
//!
//! 目前包含：
//! - `paths`：数据根目录探测（便携优先，回落用户目录）
//! - `config`：配置读写
//! - `cache`：统一接口缓存（TTL + 手动失效）
//! - `versions`：版本清单（含「自动选源」的探测）
//! - `install`：安装清单（要下什么）
//! - `download`：安装执行（并发下载 + 进度聚合）
//! - `local`：本地已安装版本的扫描（装了什么）
//! - `folders`：用系统文件管理器打开版本相关文件夹
//! - `launch`：启动游戏（找 Java → 解 natives → 起进程 → 推日志）
//! - `java_runtime`：Java 运行时的下载与管理（Mojang 官方 java-runtime）
//! - `auth`：账号（离线 / 第三方 Yggdrasil 登录）
//! - `skin`：皮肤（从认证服务器拿 textures，供前端 3D 渲染）

pub mod auth;
pub mod background;
pub mod cache;
pub mod config;
pub mod connect;
pub mod connect_relay;
pub mod connect_account;
pub mod connect_job;
pub mod connect_node;
pub mod connect_session;
pub mod download;
pub mod favorites;
pub mod folders;
pub mod install;
pub mod isolation;
pub mod java_runtime;
pub mod launch;
pub mod loader;
mod forgelike_install;
pub mod local;
pub mod memory;
pub mod modpack;
pub mod paths;
pub mod resources;
pub mod skin;
pub mod speed;
pub mod storage;
pub mod update;
pub mod version_admin;
pub mod version_settings;
pub mod versions;

use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use tracing::{debug, warn};
use xcl_auth::AccountStore;
use xcl_core::config::{auto_concurrency, MirrorProvider, SourceStrategy, CONCURRENCY_AUTO};
use xcl_core::{AppError, Config, PathLayout};
use xcl_download::{Fetcher, HttpFetcher};
use xcl_net::HttpClient;
use xcl_source::{Mirror, SourcePolicy, Strategy};

use crate::cache::ApiCache;
use crate::download::DownloadTasks;

/// 版本清单的缓存时长。清单本身一天只变几次，10 分钟足够。
const VERSION_CACHE_TTL: Duration = Duration::from_secs(600);

/// 「自动选源」的探测结论。
///
/// 只在进程内保存，重启后重测一次 ——
/// （它是内存变量，见 `ModDownload.vb:1215`）。落盘会让一次偶然的慢响应
/// 影响很大，而重测的成本只是首次拉清单时多等一会儿（有缓存挡着，
/// 一天也就几次）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProbeResult {
    /// 官方源是否够快（判据见 `xcl_source::OFFICIAL_FAST_MS`）
    pub official_fast: bool,
    /// 官方源实测耗时（毫秒）；没测出来时为 `None`
    pub official_ms: Option<u64>,
}

/// 全局运行时状态。
pub struct AppState {
    pub layout: PathLayout,
    /// 全局配置。
    ///
    /// 用 `RwLock` 而不是裸字段：设置页要在 `&self` 下改配置，而 `AppState`
    /// 被 `Arc` 持有，Tauri 命令永远拿不到 `&mut  self`。
    /// 读多写少，`RwLock` 是这里最直白的选择（`accounts` 同理）。
    config: RwLock<Config>,
    http: HttpClient,
    /// 统一接口缓存：所有命令共用这一个入口，加接口时不用改结构
    cache: ApiCache,
    /// 下载传输层。长生命周期持有，连接池才能复用。
    fetcher: Arc<dyn Fetcher>,
    /// 下载任务表（含取消令牌与最新进度）
    tasks: DownloadTasks,
    /// 正在运行的游戏会话
    launches: launch::LaunchSessions,
    /// 账号表。用 `RwLock`：读多写少，且 `AppState` 被 `Arc` 持有，
    /// 命令只能拿 `&self`。
    accounts: RwLock<AccountStore>,
    /// 资源收藏（Mod / 资源包 / 光影 / 整合包）。
    ///
    /// 只在本机，不做账号同步。读多写少（每次搜索都会查一遍「收没收藏」），
    /// 所以整份放内存，写盘只在增删时发生。
    pub(crate) favorites: RwLock<favorites::FavoriteStore>,
    /// 串行化 authlib-injector 的下载/落盘：登录后的后台预取和启动前的
    /// 保证可能同时发生，两个任务不能同时写同一个临时文件。
    authlib_jar_lock: tokio::sync::Mutex<()>,
    /// 自动选源的探测结论。`None` 表示还没测过。
    probe: Mutex<Option<ProbeResult>>,
    /// 自更新的状态机（检查 / 下载 / 应用）。
    ///
    /// 内部自带 `Mutex`，所以这里不需要 `RwLock` 包裹。
    update: update::UpdateState,
}

impl AppState {
    /// 初始化：探测数据目录 → 建目录 → 读配置 → 建 HTTP 客户端。
    pub fn init() -> Result<Self, AppError> {
        let layout = paths::detect_layout();
        paths::ensure_dir(layout.root())?;
        let config = config::load(&layout)?;
        // 账号服务地址先交给 `connect_account`：下面那些不接 `AppState` 的
        // 内部小函数要从模块级拿到它。地址是构建期注入的，不是用户配置。
        connect_account::configure_server(xcl_core::config::ACCOUNT_SERVER_ENDPOINT);
        // 空表就是空表：不再自动补离线账号。约定「没有账号就不许启动」，
        // 补一个 invisible 的账号只会让「为什么没添加却能玩」变成灵异问题
        let accounts = auth::load(&layout)?;
        let favorites = favorites::FavoriteStore::load(&layout);
        let http = HttpClient::new(policy_from(
            &config.mirror.provider,
            config.mirror.version_strategy,
            None,
        ))?;
        let fetcher: Arc<dyn Fetcher> = Arc::new(HttpFetcher::new()?);

        debug!(
            root = %layout.root().display(),
            cache = %layout.cache_root().display(),
            mirror = ?config.mirror.provider,
            accounts = accounts.accounts.len(),
            "XCL 数据目录已就绪"
        );

        // 强制更新状态要在建 AppState 时就从配置恢复：启动第一帧就得拦人，
        // 不能等联网回来。所以必须在 config 被 move 进 RwLock 之前读。
        let update = update::UpdateState::new(&config.update);

        Ok(Self {
            layout,
            config: RwLock::new(config),
            http,
            cache: ApiCache::new(VERSION_CACHE_TTL),
            fetcher,
            tasks: DownloadTasks::default(),
            launches: launch::LaunchSessions::default(),
            accounts: RwLock::new(accounts),
            favorites: RwLock::new(favorites),
            authlib_jar_lock: tokio::sync::Mutex::new(()),
            probe: Mutex::new(None),
            update,
        })
    }

    /// 初始化失败也不该让程序起不来：退回默认配置继续运行。
    pub fn init_or_default() -> Self {
        match Self::init() {
            Ok(state) => state,
            Err(err) => {
                warn!("初始化失败，使用默认配置继续：{err}");
                let layout = paths::detect_layout();
                let config = Config::default();
                let http = HttpClient::new(policy_from(
                    &config.mirror.provider,
                    config.mirror.version_strategy,
                    None,
                ))
                .expect("使用默认配置时不应失败");
                let fetcher: Arc<dyn Fetcher> =
                    Arc::new(HttpFetcher::new().expect("使用默认配置时不应失败"));
                // 降级路径：账号表与收藏都用默认值（不落盘，此时磁盘可能都不可用）
                let accounts = AccountStore::default();
                let favorites = favorites::FavoriteStore::default();
                let update = update::UpdateState::new(&config.update);
                Self {
                    layout,
                    config: RwLock::new(config),
                    http,
                    cache: ApiCache::new(VERSION_CACHE_TTL),
                    fetcher,
                    tasks: DownloadTasks::default(),
                    launches: launch::LaunchSessions::default(),
                    accounts: RwLock::new(accounts),
                    favorites: RwLock::new(favorites),
                    authlib_jar_lock: tokio::sync::Mutex::new(()),
                    probe: Mutex::new(None),
                    update,
                }
            }
        }
    }

    pub fn http(&self) -> &HttpClient {
        &self.http
    }

    /// 下载传输层。
    pub fn fetcher(&self) -> &Arc<dyn Fetcher> {
        &self.fetcher
    }

    /// 下载任务表。
    pub fn tasks(&self) -> &DownloadTasks {
        &self.tasks
    }

    /// 统一缓存入口。所有命令都从这里读写缓存。
    pub fn cache(&self) -> &ApiCache {
        &self.cache
    }

    /// 当前配置的副本。
    ///
    /// 返回克隆而不是守卫：调用方经常要把 `&Config` 一路传进纯函数或异步流程，
    /// 拿着读锁跨 `await` 容易顺手写出「读锁里做 IO」，也会把 `AppState` 推向非 `Send`。
    /// 配置结构很小，克隆成本可以忽略。
    pub fn config(&self) -> Config {
        self.config
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    // ---------- 源策略 ----------

    /// 拉元数据用的策略（版本清单、版本 JSON、资源索引）。
    ///
    /// 跟下载文件用的是两套配置项 —— 列表要新鲜度（镜像可能滞后），
    /// 文件要带宽。见 `MirrorConfig::download_strategy` 的注释。
    pub fn http_policy(&self) -> SourcePolicy {
        let config = self.config();
        policy_from(
            &config.mirror.provider,
            config.mirror.version_strategy,
            self.probe_result(),
        )
    }

    /// 下载文件用的策略。
    pub fn download_policy(&self) -> SourcePolicy {
        let config = self.config();
        policy_from(
            &config.mirror.provider,
            config.mirror.download_strategy,
            self.probe_result(),
        )
    }

    pub fn probe_result(&self) -> Option<ProbeResult> {
        *self.lock_probe()
    }

    /// 记录探测结论并立即生效：不用重启，也不用等下一次请求。
    pub fn record_probe(&self, result: ProbeResult) {
        *self.lock_probe() = Some(result);
        self.http.set_policy(self.http_policy());
        debug!(
            official_fast = result.official_fast,
            official_ms = ?result.official_ms,
            "自动选源：官方源{}",
            if result.official_fast {
                "够快，优先官方"
            } else {
                "偏慢，优先镜像"
            }
        );
    }

    /// 实际使用的并发数。配置为 `0`（自动）时按最终选中的源决定。
    pub fn effective_concurrency(&self) -> usize {
        let configured = self.config().download.concurrency;
        let value = if configured == CONCURRENCY_AUTO {
            auto_concurrency(self.download_policy().official_first())
        } else {
            configured
        };
        value.max(1) as usize
    }

    /// 分片参数（与具体文件无关的部分）。
    ///
    /// 是否分片按每个文件的真实 URL 决定，不再在这里用全局下载源一票否决：
    /// 同一批清单里可能既有官方 CDN 又有 BMCLAPI，一刀切会把 CDN 文件也压成单连接。
    /// 每个 `DownloadItem` 的 `split_urls` 由 `split_urls_for` 按实际候选地址生成。
    pub fn split_config(&self) -> xcl_download::SplitConfig {
        self.split_config_with(true)
    }

    /// 同上，但由调用方直接指定「这个源允许分片」。
    ///
    /// Java 运行时是唯一的调用者：它的文件全部来自 `piston-data`，
    /// 我们把官方地址排到了候选最前面（见 `java_runtime`），
    /// 所以不该再被 `download_policy()` 的镜像判断一票否决。
    pub fn split_config_with(&self, source_allows: bool) -> xcl_download::SplitConfig {
        let config = self.config();
        let allowed = config.download.split_enabled && source_allows;
        let per_file = if allowed {
            config.download.split_connections.max(2)
        } else {
            1
        };
        xcl_download::SplitConfig {
            max_connections: per_file as usize,
            total_connections: config.download.split_total_connections.max(2) as usize,
            ..Default::default()
        }
    }

    fn lock_probe(&self) -> std::sync::MutexGuard<'_, Option<ProbeResult>> {
        self.probe.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// 保存配置到磁盘，并更新内存状态（含下载源）。
    ///
    /// 返回落盘后的配置（已 `normalized()`）：前端直接拿它覆盖本地状态，
    /// 就不用自己再算一遍钳制后的值（内存下限、并发上限这些）。
    pub fn update_config(&self, next: Config) -> Result<Config, AppError> {
        let next = next.normalized();
        let mirror_changed = {
            let current = self.config.read().unwrap_or_else(|e| e.into_inner());
            next.mirror != current.mirror
        };

        config::save(&self.layout, &next)?;
        *self.config.write().unwrap_or_else(|e| e.into_inner()) = next.clone();

        if mirror_changed {
            // 换了源：旧缓存来自另一个源，旧探测结论也该重测
            self.cache.clear();
            *self.lock_probe() = None;
        }
        self.http.set_policy(self.http_policy());
        Ok(next)
    }

    /// 局部改配置：只提交改动过的字段，其余保持现状。
    ///
    /// 前端传的是部分 JSON（`{"launch":{"memoryMb":2048}}`），这里递归合并进
    /// 当前配置再反序列化。比「整份覆盖」安全，而且后端加字段时前端不用跟着改。
    pub fn patch_config(&self, patch: serde_json::Value) -> Result<Config, AppError> {
        let mut merged = serde_json::to_value(self.config())
            .map_err(|e| AppError::parse("配置序列化失败", e.to_string()))?;
        merge_json(&mut merged, patch);
        let next: Config = serde_json::from_value(merged)
            .map_err(|e| AppError::validation("设置值不合法", e.to_string()))?;
        self.update_config(next)
    }

    /// 恢复默认配置。只动 `config.toml`，账号、实例、缓存都不碰。
    pub fn reset_config(&self) -> Result<Config, AppError> {
        self.update_config(Config::default())
    }
}

/// 递归合并 JSON 对象：`patch` 里出现的键逐个覆盖，非对象直接替换。
///
/// 数组整体替换而不是拼接 —— 配置里没有「往数组追加」的语义，
/// 拼接只会让「清空列表」变成做不到的事。
fn merge_json(base: &mut serde_json::Value, patch: serde_json::Value) {
    match (base, patch) {
        (serde_json::Value::Object(base_map), serde_json::Value::Object(patch_map)) => {
            for (key, value) in patch_map {
                match base_map.get_mut(&key) {
                    Some(slot) => merge_json(slot, value),
                    None => {
                        base_map.insert(key, value);
                    }
                }
            }
        }
        (base, patch) => *base = patch,
    }
}

/// 配置 + 探测结论 → 源策略。
pub fn policy_from(
    provider: &MirrorProvider,
    strategy: SourceStrategy,
    probe: Option<ProbeResult>,
) -> SourcePolicy {
    let mirror = match provider {
        MirrorProvider::Official {} => Mirror::Official,
        MirrorProvider::Bmclapi {} => Mirror::Bmclapi,
        MirrorProvider::Custom { value } => Mirror::Custom(value.clone()),
    };
    let strategy = match strategy {
        SourceStrategy::MirrorFirst => Strategy::MirrorFirst,
        SourceStrategy::Auto => Strategy::Auto,
        SourceStrategy::OfficialFirst => Strategy::OfficialFirst,
    };
    SourcePolicy::new(mirror, strategy)
        .with_probe(probe.map(|p| p.official_fast).unwrap_or(false))
}

/// 给前端看的源名称。
pub fn mirror_label(policy: &SourcePolicy) -> String {
    match &policy.mirror {
        Mirror::Official => "官方源".to_string(),
        Mirror::Bmclapi => "BMCLAPI".to_string(),
        Mirror::Custom(url) => url.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::merge_json;
    use serde_json::json;

    #[test]
    fn merge_overrides_nested_keys_only() {
        let mut base = json!({
            "launch": { "memoryMb": 4096, "language": "zh_cn" },
            "ui": { "rememberWindow": true }
        });
        merge_json(&mut base, json!({ "launch": { "memoryMb": 2048 } }));

        // 改的那一项变了，同层其它项没被动
        assert_eq!(base["launch"]["memoryMb"], json!(2048));
        assert_eq!(base["launch"]["language"], json!("zh_cn"));
        assert_eq!(base["ui"]["rememberWindow"], json!(true));
    }

    #[test]
    fn merge_adds_unknown_keys_and_replaces_non_objects() {
        let mut base = json!({ "launch": { "isolation": "all" } });
        merge_json(&mut base, json!({ "launch": { "isolation": "off" }, "brand": "xcl" }));
        assert_eq!(base["launch"]["isolation"], json!("off"));
        assert_eq!(base["brand"], json!("xcl"));

        // 数组整体替换，不做拼接：配置里没有「追加」的语义
        let mut list = json!({ "a": [1, 2, 3] });
        merge_json(&mut list, json!({ "a": [] }));
        assert_eq!(list["a"], json!([]));
    }

    #[test]
    fn merge_into_empty_object_takes_patch() {
        let mut base = json!({});
        merge_json(&mut base, json!({ "download": { "concurrency": 16 } }));
        assert_eq!(base["download"]["concurrency"], json!(16));
    }
}
