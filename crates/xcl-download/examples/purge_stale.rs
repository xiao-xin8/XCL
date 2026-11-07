//! 手动清扫工具：把一批陈旧半截文件（`.xclpart`）清掉，并报告动过什么。
//!
//! 正常流程用不到它 —— 每次下载开始前 `run()` 会自己清一遍。
//! 留着是为了在数据目录被反复中断搞脏、或者想手工确认「到底还剩多少垃圾」时有个入口。
//!
//! 用法：`cargo run -p xcl-download --example purge_stale -- <数据根目录>`
//! 不带参数时默认 `target/debug/data`。

use std::path::{Path, PathBuf};

use xcl_download::{purge_stale_parts, DownloadItem};

/// 递归收出目录下所有半截文件，再把后缀去掉，还原成「本来该在哪」的目标路径。
fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect(&path, out);
        } else if path.extension().and_then(|e| e.to_str()) == Some("xclpart") {
            out.push(path);
        }
    }
}

#[tokio::main]
async fn main() {
    let root = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("target/debug/data"));

    if !root.exists() {
        eprintln!("数据目录不存在：{}", root.display());
        std::process::exit(1);
    }

    let mut parts = Vec::new();
    collect(&root, &mut parts);

    let total_bytes: u64 = parts
        .iter()
        .filter_map(|p| std::fs::metadata(p).ok())
        .map(|m| m.len())
        .sum();

    println!("扫描 {}：找到 {} 个半截文件，共 {} 字节", root.display(), parts.len(), total_bytes);
    for part in &parts {
        println!("  {}", part.display());
    }

    if parts.is_empty() {
        println!("没有需要清理的东西。");
        return;
    }

    // DownloadItem 的 dest 是「正式文件」路径，清理逻辑自己会拼回 .xclpart
    let items: Vec<DownloadItem> = parts
        .iter()
        .map(|part| {
            let dest = PathBuf::from(part.to_string_lossy().trim_end_matches(".xclpart"));
            DownloadItem {
                name: dest
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                urls: Vec::new(),
                split_urls: Vec::new(),
                dest,
                sha1: None,
                size: 0,
            }
        })
        .collect();

    let removed = purge_stale_parts(&items).await;
    println!("实际删除 {removed} 个");

    let mut leftover = Vec::new();
    collect(&root, &mut leftover);
    println!("复查：还剩 {} 个半截文件", leftover.len());

    // 顺手确认正式文件没被动过
    let survivors = items
        .iter()
        .filter(|item| item.dest.exists())
        .count();
    println!("同批目标路径中已存在的正式文件：{survivors} 个（这些绝不能被删）");
}
