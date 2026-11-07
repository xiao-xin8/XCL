//! 把联机模块绑进 Windows Job Object，让它的命拴在启动器上。
//!
//! 为什么不能只靠「退出时 taskkill」：清理代码只能覆盖「我们正常走到退出流程」那一条路。
//! 崩溃、任务管理器强杀、蓝屏重启，任何一种都轮不到我们执行清理 —— 联机模块就会留在
//! 房间里当幽灵成员，好友那边一直看到一个连不上的人，还占着虚拟网和端口。
//!
//! Job Object 是内核级方案：给 job 打上 `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` 之后，
//! 最后一个 job 句柄关闭时，Windows 内核会把 job 里所有进程直接终结。
//! 启动器进程一死（不管怎么死），它持有的句柄就被系统关掉，子进程跟着没命 ——
//! 不需要任何一行清理代码参与。
//!
//! 只绑联机模块，不绑自己：如果把当前进程自己也 assign 进去，那么启动器 spawn 的
//! Minecraft 也会进同一个 job，退出启动器会把正在玩的游戏一起杀掉。

#[cfg(windows)]
mod imp {
    use std::sync::OnceLock;

    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
        SetInformationJobObject,
    };

    /// Job 句柄。必须活到进程结束：句柄一关，内核立刻终结 job 里所有进程。
    /// 用 `usize` 存是为了让 `OnceLock` 满足 `Sync`（`HANDLE` 是裸指针）。
    static JOB: OnceLock<usize> = OnceLock::new();

    fn handle() -> usize {
        *JOB.get_or_init(|| unsafe {
            let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if job.is_null() {
                return 0;
            }
            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            let ok = SetInformationJobObject(
                job,
                JobObjectExtendedLimitInformation,
                &info as *const _ as *const core::ffi::c_void,
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            );
            if ok == 0 {
                CloseHandle(job);
                return 0;
            }
            job as usize
        })
    }

    pub fn bind(child: &tokio::process::Child) -> bool {
        let job = handle();
        if job == 0 {
            return false;
        }
        // 必须在 spawn 之后立刻绑：子进程之后再 fork 出来的东西会自动继承 job
        let Some(raw) = child.raw_handle() else {
            return false;
        };
        unsafe { AssignProcessToJobObject(job as HANDLE, raw as HANDLE) != 0 }
    }
}

#[cfg(not(windows))]
mod imp {
    /// 非 Windows 平台不处理（进程组语义不同，暂不支持）。
    pub fn bind(_child: &tokio::process::Child) -> bool {
        false
    }
}

pub use imp::bind;
