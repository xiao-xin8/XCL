//! helper 的主流程。
//!
//! 这是 `main()` 的第一件事：参数里带 `--xcl-apply-update` 就完全不碰 Tauri，
//! 只做「等旧进程退出 → 换文件 → 起新版 → 看着它别死」。
//!
//! 顺序上有两条不能动：
//!
//! 1. 握手要在拿到互斥体之后。父进程收到 ready 就会退出，此时 helper
//!    必须已经确定自己能干活（文件校验过、互斥体在手），否则等于把
//!    一个人丢在空窗里。
//! 2. 互斥体要在启动新版之前释放。新版一启动会探测这个互斥体，发现被
//!    占用就以为「正在更新」而直接退出 —— 然后被 helper 判成启动失败去回滚，
//!    于是每次更新都白干。这个 bug 极难查。

use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

use xcl_core::{AppError, ErrorCode};

use crate::args::{ApplyArgs, UPDATED_FLAG, parse_apply_args};
use crate::log;
use crate::mutex::UpdateMutex;
use crate::ready::ReadyEvent;
use crate::replace;
use crate::splash::Splash;
use crate::verify;
use crate::wait::{WaitOutcome, wait_for_exit};

/// 等旧进程退出的上限。再久就不正常了，宁可放弃也不无限等。
pub const OLD_EXIT_TIMEOUT: Duration = Duration::from_secs(30);
/// 新版起来后观察多久算「活着」。
///
/// 5 秒足够盖住「配置读错 / dll 缺失 / 被隔离」这类立刻退出，又不会让
/// 盯着 splash 干等。
pub const NEW_ALIVE_WATCH: Duration = Duration::from_secs(5);

/// apply 模式的入口。
///
/// 返回 `true` 表示「本进程已经处理完（或已放弃）更新，不要继续启动界面」。
/// 返回 `false` 表示不是 apply 模式，走正常启动。
///
/// 注意：参数不合法时也返回 `true`。此时退回正常启动只会产生一个
/// 没有主窗口的僵尸进程，不如老老实实记日志退出。
pub fn try_run() -> bool {
    let argv: Vec<std::ffi::OsString> = std::env::args_os().collect();
    let args = match parse_apply_args(argv) {
        Ok(None) => return false,
        Ok(Some(args)) => args,
        Err(err) => {
            log::append(&format!("更新参数不合法，已放弃：{} / {}", err.title, err.message));
            return true;
        }
    };

    // 窗口建不出来也照常更新（`Splash::show` 自己会返回 None）
    let splash = Splash::show();
    match run(&args, splash.as_ref()) {
        Ok(()) => {
            if let Some(s) = &splash {
                s.close();
            }
        }
        Err(err) => {
            log::append(&format!(
                "更新失败：{} / {}{}",
                err.title,
                err.message,
                err.hint.as_deref().map(|h| format!("（{h}）")).unwrap_or_default()
            ));
            // 失败可能发生在「新版起来后立刻崩」——那时窗口已经关了，
            // 所以这里统一先收窗口，再用系统弹窗说清楚原因。
            if let Some(s) = &splash {
                s.close();
                std::thread::sleep(Duration::from_millis(200));
            }
            Splash::alert(
                "Xin Craft Launcher 更新失败",
                &format!(
                    "{}\n\n{}{}",
                    err.title,
                    err.message,
                    err.hint
                        .as_deref()
                        .map(|hint| format!("\n\n{hint}"))
                        .unwrap_or_default()
                ),
            );
        }
    }
    true
}

/// 启动入口：有没有「下好了还没装」的更新。
///
/// 返回 `true` 表示 helper 已经接手，本进程应当立刻退出，不要建主窗口。
///
/// 这是「双击即更新」：上一轮下载完直接关掉程序，这次打开看到的就是
/// 小窗口一闪 → 新版，而不是先开旧版再让他点一次。
pub fn try_apply_pending() -> bool {
    let Ok(exe) = std::env::current_exe() else {
        return false;
    };
    let Some(dir) = exe.parent() else {
        return false;
    };
    let work = crate::work_dir(dir);
    let Some(pending) = crate::pending::load(&work) else {
        return false;
    };

    // 已经追上了（或者本来就是过期记录）：清掉，正常启动
    if !pending.is_pending(env!("CARGO_PKG_VERSION")) {
        crate::pending::clear(&work);
        return false;
    }

    // 包没了 / 哈希不对：这次装不成了，清掉记录，别每次启动都卡在这
    if crate::verify::verify_artifact(&pending.file, pending.size, &pending.sha256).is_err() {
        log::append("待应用的更新包已失效，放弃本次自动更新");
        crate::pending::clear(&work);
        return false;
    }

    log::append(&format!("发现待应用的更新：{}", pending.version));
    // helper 之间会有一段重叠：父进程的窗口先出现（等握手），helper 起来后
    // 也在同一位置建自己的窗口，父进程退出时自然消失，视觉上是连续的。
    let _splash = Splash::show();
    match crate::spawn::launch(&exe, &pending.file, &pending.sha256, true) {
        // helper 已就绪：本进程退出，把活交给它
        Ok(()) => true,
        Err(err) => {
            log::append(&format!(
                "自动更新启动失败：{} / {}",
                err.title, err.message
            ));
            crate::pending::clear(&work);
            false
        }
    }
}

fn run(args: &ApplyArgs, splash: Option<&Splash>) -> Result<(), AppError> {    log::append(&format!(
        "helper 启动：旧进程 {}，目标 {}",
        args.pid,
        args.target.display()
    ));

    // 1. 复核更新包。父进程校验过一次，这里再确认等待期间没被换掉。
    verify::verify_artifact(&args.new_file, 0, &args.sha256)?;
    log::append("更新包校验通过");

    // 2. 互斥体：同一时间只允许一个 helper 在换文件
    let guard = UpdateMutex::acquire().ok_or_else(|| {
        AppError::new(
            ErrorCode::Validation,
            "已有更新正在进行",
            "另一个更新进程正在替换启动器",
        )
        .with_hint("等它完成后再试")
    })?;

    // 3. 握手：到这里才算「我真的准备好了」，父进程可以退
    if let Some(event) = ReadyEvent::open(args.pid) {
        event.signal();
    } else {
        // 事件打不开不致命：父进程会自己超时，重新点一次就行
        log::append("无法通知父进程（事件句柄打不开），继续执行");
    }

    // 4. 等旧进程真正退出
    log::append("等待旧进程退出");
    if wait_for_exit(args.pid, OLD_EXIT_TIMEOUT) == WaitOutcome::Timeout {
        return Err(AppError::new(
            ErrorCode::Io,
            "更新已中止",
            format!(
                "等待旧版本退出超过 {} 秒",
                OLD_EXIT_TIMEOUT.as_secs()
            ),
        )
        .with_hint("旧版本可能还有窗口没关，请关闭后重新检查更新"));
    }

    // 5. 换文件
    log::append("开始替换文件");
    let backup = replace::apply(&args.target, &args.new_file)?;
    log::append(&format!("替换完成，旧版本备份在 {}", backup.display()));

    // 6. 先放互斥体，再启动新版（顺序错误会导致新版自杀）
    drop(guard);

    if !args.restart {
        log::append("未要求重启，替换完成即退出");
        return Ok(());
    }

    // 7. 启动新版并观察
    log::append("启动新版本");
    match start_and_watch(&args.target, splash) {        Ok(()) => {
            log::append("更新成功");
            Ok(())
        }
        Err(err) => {
            log::append(&format!("新版启动失败：{}，开始回滚", err.message));
            rollback_and_restart(&args.target, &backup);
            Err(err)
        }
    }
}

/// 启动新版，并盯住前 5 秒。
///
/// 新版窗口一出现就关掉提示窗：那 5 秒是给「起来后又崩了」留的观察期，
/// 让人对着一个空壳等 5 秒是没必要的。真崩了会用系统弹窗兜底。
///
/// 进程还活着就返回 `Ok`（不去 wait，让它继续跑）；立刻退出就当作失败。
fn start_and_watch(target: &Path, splash: Option<&Splash>) -> Result<(), AppError> {
    let mut child = Command::new(target)
        .arg(UPDATED_FLAG)
        .current_dir(target.parent().unwrap_or_else(|| Path::new(".")))
        .spawn()
        .map_err(|err| {
            AppError::io(&err, "启动新版本")
                .with_hint("可能被安全软件拦截，请手动打开；旧版本已恢复")
        })?;

    // 新进程起来了：提示窗的使命结束，立刻撤
    if let Some(s) = splash {
        s.close();
    }

    let deadline = Instant::now() + NEW_ALIVE_WATCH;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                return Err(AppError::new(
                    ErrorCode::Io,
                    "新版本启动失败",
                    format!("新版本打开后立刻退出（{}）", describe(status)),
                )
                .with_hint("已自动恢复旧版本；可以稍后重试，或手动下载新版"));
            }
            Ok(None) => {}
            Err(err) => return Err(AppError::io(&err, "检查新版本状态")),
        }
        if Instant::now() >= deadline {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

/// 恢复备份，并把旧版拉起来 —— 至少还有个能用的启动器。
fn rollback_and_restart(target: &Path, backup: &Path) {
    match replace::rollback(target, backup) {
        Ok(()) => log::append("已恢复旧版本"),
        Err(err) => {
            // 到了这里只能手动改名了，日志里必须写清楚路径
            log::append(&format!(
                "回滚失败：{}（备份仍在 {}）",
                err.message,
                backup.display()
            ));
            return;
        }
    }
    match Command::new(target).spawn() {
        Ok(_) => log::append("已拉起旧版本"),
        Err(err) => log::append(&format!("拉起旧版本失败：{err}")),
    }
}

fn describe(status: std::process::ExitStatus) -> String {
    match status.code() {
        Some(code) => format!("退出码 {code}"),
        None => "被系统终止".to_string(),
    }
}
