//! helper 的握手事件。
//!
//! 为什么需要它：helper 是旧 exe 的副本，它启动后可能因为参数错、
//! 被杀软拦、文件缺失等原因立刻死掉。如果旧进程不等一句「我起来了」就
//! 自己退出，会面对「主界面关了、新界面没开、文件也没换」的空窗。
//!
//! 所以顺序是：父进程先建事件 → 启动 helper → helper 校验完、拿到互斥体、
//! 准备开始等待之后再 signal → 父进程收到才退出。
//!
//! 事件名里带旧进程 PID，同一秒内多次尝试也不会串。

/// 事件名里带旧进程 PID，同一秒内多次尝试也不会串。

/// 握手事件名。
pub fn event_name(pid: u32) -> String {
    format!("Local\\XCL-Update-Ready-{pid}")
}

/// 父进程与 helper 各持一端。
#[cfg(windows)]
mod imp {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;
    use std::time::Duration;

    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0};
    use windows_sys::Win32::System::Threading::{
        CreateEventW, EVENT_MODIFY_STATE, OpenEventW, SetEvent, WaitForSingleObject,
    };

    use super::event_name;

    /// 一个命名事件句柄。父进程 `create`，helper `open`。
    pub struct ReadyEvent {
        handle: HANDLE,
    }

    impl ReadyEvent {
        /// 父进程侧：创建（manual-reset，初始无信号）。
        ///
        /// 必须在启动 helper 之前调用，否则 helper 可能打开失败。
        pub fn create(pid: u32) -> Option<Self> {
            let name = wide(&event_name(pid));
            // SAFETY: 只创建一个命名事件对象。
            unsafe {
                let handle = CreateEventW(std::ptr::null(), 1, 0, name.as_ptr());
                if handle.is_null() {
                    None
                } else {
                    Some(Self { handle })
                }
            }
        }

        /// helper 侧：打开父进程建好的事件。
        pub fn open(pid: u32) -> Option<Self> {
            let name = wide(&event_name(pid));
            // SAFETY: 只打开一个已存在的命名事件对象。
            unsafe {
                let handle = OpenEventW(EVENT_MODIFY_STATE, 0, name.as_ptr());
                if handle.is_null() {
                    None
                } else {
                    Some(Self { handle })
                }
            }
        }

        /// 告诉父进程「可以退了」。失败不致命：父进程会走它自己的超时分支。
        pub fn signal(&self) {
            // SAFETY: handle 有效且由本结构持有。
            unsafe {
                SetEvent(self.handle);
            }
        }

        /// 等对方 signal。返回是否在超时前收到。
        pub fn wait(&self, timeout: Duration) -> bool {
            // SAFETY: handle 有效。
            unsafe { WaitForSingleObject(self.handle, timeout.as_millis() as u32) == WAIT_OBJECT_0 }
        }
    }

    impl Drop for ReadyEvent {
        fn drop(&mut self) {
            // SAFETY: handle 来自 Create/OpenEventW，Drop 只跑一次。
            unsafe {
                CloseHandle(self.handle);
            }
        }
    }

    fn wide(text: &str) -> Vec<u16> {
        OsStr::new(text)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect()
    }
}

/// 其它平台上没有这套机制，保留同名 API 让上层逻辑不必写 cfg。
#[cfg(not(windows))]
mod imp {
    use std::time::Duration;

    pub struct ReadyEvent;

    impl ReadyEvent {
        pub fn create(_pid: u32) -> Option<Self> {
            Some(Self)
        }

        pub fn open(_pid: u32) -> Option<Self> {
            Some(Self)
        }

        pub fn signal(&self) {}

        pub fn wait(&self, _timeout: Duration) -> bool {
            true
        }
    }
}

pub use imp::ReadyEvent;

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn event_name_is_per_pid() {
        assert!(event_name(1234).ends_with("1234"));
        assert_ne!(event_name(1), event_name(2));
    }

    #[test]
    #[cfg(windows)]
    fn signal_and_wait_roundtrip() {
        let pid = 4242;
        let parent = ReadyEvent::create(pid).unwrap();
        let helper = ReadyEvent::open(pid).unwrap();
        // 还没 signal 时等一小会儿应该是超时
        assert!(!parent.wait(Duration::from_millis(30)));
        helper.signal();
        assert!(parent.wait(Duration::from_millis(500)));
    }
}
