//! 主窗口的创建与唤回。
//!
//! 托盘模式下「关闭窗口」会真的销毁窗口（连同 WebView），所以这个文件
//! 必须能在「窗口不存在」的前提下重建它 —— 不能假设窗口一直都在。

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

use tauri::{AppHandle, Manager, WebviewWindowBuilder};
use tauri_plugin_window_state::{StateFlags, WindowExt as _};
use xcl_service::AppState;

pub const MAIN_LABEL: &str = "main";

/// 正在重建窗口。
///
/// 从多个入口同时唤回时（双击图标 + 点托盘、或者手快连点两下），
/// 不能让它们各建一个同名窗口。而且重建期间主线程会派发排队消息，
/// 回调有机会被重入 —— 这道闸是必要的，不是防御性代码。
static REBUILDING: AtomicBool = AtomicBool::new(false);

/// 本次重建是什么时候开始的（毫秒时间戳）。
///
/// 有了它才能判断「上次重建是还在跑，还是已经卡死了」。没有这个时间戳的话，
/// `REBUILDING` 一旦因为阻塞（不是失败、不是 panic，就是再也不返回）停在
/// `true`，之后每一次双击图标 / 点托盘都会在闸口直接 return ——
/// 表现就是「整个启动器像死了一样，只能任务管理器结束」。
static REBUILD_STARTED_MS: AtomicU64 = AtomicU64::new(0);

/// 重建窗口的容忍上限。超过它就认为上一次卡死了，强行放行重试。
///
/// 正常重建（建 WebView + 恢复位置 + show）在几百毫秒内完成，10 秒足够宽松。
const REBUILD_TIMEOUT_MS: u64 = 10_000;

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// 取当前配置。AppState 还没建好时（启动极早期）返回 `None`。
pub fn config_of(app: &AppHandle) -> Option<xcl_core::Config> {
    app.try_state::<Arc<AppState>>().map(|s| s.config())
}

/// 打开主窗口：还在就叫到前面，已经销毁就重建。
///
/// 三个入口都走这里 —— 托盘单击、托盘菜单、单实例的第二次启动。
///
/// ⚠️ 重建必须走 `tauri::async_runtime`，不能用裸 `std::thread`：
/// 这是在 Windows 上能不能建出 WebView2 的关键（见下文 spawn 处注释）。
pub fn open_main(app: &AppHandle) {
    if let Some(window) = app.get_webview_window(MAIN_LABEL) {
        // 窗口还在（只是最小化 / 被藏起来）：这些调用都是投递消息，不阻塞
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
        return;
    }

    // 走到这里说明窗口真的没了（关窗口后又点托盘/双击图标）。
    // 这是正常流程，用 debug 就好 —— 别每次唤回都刷一条 warn。
    tracing::debug!("主窗口不存在，重建");
    // 闸口不是简单的「有人在建就返回」：卡死的重建会永远占着这道闸。
    // 先看它跑了多久，超时就强行复位 —— 宁可多建一次，也不能让唤回彻底失灵。
    if REBUILDING.load(Ordering::SeqCst) {
        let started = REBUILD_STARTED_MS.load(Ordering::SeqCst);
        if now_ms().saturating_sub(started) < REBUILD_TIMEOUT_MS {
            return;
        }
        tracing::warn!(
            elapsed_ms = now_ms().saturating_sub(started),
            "上次重建主窗口超时未返回，强制复位后重试"
        );
        REBUILDING.store(false, Ordering::SeqCst);
    }
    if REBUILDING.swap(true, Ordering::SeqCst) {
        return;
    }
    REBUILD_STARTED_MS.store(now_ms(), Ordering::SeqCst);

    let handle = app.clone();
    // 必须用 Tauri 的 async 运行时，不能用裸 `std::thread`。
    //
    // 这是对照 clash-verge-rev 验证过的实现改的：它的窗口重建是
    // `pub async fn build_new_window()` + `AsyncHandler::spawn`（tokio），
    // 在 Windows 上销毁主窗口后能从托盘正常重建，内存也确实回收了。
    //
    // 我们之前用 `std::thread::spawn`，同样一段 `build()` 会卡死在 wry 的
    // `wait_with_pump` 里不再返回 —— 日志停在「尝试重建」那一行。
    // 差别就在执行环境：裸线程没有 Tauri 运行时给的那套 COM/消息泵上下文，
    // WebView2 的 `CreateCoreWebView2Controller` 完成回调投递不回来。
    tauri::async_runtime::spawn(async move {
        // 必须兜住 panic：一旦炸掉，下面的复位就执行不到，
        // `REBUILDING` 会永久停在 true —— 那正是「点图标、点托盘都没反应」的成因。
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| build(&handle)));
        REBUILDING.store(false, Ordering::SeqCst);
        match result {
            Ok(Ok(())) => tracing::info!("主窗口已重建"),
            Ok(Err(err)) => tracing::error!(%err, "重建主窗口失败"),
            Err(_) => tracing::error!("重建主窗口时发生 panic（已复位闸口，可再次点击唤回）"),
        }
    });
}

fn build(app: &AppHandle) -> tauri::Result<()> {
    // 以 tauri.conf.json 里的定义为准：尺寸、最小尺寸、无边框都从那儿来，
    // 不在这里重写一遍（重写就会出现「重启后窗口变了样」这类漂移）
    let conf = app
        .config()
        .app
        .windows
        .iter()
        .find(|w| w.label == MAIN_LABEL)
        .cloned()
        .ok_or(tauri::Error::WindowNotFound)?;

    let window = WebviewWindowBuilder::from_config(app, &conf)?.build()?;

    // ⚠️ 窗口建出来之后的操作必须回主线程做。
    //
    // `restore_state` / `show` / `set_focus` 都不是「投递完就走」，它们会发
    // 同步窗口消息（`SendMessageTimeoutW` 那一类）。在 worker 线程上调，主线程
    // 会被拖在消息循环里出不来 —— 现象就是「窗口出来了，但整个窗口无响应」。
    //
    // `build()` 本身倒是安全的：Tauri 把它投递给主线程创建，worker 只是等回执。
    let handle = app.clone();
    let _ = app.run_on_main_thread(move || {
        if config_of(&handle).map(|c| c.ui.remember_window).unwrap_or(true) {
            if let Err(err) = window.restore_state(
                StateFlags::SIZE | StateFlags::POSITION | StateFlags::MAXIMIZED,
            ) {
                tracing::warn!(?err, "恢复窗口状态失败，使用默认尺寸");
            }
        }
        let _ = window.show();
        let _ = window.set_focus();
    });
    Ok(())
}
