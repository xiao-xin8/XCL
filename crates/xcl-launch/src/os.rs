//! 平台相关的启动前 / 启动后处理。
//!
//! 两个都只在 Windows 上有实际动作，其它平台直接返回 `Ok(())` —— 设置项本身
//! 是跨平台的，只是这里没有可做的事。
//!
//! 进程优先级三档：
//! - 高性能显卡不是 JVM 参数，而是往注册表
//!   `HKCU\Software\Microsoft\DirectX\UserGpuPreferences` 按 exe 全路径写
//!   `GpuPreference=2;`（`ModMain.vb:802` 的 `SetGPUPreference`）；
//! - 进程优先级是拉起之后用 `SetPriorityClass` 设一次（`ModLaunch.vb:2127`），
//!   只有 低 / 普通 / 高 三档，没有「实时」。

use std::path::Path;

use xcl_core::config::ProcessPriority;

/// 把给定的可执行文件标记为「使用高性能显卡」。
///
/// 显卡偏好按 exe 全路径记，所以 java.exe 和启动器本身都要写 —— 只写 java
/// 的话，从某些启动入口拉起时仍可能落到核显上。
pub fn prefer_high_perf_gpu(exes: &[&Path]) -> Result<(), String> {
    #[cfg(windows)]
    {
        use winreg::enums::HKEY_CURRENT_USER;
        use winreg::RegKey;

        // 值名就是 exe 的完整路径，值是固定的 `GpuPreference=2;`（2 = 高性能）
        const VALUE: &str = "GpuPreference=2;";

        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        let (key, _) = hkcu
            .create_subkey(r"Software\Microsoft\DirectX\UserGpuPreferences")
            .map_err(|e| format!("打开显卡偏好注册表失败：{e}"))?;
        for exe in exes {
            let name = exe.to_string_lossy();
            // 已经是目标值就不写：先读再写，避免每次启动都碰一遍注册表，
            // 也免得在没写权限的账户上每次启动都刷一条失败日志
            if key
                .get_value::<String, _>(name.as_ref())
                .ok()
                .is_some_and(|current| current == VALUE)
            {
                continue;
            }
            key.set_value(name.as_ref(), &VALUE)
                .map_err(|e| format!("写入显卡偏好失败（{name}）：{e}"))?;
        }
        Ok(())
    }
    #[cfg(not(windows))]
    {
        let _ = exes;
        Ok(())
    }
}

/// 设置游戏进程优先级。`pid` 是刚拉起来的子进程。
pub fn set_process_priority(pid: u32, priority: ProcessPriority) -> Result<(), String> {
    #[cfg(windows)]
    {
        use windows_sys::Win32::Foundation::CloseHandle;
        use windows_sys::Win32::System::Threading::{
            ABOVE_NORMAL_PRIORITY_CLASS, BELOW_NORMAL_PRIORITY_CLASS, NORMAL_PRIORITY_CLASS,
            OpenProcess, PROCESS_SET_INFORMATION, SetPriorityClass,
        };

        // 「高」用 AboveNormal 而不是 High：别去跟系统输入抢
        let class = match priority {
            ProcessPriority::Low => BELOW_NORMAL_PRIORITY_CLASS,
            ProcessPriority::Normal => NORMAL_PRIORITY_CLASS,
            ProcessPriority::High => ABOVE_NORMAL_PRIORITY_CLASS,
        };

        // SAFETY: 只用于拿句柄设优先级；失败返回空句柄 / 0，均已判掉，
        // 句柄无论成功失败都会关闭。
        unsafe {
            let handle = OpenProcess(PROCESS_SET_INFORMATION, 0, pid);
            if handle.is_null() {
                return Err(format!("打开进程 {pid} 失败"));
            }
            let ok = SetPriorityClass(handle, class);
            CloseHandle(handle);
            if ok == 0 {
                return Err(format!("设置进程 {pid} 优先级失败"));
            }
        }
        Ok(())
    }
    #[cfg(not(windows))]
    {
        let _ = (pid, priority);
        Ok(())
    }
}
