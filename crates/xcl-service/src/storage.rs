//! 缓存目录的体积统计与清理。
//!
//! 缓存全部在 `cache_root` 下（`%LOCALAPPDATA%\XCL\cache`），不属于数据：
//! 删了只会让下次重新下载 / 重新取皮肤，不会丢版本、存档、账号。
//! 分类是按子目录来的，跟 `PathLayout` 的划分一一对应。

use std::collections::HashSet;
use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};
use xcl_core::path::PathLayout;
use xcl_core::AppError;

use crate::AppState;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CacheKind {
    /// 接口 / 版本清单缓存
    Http,
    /// 下载中的半截文件
    Temp,
    /// 皮肤地址索引与位图
    Skin,
    /// 以上全部
    All,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CacheEntry {
    /// 清哪个（回传给 `storage_clear`）
    pub kind: &'static str,
    pub label: &'static str,
    pub desc: &'static str,
    pub bytes: u64,
}

/// 目录总大小。读不了的项直接跳过 —— 统计不准好过弹错。
pub fn dir_size(path: &Path) -> u64 {
    let Ok(meta) = fs::symlink_metadata(path) else {
        return 0;
    };
    if meta.is_file() {
        return meta.len();
    }
    if !meta.is_dir() {
        return 0;
    }
    let Ok(read) = fs::read_dir(path) else {
        return 0;
    };
    read.filter_map(|entry| entry.ok())
        .map(|entry| dir_size(&entry.path()))
        .sum()
}

impl AppState {
    fn cache_path(&self, kind: CacheKind) -> Option<Vec<std::path::PathBuf>> {
        let layout = &self.layout;
        Some(match kind {
            CacheKind::Http => vec![layout.http_cache_dir()],
            CacheKind::Temp => vec![layout.temp_dir()],
            CacheKind::Skin => vec![layout.skin_cache_dir()],
            CacheKind::All => vec![
                layout.http_cache_dir(),
                layout.temp_dir(),
                layout.skin_cache_dir(),
            ],
        })
    }

    /// 各类缓存的当前体积。前端只负责格式化，不再自己算。
    pub fn cache_entries(&self) -> Vec<CacheEntry> {
        let layout = &self.layout;
        vec![
            CacheEntry {
                kind: "http",
                label: "下载缓存",
                desc: "版本清单与接口响应",
                bytes: dir_size(&layout.http_cache_dir()),
            },
            CacheEntry {
                kind: "temp",
                label: "半截文件",
                desc: "没下完的分片",
                bytes: dir_size(&layout.temp_dir()),
            },
            CacheEntry {
                kind: "skin",
                label: "皮肤",
                desc: "地址索引与位图",
                bytes: dir_size(&layout.skin_cache_dir()),
            },
        ]
    }

    /// 清掉指定缓存。目录不存在算成功（结果一样）。
    pub fn cache_clear(&self, kind: CacheKind) -> Result<(), AppError> {
        let paths = self
            .cache_path(kind)
            .ok_or_else(|| AppError::validation("缓存类别不正确", String::new()))?;

        for path in paths {
            if !path.exists() {
                continue;
            }
            // 只删 cache_root 下的东西：路径是 layout 自己拼的，这里再兜一次底，
            // 防止将来有人把 `..` 拼进来
            if !path.starts_with(self.layout.cache_root()) {
                continue;
            }
            fs::remove_dir_all(&path).map_err(|e| AppError::io(&e, "清理缓存"))?;
        }

        // 接口缓存是内存里的那份，也要一起失效，否则「清了还在」
        if matches!(kind, CacheKind::Http | CacheKind::All) {
            self.cache().clear();
        }
        Ok(())
    }
}

// ---------- 未使用的资源（libraries / assets） ----------

/// 「没有任何已安装版本引用」的库与资源。
///
/// 删除是不可逆的，但代价可接受：这些文件重新装一次版本就会下回来。
#[derive(Debug, Clone, Copy, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UnusedReport {
    pub libraries_files: u64,
    pub libraries_bytes: u64,
    pub indexes_files: u64,
    pub indexes_bytes: u64,
    pub objects_files: u64,
    pub objects_bytes: u64,
}

impl UnusedReport {
    pub fn total_files(self) -> u64 {
        self.libraries_files + self.indexes_files + self.objects_files
    }

    pub fn total_bytes(self) -> u64 {
        self.libraries_bytes + self.indexes_bytes + self.objects_bytes
    }
}

/// 三类「被引用」的集合。
#[derive(Default)]
struct Usage {
    /// 相对 `libraries/` 的路径，统一成 `/` 分隔
    libraries: HashSet<String>,
    /// 资源索引 id
    indexes: HashSet<String>,
    /// 资源对象的 hash
    objects: HashSet<String>,
}

/// 把所有已安装版本引用到的东西收进来。
///
/// 推导方式必须和安装时一致（`artifact()` → `resolved_artifact()` 的 maven 兜底），
/// 只认 `downloads.artifact` 的话，Fabric / NeoForge 的库全会被判成垃圾。
fn collect_usage(layout: &PathLayout) -> Usage {
    let mut usage = Usage::default();

    for version in crate::local::scan(layout) {
        let Ok(mut detail) = crate::local::load(layout, &version.id) else {
            continue;
        };

        // 沿 `inheritsFrom` 合并，别漏掉父版本的库
        let mut next = detail.inherits_from.clone();
        let mut depth = 0;
        while let Some(parent) = next {
            if depth >= 8 {
                break;
            }
            let Ok(parent_detail) = crate::local::load(layout, &parent) else {
                break;
            };
            next = parent_detail.inherits_from.clone();
            detail = xcl_core::merge_versions(detail, parent_detail);
            depth += 1;
        }

        for library in &detail.libraries {
            let main = library
                .artifact()
                .and_then(|artifact| artifact.path.clone())
                .or_else(|| {
                    library
                        .resolved_artifact()
                        .and_then(|artifact| artifact.path)
                });
            if let Some(path) = main {
                usage.libraries.insert(path.replace('\\', "/"));
            }
            // natives 的包和解压用的 classifier，少收一个就会误删
            if let Some(artifact) = library.native_artifact(OS_NAME) {
                if let Some(path) = &artifact.path {
                    usage.libraries.insert(path.replace('\\', "/"));
                }
            }
        }

        if let Some(index) = &detail.asset_index {
            usage.indexes.insert(index.id.clone());
        }
    }

    // 资源对象：hash 在 index 的 value 里，key 是逻辑路径（踩过）
    for id in &usage.indexes {
        let path = layout.assets_indexes_dir().join(format!("{id}.json"));
        let Ok(text) = fs::read_to_string(&path) else {
            continue;
        };
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
            continue;
        };
        if let Some(objects) = value.get("objects").and_then(|v| v.as_object()) {
            for info in objects.values() {
                if let Some(hash) = info.get("hash").and_then(|h| h.as_str()) {
                    usage.objects.insert(hash.to_string());
                }
            }
        }
    }

    usage
}

/// 项目只发 Windows，natives 的 classifier 固定按它取。
const OS_NAME: &str = "windows";

/// 递归遍历，回调给出「相对 `base` 的 `/` 分隔路径」。
fn walk_relative(base: &Path, dir: &Path, visit: &mut impl FnMut(&Path, &str)) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk_relative(base, &path, visit);
            continue;
        }
        let rel = path
            .strip_prefix(base)
            .map(|p| p.to_string_lossy().replace('\\', "/"))
            .unwrap_or_default();
        visit(&path, &rel);
    }
}

/// 遍历 `assets/objects/<bucket>/<hash>` 这一层。
fn walk_buckets(dir: &Path, visit: &mut impl FnMut(&Path, &str)) {
    let Ok(buckets) = fs::read_dir(dir) else {
        return;
    };
    for bucket in buckets.flatten() {
        let bucket_path = bucket.path();
        if !bucket_path.is_dir() {
            continue;
        }
        let Ok(files) = fs::read_dir(&bucket_path) else {
            continue;
        };
        for file in files.flatten() {
            let path = file.path();
            if path.is_dir() {
                continue;
            }
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            visit(&path, &name);
        }
    }
}

/// 统计（或删除）未被引用的文件。
///
/// `remove = false` 只算不动；`true` 时删不掉的文件（被占用）跳过、不计入结果 ——
/// 报告里的数字永远是「真的处理掉的」，不是「打算处理的」。
fn sweep(layout: &PathLayout, remove: bool) -> UnusedReport {
    let usage = collect_usage(layout);
    let mut report = UnusedReport::default();

    let visit = |path: &Path, in_use: bool, files: &mut u64, bytes: &mut u64| {
        if in_use {
            return;
        }
        let size = fs::metadata(path).map(|meta| meta.len()).unwrap_or(0);
        if remove && fs::remove_file(path).is_err() {
            return;
        }
        *files += 1;
        *bytes += size;
    };

    walk_relative(&layout.libraries_dir(), &layout.libraries_dir(), &mut |path, rel| {
        visit(
            path,
            usage.libraries.contains(rel),
            &mut report.libraries_files,
            &mut report.libraries_bytes,
        );
    });

    if let Ok(entries) = fs::read_dir(layout.assets_indexes_dir()) {
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            visit(
                &path,
                usage.indexes.contains(stem),
                &mut report.indexes_files,
                &mut report.indexes_bytes,
            );
        }
    }

    walk_buckets(&layout.assets_objects_dir(), &mut |path, hash| {
        visit(
            path,
            usage.objects.contains(hash),
            &mut report.objects_files,
            &mut report.objects_bytes,
        );
    });

    report
}

impl AppState {
    /// 扫描可清理的库与资源（只统计，不动文件）。
    pub fn storage_unused_scan(&self) -> UnusedReport {
        sweep(&self.layout, false)
    }

    /// 删掉未被任何版本引用的库与资源，返回实际删掉的量。
    pub fn storage_unused_clean(&self) -> Result<UnusedReport, AppError> {
        // 游戏跑着的时候删 assets，它切维度 / 播音效就可能读不到文件
        if self.launches.any_active() {
            return Err(AppError::validation(
                "游戏正在运行",
                "关掉游戏之后再清理，否则它可能读不到需要的资源",
            ));
        }
        Ok(sweep(&self.layout, true))
    }

    /// 扫描放到阻塞线程里：要遍历上万个文件，别占 async worker。
    pub async fn storage_unused_scan_async(self: &std::sync::Arc<Self>) -> UnusedReport {
        let state = std::sync::Arc::clone(self);
        tokio::task::spawn_blocking(move || state.storage_unused_scan())
            .await
            .unwrap_or_default()
    }

    /// 清理同样放阻塞线程。
    pub async fn storage_unused_clean_async(
        self: &std::sync::Arc<Self>,
    ) -> Result<UnusedReport, AppError> {
        let state = std::sync::Arc::clone(self);
        tokio::task::spawn_blocking(move || state.storage_unused_clean())
            .await
            .map_err(|err| AppError::unsupported("清理未使用资源失败", err.to_string()))?
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn temp_dir(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("xcl-storage-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn dir_size_counts_nested_files() {
        let dir = temp_dir("size");
        fs::create_dir_all(dir.join("a/b")).unwrap();
        fs::write(dir.join("a/one.bin"), vec![0u8; 100]).unwrap();
        let mut file = fs::File::create(dir.join("a/b/two.bin")).unwrap();
        file.write_all(&vec![0u8; 50]).unwrap();
        assert_eq!(dir_size(&dir), 150);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn dir_size_of_missing_dir_is_zero() {
        let dir = temp_dir("missing").join("nothing-here");
        assert_eq!(dir_size(&dir), 0);
    }

    #[test]
    fn unused_report_totals_add_up() {
        let report = UnusedReport {
            libraries_files: 1,
            libraries_bytes: 10,
            indexes_files: 2,
            indexes_bytes: 20,
            objects_files: 3,
            objects_bytes: 30,
        };
        assert_eq!(report.total_files(), 6);
        assert_eq!(report.total_bytes(), 60);
    }

    /// 没有任何版本时，libraries 和 assets 里的东西全该被判为「未引用」。
    #[test]
    fn scan_and_clean_without_any_version() {
        let root = temp_dir("unused-none");
        let layout = PathLayout::new(root.clone(), root.join("cache"));

        let library = root.join("libraries/a/b/1.0/b-1.0.jar");
        fs::create_dir_all(library.parent().unwrap()).unwrap();
        fs::write(&library, vec![0u8; 100]).unwrap();

        let object = root.join("assets/objects/ab/abcdef0123456789");
        fs::create_dir_all(object.parent().unwrap()).unwrap();
        fs::write(&object, vec![0u8; 50]).unwrap();

        // 只统计不删：文件必须还在
        let report = sweep(&layout, false);
        assert_eq!(report.libraries_files, 1);
        assert_eq!(report.libraries_bytes, 100);
        assert_eq!(report.objects_files, 1);
        assert_eq!(report.objects_bytes, 50);
        assert!(library.exists(), "扫描不该动文件");

        // 真删
        let cleaned = sweep(&layout, true);
        assert_eq!(cleaned.libraries_files, 1);
        assert_eq!(cleaned.objects_files, 1);
        assert!(!library.exists());
        assert!(!object.exists());

        let _ = fs::remove_dir_all(&root);
    }
}
