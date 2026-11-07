//! 原地替换 `XCL.exe`。
//!
//! Windows 不允许覆盖或删除正在运行的 exe，但允许重命名。所以替换必须
//! 等旧进程退出之后再做，并且要留一份 `.old` 以便回滚。
//!
//! 策略是「先用系统 API，不行再退化」：
//!
//! 1. `ReplaceFileW` —— 原子替换，同时把旧文件变成备份，还会尽力保留
//!    旧文件的属性与 ACL；
//! 2. 退化到两次重命名 —— 先 target → `.old`，再 new → target；
//!    第二步失败就把 `.old` 改回来。
//!
//! 杀软扫描会让文件短暂被占用（`SHARING_VIOLATION`），所以第一次尝试
//! 要重试若干次；重试策略写在 [`retry_delays`] 里。
//!
//! 这一层只碰「一个 exe 和它的备份」，不碰 `data/`、不碰缓存。

use std::path::{Path, PathBuf};
use std::time::Duration;

use xcl_core::{AppError, ErrorCode};

/// 备份文件的后缀。最终形态是 `XCL.exe.old`。
const BACKUP_SUFFIX: &str = ".old";

/// 被替换文件的备份路径（放在程序目录旁边，兜底用）。
pub fn backup_path(target: &Path) -> PathBuf {
    let mut name = target.as_os_str().to_os_string();
    name.push(BACKUP_SUFFIX);
    PathBuf::from(name)
}

/// 把 `new_exe` 换到 `target` 的位置，旧版本留成 `.old`。
///
/// 备份放在 `new_exe` 所在目录（也就是 `data/update/`），不放程序目录 ——
/// 程序目录里应该只剩一个 exe，否则会看见 `xxx.exe.old` 这种东西并误点。
///
/// 成功时返回备份路径。失败时保证 `target` 仍是原来那个文件 ——
/// 这是整个更新流程的底线：任何一步出错，手里都得有一个能用的启动器。
pub fn apply(target: &Path, new_exe: &Path) -> Result<PathBuf, AppError> {
    ensure_ready(target, new_exe)?;
    let backup = backup_next_to(new_exe, target);
    // 上次更新残留的 .old 会挡住 ReplaceFileW（它要求备份不存在）
    remove_if_exists(&backup)?;

    match retry_replace(target, new_exe, &backup) {
        Ok(()) => Ok(backup),
        // 系统 API 不通就走两次重命名；`first` 只用来拼错误信息
        Err(first) => match rename_swap(target, new_exe, &backup) {
            Ok(()) => Ok(backup),
            Err(second) => Err(AppError::new(
                ErrorCode::Io,
                "替换文件失败",
                format!("{first}；退化方案也失败：{second}"),
            )
            .with_hint("旧版本已保留。可能是文件被占用或被安全软件拦截，可稍后重试或手动覆盖")
            .with_retryable(true)),
        },
    }
}

/// 回滚：把备份移回去。
///
/// 只在「新版起来后立刻退出」时用。此时新版进程已经没了，`target` 可以让位。
pub fn rollback(target: &Path, backup: &Path) -> Result<(), AppError> {
    if !backup.is_file() {
        return Err(AppError::not_found(
            "找不到旧版本备份",
            format!("{} 不存在，无法自动回滚", backup.display()),
        )
        .with_hint("请手动下载新版启动器覆盖"));
    }
    let mut last: Option<std::io::Error> = None;
    for delay in retry_delays() {
        match move_file(backup, target) {
            Ok(()) => return Ok(()),
            Err(err) => {
                if !is_transient(&err) {
                    last = Some(err);
                    break;
                }
                last = Some(err);
                std::thread::sleep(delay);
            }
        }
    }
    let err = last.expect("重试循环至少执行一次");
    Err(AppError::io(&err, "恢复旧版本")
        .with_hint("旧版本仍在 `XCL.exe.old`，可手动改回 `XCL.exe`"))
}

/// 安装目录能不能写。
///
/// 更新前必须先问这个：`Program Files` 下的程序目录对普通账号只读，
/// 那种情况下连 `.XCL-new-*.exe` 都放不下，应该在下载前就提示
/// 「当前文件夹不能写入，无法自动更新」，而不是让他下完 40MB 再失败。
///
/// 判据是真写一个探针文件 —— 这比检查只读属性可靠（属性可以对但 ACL 不允许）。
pub fn dir_is_writable(dir: &Path) -> bool {
    let probe = dir.join(".XCL-write-probe");
    let ok = std::fs::write(&probe, b"1").is_ok();
    if ok {
        std::fs::remove_file(&probe).ok();
    }
    ok
}

/// 备份放在更新包同目录，文件名跟着 `target` 走：`…/data/update/Xin Craft Launcher.exe.old`。
fn backup_next_to(new_exe: &Path, target: &Path) -> PathBuf {
    let Some(dir) = new_exe.parent() else {
        return backup_path(target);
    };
    let name = target
        .file_name()
        .map(|name| name.to_os_string())
        .unwrap_or_else(|| "Xin Craft Launcher.exe".into());
    let mut file = name;
    file.push(BACKUP_SUFFIX);
    dir.join(file)
}

/// 前置检查。做完整的失败判定比中途失败更好 —— 此时还什么都没动。
fn ensure_ready(target: &Path, new_exe: &Path) -> Result<(), AppError> {
    if !target.is_file() {
        return Err(AppError::not_found(
            "找不到当前程序",
            format!("{} 不存在或不是文件", target.display()),
        ));
    }
    if !new_exe.is_file() {
        return Err(AppError::not_found(
            "找不到更新文件",
            format!("{} 不存在或不是文件", new_exe.display()),
        )
        .with_hint("文件可能已被安全软件隔离，请重新下载"));
    }
    // 更新包放在 `<程序目录>/data/update/` 下，和程序同卷 ——
    // `ReplaceFileW` 要求源和目标在同一卷，这是它能工作的前提。
    Ok(())
}

/// 带重试的 `ReplaceFileW`。
fn retry_replace(target: &Path, new_exe: &Path, backup: &Path) -> Result<(), std::io::Error> {
    let mut last = None;
    for delay in retry_delays() {
        match replace_once(target, new_exe, backup) {
            Ok(()) => return Ok(()),
            Err(err) => {
                let transient = is_transient(&err);
                last = Some(err);
                if !transient {
                    break;
                }
                std::thread::sleep(delay);
            }
        }
    }
    Err(last.expect("重试循环至少执行一次"))
}

/// 重试节奏：先密集（200ms × 10），再稀疏（1s × 5）。
///
/// 杀软刚扫完就放行，密集重试能立刻接上；真被长期占用时，
/// 稀疏那几次又给了关掉占用程序的时间。总上限约 7 秒。
fn retry_delays() -> Vec<Duration> {
    let mut delays = vec![Duration::from_millis(200); 10];
    delays.extend(std::iter::repeat_n(Duration::from_secs(1), 5));
    delays
}

/// 两次重命名的退化方案。
///
/// 第二步失败必须把备份改回来，否则 `target` 就空了 —— 既没有新版
/// 也没有旧版，这是绝对不能出现的状态。
fn rename_swap(target: &Path, new_exe: &Path, backup: &Path) -> Result<(), String> {
    move_file(target, backup).map_err(|err| format!("备份旧文件失败：{err}"))?;
    match move_file(new_exe, target) {
        Ok(()) => Ok(()),
        Err(err) => {
            let restore = move_file(backup, target);
            match restore {
                Ok(()) => Err(format!("移入新文件失败：{err}（已恢复旧文件）")),
                Err(restore_err) => Err(format!(
                    "移入新文件失败：{err}；恢复旧文件也失败：{restore_err}（旧文件仍在 {}）",
                    backup.display()
                )),
            }
        }
    }
}

/// 移动文件，兼容「目标已存在」的情况。
///
/// Windows 的 `rename` 不能覆盖已存在的文件，所以先删掉目标。
fn move_file(from: &Path, to: &Path) -> std::io::Result<()> {
    if to.exists() {
        std::fs::remove_file(to)?;
    }
    std::fs::rename(from, to)
}

fn remove_if_exists(path: &Path) -> Result<(), AppError> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(AppError::io(&err, "清理上次的更新备份")
            .with_hint("可能是备份文件被占用，请重启电脑后重试")),
    }
}

/// 临时性失败：文件被占用、没有权限。值得等一会儿再试。
fn is_transient(err: &std::io::Error) -> bool {
    // 5 = ERROR_ACCESS_DENIED，32 = ERROR_SHARING_VIOLATION
    matches!(err.raw_os_error(), Some(5) | Some(32))
}

#[cfg(windows)]
fn replace_once(target: &Path, new_exe: &Path, backup: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        REPLACEFILE_IGNORE_MERGE_ERRORS, ReplaceFileW,
    };

    fn wide(path: &Path) -> Vec<u16> {
        path.as_os_str().encode_wide().chain(std::iter::once(0)).collect()
    }

    let target = wide(target);
    let new_exe = wide(new_exe);
    let backup = wide(backup);
    let ok = unsafe {
        ReplaceFileW(
            target.as_ptr(),
            new_exe.as_ptr(),
            backup.as_ptr(),
            REPLACEFILE_IGNORE_MERGE_ERRORS,
            std::ptr::null(),
            std::ptr::null(),
        )
    };
    if ok == 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

/// 非 Windows 上没有 `ReplaceFileW`，直接走退化路径。
#[cfg(not(windows))]
fn replace_once(target: &Path, new_exe: &Path, backup: &Path) -> std::io::Result<()> {
    rename_swap(target, new_exe, backup).map_err(|msg| std::io::Error::other(msg))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::temp_dir;

    fn setup(name: &str) -> (PathBuf, PathBuf, PathBuf) {
        let dir = temp_dir(name);
        let target = dir.join("Xin Craft Launcher.exe");
        let new_exe = dir.join(".Xin Craft Launcher-new-0.2.0.exe");
        std::fs::write(&target, b"old-build").unwrap();
        std::fs::write(&new_exe, b"new-build").unwrap();
        (dir, target, new_exe)
    }

    #[test]
    fn apply_swaps_content_and_keeps_backup() {
        let (dir, target, new_exe) = setup("swap");
        let backup = apply(&target, &new_exe).unwrap();

        assert_eq!(std::fs::read(&target).unwrap(), b"new-build");
        assert_eq!(std::fs::read(&backup).unwrap(), b"old-build");
        // 新文件被搬走，不该再留在目录里
        assert!(!new_exe.exists());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn apply_replaces_stale_backup_instead_of_failing() {
        let (dir, target, new_exe) = setup("stale-backup");
        // 上一次更新残留的备份
        std::fs::write(backup_path(&target), b"ancient").unwrap();

        let backup = apply(&target, &new_exe).unwrap();
        assert_eq!(std::fs::read(&backup).unwrap(), b"old-build");
        assert_eq!(std::fs::read(&target).unwrap(), b"new-build");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn apply_refuses_missing_new_file_without_touching_target() {
        let (dir, target, new_exe) = setup("missing-new");
        std::fs::remove_file(&new_exe).unwrap();

        let err = apply(&target, &new_exe).unwrap_err();
        assert_eq!(err.code, ErrorCode::NotFound);
        // 关键：target 一个字节都不能变
        assert_eq!(std::fs::read(&target).unwrap(), b"old-build");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn apply_works_when_update_lives_in_another_directory() {
        // 实际布局就是这样：程序在根目录，更新包在 data/update/ 下
        let (dir, target, new_exe) = setup("cross-dir");
        let work = temp_dir("cross-dir-work");
        let moved = work.join(".Xin Craft Launcher-new-0.2.0.exe");
        std::fs::rename(&new_exe, &moved).unwrap();

        let backup = apply(&target, &moved).unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"new-build");
        // 备份跟着更新包走进 data/update，程序目录只剩一个 exe
        assert_eq!(backup.parent(), Some(work.as_path()));
        assert_eq!(std::fs::read(&backup).unwrap(), b"old-build");
        assert!(!dir.join("Xin Craft Launcher.exe.old").exists());

        std::fs::remove_dir_all(&dir).ok();
        std::fs::remove_dir_all(&work).ok();
    }

    #[test]
    fn rollback_restores_previous_build() {
        let (dir, target, new_exe) = setup("rollback");
        let backup = apply(&target, &new_exe).unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"new-build");

        rollback(&target, &backup).unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"old-build");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn rollback_without_backup_is_a_clear_error() {
        let (dir, target, _new_exe) = setup("no-backup");
        let err = rollback(&target, &backup_path(&target)).unwrap_err();
        assert_eq!(err.code, ErrorCode::NotFound);
        assert!(err.hint.is_some());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn backup_path_appends_suffix() {
        let path = backup_path(Path::new("C:\\game\\XCL.exe"));
        assert!(path.to_string_lossy().ends_with("XCL.exe.old"));
    }

    #[test]
    fn writable_directory_is_detected() {
        let dir = temp_dir("writable");
        assert!(dir_is_writable(&dir));
        // 探针文件不能留下
        assert!(!dir.join(".XCL-write-probe").exists());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn missing_directory_is_not_writable() {
        // 不存在的目录写不进去（更新前会先建目录，这里只确认判定不 panic）
        let dir = temp_dir("no-such").join("deep").join("deeper");
        assert!(!dir_is_writable(&dir));
    }

    #[test]
    fn retry_budget_is_bounded() {
        // 别让一次失败挂太久：总等待约 7 秒
        let total: Duration = retry_delays().iter().sum();
        assert!(total <= Duration::from_secs(8));
    }
}
