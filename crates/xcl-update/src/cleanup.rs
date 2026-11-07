//! 启动时清理更新残留。
//!
//! 一次成功的更新会在安装目录里留下几样东西，都要在之后某次启动时扫掉：
//!
//! - `.XCL-new-<ver>.exe`：下载好但已经被换走的更新包（正常情况下换完就没了，
//!   这里兜的是「替换前就放弃」的残留）；
//! - `.XCL-helper-<pid>.exe`：跑完退出的 helper（正在跑的那个删不掉，跳过）；
//! - `XCL.exe.old`：上一版备份。
//!
//! 备份多留一代：刚更新完的那次启动不删，等下一次再删。这样万一新版
//! 用起来有问题，手上还有可以改名回去的文件。
//!
//! 删不掉一律跳过，不报错 —— 清理失败不该影响启动。

use std::path::Path;

// 临时文件的形状是 `.<程序名>-new-<版本>.exe` / `.<程序名>-helper-<pid>.exe`。
// 不匹配完整前缀：程序名随时会变，用「点开头 + 含标记段 + .exe」判断更稳。
const NEW_MARK: &str = "-new-";
const HELPER_MARK: &str = "-helper-";
const BACKUP_SUFFIX: &str = ".old";

/// 清理结果，调用方拿它写日志。
#[derive(Debug, Default, PartialEq, Eq)]
pub struct CleanupReport {
    pub removed: Vec<String>,
    /// 这次有没有保留 `.old`
    pub kept_backup: bool,
}

/// 扫描并删除。
///
/// - `keep`：必须留着的文件（已经下好、等着下次启动应用的那个包）。
///   按文件名无脑删是错的 —— 那会把刚下好的更新删掉。
/// - `just_updated`：这次启动是不是「刚更新完」的那次。为真时保留 `.old` 一代。
pub fn run(dir: &Path, keep: Option<&Path>, just_updated: bool) -> CleanupReport {
    let mut report = CleanupReport {
        kept_backup: just_updated,
        ..CleanupReport::default()
    };
    let Ok(entries) = std::fs::read_dir(dir) else {
        return report;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if keep.is_some_and(|keep| keep == path) {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        let lower = name.to_ascii_lowercase();
        // 只动「隐藏的 exe 临时文件」，绝不碰别的东西
        let is_update_artifact = lower.starts_with('.')
            && (lower.ends_with(".exe") || lower.ends_with(".zip"))
            && (lower.contains(NEW_MARK) || lower.contains(HELPER_MARK));
        let is_backup = lower.ends_with(BACKUP_SUFFIX) && !just_updated;
        if !is_update_artifact && !is_backup {
            continue;
        }
        // 正在被占用的文件（比如另一个 helper）删不掉，跳过就行
        if std::fs::remove_file(&path).is_ok() {
            report.removed.push(name);
        }
    }
    report
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::temp_dir;

    fn touch(dir: &Path, name: &str) -> std::path::PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, b"x").unwrap();
        path
    }

    #[test]
    fn removes_new_and_helper_leftovers() {
        let dir = temp_dir("cleanup-new");
        touch(&dir, "Xin Craft Launcher.exe");
        touch(&dir, ".Xin Craft Launcher-new-0.2.0.exe");
        touch(&dir, ".Xin Craft Launcher-helper-123.exe");
        touch(&dir, "data-file.txt");

        let report = run(&dir, None, false);
        assert_eq!(report.removed.len(), 2);
        assert!(dir.join("Xin Craft Launcher.exe").exists());
        assert!(dir.join("data-file.txt").exists());
        assert!(!dir.join(".Xin Craft Launcher-new-0.2.0.exe").exists());
        assert!(!dir.join(".Xin Craft Launcher-helper-123.exe").exists());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn keeps_the_package_waiting_to_be_applied() {
        // 下好还没装的包必须留着 —— 按文件名无脑删会把它一起删掉，
        // 下次启动就没得装了
        let dir = temp_dir("cleanup-keep");
        touch(&dir, "Xin Craft Launcher.exe");
        let waiting = touch(&dir, ".Xin Craft Launcher-new-0.2.2.exe");
        touch(&dir, ".Xin Craft Launcher-helper-1.exe");

        let report = run(&dir, Some(&waiting), false);
        assert!(waiting.exists());
        assert!(!dir.join(".Xin Craft Launcher-helper-1.exe").exists());
        assert_eq!(report.removed.len(), 1);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn does_not_touch_ordinary_files() {
        // 只认「点开头 + 含 -new-/-helper- + .exe」，别的 exe 一律不动
        let dir = temp_dir("cleanup-safe");
        touch(&dir, "Xin Craft Launcher.exe");
        touch(&dir, "mods-new-helper.exe");
        touch(&dir, ".config.json");

        let report = run(&dir, None, false);
        assert!(report.removed.is_empty());
        assert!(dir.join("mods-new-helper.exe").exists());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn backup_is_kept_for_one_generation() {
        let dir = temp_dir("cleanup-backup");
        touch(&dir, "Xin Craft Launcher.exe");
        touch(&dir, "Xin Craft Launcher.exe.old");

        // 刚更新完：留着，便于回滚
        let report = run(&dir, None, true);
        assert!(report.kept_backup);
        assert!(dir.join("Xin Craft Launcher.exe.old").exists());

        // 下一次启动：可以删了
        let report = run(&dir, None, false);
        assert!(!report.kept_backup);
        assert!(report.removed.contains(&"Xin Craft Launcher.exe.old".to_string()));
        assert!(!dir.join("Xin Craft Launcher.exe.old").exists());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn missing_directory_is_not_an_error() {
        let report = run(Path::new("Z:\\definitely-not-here"), None, false);
        assert!(report.removed.is_empty());
    }
}
