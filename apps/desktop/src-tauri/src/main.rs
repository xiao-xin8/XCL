#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // 更新模式必须在一切之前分流。
    //
    // `try_run` 里读的是 `--xcl-apply-update`：命中就只做「等旧进程退出 →
    // 换文件 → 起新版」，完全不碰 logging、Tauri builder、单实例插件。
    //
    // 顺序反了会怎样：helper 被单实例插件当成「第二次启动」，直接退出，
    // 于是谁也没去换文件，看到的是「点了更新，什么都没发生」。
    if xcl_update::try_run() {
        return;
    }

    // 上次下好了更新但没装（直接关了程序）：这次打开直接装完，
    // 不开主窗口。返回 true 表示 helper 已经接手，本进程退出即可。
    if xcl_update::try_apply_pending() {
        return;
    }

    // 上次点「清理缓存」时界面正占着的那些文件，现在 WebView2 还没起来、文件是自由的。
    // 必须早于 `run()`：Tauri 一进 setup 窗口就已经建好，缓存立刻又被锁住。
    let _ = xcl_service::webview_cache::purge_on_boot();

    xcl_desktop::run()
}
