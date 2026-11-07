//! XCL 桌面外壳。
//!
//! 职责边界：只做 IPC 编排。任何业务逻辑都应落在 `crates/` 下的库中，
//! 这里只负责把命令转成库调用、把事件转给前端。

mod commands;
mod logging;
mod tray;
mod window;

// `get_webview_window` / `try_state` 都来自 Manager，必须显式引入
use tauri::Manager as _;
use xcl_service::AppState;

/// 关掉主窗口之后要不要继续留在右下角。
fn stays_in_background(app: &tauri::AppHandle) -> bool {
    window::config_of(app)
        .map(|c| c.ui.close_action == xcl_core::CloseAction::Background)
        .unwrap_or(true)
}

/// 处理上一轮更新留下的痕迹。
///
/// 删的都是 `data/update/` 里的临时文件：更新包、helper、`Xin Craft Launcher.exe.old`。
/// `data/` 里的其它东西一个字节都不碰。
///
/// 前提是「确认没有 helper 正在替换」。这里只探测一次、不等待 ——
/// helper 在等旧进程退出的整个过程里都持有互斥体，可能长达几十秒；
/// 启动路径上等它没有意义，只会让人觉得双击了没反应。
/// 被持有时这一轮就不清理，少删一次没有代价。
fn prepare_update_leftovers() {
    let just_updated = xcl_update::has_updated_flag(std::env::args_os());
    if just_updated {
        tracing::info!("本次启动来自一次更新");
    }

    if xcl_update::UpdateMutex::is_held() {
        tracing::warn!("更新正在进行，本次启动跳过清理");
        return;
    }

    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    let Some(dir) = exe.parent() else {
        return;
    };
    // 临时文件都在 `data/update/` 里，程序目录只剩一个 exe
    let work = xcl_update::work_dir(dir);
    // 下好但还没装的包必须留着 —— 它是下次启动要应用的那一个。
    // 按文件名无脑删，刚下好的更新就没了。
    let keep = xcl_update::pending::load(&work)
        .filter(|pending| pending.is_pending(env!("CARGO_PKG_VERSION")))
        .map(|pending| pending.file);
    let report = xcl_update::cleanup_leftovers(&work, keep.as_deref(), just_updated);
    if !report.removed.is_empty() {
        tracing::info!(files = ?report.removed, "已清理更新残留");
    }
    if report.kept_backup {
        tracing::debug!("保留上一版备份 XCL.exe.old，下次启动再清理");
    }
}

pub fn run() {
    // 默认只输出警告；设置页可以改级别，RUST_LOG 仍然优先，例如：
    //   RUST_LOG=xcl_download=debug,xcl_net=debug cargo tauri dev
    logging::init();

    // 上一轮更新可能留下 .new / .helper / .old，趁建窗口之前扫掉。
    prepare_update_leftovers();

    tauri::Builder::default()
        // 单实例必须最先注册：第二次启动时它负责把已有窗口叫到前面，
        // 而不是再开一个进程 —— 两个实例同时写配置/存档会互相踩。
        // 注意 helper 走的是 main() 里的 apply 分支，根本到不了这里。
        //
        // 必须走 `open_main`：托盘模式下主窗口可能已经被销毁，
        // 这里要是还写 `if let Some(window)`，双击图标会「没反应」。
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            window::open_main(app);
        }))
        // 窗口大小/位置记忆：官方标准插件，退出时自动落盘、启动时自动恢复。
        .plugin(tauri_plugin_window_state::Builder::new().build())
        // 选目录（Mod 手动安装）。只在 Rust 侧调用，不需要前端权限。
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            use std::sync::Arc;
            use tauri::Manager;
            // WindowExt 提供 restore_state（对 WebviewWindow / Window 都有实现）
            use tauri_plugin_window_state::WindowExt as _;

            // 上次如果是崩溃 / 被任务管理器结束的，退出清理跑不到，联机模块会留在
            // 房间里当幽灵成员（好友那边一直看到一个连不上的人）。
            // 启动时先扫一遍，保证「这一局从干净状态开始」。
            // 单实例插件先于本 setup 执行，所以不会误伤另一个正在用的启动器。
            xcl_service::connect_session::cleanup_orphans();

            // 先建状态：窗口要不要恢复位置，取决于配置里的「记住窗口大小与位置」，
            // 所以配置必须先读到（顺序反了就只能无条件恢复）
            let state = Arc::new(AppState::init_or_default());
            // 配置里的日志级别在 AppState 建好之后才有得读
            logging::set_level(state.config().log.level);

            // 在窗口「首次显示之前」把上次的大小/位置/最大化状态恢复掉。
            // 插件的 on_window_ready 恢复发生在窗口已按 conf 默认尺寸显示之后，
            // 会看到先小一下再跳大；conf 配了 visible:false，这里恢复完再
            // show，从第一帧起就是最终的窗口大小。
            if let Some(window) = app.get_webview_window("main") {
                if state.config().ui.remember_window {
                    match window.restore_state(
                        tauri_plugin_window_state::StateFlags::SIZE
                            | tauri_plugin_window_state::StateFlags::POSITION
                            | tauri_plugin_window_state::StateFlags::MAXIMIZED,
                    ) {
                        Ok(()) => {}
                        Err(e) => {
                            // 状态文件损坏等：不拦启动，按 conf 尺寸显示
                            tracing::warn!(?e, "恢复窗口状态失败，使用默认尺寸");
                        }
                    }
                }
                // VISIBLE flag 没交给插件（上次最小化时退出会存到 invisible），
                // 显示与否由我们决定：主窗口必须总可见
                let _ = window.show();
            }
            // 存成 Arc：下载任务要在后台存活，命令结束后仍需持有状态
            let prefetch_state = Arc::clone(&state);
            app.manage(state);

            // 启动时就检查登录组件：有就不管，没有就在后台静默下载。
            // 约 342 KB，不弹提示；失败只记日志，启动第三方账号前还会再保证一次。
            if !prefetch_state.authlib_jar_present() {
                let _ = tauri::async_runtime::spawn(async move {
                    if let Err(err) = prefetch_state.prefetch_authlib_jar().await {
                        tracing::warn!(error = %err, "启动时预下载 authlib-injector 失败");
                    }
                });
            }

            // 托盘要不要建，取决于配置。放在最后：需要 AppState 已 manage。
            tray::sync(app.handle());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::app_info,
            commands::app_paths,
            commands::connect_module_status,
            commands::connect_module_install,
            commands::connect_scan_port,
            commands::connect_start_host,
            commands::connect_start_guest,
            commands::connect_stop,
            commands::connect_set_port,
            commands::connect_state,
            commands::connect_account_state,
            commands::connect_account_login,
            commands::connect_account_register,
            commands::connect_account_logout,
            commands::connect_account_refresh,
            commands::connect_account_check,
            commands::connect_log,
            commands::app_open_dir,
            commands::config_get,
            commands::config_patch,
            commands::config_reset,
            commands::config_export,
            commands::config_import,
            commands::download_source_test,
            commands::background_pick,
            commands::background_image,
            commands::background_clear,
            commands::storage_info,
            commands::storage_clear,
            commands::version_list,
            commands::install_preview,
            commands::loader_kinds,
            commands::loader_versions,
            commands::mod_search,
            commands::mod_version_lines,
            commands::favorites_list,
            commands::favorite_set,
            commands::memory_trim,
            commands::memory_stats,
            commands::storage_unused_scan,
            commands::storage_unused_clean,
            commands::resource_project,
            commands::resource_targets,
            commands::resource_files,
            commands::resource_file_groups,
            commands::resource_install_targets,
            commands::resource_dependencies,
            commands::resource_install,
            commands::modpack_install,
            commands::pick_folder,
            commands::install_start,
            commands::install_cancel,
            commands::install_tasks,
            commands::install_forget,
            commands::installed_versions,
            commands::version_delete,
            commands::version_rename,
            commands::version_set_description,
            commands::version_set_favorite,
            commands::version_settings_get,
            commands::version_settings_patch,
            commands::version_stats,
            commands::version_set_icon,
            commands::version_clear_icon,
            commands::version_isolation,
            commands::version_set_isolation,
            commands::version_open_folder,
            commands::java_list,
            commands::java_detected,
            commands::java_components,
            commands::java_install,
            commands::java_remove,
            commands::java_suggest,
            commands::account_list,
            commands::account_add_offline,
            commands::account_add_authlib,
            commands::account_set_active,
            commands::account_set_skin_model,
            commands::account_refresh,
            commands::account_remove,
            commands::skin_current,
            commands::launch_start,
            commands::launch_stop,
            commands::launch_sessions,
            commands::update_status,
            commands::update_check,
            commands::update_skip,
            commands::update_download,
            commands::update_cancel,
            commands::update_apply,
            commands::ping
        ])
        .build(tauri::generate_context!())
        .expect("启动 Tauri 应用失败")
        .run(|app_handle, event| match event {
            // 托盘常驻时「最后一个窗口关了」不等于「该退出了」——
            // 那个窗口是我们主动销毁的，托盘图标还等着把界面叫回来。
            tauri::RunEvent::ExitRequested { api, code, .. } => {
                // code 非空表示有人明确要求退出（托盘菜单的「退出」），必须放行
                if code.is_none() && stays_in_background(app_handle) {
                    api.prevent_exit();
                }
            }
            tauri::RunEvent::WindowEvent {
                label,
                event: tauri::WindowEvent::CloseRequested { api, .. },
                ..
            } => {
                if label != window::MAIN_LABEL || !stays_in_background(app_handle) {
                    return;
                }
                // 不 hide 而是 destroy：hide 只是把窗口藏起来，WebView2 进程
                // 一个都不退，内存和开着窗口时几乎一样。destroy 才是真的释放。
                // 注意顺序 —— 先拦默认关闭，再自己销毁，否则窗口先没了。
                //
                // 同步销毁是安全的（clash-verge-rev 也这么写）：主线程自己持锁拆完
                // 再返回，不存在「后台线程持锁、主线程来抢」的竞争。
                // 真正会死锁的是之后的重建，那边已经改成走 async 运行时。
                api.prevent_close();
                if let Some(w) = app_handle.get_webview_window(&label) {
                    if let Err(err) = w.destroy() {
                        tracing::error!(%err, "销毁主窗口失败");
                    }
                }
            }
            // 真退出（托盘菜单「退出」，或选择「直接退出」时的关窗）：
            // 必须把联机模块收掉。它是独立进程，父进程死了不会跟着走，
            // 会一直留在房间里 —— 表现就是「启动器已经关了，好友那边还挂着一个
            // 幽灵成员」，还会占着虚拟网和端口，下次开房撞车。
            tauri::RunEvent::Exit => {
                xcl_service::connect_session::cleanup_orphans();
            }
            _ => {}
        });
}
