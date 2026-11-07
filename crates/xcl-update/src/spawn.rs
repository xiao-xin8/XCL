//! 父进程侧：准备并启动 helper。
//!
//! 为什么要复制自己而不是直接跑下载下来的新 exe：
//!
//! - 新 exe 在换到位之前绝不执行，避免「从临时目录运行未知程序」这个
//!   典型的杀软特征；
//! - helper 与当前进程同哈希，能复用已有的文件信誉；
//! - 仍然是单文件分发，不需要额外带一个 `updater.exe`。
//!
//! 调用 [`launch`] 成功之后，父进程就可以正常收尾退出了 —— helper 已经
//! 明确表示「我起来了」。

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use xcl_core::{AppError, ErrorCode};

use crate::args::APPLY_FLAG;
use crate::log;
use crate::ready::ReadyEvent;

/// 等 helper 握手的时间上限。超过就认为它没起来。
pub const READY_TIMEOUT: Duration = Duration::from_secs(5);

/// helper 的位置：放在更新工作目录里（`<程序目录>/data/update/`），
/// 名字由当前程序的名字派生。
///
/// 不写死成某个固定名：包名改了（比如从内部名换成正式全名）之后，
/// 写死的名字会让 helper 复制到一个跟程序无关的文件上。
/// 也不放程序目录：那里应该只剩一个 exe，会看见并误点其他 exe。
pub fn helper_path(target: &Path, work_dir: &Path, pid: u32) -> PathBuf {
    let stem = binary_stem(target);
    work_dir.join(format!(".{stem}-helper-{pid}.exe"))
}

/// 程序文件名（不含扩展名）。取不到就退回 `Xin Craft Launcher` ——
/// 这只是临时文件名，退回默认值不影响正确性。
fn binary_stem(target: &Path) -> String {
    target
        .file_stem()
        .and_then(|stem| stem.to_str())
        .filter(|stem| !stem.trim().is_empty())
        .unwrap_or("Xin Craft Launcher")
        .to_string()
}

/// 复制自己 → 启动 helper → 等它 ready。
///
/// 成功后父进程应当尽快收尾退出。失败时不会留下 helper 文件。
pub fn launch(
    target: &Path,
    new_file: &Path,
    sha256: &str,
    restart: bool,
) -> Result<(), AppError> {
    let pid = std::process::id();
    // helper 跟着更新包走：两者都在 `<程序目录>/data/update/` 下
    let work_dir = new_file
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| target.parent().unwrap_or_else(|| Path::new(".")).to_path_buf());
    let helper = helper_path(target, &work_dir, pid);

    // 先把事件建好，再启动 helper —— 反过来的话 helper 可能打不开句柄
    let event = ReadyEvent::create(pid).ok_or_else(|| {
        AppError::new(
            ErrorCode::Io,
            "无法准备更新",
            "创建握手事件失败",
        )
        .with_hint("可能是系统资源不足，重启后再试")
    })?;

    copy_self(&helper)?;

    // 事件已创建：后续任何失败都要把 helper 清掉
    let mut child = match spawn_helper(&helper, target, pid, new_file, sha256, restart) {
        Ok(child) => child,
        Err(err) => {
            std::fs::remove_file(&helper).ok();
            return Err(err);
        }
    };

    let deadline = Instant::now() + READY_TIMEOUT;
    loop {
        if event.wait(Duration::from_millis(100)) {
            log::append("helper 已就绪，父进程可以退出");
            return Ok(());
        }
        match child.try_wait() {
            Ok(Some(status)) => {
                std::fs::remove_file(&helper).ok();
                return Err(AppError::new(
                    ErrorCode::Io,
                    "更新程序启动失败",
                    format!("更新程序刚启动就退出了（退出码 {:?}）", status.code()),
                )
                .with_hint("可以在日志目录查看 apply.log"));
            }
            Ok(None) => {}
            Err(_) => {}
        }
        if Instant::now() >= deadline {
            // helper 还活着但没握手：别留着它，免得下次更新被判成「已有更新在进行」
            child.kill().ok();
            child.wait().ok();
            std::fs::remove_file(&helper).ok();
            return Err(AppError::new(
                ErrorCode::Io,
                "更新程序没有响应",
                format!("等待更新程序准备就绪超过 {} 秒", READY_TIMEOUT.as_secs()),
            )
            .with_hint("请重试；若反复出现，可以手动下载新版覆盖"));
        }
    }
}

fn copy_self(helper: &Path) -> Result<(), AppError> {
    let exe = std::env::current_exe().map_err(|err| AppError::io(&err, "定位当前程序"))?;
    // 上次残留的同名文件会挡住复制
    std::fs::remove_file(helper).ok();
    std::fs::copy(&exe, helper)
        .map(|_| ())
        .map_err(|err| {
            AppError::io(&err, "准备更新程序").with_hint(
                "安装目录可能不可写，或被安全软件拦截；可以把启动器换到普通文件夹再试",
            )
        })
}

fn spawn_helper(
    helper: &Path,
    target: &Path,
    pid: u32,
    new_file: &Path,
    sha256: &str,
    restart: bool,
) -> Result<std::process::Child, AppError> {
    let mut cmd = Command::new(helper);
    // 逐个 arg 传，不拼命令行：路径里带空格、引号都不会出事
    cmd.arg(APPLY_FLAG)
        .arg("--pid")
        .arg(pid.to_string())
        .arg("--target")
        .arg(target)
        .arg("--new")
        .arg(new_file)
        .arg("--sha256")
        .arg(sha256);
    if restart {
        cmd.arg("--restart");
    }
    cmd.current_dir(target.parent().unwrap_or_else(|| Path::new(".")));
    cmd.spawn().map_err(|err| {
        AppError::io(&err, "启动更新程序")
            .with_hint("可能被安全软件拦截；可以手动下载新版覆盖")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn helper_lives_in_the_work_dir() {
        let helper = helper_path(
            Path::new("C:\\game\\Xin Craft Launcher.exe"),
            Path::new("C:\\game\\data\\update"),
            99,
        );
        assert_eq!(
            helper,
            PathBuf::from("C:\\game\\data\\update\\.Xin Craft Launcher-helper-99.exe")
        );
    }

    #[test]
    fn helper_name_follows_the_real_binary_name() {
        // 包名换了，临时文件跟着换 —— 不写死任何品牌名
        let helper = helper_path(
            Path::new("C:\\game\\Whatever.exe"),
            Path::new("C:\\game\\data\\update"),
            7,
        );
        assert_eq!(
            helper,
            PathBuf::from("C:\\game\\data\\update\\.Whatever-helper-7.exe")
        );
    }

    #[test]
    fn helper_name_varies_with_pid() {
        let dir = Path::new("C:\\game\\data\\update");
        let target = Path::new("C:\\game\\Xin Craft Launcher.exe");
        assert_ne!(helper_path(target, dir, 1), helper_path(target, dir, 2));
    }
}
