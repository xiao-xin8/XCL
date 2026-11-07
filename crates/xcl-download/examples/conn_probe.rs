//! 验证「HTTP/2 多路复用会不会拖慢多连接下载」。
//!
//! 做法：用同一批 Range 请求分别打两个客户端 ——
//! `build_api_client`（允许 h2，会被多路复用进一条 TCP 连接）与
//! `build_download_client`（强制 HTTP/1.1，每条请求一条独立 TCP 连接），
//! 打印耗时与聚合吞吐。
//!
//! 用法：
//! ```text
//! cargo run -q -p xcl-download --example conn_probe
//! cargo run -q -p xcl-download --example conn_probe -- <url> [并发数] [每段字节]
//! ```
//!
//! 判读：如果 h1 明显快于 h2，说明瓶颈确实是「按连接限速」，
//! 分片必须走 1.1 才有意义。如果两者一样，说明瓶颈在链路本身。

use std::time::{Duration, Instant};

use tokio::task::JoinSet;
use xcl_core::AppError;

/// 1.20.1 的 client.jar，23MB，支持 Range。
const DEFAULT_URL: &str =
    "https://piston-data.mojang.com/v1/objects/0f275bc1547d01fa5f56ba34bdc87d981ee12daf/client.jar";

#[tokio::main]
async fn main() -> Result<(), AppError> {
    let mut args = std::env::args().skip(1);
    let url = args.next().unwrap_or_else(|| DEFAULT_URL.to_string());
    let concurrency: usize = args.next().and_then(|s| s.parse().ok()).unwrap_or(8);
    let chunk: u64 = args
        .next()
        .and_then(|s| s.parse().ok())
        .unwrap_or(512 * 1024);
    // both（默认）| h1 | h2 —— 扫描连接数时只跑 h1，省一半时间
    let mode = args.next().unwrap_or_else(|| "both".to_string());

    let total = concurrency as u64 * chunk;
    println!("目标   {url}");
    println!(
        "请求   {concurrency} 个并发 Range，每段 {:.1} KB，合计 {:.1} MB",
        chunk as f64 / 1024.0,
        total as f64 / 1_048_576.0
    );

    let api = xcl_net::build_api_client()?;
    let download = xcl_net::build_download_client()?;

    let (h2_elapsed, h2_kbps, h2_bad) = if mode == "h1" {
        (Duration::ZERO, 0.0, 0)
    } else {
        probe(&api, &url, concurrency, chunk, "h2").await
    };
    let (h1_elapsed, h1_kbps, h1_bad) = if mode == "h2" {
        (Duration::ZERO, 0.0, 0)
    } else {
        probe(&download, &url, concurrency, chunk, "h1").await
    };

    println!();
    if mode != "h1" {
        println!(
            "h2（多路复用）  {:>7.2}s  {:>7.0} KB/s  非 206/失败 {h2_bad}",
            h2_elapsed.as_secs_f64(),
            h2_kbps
        );
    }
    if mode != "h2" {
        println!(
            "h1（独立连接）  {:>7.2}s  {:>7.0} KB/s  非 206/失败 {h1_bad}",
            h1_elapsed.as_secs_f64(),
            h1_kbps
        );
    }

    if h2_kbps > 0.0 && h1_kbps > 0.0 {
        let ratio = h1_kbps / h2_kbps;
        println!();
        if ratio > 1.3 {
            println!("→ h1 快 {ratio:.2}x：多连接确实能叠加带宽，分片走 1.1 是对的");
        } else if ratio < 0.77 {
            println!(
                "→ h1 慢 {:.2}x：这条链路上 h2 反而更好，分片策略要重新考虑",
                1.0 / ratio
            );
        } else {
            println!("→ 两者接近（{ratio:.2}x）：瓶颈大概率是链路总带宽，不是按连接限速");
        }
    }
    Ok(())
}

/// 打一批 Range 请求，返回 (耗时, 聚合 KB/s, 异常数)。
async fn probe(
    client: &reqwest::Client,
    url: &str,
    concurrency: usize,
    chunk: u64,
    label: &str,
) -> (Duration, f64, usize) {
    let started = Instant::now();
    let mut set: JoinSet<Result<(u16, usize), reqwest::Error>> = JoinSet::new();

    for index in 0..concurrency {
        let client = client.clone();
        let url = url.to_string();
        let start = index as u64 * chunk;
        let end = start + chunk - 1;
        set.spawn(async move {
            let response = client
                .get(&url)
                .header(reqwest::header::RANGE, format!("bytes={start}-{end}"))
                .send()
                .await?;
            let status = response.status().as_u16();
            let body = response.bytes().await?;
            Ok::<_, reqwest::Error>((status, body.len()))
        });
    }

    let mut bytes = 0usize;
    let mut bad = 0usize;
    while let Some(joined) = set.join_next().await {
        match joined {
            Ok(Ok((status, len))) => {
                if status != 206 {
                    bad += 1;
                }
                bytes += len;
            }
            _ => bad += 1,
        }
    }

    let elapsed = started.elapsed();
    let kbps = bytes as f64 / elapsed.as_secs_f64() / 1024.0;
    // 机器可读的一行，方便扫描连接数时直接 grep
    println!(
        "RESULT {label} conc={concurrency} bytes={bytes} secs={:.2} kbps={kbps:.0} bad={bad}",
        elapsed.as_secs_f64()
    );
    (elapsed, kbps, bad)
}
