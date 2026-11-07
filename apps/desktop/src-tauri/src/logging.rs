//! 日志级别：配置文件里可改，改完立刻生效。
//!
//! 用 `reload` 层的句柄替换过滤器。`RUST_LOG` 仍然优先 —— 它是给开发排查用的，
//! 设了它就以它为准，设置页那一项不生效（否则调试时改一下设置就把日志关掉了）。

use std::sync::OnceLock;

use tracing_subscriber::EnvFilter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::reload;
use tracing_subscriber::util::SubscriberInitExt;
use xcl_core::config::LogLevel;

type LevelHandle = reload::Handle<EnvFilter, tracing_subscriber::Registry>;

static LEVEL: OnceLock<LevelHandle> = OnceLock::new();
/// `RUST_LOG` 是否已经把级别钉死
static ENV_PINNED: OnceLock<bool> = OnceLock::new();

/// 初始化全局订阅者。默认只输出警告，正常流程的 info/debug 会把控制台刷满。
pub fn init() {
    let pinned = std::env::var_os("RUST_LOG").is_some();
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("warn"));
    let (layer, handle) = reload::Layer::new(filter);

    tracing_subscriber::registry()
        .with(layer)
        .with(tracing_subscriber::fmt::layer().with_target(false))
        .init();

    let _ = ENV_PINNED.set(pinned);
    let _ = LEVEL.set(handle);
}

/// 按配置里的级别换过滤器。设置页改完调用一次。
pub fn set_level(level: LogLevel) {
    if ENV_PINNED.get().copied().unwrap_or(false) {
        return;
    }
    if let Some(handle) = LEVEL.get() {
        if let Err(err) = handle.reload(EnvFilter::new(level.as_filter())) {
            // 换过滤器失败不该影响运行，原级别继续生效
            tracing::warn!("切换日志级别失败：{err}");
        }
    }
}
