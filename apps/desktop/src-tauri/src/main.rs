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

    xcl_desktop::run()
}
