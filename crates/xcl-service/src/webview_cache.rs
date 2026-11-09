//! 界面内核（WebView2）的浏览器缓存。
//!
//! 界面是 Tauri 内嵌的 WebView2 画的，它自己的浏览缓存（`Cache` / `Code Cache` /
//! `GPUCache` 等）默认落在 `%LOCALAPPDATA%\<identifier>\EBWebView` —— 不在我们的
//! `cache_root` 里。所以过去它既统计不到也清不掉，实测能涨到 600 MB 以上，
//! 而安装包本身才 25 MB。
//!
//! 唯一的难点：界面运行时这些文件正被 WebView2 用着。
//! 哪怕 Windows 允许删（有些句柄带 FILE_SHARE_DELETE），内核随后再去读缓存 /
//! V8 代码缓存也可能直接把渲染进程带崩 —— 实测 dev 模式下会崩，而 debug 和 release
//! 用的是同一个 WebView2 Runtime、同一套文件，只是 dev 缓存命中更密、更容易撞上，
//! 所以不能赌正式版安全。
//!
//! 策略因此是「一个字节都不在运行时删」：点清理只写个标记，然后由前端自动重启应用；
//! `purge_on_boot()` 在下一次进程刚起来、WebView2 还没创建时把缓存真正删掉。
//! 万一重启没成功（强杀、崩溃），标记还在，再下次启动照样清 —— 不会漏。
//!
//! 只清「纯缓存」子目录。`Extensions` / `Local Storage` 留着：
//! 删了会丢主题偏好、"提示已读"之类的界面状态，省那 28 MB 不划算。

use std::fs;
use std::path::{Path, PathBuf};

use crate::storage::dir_size;

/// 与 `tauri.conf.json` 的 `identifier` 一致。改包名时要一起改，
/// 否则这里会指向一个不存在的目录 —— 表现是「浏览器缓存」一直显示 0。
const WEBVIEW_DIR: &str = "com.xin.xcl";
const WEBVIEW_SUBDIR: &str = "EBWebView";
/// 「还有东西没删掉」的标记文件，放在 WebView 目录旁边。
const PENDING_MARKER: &str = ".cache-clean-pending";

/// 允许清理的纯缓存子目录（相对 `EBWebView`）。
///
/// 这些删掉只是下次重新生成，不会丢任何本地数据。Chromium 的缓存目录名比较固定，
/// 多写几个是防不同内核版本换路径，不存在就跳过。
const CACHE_SUBDIRS: &[&str] = &[
    "Default/Cache",
    "Default/Code Cache",
    "Default/GPUCache",
    "Default/DawnGraphiteCache",
    "Default/DawnWebGPUCache",
    "GPUCache",
    "GrShaderCache",
    "ShaderCache",
    "Subresource Filter",
];

/// 一次清理的结果。
///
/// `pending` 为真表示这次只登记了清理请求，需要重启应用才能真正删。
#[derive(Debug, Clone, Copy, Default)]
pub struct CleanOutcome {
    pub freed: u64,
    pub pending: bool,
}

fn app_data_root() -> Option<PathBuf> {
    let base = std::env::var_os("LOCALAPPDATA")?;
    let path = PathBuf::from(base).join(WEBVIEW_DIR);
    path.is_dir().then_some(path)
}

/// `%LOCALAPPDATA%\<identifier>\EBWebView`。
pub fn webview_root() -> Option<PathBuf> {
    let root = app_data_root()?.join(WEBVIEW_SUBDIR);
    root.is_dir().then_some(root)
}

fn marker_path() -> Option<PathBuf> {
    Some(app_data_root()?.join(PENDING_MARKER))
}

fn cache_paths(root: &Path) -> impl Iterator<Item = PathBuf> + '_ {
    CACHE_SUBDIRS.iter().map(move |sub| root.join(sub))
}

/// 浏览器缓存当前占多少字节。
pub fn size() -> u64 {
    let Some(root) = webview_root() else {
        return 0;
    };
    cache_paths(&root).map(|path| dir_size(&path)).sum()
}

/// 递归清空目录内容，但保留目录本身（内核会自己重建）。
///
/// 返回「删掉了多少字节」和「是否有文件因为被占用而没删掉」。
/// 单个文件失败不算错误 —— 运行中必然有文件被锁，这里的目标是尽力而为。
fn remove_contents(dir: &Path) -> (u64, bool) {
    let Ok(entries) = fs::read_dir(dir) else {
        return (0, false);
    };
    let mut freed = 0;
    let mut blocked = false;
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(meta) = fs::symlink_metadata(&path) else {
            continue;
        };
        if meta.is_dir() {
            let (sub_freed, sub_blocked) = remove_contents(&path);
            freed += sub_freed;
            blocked |= sub_blocked;
            // 清空了才删目录；里面还有东西就留着，下次再试
            if !sub_blocked {
                let _ = fs::remove_dir(&path);
            }
        } else {
            let len = meta.len();
            match fs::remove_file(&path) {
                Ok(()) => freed += len,
                Err(_) => blocked = true,
            }
        }
    }
    (freed, blocked)
}

/// 登记一次清理请求：不删任何文件，只写标记。
///
/// 真正动手在 `purge_on_boot()`。运行时删 WebView2 正在用的缓存会把界面搞崩，
/// 详见模块头注释。调用方拿到 `pending = true` 后应当重启应用。
pub fn schedule_clean() -> CleanOutcome {
    if webview_root().is_none() {
        return CleanOutcome::default();
    }
    let Some(marker) = marker_path() else {
        return CleanOutcome::default();
    };

    // 没有缓存可清就别写标记，省得前端白重启一次
    if size() == 0 {
        let _ = fs::remove_file(&marker);
        return CleanOutcome::default();
    }

    let _ = fs::write(&marker, b"1");
    CleanOutcome {
        freed: 0,
        pending: true,
    }
}

/// 启动时清掉上次登记的清理请求。必须在创建窗口之前调用（`main.rs` 里）。
///
/// 如果个别文件仍被上一轮残留的进程占着（重启太快），就保留标记，
/// 下次启动再试 —— 不能清完无条件删标记，否则那部分永远清不掉了。
pub fn purge_on_boot() -> CleanOutcome {
    let empty = CleanOutcome::default();
    let Some(marker) = marker_path() else {
        return empty;
    };
    if !marker.is_file() {
        return empty;
    }
    let Some(root) = webview_root() else {
        // 目录都没了，标记也没意义
        let _ = fs::remove_file(&marker);
        return empty;
    };

    let mut total_freed = 0;
    let mut still_blocked = true;
    // `AppHandle::restart()` 是「spawn 新进程 → 立刻 exit(0)」，旧进程的 WebView2
    // 子进程可能还没退干净、缓存仍被占着。给三轮机会，别白重启一次。
    // 只有标记存在时才走到这里，正常启动零开销。
    for attempt in 0..3 {
        let mut blocked = false;
        for dir in cache_paths(&root) {
            if dir.is_dir() {
                let (freed, failed) = remove_contents(&dir);
                total_freed += freed;
                blocked |= failed;
            }
        }
        still_blocked = blocked;
        if !blocked || attempt == 2 {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(250));
    }

    let outcome = CleanOutcome {
        freed: total_freed,
        pending: still_blocked,
    };
    if !outcome.pending {
        let _ = fs::remove_file(&marker);
    }
    outcome
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_subdirs_are_relative_and_unique() {
        let mut seen = std::collections::HashSet::new();
        for sub in CACHE_SUBDIRS {
            assert!(!sub.is_empty());
            assert!(!sub.starts_with('/') && !sub.contains(".."));
            assert!(seen.insert(*sub), "重复项 {sub}");
        }
    }

    #[test]
    fn remove_contents_keeps_dir_and_reports_bytes() {
        let base = std::env::temp_dir().join("xcl-webview-cache-test");
        let _ = fs::remove_dir_all(&base);
        let nested = base.join("nested");
        fs::create_dir_all(&nested).unwrap();
        fs::write(base.join("a.bin"), vec![0u8; 1024]).unwrap();
        fs::write(nested.join("b.bin"), vec![0u8; 2048]).unwrap();

        let (freed, blocked) = remove_contents(&base);
        assert_eq!(freed, 3072);
        assert!(!blocked);
        assert!(base.is_dir(), "目录本身要留着");
        assert_eq!(fs::read_dir(&base).unwrap().count(), 0);

        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn clean_outcome_defaults_to_nothing() {
        let outcome = CleanOutcome::default();
        assert_eq!(outcome.freed, 0);
        assert!(!outcome.pending);
    }
}
