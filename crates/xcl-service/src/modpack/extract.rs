//! 整合包 zip 里的路径处理与目录树解压。
//!
//! 包内的路径全部来自别人的文件，一律当不可信输入处理：
//! 只接受普通路径段，`..`、绝对路径、盘符、反斜杠一律拒绝。
//! 三个参考实现（conic `target_path`、shard `sanitize_rel_path`、
//! basalt `sanitize_relative`）在这一点上完全一致。

use std::fs;
use std::io::{self, Read};
use std::path::{Component, Path, PathBuf};

use xcl_core::AppError;

use super::PackManifest;

/// 包内相对路径 → 安全的相对 `PathBuf`。
///
/// 反斜杠单独拦一次：zip 规范用 `/`，出现 `\` 基本就是恶意包在绕 Windows
/// 的路径解析（`..\..\x` 在 `Path::components` 里会被当成分隔符，但不同平台
/// 行为不一致，不赌）。
pub(crate) fn safe_relative(path: &str) -> Result<PathBuf, AppError> {
    if path.contains('\\') {
        return Err(unsafe_path(path));
    }
    let mut out = PathBuf::new();
    let mut has_part = false;
    for component in Path::new(path).components() {
        match component {
            Component::Normal(part) => {
                out.push(part);
                has_part = true;
            }
            Component::CurDir => {}
            _ => return Err(unsafe_path(path)),
        }
    }
    if !has_part {
        return Err(unsafe_path(path));
    }
    Ok(out)
}

fn unsafe_path(path: &str) -> AppError {
    AppError::validation("整合包里的路径不安全", path.to_string())
        .with_hint("这个包可能被改过，建议换一个来源")
}

/// 把 `prefixes` 这几个目录下的文件解到 `dest`（前缀本身剥掉）。
///
/// 后出现的覆盖先出现的：`overrides/` 先、`client-overrides/` 后，
/// 和 Modrinth 规范一致。
pub(crate) fn extract_prefixes(
    archive: &mut zip::ZipArchive<fs::File>,
    prefixes: &[String],
    dest: &Path,
) -> Result<usize, AppError> {
    let mut written = 0;
    // 先按顺序把前缀排一遍，保证后面的覆盖前面的
    for prefix in prefixes {
        for index in 0..archive.len() {
            // ZipArchive 是 &mut 借出条目，所以每轮都按名字判一次
            let (name, is_dir) = {
                let entry = archive
                    .by_index(index)
                    .map_err(|err| AppError::parse("整合包文件损坏", err.to_string()))?;
                (entry.name().to_string(), entry.is_dir())
            };
            if is_dir {
                continue;
            }
            let Some(rest) = name.strip_prefix(prefix.as_str()) else {
                continue;
            };
            if rest.is_empty() {
                continue;
            }
            let relative = safe_relative(rest)?;
            let target = dest.join(relative);
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent).map_err(|err| AppError::io(&err, "创建目录"))?;
            }
            let mut entry = archive
                .by_index(index)
                .map_err(|err| AppError::parse("整合包文件损坏", err.to_string()))?;
            let mut out = fs::File::create(&target).map_err(|err| AppError::io(&err, "解压文件"))?;
            io::copy(&mut entry, &mut out).map_err(|err| AppError::io(&err, "解压文件"))?;
            written += 1;
        }
    }
    Ok(written)
}

/// 从 zip 里读一个条目的小文件（manifest 用），限制大小避免解压炸弹。
pub(crate) fn read_entry(
    archive: &mut zip::ZipArchive<fs::File>,
    index: usize,
    limit: u64,
) -> Result<String, AppError> {
    let entry = archive
        .by_index(index)
        .map_err(|err| AppError::parse("整合包文件损坏", err.to_string()))?;
    if entry.size() > limit {
        return Err(AppError::unsupported(
            "整合包信息异常",
            format!("{} 有 {} 字节，不像是一个正常的清单", entry.name(), entry.size()),
        ));
    }
    let mut bytes = Vec::new();
    entry
        .take(limit)
        .read_to_end(&mut bytes)
        .map_err(|err| AppError::io(&err, "读取整合包清单"))?;
    // 带 BOM 的 manifest 见过（CF 上 Windows 作者打包）
    let text = String::from_utf8_lossy(&bytes).to_string();
    Ok(text.trim_start_matches('\u{feff}').to_string())
}

/// 在 zip 里找一个条目：先根目录，再最浅的一层。
///
/// 返回 `(index, 前缀)`，前缀是条目所在的目录（含结尾 `/`），根目录为空串。
/// 同层出现两个 → 报错（拿了也分不清是哪个）。
pub(crate) fn find_entry(
    archive: &zip::ZipArchive<fs::File>,
    file_name: &str,
) -> Result<Option<(usize, String)>, AppError> {
    let mut candidates: Vec<(usize, usize, String)> = Vec::new();
    for index in 0..archive.len() {
        let Some(name) = archive.name_for_index(index) else {
            continue;
        };
        if !name.ends_with(file_name) {
            continue;
        }
        let depth = name.matches('/').count();
        if depth > 1 {
            continue;
        }
        if depth == 1 {
            // 只认「一级目录/文件名」
            let (dir, rest) = name.split_once('/').unwrap_or(("", name));
            if rest != file_name {
                continue;
            }
            candidates.push((depth, index, format!("{dir}/")));
        } else if name == file_name {
            candidates.push((depth, index, String::new()));
        }
    }
    candidates.sort_by_key(|(depth, _, _)| *depth);
    match candidates.len() {
        0 => Ok(None),
        1 => Ok(Some((candidates[0].1, candidates[0].2.clone()))),
        _ => {
            let same_depth = candidates.iter().filter(|(d, ..)| *d == candidates[0].0).count();
            if same_depth > 1 {
                return Err(AppError::unsupported(
                    "压缩包里有两个整合包",
                    format!("找到多个 {file_name}，无法判断该装哪个"),
                ));
            }
            Ok(Some((candidates[0].1, candidates[0].2.clone())))
        }
    }
}

/// 从 zip 里读图标（根 / overrides / client-overrides 下的 `icon.png`）。
pub(crate) fn read_icon(
    archive: &mut zip::ZipArchive<fs::File>,
    prefixes: &[String],
) -> Option<Vec<u8>> {
    const MAX_ICON: u64 = 2 * 1024 * 1024;
    let mut names = vec!["icon.png".to_string()];
    for prefix in prefixes {
        names.push(format!("{prefix}icon.png"));
    }
    for name in names {
        for index in 0..archive.len() {
            if archive.name_for_index(index) != Some(name.as_str()) {
                continue;
            }
            let mut entry = archive.by_index(index).ok()?;
            if entry.size() > MAX_ICON {
                return None;
            }
            let mut bytes = Vec::new();
            std::io::Read::take(&mut entry, MAX_ICON)
                .read_to_end(&mut bytes)
                .ok()?;
            if bytes.starts_with(&[0x89, b'P', b'N', b'G']) {
                return Some(bytes);
            }
            return None;
        }
    }
    None
}

impl PackManifest {
    /// 把已经解析好的清单里的 overrides 解到 `dest`。
    pub(crate) fn unpack_overrides(
        &self,
        archive: &mut zip::ZipArchive<fs::File>,
        dest: &Path,
    ) -> Result<usize, AppError> {
        extract_prefixes(archive, &self.overrides, dest)
    }
}

#[cfg(test)]
mod tests {
    use super::safe_relative;

    #[test]
    fn keeps_a_plain_relative_path() {
        // 用 Path 比较：Windows 上 `to_string_lossy` 会给反斜杠
        assert_eq!(
            safe_relative("mods/a.jar").unwrap(),
            std::path::Path::new("mods").join("a.jar")
        );
        assert_eq!(
            safe_relative("./mods/a.jar").unwrap(),
            std::path::Path::new("mods").join("a.jar")
        );
    }

    #[test]
    fn rejects_escapes_and_absolute_paths() {
        assert!(safe_relative("../a.jar").is_err());
        assert!(safe_relative("mods/../../a.jar").is_err());
        assert!(safe_relative("C:/a.jar").is_err());
        assert!(safe_relative("/etc/passwd").is_err());
        assert!(safe_relative("mods\\a.jar").is_err());
        assert!(safe_relative("").is_err());
        assert!(safe_relative(".").is_err());
    }
}
