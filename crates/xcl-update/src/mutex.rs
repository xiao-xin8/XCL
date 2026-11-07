//! 更新期间的互斥。
//!
//! 目的只有一个：别让两个进程同时改同一个 exe。具体挡两件事：
//!
//! - 已经被 helper 拿住时，又双击了旧 exe → 新实例看得到互斥体，
//!   提示「正在更新」然后退出，不会起出第二个旧版；
//! - 同时跑起两个 helper → 第二个直接退出。
//!
//! 注意顺序：helper 必须在启动新版之前释放它，否则新版一启动就把自己
//! 当成「正在更新」直接退出，然后被 helper 判成启动失败去回滚。

/// 互斥体名字。`Local\` 前缀表示只在当前登录会话内生效 ——
/// 换用户或换会话各管各的，符合便携版的语义。
pub const MUTEX_NAME: &str = "Local\\XCL-Update-InProgress";

#[cfg(windows)]
mod imp {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;

    use windows_sys::Win32::Foundation::{
        CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, HANDLE, SetLastError,
    };
    use windows_sys::Win32::System::Threading::{CreateMutexW, ReleaseMutex};

    use super::MUTEX_NAME;

    /// 持有期间别的进程拿不到它。`Drop` 时自动释放。
    pub struct UpdateMutex {
        handle: HANDLE,
    }

    impl UpdateMutex {
        /// 尝试拿到互斥体。`None` 表示已经有人拿着（正在更新）。
        pub fn acquire() -> Option<Self> {
            let name: Vec<u16> = OsStr::new(MUTEX_NAME)
                .encode_wide()
                .chain(std::iter::once(0))
                .collect();
            // SAFETY: 只创建/打开一个命名互斥体，不共享内存。
            unsafe {
                SetLastError(0);
                let handle = CreateMutexW(std::ptr::null(), 0, name.as_ptr());
                if handle.is_null() {
                    return None;
                }
                if GetLastError() == ERROR_ALREADY_EXISTS {
                    // 已经有实例拿着：句柄是「打开」来的，关掉不出错
                    CloseHandle(handle);
                    return None;
                }
                Some(Self { handle })
            }
        }

        /// 当前是否有别的进程正在更新。
        ///
        /// 内部是「试着拿一下，拿到就立刻放」：探测本身会短暂创建这个
        /// 互斥体，所以只适合启动早期的判断，不要放进循环。
        pub fn is_held() -> bool {
            Self::acquire().is_none()
        }
    }

    impl Drop for UpdateMutex {
        fn drop(&mut self) {
            // SAFETY: handle 来自 CreateMutexW，且 Drop 只会执行一次。
            unsafe {
                ReleaseMutex(self.handle);
                CloseHandle(self.handle);
            }
        }
    }
}

#[cfg(not(windows))]
mod imp {
    /// 非 Windows 上不需要（也没有）这套机制，保留同名 API 方便编译。
    pub struct UpdateMutex;

    impl UpdateMutex {
        pub fn acquire() -> Option<Self> {
            Some(Self)
        }

        pub fn is_held() -> bool {
            false
        }
    }
}

pub use imp::UpdateMutex;

/// 启动时用：正有更新在进行就等一等。
///
/// 返回 `true` 表示互斥体是空的（本来没更新，或者等到对方放手了）。
/// 返回 `false` 表示等到超时还没空出来 —— 调用方不该因此拒绝启动，
/// 只是意味着这会儿打开的可能还是旧镜像。
///
/// 为什么要等而不是直接退出：双击图标却什么都没发生，比打开一个旧版
/// 更让人困惑。而替换过程本身只持续几秒。
pub fn wait_until_free(timeout: std::time::Duration) -> bool {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        if let Some(guard) = UpdateMutex::acquire() {
            drop(guard);
            return true;
        }
        if std::time::Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 三个断言合在一个测试里：互斥体是进程内共享的命名对象，
    /// 拆成多个 `#[test]` 会被 cargo 并行执行互相干扰。
    #[test]
    fn mutex_semantics() {
        // 空着的时候：能拿到，wait 也立刻过
        assert!(wait_until_free(std::time::Duration::from_millis(50)));

        let first = UpdateMutex::acquire().expect("第一次应该拿到");
        assert!(UpdateMutex::acquire().is_none(), "已经被拿着时不该再拿到");
        assert!(UpdateMutex::is_held());
        assert!(!wait_until_free(std::time::Duration::from_millis(120)));

        drop(first);
        assert!(!UpdateMutex::is_held(), "释放后应该能再拿到");
        assert!(wait_until_free(std::time::Duration::from_millis(50)));
    }
}
