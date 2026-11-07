//! 系统托盘。
//!
//! 托盘是「窗口全部销毁之后，进程还活着的唯一入口」。托盘模式下主窗口被
//! `destroy` 掉，WebView2 随之退出，只能靠这里的图标把界面叫回来。

use tauri::menu::{MenuBuilder, MenuItemBuilder};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::AppHandle;

/// 托盘图标 id。固定值，用来判断「当前有没有托盘」。
const TRAY_ID: &str = "main-tray";
const MENU_SHOW: &str = "tray-show";
const MENU_QUIT: &str = "tray-quit";

/// 按配置同步托盘：该有就建，不该有就删。
///
/// 可以重复调用 —— 设置项改完立刻调一次，不需要重启应用。
pub fn sync(app: &AppHandle) {
    // 选「后台运行」才有托盘；选「直接退出」时托盘没有存在的意义
    let want = crate::window::config_of(app)
        .map(|c| c.ui.close_action == xcl_core::CloseAction::Background)
        .unwrap_or(true);
    let have = app.tray_by_id(TRAY_ID).is_some();
    match (want, have) {
        (true, false) => {
            if let Err(err) = create(app) {
                tracing::error!(%err, "创建托盘图标失败");
            }
        }
        (false, true) => {
            let _ = app.remove_tray_by_id(TRAY_ID);
        }
        _ => {}
    }
}

fn create(app: &AppHandle) -> tauri::Result<()> {
    let show = MenuItemBuilder::with_id(MENU_SHOW, "显示主界面").build(app)?;
    let quit = MenuItemBuilder::with_id(MENU_QUIT, "退出 Xin Craft Launcher").build(app)?;
    let menu = MenuBuilder::new(app).items(&[&show, &quit]).build()?;

    let mut builder = TrayIconBuilder::with_id(TRAY_ID)
        .tooltip("Xin Craft Launcher")
        .menu(&menu)
        // 左键单击直接叫回窗口；右键才展开菜单。
        // 双击在 Windows 上各家习惯不统一，单击更快也不会误触。
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id().0.as_str() {
            MENU_SHOW => crate::window::open_main(app),
            // 必须走 exit：如果调 window.close() 会被 CloseRequested 拦一次，
            // 于是又缩回托盘，永远退不掉
            MENU_QUIT => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                crate::window::open_main(tray.app_handle());
            }
        });

    // 直接用打包进 exe 的窗口图标，不额外维护一套托盘图
    if let Some(icon) = app.default_window_icon().cloned() {
        builder = builder.icon(icon);
    }
    builder.build(app)?;
    Ok(())
}
