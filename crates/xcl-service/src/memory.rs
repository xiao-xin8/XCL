//! 系统内存查询、自动内存、以及「清理内存」。
//!
//! 自动分配只取总物理内存，不取可用内存：那个值要在设置页稳定显示，
//! 不能因为后台多开了一个程序就变。算法本身在 `xcl_core::config::auto_memory_mb`。
//!
//! 清理内存分两级，由权限决定用哪一级，不打扰使用：
//! - 深度：管理员 token 下，进程内启用 `SeProfileSingleProcessPrivilege` /
//!   `SeIncreaseQuotaPrivilege` 之后调 `NtSetSystemInformation`，清全系统工作集、
//!   把脏页落盘、清备用列表。不弹任何 UAC —— 前提是进程本身有管理员 token。
//! - 标准：没有管理员 token 时，逐进程 `EmptyWorkingSet`。任何用户都能做，
//!   但清不到系统级的那几块。
//!
//! 为什么必须显式启用特权：即使是管理员，`NtSetSystemInformation` 默认也返回
//! `0xC0000061`（STATUS_PRIVILEGE_NOT_HELD）—— 实测过，不是猜的。

use std::sync::OnceLock;

use xcl_core::config::{auto_memory_mb as calculate, MemoryMode};
use xcl_core::AppError;

/// 总内存很少变化，进程内只读一次。
static TOTAL_MB: OnceLock<Option<u64>> = OnceLock::new();

fn detect_total_mb() -> Option<u64> {
    let mut system = sysinfo::System::new();
    system.refresh_memory();
    let bytes = system.total_memory();
    (bytes > 0).then(|| bytes / 1024 / 1024)
}

/// 总物理内存（MB）。读不到返回 `None`。
pub fn total_memory_mb() -> Option<u64> {
    *TOTAL_MB.get_or_init(detect_total_mb)
}

/// 当前可用物理内存（MB）。含备用列表，反映「系统现在还能给出去多少」。
pub fn available_mb() -> u64 {
    let mut system = sysinfo::System::new();
    system.refresh_memory();
    system.available_memory() / 1024 / 1024
}

/// 设置页「内存分配」那块要显示的一组实时数据。
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryStats {
    pub total_mb: u64,
    /// 已被占用（总量 − 可用）。可用里含备用列表，所以这个值反映的是「真占着」的部分。
    pub used_mb: u64,
    pub available_mb: u64,
    /// 当前设置下游戏会拿到的内存。
    pub game_mb: u32,
    /// 自动模式下是按多少个 mod 算的
    pub mod_count: u32,
    /// `true` = 自动分配
    pub auto: bool,
    /// 自动模式参照的版本；`None` 表示没选版本，按纯原版算
    pub version_id: Option<String>,
}

/// 数 `mods/` 下的 mod 文件。和后缀判定保持一致（只认这三种）。
fn count_mods(game_dir: &std::path::Path) -> usize {
    let Ok(entries) = std::fs::read_dir(game_dir.join("mods")) else {
        return 0;
    };
    entries
        .flatten()
        .filter(|entry| {
            let path = entry.path();
            path.is_file()
                && path
                    .extension()
                    .and_then(|ext| ext.to_str())
                    .is_some_and(|ext| {
                        matches!(
                            ext.to_ascii_lowercase().as_str(),
                            "jar" | "zip" | "litemod"
                        )
                    })
        })
        .count()
}

/// 清理强度。由当前进程有没有管理员 token 决定，不靠选。
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub enum TrimMode {
    /// 系统级：全系统工作集 + 脏页落盘 + 备用列表。
    Deep,
    /// 逐进程清工作集，任何用户都能做。
    Standard,
}

impl TrimMode {
    pub fn label(self) -> &'static str {
        match self {
            Self::Deep => "深度清理",
            Self::Standard => "标准清理",
        }
    }
}

/// 一次清理的结果。
#[derive(Debug, Clone, Copy, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrimReport {
    pub mode: TrimMode,
    /// 深度模式是「成功执行的系统清理项数」，标准模式是「清到的进程数」。
    pub count: u32,
    /// 清理前后可用物理内存的差值（MB）。
    pub freed_mb: i64,
}

/// 清一次内存。先试深度，做不到就自动降级，调用方不需要关心权限。
pub fn trim_memory() -> TrimReport {
    let before = available_mb();

    #[cfg(windows)]
    {
        if let Some(count) = windows_impl::deep_clean() {
            return TrimReport {
                mode: TrimMode::Deep,
                count,
                freed_mb: available_mb() as i64 - before as i64,
            };
        }
        let count = windows_impl::standard_clean();
        TrimReport {
            mode: TrimMode::Standard,
            count,
            freed_mb: available_mb() as i64 - before as i64,
        }
    }

    #[cfg(not(windows))]
    {
        let _ = before;
        TrimReport {
            mode: TrimMode::Standard,
            count: 0,
            freed_mb: 0,
        }
    }
}

#[cfg(windows)]
mod windows_impl {
    use std::ffi::c_void;
    use std::ptr::null_mut;

    use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, HANDLE, LUID};
    use windows_sys::Win32::Security::{
        AdjustTokenPrivileges, LookupPrivilegeValueW, LUID_AND_ATTRIBUTES, SE_PRIVILEGE_ENABLED,
        TOKEN_ADJUST_PRIVILEGES, TOKEN_PRIVILEGES, TOKEN_QUERY,
    };
    use windows_sys::Win32::System::ProcessStatus::{EmptyWorkingSet, EnumProcesses};
    use windows_sys::Win32::System::Threading::{
        GetCurrentProcess, OpenProcess, OpenProcessToken, PROCESS_QUERY_LIMITED_INFORMATION,
        PROCESS_SET_QUOTA,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetForegroundWindow, GetWindowThreadProcessId,
    };

    #[link(name = "ntdll")]
    unsafe extern "system" {
        fn NtSetSystemInformation(class: i32, info: *mut c_void, len: u32) -> i32;
    }

    /// `SYSTEM_INFORMATION_CLASS::SystemMemoryListInformation`
    const SYSTEM_MEMORY_LIST_INFORMATION: i32 = 80;
    /// `SYSTEM_MEMORY_LIST_COMMAND`
    const MEMORY_EMPTY_WORKING_SETS: i32 = 2;
    const MEMORY_FLUSH_MODIFIED_LIST: i32 = 3;
    const MEMORY_PURGE_STANDBY_LIST: i32 = 4;

    /// 启用一个特权。返回是否真的启用成功。
    ///
    /// 坑：`AdjustTokenPrivileges` 即使一个特权都没启用也会返回非零，
    /// 必须再查 `GetLastError()` 是不是 `ERROR_SUCCESS`(0)，否则会把
    /// 「没启用」误判成「启用成功」，后面调用再失败一次。
    fn enable_privilege(name: &str) -> bool {
        let mut token: HANDLE = null_mut();
        let opened = unsafe {
            OpenProcessToken(
                GetCurrentProcess(),
                TOKEN_ADJUST_PRIVILEGES | TOKEN_QUERY,
                &mut token,
            )
        };
        if opened == 0 {
            return false;
        }

        let wide: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
        let mut luid = LUID::default();
        if unsafe { LookupPrivilegeValueW(null_mut(), wide.as_ptr(), &mut luid) } == 0 {
            unsafe { CloseHandle(token) };
            return false;
        }

        let mut state = TOKEN_PRIVILEGES {
            PrivilegeCount: 1,
            Privileges: [LUID_AND_ATTRIBUTES {
                Luid: luid,
                Attributes: SE_PRIVILEGE_ENABLED,
            }],
        };
        let ok = unsafe { AdjustTokenPrivileges(token, 0, &mut state, 0, null_mut(), null_mut()) };
        let err = unsafe { GetLastError() };
        unsafe { CloseHandle(token) };
        ok != 0 && err == 0
    }

    /// 系统级清理。返回成功执行的项数；一项都做不了就返回 `None`（调用方降级）。
    ///
    /// 顺序有讲究：先把工作集换出（页进备用列表），再把脏页落盘成可回收页，
    /// 最后清备用列表。反过来做等于白清 —— 前面的动作又会造出新的备用页。
    pub(super) fn deep_clean() -> Option<u32> {
        let profile = enable_privilege("SeProfileSingleProcessPrivilege");
        let quota = enable_privilege("SeIncreaseQuotaPrivilege");
        if !profile && !quota {
            return None;
        }

        let mut done = 0u32;
        for command in [
            MEMORY_EMPTY_WORKING_SETS,
            MEMORY_FLUSH_MODIFIED_LIST,
            MEMORY_PURGE_STANDBY_LIST,
        ] {
            let mut value = command;
            let status = unsafe {
                NtSetSystemInformation(
                    SYSTEM_MEMORY_LIST_INFORMATION,
                    &mut value as *mut i32 as *mut c_void,
                    std::mem::size_of::<i32>() as u32,
                )
            };
            // NTSTATUS：非负即成功
            if status >= 0 {
                done += 1;
            }
        }

        (done > 0).then_some(done)
    }

    /// 逐进程清工作集。不需要任何权限，打不开的进程静默跳过。
    ///
    /// 跳过 `System`(4)、自己、以及当前前台窗口的进程 —— 正在用的
    /// 东西不该被换出，否则切回来会愣一下。
    pub(super) fn standard_clean() -> u32 {
        let mut pids: Vec<u32> = vec![0; 1024];
        let count = loop {
            let mut needed = 0u32;
            let ok = unsafe {
                EnumProcesses(
                    pids.as_mut_ptr(),
                    (pids.len() * std::mem::size_of::<u32>()) as u32,
                    &mut needed,
                )
            };
            if ok == 0 {
                return 0;
            }
            let count = (needed as usize) / std::mem::size_of::<u32>();
            if count < pids.len() {
                break count;
            }
            pids.resize(pids.len() * 2, 0);
        };

        let mut foreground = 0u32;
        unsafe {
            let hwnd = GetForegroundWindow();
            if !hwnd.is_null() {
                GetWindowThreadProcessId(hwnd, &mut foreground);
            }
        }

        let me = std::process::id();
        let mut trimmed = 0u32;

        for &pid in &pids[..count] {
            if pid == 0 || pid == 4 || pid == me || pid == foreground {
                continue;
            }
            let handle = unsafe {
                OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SET_QUOTA, 0, pid)
            };
            if handle.is_null() {
                continue;
            }
            let ok = unsafe { EmptyWorkingSet(handle) };
            unsafe { CloseHandle(handle) };
            if ok != 0 {
                trimmed += 1;
            }
        }

        trimmed
    }
}

impl crate::AppState {
    /// 自动内存：可用内存 + 这个实例的 mod 数（阶梯分配）。
    pub fn auto_memory_for(&self, version_id: &str) -> u32 {
        let (mod_count, modable) = self.instance_mod_info(version_id);
        calculate(available_mb(), mod_count, modable)
    }

    /// `(mods/ 里的 mod 数, 这个版本能不能装 mod)`。
    ///
    /// 能不能装 mod 沿用现成的 `VersionKind`，不另外猜；游戏目录按隔离策略解析 ——
    /// 开了隔离时 mods 在 `versions/<id>/mods/`，没开时在数据根目录。
    fn instance_mod_info(&self, version_id: &str) -> (usize, bool) {
        let config = self.config();

        let modable = crate::local::load(&self.layout, version_id)
            .map(|detail| {
                xcl_core::detect_kind(
                    version_id,
                    detail.version_type.as_deref(),
                    detail.main_class.as_deref(),
                    &detail.libraries,
                )
                .is_modable()
            })
            .unwrap_or(false);

        let game_dir =
            crate::isolation::resolution_for(&self.layout, &config, version_id)
                .map(|resolution| resolution.game_dir)
                .unwrap_or_else(|_| self.layout.version_dir(version_id));

        (count_mods(&game_dir), modable)
    }

    /// 设置页「内存分配」要的实时数据。
    ///
    /// `version_id`：版本设置页传自己那个版本；全局设置页不传，用「上次选中的版本」。
    /// 自动内存按这个版本的 mod 数算，所以两个页面看到的值是一致的。
    pub fn memory_stats(&self, version_id: Option<&str>) -> MemoryStats {
        let config = self.config();
        let total_mb = total_memory_mb().unwrap_or(0);
        let available = available_mb();
        let auto = config.launch.memory_mode == MemoryMode::Auto;

        let version_id = version_id
            .map(str::to_string)
            .or_else(|| Some(config.ui.selected_version.clone()).filter(|id| !id.is_empty()))
            // 从没在启动页选过版本时（新装 / 还没点过），退到列表第一个 ——
            // 不能按「纯原版」算，否则装了 200 个 mod 的包会显示一个明显偏小的值
            .or_else(|| {
                crate::local::scan(&self.layout)
                    .into_iter()
                    .next()
                    .map(|version| version.id)
            });

        let (game_mb, mod_count) = if auto {
            match &version_id {
                Some(id) => {
                    let (mods, _) = self.instance_mod_info(id);
                    (self.auto_memory_for(id), mods as u32)
                }
                None => (calculate(available, 0, false), 0),
            }
        } else {
            // 手动模式下 game_mb 用全局值；版本设置页的覆盖值由前端自己显示
            (config.launch.memory_mb.max(512), 0)
        };

        MemoryStats {
            total_mb,
            used_mb: total_mb.saturating_sub(available),
            available_mb: available,
            game_mb,
            mod_count,
            auto,
            version_id,
        }
    }

    /// 清一次内存（设置页手动触发，或启动前自动执行）。
    pub async fn trim_memory(&self) -> Result<TrimReport, AppError> {
        tokio::task::spawn_blocking(trim_memory)
            .await
            .map_err(|err| AppError::unsupported("内存清理失败", err.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_memory_is_in_range() {
        // 算法细节在 `xcl_core::config` 的单测里；这里只做一次烟测，
        // 确认接到真实的可用内存上不会算出界外值
        let value = calculate(available_mb().max(1024), 0, false);
        assert!((512..=16_384).contains(&value), "自动内存越界：{value}");
        assert_eq!(value % 256, 0, "自动内存应当按 256MB 取整：{value}");
    }

    #[test]
    fn trim_mode_has_a_label() {
        assert_eq!(TrimMode::Deep.label(), "深度清理");
        assert_eq!(TrimMode::Standard.label(), "标准清理");
    }
}
