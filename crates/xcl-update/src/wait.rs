//! 等旧进程退出。
//!
//! 替换 exe 的唯一前提是「旧进程真的没了」。这里不用管进程是不是崩了、
//! 是不是被杀了 —— 只关心句柄还能不能拿到、能不能等到它变成有信号状态。

use std::time::Duration;

/// 等待结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WaitOutcome {
    /// 进程已经不在（拿不到句柄也按这个处理）
    Exited,
    /// 等到超时，旧版本可能还开着
    Timeout,
}

#[cfg(windows)]
pub fn wait_for_exit(pid: u32, timeout: Duration) -> WaitOutcome {
    use windows_sys::Win32::Foundation::{CloseHandle, WAIT_OBJECT_0};
    use windows_sys::Win32::System::Threading::{
        OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject,
    };

    // SAFETY: 只拿一个等待用的句柄，不读不写进程内存；句柄用后立刻关闭。
    unsafe {
        let handle = OpenProcess(PROCESS_SYNCHRONIZE, 0, pid);
        if handle.is_null() {
            // 两种可能：进程已经没了（正常路径），或者权限不足拿不到句柄。
            // 后者会在替换文件时变成 sharing violation，被重试逻辑兜住，
            // 所以这里统一当作「已退出」，不额外报错。
            return WaitOutcome::Exited;
        }
        let result = WaitForSingleObject(handle, timeout.as_millis() as u32);
        CloseHandle(handle);
        if result == WAIT_OBJECT_0 {
            WaitOutcome::Exited
        } else {
            WaitOutcome::Timeout
        }
    }
}

/// 非 Windows 上没有 helper 的用武之地，只保证能编译、能测。
#[cfg(not(windows))]
pub fn wait_for_exit(pid: u32, timeout: Duration) -> WaitOutcome {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        if !std::path::Path::new(&format!("/proc/{pid}")).exists() {
            return WaitOutcome::Exited;
        }
        if std::time::Instant::now() >= deadline {
            return WaitOutcome::Timeout;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absent_process_is_treated_as_exited() {
        // 一个几乎不可能存在的 PID
        assert_eq!(
            wait_for_exit(0x7FFF_FFF0, Duration::from_millis(100)),
            WaitOutcome::Exited
        );
    }

    #[test]
    fn live_process_times_out() {
        // 自己肯定活着，所以只能是超时
        assert_eq!(
            wait_for_exit(std::process::id(), Duration::from_millis(120)),
            WaitOutcome::Timeout
        );
    }
}
