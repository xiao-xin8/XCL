//! 数据根目录与缓存目录探测。
//!
//! 数据目录优先级（这是产品的可移植性承诺）：
//! 1. 环境变量 `XCL_HOME`（调试/多环境用）
//! 2. 便携模式：exe 同级的 `data/`，可写就用它 —— 放 U 盘能整个带走
//! 3. 回落 `%APPDATA%\XCL`（程序装在 Program Files 等只读位置时）
//!
//! 缓存目录不进便携包：缓存是可再生的，不是数据。
//! 固定走 `%LOCALAPPDATA%\XCL\cache`，拿不到就退到系统临时目录。
//!
//! 注意：只做到「游戏数据便携」是不够的，配置也要跟着走；XCL 两边都便携。

use std::fs;
use std::path::{Path, PathBuf};

use tracing::{debug, warn};
use xcl_core::{AppError, PathLayout};

const APP_DIR_NAME: &str = "XCL";
const PORTABLE_DIR_NAME: &str = "data";
const ENV_HOME: &str = "XCL_HOME";
const PROBE_FILE: &str = ".xcl-write-probe";

pub fn detect_layout() -> PathLayout {
    PathLayout::new(detect_root(), detect_cache_root())
}

/// 缓存目录：始终在本机缓存位置，不跟随便携目录。
pub fn detect_cache_root() -> PathBuf {
    if let Ok(local) = std::env::var("LOCALAPPDATA") {
        if !local.trim().is_empty() {
            return PathBuf::from(local).join(APP_DIR_NAME).join("cache");
        }
    }
    std::env::temp_dir().join(APP_DIR_NAME).join("cache")
}

pub fn detect_root() -> PathBuf {
    if let Some(dir) = env_override() {
        debug!(path = %dir.display(), "使用 XCL_HOME 指定的数据目录");
        return dir;
    }

    if let Some(dir) = portable_candidate() {
        debug!(path = %dir.display(), "便携模式：数据目录位于程序同级");
        return dir;
    }

    let dir = appdata_dir();
    warn!(path = %dir.display(), "程序目录不可写，回落到用户目录");
    dir
}

fn env_override() -> Option<PathBuf> {
    let raw = std::env::var(ENV_HOME).ok()?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(PathBuf::from(trimmed))
    }
}

/// exe 同级 `data/`。只有在实际写入成功时才认为可用。
fn portable_candidate() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let exe_dir = exe.parent()?;
    let candidate = exe_dir.join(PORTABLE_DIR_NAME);
    if is_writable(&candidate) {
        Some(candidate)
    } else {
        None
    }
}

fn appdata_dir() -> PathBuf {
    if let Ok(appdata) = std::env::var("APPDATA") {
        if !appdata.trim().is_empty() {
            return PathBuf::from(appdata).join(APP_DIR_NAME);
        }
    }
    if let Ok(home) = std::env::var("USERPROFILE") {
        if !home.trim().is_empty() {
            return PathBuf::from(home).join(APP_DIR_NAME);
        }
    }
    // 最后兜底：当前目录（几乎不会走到）
    PathBuf::from(APP_DIR_NAME)
}

/// 通过真实写一个探针文件判断目录可写，比检查只读属性可靠。
fn is_writable(dir: &Path) -> bool {
    if fs::create_dir_all(dir).is_err() {
        return false;
    }
    let probe = dir.join(PROBE_FILE);
    let ok = fs::write(&probe, b"1").is_ok();
    if ok {
        let _ = fs::remove_file(&probe);
    }
    ok
}

/// 确保目录存在，失败时转成可读错误。
pub fn ensure_dir(dir: &Path) -> Result<(), AppError> {
    fs::create_dir_all(dir).map_err(|e| AppError::io(&e, "创建数据目录"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn appdata_path_contains_app_name() {
        let dir = appdata_dir();
        assert!(dir.to_string_lossy().contains(APP_DIR_NAME));
    }

    #[test]
    fn writable_dir_is_detected() {
        let dir = std::env::temp_dir().join(format!("xcl-writable-{}", std::process::id()));
        assert!(is_writable(&dir));
        assert!(dir.exists());
        let _ = fs::remove_dir_all(&dir);
    }
}
