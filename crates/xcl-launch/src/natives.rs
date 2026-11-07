//! natives 解压。
//!
//! 老版本把 natives 当独立 jar 下发，现代版本也一样（只是坐标不同），
//! 都要在启动前解到本地目录，再通过 `-Djava.library.path` 告诉 JVM。
//!
//! 语义见下，有两点比较保守：
//! - 只平铺动态库文件（`.dll` / `.so` / `.dylib`），jar 里的 `META-INF/` 一律不落地
//! - 解压完清掉目录里多余的旧文件 —— 换版本后残留的旧 dll 会被 JVM 优先加载，
//!   表现是莫名其妙的 `UnsatisfiedLinkError`，极难排查

use std::collections::HashSet;
use std::io::Write;
use std::path::{Path, PathBuf};

use tracing::{debug, warn};
use xcl_core::AppError;

/// 一个待解压的 natives 包。
#[derive(Debug, Clone)]
pub struct NativeArchive {
    pub jar: PathBuf,
    /// 命中这些前缀的条目不落地，通常是 `META-INF/`
    pub excludes: Vec<String>,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ExtractReport {
    /// 真正写出去的
    pub extracted: usize,
    /// 已存在且内容大小一致，跳过的
    pub skipped: usize,
    /// 清掉的旧残留
    pub removed: usize,
}

impl ExtractReport {
    pub fn is_empty(&self) -> bool {
        self.extracted == 0 && self.skipped == 0
    }
}

/// 解压到 `target`（先清旧内容）。
pub fn extract(archives: &[NativeArchive], target: &Path) -> Result<ExtractReport, AppError> {
    std::fs::create_dir_all(target).map_err(|e| AppError::io(&e, "创建 natives 目录"))?;

    let mut report = ExtractReport::default();
    // 本次应当存在的文件名单，用来判断哪些是旧残留
    let mut expected: HashSet<String> = HashSet::new();

    for archive in archives {
        let file = match std::fs::File::open(&archive.jar) {
            Ok(file) => file,
            Err(err) => {
                // 单个包坏掉不该拖垮整个启动，但要留下痕迹
                warn!(path = %archive.jar.display(), error = %err, "natives 包打不开，已跳过");
                continue;
            }
        };

        let mut zip = match zip::ZipArchive::new(file) {
            Ok(zip) => zip,
            Err(err) => {
                warn!(path = %archive.jar.display(), error = %err, "natives 包不是有效 zip，已跳过");
                continue;
            }
        };

        for index in 0..zip.len() {
            let mut entry = match zip.by_index(index) {
                Ok(entry) => entry,
                Err(err) => {
                    debug!(index, error = %err, "跳过损坏的 zip 条目");
                    continue;
                }
            };

            if entry.is_dir() {
                continue;
            }
            // enclosed_name 会拒绝 `../` 这类穿越路径
            let Some(path) = entry.enclosed_name() else {
                continue;
            };
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };

            if !is_native_library(name) {
                continue;
            }
            if archive.excludes.iter().any(|ex| {
                path.to_string_lossy().starts_with(ex.as_str())
                    || entry.name().starts_with(ex.as_str())
            }) {
                continue;
            }

            let dest = target.join(name);
            expected.insert(name.to_string());

            // 大小一致就认为已经是同一份，省掉 IO
            if let Ok(meta) = std::fs::metadata(&dest) {
                if meta.len() == entry.size() {
                    report.skipped += 1;
                    continue;
                }
            }

            match write_entry(&mut entry, &dest) {
                Ok(()) => report.extracted += 1,
                Err(err) => {
                    // 文件被占用（游戏还开着）时这里会失败，跳过即可，下次启动会重来
                    warn!(path = %dest.display(), error = %err, "写出 natives 失败，已跳过");
                }
            }
        }
    }

    report.removed = remove_stale(target, &expected);
    Ok(report)
}

fn write_entry<R: std::io::Read>(
    entry: &mut R,
    dest: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut out = std::fs::File::create(dest)?;
    std::io::copy(entry, &mut out)?;
    out.flush()?;
    Ok(())
}

/// 删掉目录里不在本次清单中的文件（上一版留下的旧 dll）。
fn remove_stale(target: &Path, expected: &HashSet<String>) -> usize {
    let Ok(entries) = std::fs::read_dir(target) else {
        return 0;
    };
    let mut removed = 0;
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if expected.contains(name) {
            continue;
        }
        if std::fs::remove_file(&path).is_ok() {
            debug!(path = %path.display(), "清理上一版残留的 natives");
            removed += 1;
        }
    }
    removed
}

/// 是不是动态库。jar 里的 `META-INF`、签名、`.git` 之类一律不要。
fn is_native_library(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.ends_with(".dll") || lower.ends_with(".so") || lower.ends_with(".dylib")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognises_native_libraries() {
        assert!(is_native_library("lwjgl.dll"));
        assert!(is_native_library("liblwjgl.so"));
        assert!(is_native_library("liblwjgl.dylib"));
        assert!(is_native_library("OpenAL64.DLL"), "后缀判断要不区分大小写");
        assert!(!is_native_library("MANIFEST.MF"));
        assert!(!is_native_library("lwjgl.jar"));
    }

    #[test]
    fn report_empty_when_nothing_done() {
        assert!(ExtractReport::default().is_empty());
        assert!(!ExtractReport {
            extracted: 1,
            ..Default::default()
        }
        .is_empty());
    }
}
