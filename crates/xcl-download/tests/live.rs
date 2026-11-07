//! 真实联网的冒烟测试。
//!
//! 离线测试只能证明"调度逻辑对"，证明不了"真实 HTTP + 真实镜像 + 64 并发能跑"。
//! 这里捞真实的版本 JSON，取 12 个真实的小库，分别用串行和并发 8下两遍，
//! 然后把耗时打出来对比。
//!
//! 默认忽略（不联网的环境不该红）。跑法：
//!
//! ```text
//! cargo test -p xcl-download --test live -- --ignored --nocapture
//! ```

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio_util::sync::CancellationToken;
use xcl_core::path::maven_to_relative_path;
use xcl_core::{Env, VersionDetail, VersionManifest};
use xcl_download::{
    DownloadItem, DownloadOptions, Fetcher, FileLedger, HttpFetcher, Progress, SplitConfig, run,
};
use xcl_net::HttpClient;
use xcl_source::{Mirror, SourcePolicy, Strategy, UrlKind};

const MANIFEST: &str = "https://piston-meta.mojang.com/mc/game/version_manifest_v2.json";

/// 用 `XCL_LIVE_MIRROR=official` 可切到官方源，用来对比"是引擎慢还是镜像慢"；
/// `XCL_LIVE_MIRROR=auto` 则走自动选源（先官方 5 秒，超时再并发镜像）。
fn live_policy() -> SourcePolicy {
    match std::env::var("XCL_LIVE_MIRROR").as_deref() {
        Ok("official") => SourcePolicy::new(Mirror::Official, Strategy::MirrorFirst),
        Ok("auto") => SourcePolicy::new(Mirror::Bmclapi, Strategy::Auto),
        _ => SourcePolicy::new(Mirror::Bmclapi, Strategy::MirrorFirst),
    }
}

fn work_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("xcl-live-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// 测试专用的下载选项。
///
/// 默认关掉分片（`max_connections: 1` 即不可分片），
/// 这样多文件并发的冒烟测试测的确实是「文件级并发」，不会被单文件分片干扰。
fn options(_dir: &std::path::Path, concurrency: usize) -> DownloadOptions {
    DownloadOptions {
        concurrency,
        attempts_per_url: 2,
        split: SplitConfig {
            max_connections: 1,
            ..SplitConfig::default()
        },
    }
}

/// 打开分片的选项，只给「分片对比」那条测试用。
fn split_options() -> DownloadOptions {
    DownloadOptions {
        concurrency: 1,
        attempts_per_url: 2,
        split: SplitConfig::default(),
    }
}

/// 拉真实的 1.20.1 版本 JSON，挑出 12 个小体积的支持库（避开 client jar 那种大包）。
async fn real_items(dir: &std::path::Path) -> Vec<DownloadItem> {
    let policy = live_policy();
    let http = HttpClient::new(policy.clone()).expect("构造 HTTP 客户端");

    let text = http
        .get_text(UrlKind::Meta, MANIFEST)
        .await
        .expect("版本清单应能拉到");
    let manifest = VersionManifest::parse(&text).expect("清单解析");
    let target = manifest
        .versions
        .iter()
        .find(|v| v.id == "1.20.1")
        .expect("清单里应有 1.20.1");

    let detail_text = http
        .get_text(UrlKind::Meta, &target.url)
        .await
        .expect("版本 JSON 应能拉到");
    let detail = VersionDetail::parse(&detail_text).expect("版本 JSON 解析");

    let env = Env::current();
    let mut items = Vec::new();
    for library in detail.libraries.iter().filter(|l| l.is_applicable(&env)) {
        let Some(artifact) = library.artifact() else {
            continue;
        };
        let size = artifact.size.unwrap_or(0);
        // 只要小文件：大包会让冒烟测试变慢，测的也不是同一回事
        if size == 0 || size > 2_000_000 {
            continue;
        }
        let Some(relative) = artifact
            .path
            .clone()
            .or_else(|| maven_to_relative_path(&library.name))
        else {
            continue;
        };
        let urls = policy.candidates(UrlKind::Library, &artifact.url);
        if urls.is_empty() {
            continue;
        }
        items.push(DownloadItem {
            name: library.name.clone(),
            urls,
            split_urls: Vec::new(),
            dest: dir.join(relative),
            sha1: artifact.sha1.clone(),
            size,
        });
        if items.len() >= 12 {
            break;
        }
    }
    items
}

#[ignore = "需要联网"]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn live_concurrent_and_sequential() {
    let serial_dir = work_dir("serial");
    let concurrent_dir = work_dir("concurrent");

    // 先拿一份清单，两个目录用同一批任务（路径换成各自目录）
    let template = real_items(&serial_dir).await;
    assert!(
        template.len() >= 8,
        "真实清单里应能挑出足够的小库，实际 {}",
        template.len()
    );

    let total_bytes: u64 = template.iter().map(|i| i.size).sum();
    println!(
        "\n[准备] 源 = {:?}，{} 个文件，合计 {:.2} MB",
        live_policy().mirror,
        template.len(),
        total_bytes as f64 / 1_048_576.0
    );

    // ---- 串行 ----
    let serial_items: Vec<DownloadItem> = template
        .iter()
        .map(|item| DownloadItem {
            dest: serial_dir.join(
                item.dest
                    .strip_prefix(&serial_dir)
                    .unwrap_or(&item.dest)
                    .to_path_buf(),
            ),
            ..item.clone()
        })
        .collect();
    let serial_progress = Progress::new(total_bytes, serial_items.len());
    let fetcher: Arc<dyn Fetcher> = Arc::new(HttpFetcher::new().unwrap());

    let started = Instant::now();
    let serial = run(
        serial_items.clone(),
        Arc::clone(&fetcher),
        Arc::clone(&serial_progress),
        DownloadOptions {
            concurrency: 1,
            attempts_per_url: 2,
            split: SplitConfig {
                max_connections: 1,
                ..SplitConfig::default()
            },
        },
        CancellationToken::new(),
    )
    .await;
    let serial_elapsed = started.elapsed();

    // ---- 并发 8 ----
    let concurrent_items: Vec<DownloadItem> = template
        .iter()
        .map(|item| DownloadItem {
            dest: concurrent_dir.join(
                item.dest
                    .strip_prefix(&serial_dir)
                    .unwrap_or(&item.dest)
                    .to_path_buf(),
            ),
            ..item.clone()
        })
        .collect();
    let concurrent_progress = Progress::new(total_bytes, concurrent_items.len());

    let started = Instant::now();
    let concurrent = run(
        concurrent_items.clone(),
        fetcher,
        Arc::clone(&concurrent_progress),
        DownloadOptions {
            concurrency: 8,
            attempts_per_url: 2,
            split: SplitConfig {
                max_connections: 1,
                ..SplitConfig::default()
            },
        },
        CancellationToken::new(),
    )
    .await;
    let concurrent_elapsed = started.elapsed();

    println!(
        "[串行]     下载 {} 跳过 {} 失败 {}，耗时 {:?}",
        serial.downloaded, serial.skipped, serial.failed, serial_elapsed
    );
    println!(
        "[并发 8]   下载 {} 跳过 {} 失败 {}，耗时 {:?}",
        concurrent.downloaded, concurrent.skipped, concurrent.failed, concurrent_elapsed
    );
    println!(
        "[进度] 串行 {:.1}% / 并发 {:.1}%",
        serial_progress.snapshot().fraction() * 100.0,
        concurrent_progress.snapshot().fraction() * 100.0
    );
    if concurrent_elapsed.as_secs_f64() > 0.0 {
        println!(
            "[加速比] {:.2}x",
            serial_elapsed.as_secs_f64() / concurrent_elapsed.as_secs_f64()
        );
    }

    assert_eq!(serial.failed, 0, "串行失败项：{:?}", serial.failures);
    assert_eq!(concurrent.failed, 0, "并发失败项：{:?}", concurrent.failures);
    assert_eq!(concurrent.downloaded, concurrent_items.len(), "应全部真实下载");

    // 落盘校验：文件存在、大小与期望一致
    for item in &concurrent_items {
        let meta = std::fs::metadata(&item.dest)
            .unwrap_or_else(|_| panic!("文件没落盘：{}", item.dest.display()));
        assert_eq!(meta.len(), item.size, "大小不符：{}", item.dest.display());
        assert!(
            !item.dest.with_extension("jar.xclpart").exists(),
            "不应残留临时文件"
        );
    }

    // 第二次跑：全部命中 sha1，应该一个请求都不发
    let skip_progress = Progress::new(total_bytes, concurrent_items.len());
    let fetcher: Arc<dyn Fetcher> = Arc::new(HttpFetcher::new().unwrap());
    let started = Instant::now();
    let rerun = run(
        concurrent_items.clone(),
        fetcher,
        skip_progress,
        DownloadOptions {
            concurrency: 8,
            attempts_per_url: 2,
            split: SplitConfig {
                max_connections: 1,
                ..SplitConfig::default()
            },
        },
        CancellationToken::new(),
    )
    .await;
    println!(
        "[重跑]     下载 {} 跳过 {}，耗时 {:?}",
        rerun.downloaded,
        rerun.skipped,
        started.elapsed()
    );
    assert_eq!(rerun.downloaded, 0, "已存在且校验通过的文件不该重下");
    assert_eq!(rerun.skipped, concurrent_items.len());

    // ---- 并发 64（生产默认值）----
    // 只有 12 个文件时 64 个许可不会全部用上，但这条路径必须能跑通：
    // 64 并发是真实安装时的默认值，不能靠"应该没问题"上线。
    let wide_dir = work_dir("wide");
    let wide_items: Vec<DownloadItem> = template
        .iter()
        .map(|item| DownloadItem {
            dest: wide_dir.join(
                item.dest
                    .strip_prefix(&serial_dir)
                    .unwrap_or(&item.dest)
                    .to_path_buf(),
            ),
            ..item.clone()
        })
        .collect();
    let wide_progress = Progress::new(total_bytes, wide_items.len());
    let fetcher: Arc<dyn Fetcher> = Arc::new(HttpFetcher::new().unwrap());
    let started = Instant::now();
    let wide = run(
        wide_items.iter().cloned().collect(),
        fetcher,
        Arc::clone(&wide_progress),
        DownloadOptions {
            concurrency: 64,
            attempts_per_url: 2,
            split: SplitConfig {
                max_connections: 1,
                ..SplitConfig::default()
            },
        },
        CancellationToken::new(),
    )
    .await;
    let wide_elapsed = started.elapsed();
    println!(
        "[并发 64]  下载 {} 跳过 {} 失败 {}，耗时 {:?}",
        wide.downloaded, wide.skipped, wide.failed, wide_elapsed
    );
    assert_eq!(wide.failed, 0, "并发 64 失败项：{:?}", wide.failures);
    assert_eq!(wide.downloaded, wide_items.len(), "并发 64 应全部下完");
    let _ = std::fs::remove_dir_all(&wide_dir);

    let _ = std::fs::remove_dir_all(&serial_dir);
    let _ = std::fs::remove_dir_all(&concurrent_dir);
}

/// 取消必须真的切断连接，而不是"停止调度、让在途请求继续跑完"。
///
/// 点取消时最容易踩的坑就是这个：界面停了，但已经建立的 keep-alive
/// 连接还在收数据，看起来像"取消没用"。这里下一个 23MB 的 client jar，
/// 400ms 后取消，要求在几秒内停下来、且不留残留文件。
#[ignore = "需要联网"]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn live_cancel_cuts_inflight_connection() {
    let policy = live_policy();
    let http = HttpClient::new(policy.clone()).expect("构造 HTTP 客户端");

    let text = http
        .get_text(UrlKind::Meta, MANIFEST)
        .await
        .expect("版本清单应能拉到");
    let manifest = VersionManifest::parse(&text).unwrap();
    let target = manifest
        .versions
        .iter()
        .find(|v| v.id == "1.20.1")
        .expect("应有 1.20.1");
    let detail_text = http.get_text(UrlKind::Meta, &target.url).await.unwrap();
    let detail = VersionDetail::parse(&detail_text).unwrap();
    let client = detail.downloads.as_ref().unwrap().client.as_ref().unwrap();

    let dir = work_dir("cancel");
    let dest = dir.join("client.jar");
    let item = DownloadItem {
        name: "client.jar".into(),
        urls: policy.candidates(UrlKind::Meta, &client.url),
        split_urls: Vec::new(),
        dest: dest.clone(),
        sha1: client.sha1.clone(),
        size: client.size.unwrap_or(0),
    };
    let expected_size = item.size;
    println!(
        "\n[取消测试] 目标 {}（{:.1} MB）",
        client.url,
        expected_size as f64 / 1_048_576.0
    );

    let cancel = CancellationToken::new();
    let trigger = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(400)).await;
        trigger.cancel();
    });

    let fetcher: Arc<dyn Fetcher> = Arc::new(HttpFetcher::new().unwrap());
    let progress = Progress::new(expected_size, 1);
    let started = Instant::now();
    let summary = run(
        vec![item],
        fetcher,
        Arc::clone(&progress),
        DownloadOptions {
            concurrency: 4,
            attempts_per_url: 2,
            split: SplitConfig {
                max_connections: 1,
                ..SplitConfig::default()
            },
        },
        cancel,
    )
    .await;
    let elapsed = started.elapsed();
    let got = progress.snapshot().done_bytes;

    println!(
        "[取消测试] 结束于 {:?}，已接收 {:.2} MB，cancelled={} downloaded={}",
        elapsed,
        got as f64 / 1_048_576.0,
        summary.cancelled,
        summary.downloaded
    );

    assert!(summary.cancelled, "应标记为已取消");
    assert_eq!(summary.downloaded, 0, "取消后不该有文件被判成功");
    assert!(
        elapsed < Duration::from_secs(6),
        "取消必须立刻生效（在途连接要被切断），实际耗时 {elapsed:?}"
    );
    assert!(
        got < expected_size / 2,
        "取消时不该把文件收完，实际收了 {got} / {expected_size}"
    );
    assert!(!dest.exists(), "取消后不该留下成品文件");
    assert!(
        !dest.with_extension("jar.xclpart").exists(),
        "取消后临时文件必须清掉"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// 分片下载到底快多少 —— 拿一个必须分片的真文件（1.20.1 client jar，约 23MB）
/// 跑两遍：单连接整文件 / 引擎分片，直接把两个耗时打出来。
///
/// 这是分片唯一的验收标准。如果加速比上不去，说明参数（阈值、片数）选错了，
/// 别靠"逻辑看起来对"就上线。
#[ignore = "需要联网"]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn live_split_speedup_on_client_jar() {
    let policy = live_policy();
    let http = HttpClient::new(policy.clone()).expect("构造 HTTP 客户端");

    let text = http
        .get_text(UrlKind::Meta, MANIFEST)
        .await
        .expect("版本清单应能拉到");
    let manifest = VersionManifest::parse(&text).unwrap();
    let target = manifest
        .versions
        .iter()
        .find(|v| v.id == "1.20.1")
        .expect("应有 1.20.1");
    let detail_text = http.get_text(UrlKind::Meta, &target.url).await.unwrap();
    let detail = VersionDetail::parse(&detail_text).unwrap();
    let client = detail.downloads.as_ref().unwrap().client.as_ref().unwrap();
    let size = client.size.unwrap_or(0);
    let urls = policy.candidates(UrlKind::Meta, &client.url);
    let url = urls.first().expect("至少一个候选地址").clone();

    let dir = work_dir("split-speed");
    println!(
        "\n[分片对比] client.jar {:.1} MB，源 = {:?}",
        size as f64 / 1_048_576.0,
        policy.mirror
    );

    // ---- 基准：单连接把整个文件拉一遍 ----
    let single_dest = dir.join("single").join("client.jar");
    std::fs::create_dir_all(single_dest.parent().unwrap()).unwrap();
    let fetcher: Arc<dyn Fetcher> = Arc::new(HttpFetcher::new().unwrap());
    let single_progress = Progress::new(size, 1);
    let started = Instant::now();
    let written = fetcher
        .fetch_to(
            &url,
            &single_dest,
            size,
            FileLedger::new(Arc::clone(&single_progress)),
            CancellationToken::new(),
        )
        .await
        .expect("单连接下载应成功");
    let single_elapsed = started.elapsed();
    println!(
        "[单连接] {:?}，{} 字节，{:.0} KB/s",
        single_elapsed,
        written,
        written as f64 / single_elapsed.as_secs_f64() / 1024.0
    );

    // ---- 走引擎：client.jar 远大于 1MiB 门槛，必然分片 ----
    let split_dest = dir.join("split").join("client.jar");
    std::fs::create_dir_all(split_dest.parent().unwrap()).unwrap();
    let split_urls = urls.clone();
    let item = DownloadItem {
        name: "client.jar".into(),
        urls,
        split_urls,
        dest: split_dest.clone(),
        sha1: client.sha1.clone(),
        size,
    };
    let split_progress = Progress::new(size, 1);
    let started = Instant::now();
    let summary = run(
        vec![item],
        fetcher,
        Arc::clone(&split_progress),
        split_options(),
        CancellationToken::new(),
    )
    .await;
    let split_elapsed = started.elapsed();

    assert_eq!(summary.failed, 0, "分片失败项：{:?}", summary.failures);
    assert_eq!(summary.downloaded, 1, "分片应把一个文件完整下下来");
    assert_eq!(
        split_progress.snapshot().done_bytes,
        size,
        "进度必须正好等于文件大小"
    );

    let single_bytes = std::fs::read(&single_dest).expect("读单连接结果");
    let split_bytes = std::fs::read(&split_dest).expect("读分片结果");
    assert_eq!(split_bytes.len() as u64, size, "分片合成长度不符");
    assert_eq!(split_bytes, single_bytes, "分片合并结果必须与单连接逐字节一致");

    println!(
        "[分片 8] {:?}，{:.0} KB/s",
        split_elapsed,
        size as f64 / split_elapsed.as_secs_f64() / 1024.0
    );
    let speedup = single_elapsed.as_secs_f64() / split_elapsed.as_secs_f64();
    println!("[加速比] {speedup:.2}x");
    if speedup < 1.0 {
        println!(
            "[注意] 本次分片没有收益。同一文件实测过 250~705 KB/s 的波动，\
             结论要以网络稳定时的多轮结果为准；若该源长期无收益，就把它列入不分片名单。"
        );
    }

    // 不对加速比做硬断言：网络波动足以让 1.3x 这种阈值随机变红，
    // 那会让这条测试变成噪音。这里只保证正确性，收益数字交给人判断。
    assert!(
        split_bytes.len() as u64 == size,
        "分片合并结果必须完整，否则收益再高也没意义"
    );

    // 不留半截文件
    let leftovers = std::fs::read_dir(dir.join("split"))
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .filter(|e| {
                    e.path()
                        .extension()
                        .is_some_and(|ext| ext == "xclpart")
                })
                .count()
        })
        .unwrap_or(0);
    assert_eq!(leftovers, 0, "分片完成后不该留下半截文件");

    let _ = std::fs::remove_dir_all(&dir);
}
