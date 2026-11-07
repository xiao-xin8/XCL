//! 并发下载引擎。
//!
//! 结构（`Semaphore + JoinSet`，另外补了三件事）：
//! - 原子落盘：先写 `.xclpart`，校验通过才 rename；中断绝不留半截文件
//! - 失败换源：同一文件多个候选地址，逐个尝试
//! - 大文件分片：≥1MB 的文件按「未完成区间 + 租约」调度，见 [`split`]
//!
//! 并发数由上层传入（`DownloadOptions::concurrency`）。引擎自己不知道也不想
//! 知道用的是官方源还是镜像 —— 官方源扛得住 64，镜像在 16 左右更稳，
//! 这个判断在 `xcl-service` 里按「最终选中的源」决定。
//! 注意这是并发任务数，不是操作系统线程数 —— 64 个下载任务可能只跑在几个线程上。

pub mod fetcher;
pub mod split;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use futures::stream::{self, StreamExt};
use sha1::{Digest, Sha1};
use tokio::sync::Semaphore;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;
use tracing::debug;
use xcl_core::ErrorCode;

pub use fetcher::{ByteRange, Fetcher, HttpFetcher, PieceFailure};
pub use split::{PieceScheduler, SplitConfig};

/// 临时文件后缀。下载完成后才会 rename 成正式文件。
const PART_SUFFIX: &str = ".xclpart";

/// 清理陈旧半截文件时的并发度。删一个不存在的文件也只是一次 syscall，
/// 但这个数不能太大 —— 清理是为了顺手收拾，不值得和下载抢 IO。
const PURGE_CONCURRENCY: usize = 32;

/// 单个地址的最多尝试次数（含「有进展」的那些）。
///
/// 正常情况下 `download.retries`（默认 1）就够了；这里给到 10 是因为续传
/// 每次都能前进一段时不该被判死，而「每次只进 1 字节」这种极端情况
/// 又必须能收敛，所以要有个硬上限。
const MAX_ATTEMPTS_PER_URL: u32 = 10;

// ---------- 分片扩容 ----------

/// 一开始就开的连接数。之前是 1，靠「每完成一个 4MiB 请求 +1」慢慢爬；
/// 对按连接限速的源，第一条 4MiB 就要几十秒，等于前半段全在单连接跑。
const INITIAL_SPLIT_CONNECTIONS: usize = 4;
/// 扩容检查间隔。
const SPLIT_RAMP_INTERVAL: Duration = Duration::from_millis(250);
/// 单连接低于这个速度就认为「再加一条可能有用」。
/// 1MB/s：4 条能稳定跑满 4MB/s 时不再加；被按连接限速（每条几百 KB/s）时
/// 会一路加到上限。比 256KB/s 的总速度下限更积极，因为单文件没有第二路可依靠。
const SPLIT_TARGET_PER_CONNECTION_BPS: u64 = 1024 * 1024;
/// 首条连接抢全局分片许可最多等多久。等不到就退回单连接 ——
/// 否则几十个 1–2MB 的文件会一起堵在分片池前，谁都不动。
const SPLIT_POOL_WAIT: Duration = Duration::from_secs(2);

// ---------- 进度 ----------

/// 线程安全的进度累加器。所有任务共享同一个实例。
#[derive(Debug, Default)]
pub struct Progress {
    total_bytes: AtomicU64,
    done_bytes: AtomicU64,
    total_files: AtomicUsize,
    finished_files: AtomicUsize,
    downloaded_files: AtomicUsize,
    skipped_files: AtomicUsize,
    failed_files: AtomicUsize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Snapshot {
    pub total_bytes: u64,
    pub done_bytes: u64,
    pub total_files: usize,
    pub finished_files: usize,
    pub downloaded_files: usize,
    pub skipped_files: usize,
    pub failed_files: usize,
}

impl Snapshot {
    /// 0.0 ~ 1.0。按字节算，不按文件数 —— 否则小文件会让进度条抽搐。
    pub fn fraction(&self) -> f32 {
        if self.total_bytes == 0 {
            return if self.total_files > 0 && self.finished_files >= self.total_files {
                1.0
            } else {
                0.0
            };
        }
        (self.done_bytes as f64 / self.total_bytes as f64).min(1.0) as f32
    }
}

impl Progress {
    pub fn new(total_bytes: u64, total_files: usize) -> Arc<Self> {
        Arc::new(Self {
            total_bytes: AtomicU64::new(total_bytes),
            total_files: AtomicUsize::new(total_files),
            ..Default::default()
        })
    }

    pub fn add_bytes(&self, bytes: u64) {
        self.done_bytes.fetch_add(bytes, Ordering::Relaxed);
    }

    /// 清单没给大小的文件，下完之后把它实际占的量补进总量。
    ///
    /// 不补的话这类文件只加已下载、不加总量，进度会一路涨过 100%。
    pub(crate) fn add_total(&self, bytes: u64) {
        if bytes > 0 {
            self.total_bytes.fetch_add(bytes, Ordering::Relaxed);
        }
    }

    /// 把已经加过的字节减回去。
    ///
    /// 只在一个场景用：某次尝试中途失败，它累加的字节必须撤销，
    /// 否则重试会再从头加一遍，进度会冲到 100% 以上（实测出现过 250MB / 143MB）。
    pub(crate) fn sub_bytes(&self, bytes: u64) {
        // `fetch_sub` 会回绕成天文数字，所以先夹一下。
        // 正常不会越界（加了多少减多少），这只是防御。
        let _ = self
            .done_bytes
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
                Some(current.saturating_sub(bytes))
            });
    }

    fn finish_file(&self) {
        self.finished_files.fetch_add(1, Ordering::Relaxed);
    }

    fn file_downloaded(&self) {
        self.downloaded_files.fetch_add(1, Ordering::Relaxed);
        self.finish_file();
    }

    fn file_skipped(&self) {
        self.skipped_files.fetch_add(1, Ordering::Relaxed);
        self.finish_file();
    }

    fn file_failed(&self) {
        self.failed_files.fetch_add(1, Ordering::Relaxed);
        self.finish_file();
    }

    pub fn snapshot(&self) -> Snapshot {
        Snapshot {
            total_bytes: self.total_bytes.load(Ordering::Relaxed),
            done_bytes: self.done_bytes.load(Ordering::Relaxed),
            total_files: self.total_files.load(Ordering::Relaxed),
            finished_files: self.finished_files.load(Ordering::Relaxed),
            downloaded_files: self.downloaded_files.load(Ordering::Relaxed),
            skipped_files: self.skipped_files.load(Ordering::Relaxed),
            failed_files: self.failed_files.load(Ordering::Relaxed),
        }
    }
}

/// 单个文件在全局进度账上的净贡献。
///
/// 进度是「收到一块就加一块」累加的，而中途失败重试、换源、分片重发都可能把
/// 同一段字节写两遍。之前的做法是每个失败分支手工退款，只要有一处口径对不上，
/// 账上就会多出永远退不掉的字节 —— 实测出现过 601 MB / 583 MB。
///
/// 所以每个文件另记一本账：全局的每一次加减都同步落到这里，文件收尾时按
/// 「它最终应该贡献多少」（成功 = 清单声明的大小，失败 = 0）一次性多退少补。
/// 这样 `done_bytes` 天然不会超过 `total_bytes`，不用再依赖各分支退款的正确性。
#[derive(Debug)]
pub struct FileLedger {
    progress: Arc<Progress>,
    /// 本文件目前已经记进全局进度的量（可为 0，不会为负）。
    added: AtomicU64,
}

impl FileLedger {
    pub fn new(progress: Arc<Progress>) -> Arc<Self> {
        Arc::new(Self {
            progress,
            added: AtomicU64::new(0),
        })
    }

    /// 本文件已记进全局进度的字节数。
    pub fn added(&self) -> u64 {
        self.added.load(Ordering::Relaxed)
    }

    pub(crate) fn add(&self, bytes: u64) {
        if bytes == 0 {
            return;
        }
        self.added.fetch_add(bytes, Ordering::Relaxed);
        self.progress.add_bytes(bytes);
    }

    /// 撤销本文件已经记过的字节（不能超过它自己加过的量）。
    pub(crate) fn sub(&self, bytes: u64) {
        if bytes == 0 {
            return;
        }
        let prev = self
            .added
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
                Some(current.saturating_sub(bytes))
            })
            .unwrap_or(0);
        self.progress.sub_bytes(prev.min(bytes));
    }

    /// 把本文件的贡献结算到 `target`。多退少补，只做一次。
    pub fn settle(&self, target: u64) {
        let added = self.added.swap(target, Ordering::Relaxed);
        match target.cmp(&added) {
            std::cmp::Ordering::Greater => self.progress.add_bytes(target - added),
            std::cmp::Ordering::Less => self.progress.sub_bytes(added - target),
            std::cmp::Ordering::Equal => {}
        }
    }
}

// ---------- 任务 ----------

#[derive(Debug, Clone)]
pub struct DownloadItem {
    /// 展示名（一般是文件名）
    pub name: String,
    /// 候选地址，按顺序尝试（整文件单连接用全部候选）
    pub urls: Vec<String>,
    /// 允许走分片的候选子集，由上层按实际 URL 过滤。
    /// 空 = 这个文件不分片；非空时只会用这里的地址发 Range 请求。
    pub split_urls: Vec<String>,
    /// 最终落盘路径
    pub dest: PathBuf,
    /// 期望的 sha1（小写十六进制）。`None` 时只做存在性/大小检查
    pub sha1: Option<String>,
    /// 期望大小，0 表示未知
    pub size: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileOutcome {
    Downloaded,
    Skipped,
    Failed,
}

#[derive(Debug, Default)]
pub struct DownloadSummary {
    pub downloaded: usize,
    pub skipped: usize,
    pub failed: usize,
    pub cancelled: bool,
    /// 失败项的名称，便于界面提示"哪些没下来"
    pub failures: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct DownloadOptions {
    /// 并发任务数
    pub concurrency: usize,
    /// 单个地址最多试几次（含首次）
    pub attempts_per_url: u32,
    /// 大文件分片参数。`max_connections <= 1` 即整体关闭分片。
    pub split: SplitConfig,
}

impl Default for DownloadOptions {
    fn default() -> Self {
        Self {
            concurrency: 64,
            attempts_per_url: 2,
            split: SplitConfig::default(),
        }
    }
}

/// 并发执行一批下载任务。
///
/// 单个文件失败不会中止整体；调用方根据 `summary` 决定是否提示重试。
pub async fn run(
    items: Vec<DownloadItem>,
    fetcher: Arc<dyn Fetcher>,
    progress: Arc<Progress>,
    options: DownloadOptions,
    cancel: CancellationToken,
) -> DownloadSummary {
    // 动手前先收拾上次的残局：进程被强杀 / 窗口被关时，`cleanup()` 来不及跑，
    // 半截文件会留在数据目录里。这里是唯一的入口，清一次就够。
    purge_stale_parts(&items).await;

    let semaphore = Arc::new(Semaphore::new(options.concurrency.max(1)));

    // 分片连接池：全局独立一个，不从主池借许可 ——
    // 借的话 concurrency=8 时八个大文件各自要 7 个额外许可，谁都在等谁，直接死锁。
    // 单文件最多 `max_connections` 条，全局最多 `total_connections` 条。
    let split_permits = Arc::new(Semaphore::new(options.split.total_connections.max(1)));

    let mut tasks: JoinSet<(String, FileOutcome)> = JoinSet::new();

    for item in items {
        // 先拿许可再投递：许可用完就等，天然形成背压。
        // 等许可的过程也要能被取消打断，否则取消后还要慢吞吞放完队列。
        let permit = tokio::select! {
            biased;
            _ = cancel.cancelled() => break,
            permit = semaphore.clone().acquire_owned() => match permit {
                Ok(permit) => permit,
                Err(_) => break,
            },
        };
        if cancel.is_cancelled() {
            break;
        }

        let fetcher = Arc::clone(&fetcher);
        let progress = Arc::clone(&progress);
        let cancel = cancel.clone();
        let attempts = options.attempts_per_url;
        let split_cfg = options.split;
        let split_permits = Arc::clone(&split_permits);

        tasks.spawn(async move {
            let result = process_item(
                item,
                fetcher,
                progress,
                cancel,
                attempts,
                split_cfg,
                split_permits,
            )
            .await;
            drop(permit);
            result
        });
    }

    let mut summary = DownloadSummary::default();
    while let Some(joined) = tasks.join_next().await {
        match joined {
            Ok((name, outcome)) => match outcome {
                FileOutcome::Downloaded => summary.downloaded += 1,
                FileOutcome::Skipped => summary.skipped += 1,
                FileOutcome::Failed => {
                    summary.failed += 1;
                    summary.failures.push(name);
                }
            },
            Err(err) => {
                summary.failed += 1;
                summary.failures.push(format!("任务异常：{err}"));
            }
        }
    }
    summary.cancelled = cancel.is_cancelled();
    summary
}

async fn process_item(
    item: DownloadItem,
    fetcher: Arc<dyn Fetcher>,
    progress: Arc<Progress>,
    cancel: CancellationToken,
    attempts_per_url: u32,
    split_cfg: SplitConfig,
    split_permits: Arc<Semaphore>,
) -> (String, FileOutcome) {
    if cancel.is_cancelled() {
        progress.file_failed();
        return (item.name, FileOutcome::Failed);
    }

    if is_up_to_date(&item).await {
        // 跳过也要计入字节，否则进度永远差一截
        progress.add_bytes(item.size);
        progress.file_skipped();
        return (item.name, FileOutcome::Skipped);
    }

    // 这个文件单独一本账：内部所有加减都过它，收尾再按最终结果结算一次。
    let ledger = FileLedger::new(Arc::clone(&progress));
    let outcome = fetch_file(
        &item,
        &fetcher,
        &ledger,
        &cancel,
        attempts_per_url,
        split_cfg,
        &split_permits,
    )
    .await;

    // 结算：下好了按清单声明的大小记账（清单没给大小就按实际写入量并补进总量），
    // 没下好一律清零 —— 那半截文件已经删了，不该继续占着进度。
    let target = match outcome {
        FileOutcome::Downloaded if item.size > 0 => item.size,
        FileOutcome::Downloaded => {
            let actual = ledger.added();
            progress.add_total(actual);
            actual
        }
        _ => 0,
    };
    ledger.settle(target);

    match outcome {
        FileOutcome::Downloaded => progress.file_downloaded(),
        FileOutcome::Skipped => progress.file_skipped(),
        FileOutcome::Failed => progress.file_failed(),
    }
    (item.name, outcome)
}

/// 真正干活的部分：只管把文件弄下来，不碰 `finished_*` 那几个计数。
async fn fetch_file(
    item: &DownloadItem,
    fetcher: &Arc<dyn Fetcher>,
    ledger: &Arc<FileLedger>,
    cancel: &CancellationToken,
    attempts_per_url: u32,
    split_cfg: SplitConfig,
    split_permits: &Arc<Semaphore>,
) -> FileOutcome {
    let part = part_path(&item.dest);
    // 登记「这个 part 我正在写」：别的安装任务启动清理时要跳过它
    let _active = ActivePart::register(part.clone());
    let max_attempts = attempts_per_url.max(1);

    // 大文件先试分片。分片是加速手段，不是必需品 —— 任何异常都退回单连接，
    // 保证「总能下下来，只是可能慢一点」。
    // `split_urls` 为空表示上层判定这个文件的真实 URL 不适合分片（如 BMCLAPI）。
    if !item.split_urls.is_empty() && split_cfg.eligible(item.size) {
        match download_split(
            item,
            fetcher,
            ledger,
            cancel,
            max_attempts,
            split_cfg,
            split_permits,
            &part,
        )
        .await
        {
            SplitOutcome::Done => return FileOutcome::Downloaded,
            SplitOutcome::Canceled | SplitOutcome::Failed => {
                cleanup(&part).await;
                return FileOutcome::Failed;
            }
            SplitOutcome::Fallback => {}
        }
    }

    for url in &item.urls {
        // 逐次重试。有进展的失败不算次数：续传每失败一次，断点都能往前推一段，
        // 说明这条链路是通的就是慢/抖，不该在第 2 次就判死 —— 小文件在慢链路上
        // 「永远差最后 50KB」就是这个原因。`MAX_ATTEMPTS_PER_URL` 兜住
        // 「每次只进 1 字节」的极端情况，不让任务挂死。
        let mut failures = 0u32;
        let mut attempts = 0u32;
        let mut reached = existing_len(&part).await;
        loop {
            if cancel.is_cancelled() {
                refund_part(&part, ledger).await;
                cleanup(&part).await;
                return FileOutcome::Failed;
            }
            attempts += 1;

            match fetcher
                .fetch_to(
                    url,
                    &part,
                    item.size,
                    Arc::clone(ledger),
                    cancel.clone(),
                )
                .await
            {
                Ok(_) => {
                    if checksum_ok(&part, item.sha1.as_deref()).await {
                        if promote(&part, &item.dest).await.is_err() {
                            // 字节下全了但文件归不了位：这个文件算失败。
                            // 结算那一层会把本文件的贡献清零，这里不用手工退款。
                            cleanup(&part).await;
                            return FileOutcome::Failed;
                        }
                        return FileOutcome::Downloaded;
                    }

                    // 内容不对：删掉临时文件，直接换下一个源。
                    debug!(url, "校验失败，换下一个源");
                    cleanup(&part).await;
                    break;
                }
                Err(err) if err.code == ErrorCode::Canceled => {
                    refund_part(&part, ledger).await;
                    cleanup(&part).await;
                    return FileOutcome::Failed;
                }
                Err(err) => {
                    debug!(url, attempts, error = %err, "下载失败");
                    // 不删半截文件、不退款：`fetch_to` 已经把字节写进 `.xclpart`
                    // 并记了进度，下一次尝试从断点续传。以前这里一失败就 `cleanup`，
                    // 等于每断一次都从头再下 —— 小文件在慢链路下永远下不完，
                    // 大文件靠分片（16 连接 + 只补缺口）侥幸躲过。
                    let now = existing_len(&part).await;
                    if now > reached {
                        reached = now;
                        failures = 0; // 有进展：这次失败是「断了一下」，不算打空
                    } else {
                        failures += 1;
                    }
                    if failures >= max_attempts || attempts >= MAX_ATTEMPTS_PER_URL {
                        break;
                    }
                    backoff(failures).await;
                }
            }
        }
    }

    // 所有地址都试完了：盘上那半截作废，进度里记的也要撤掉
    refund_part(&part, ledger).await;
    cleanup(&part).await;
    FileOutcome::Failed
}

/// 按半截文件的实际长度退款：`fetch_to` 是「写多少记多少」，
/// 所以盘上有多少字节，进度里就记了多少。
///
/// 这只是让中途数字别虚高；真正兜底的是 [`FileLedger::settle`]。
async fn refund_part(part: &Path, ledger: &FileLedger) {
    let kept = existing_len(part).await;
    if kept > 0 {
        ledger.sub(kept);
    }
}

/// 文件当前长度，不存在算 0。
async fn existing_len(path: &Path) -> u64 {
    tokio::fs::metadata(path)
        .await
        .map(|meta| meta.len())
        .unwrap_or(0)
}

/// 两次尝试之间的退避。
///
/// 以前是零间隔连打同一个地址：对方正在抖，立刻重连只是再撞一次。
/// 递增到 1.2 秒封顶，够躲开瞬时拥塞，又不至于让人觉得卡住。
async fn backoff(attempt: u32) {
    let delay = Duration::from_millis(400)
        .saturating_mul(attempt.saturating_add(1))
        .min(Duration::from_millis(1200));
    tokio::time::sleep(delay).await;
}

// ---------- 分片下载 ----------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SplitOutcome {
    /// 全部字节到位，已校验并 rename
    Done,
    /// 分片路线不通，请上层用单连接重下
    Fallback,
    /// 手动取消
    Canceled,
    /// 真的失败了（重试也没用）
    Failed,
}

/// 一次分片请求的结局（在 worker 任务里产生，由协调循环消费）。
enum PieceResult {
    /// 这一段彻底拿到了
    Done,
    /// 手动取消
    Canceled,
    /// 服务器不支持 Range，整体降级
    Unsupported,
    /// 这次失败；已写的字节已经记账，可以再试
    Retry,
}

/// 分片路线的总入口。
///
/// 和老实现的三个本质区别：
/// 1. 不是一个碎片一个文件，而是一个预分配的 `.xclpart`，各请求 seek 后直写，
///    省掉最后拼接那一轮完整读写；
/// 2. 不是静态切 N 片，而是按「剩余字节 / 可用连接」动态派发（见 [`split`]）；
/// 3. 失败不丢已下的字节：只把没拿到的区间还回调度器，下次只补缺口。
///
/// 租约分两段：`assign` 摘走区间，`commit` 把完成的部分记账、剩下的还回去。
async fn download_split(
    item: &DownloadItem,
    fetcher: &Arc<dyn Fetcher>,
    ledger: &Arc<FileLedger>,
    cancel: &CancellationToken,
    attempts: u32,
    cfg: SplitConfig,
    permits: &Arc<Semaphore>,
    part: &Path,
) -> SplitOutcome {
    // 片大小跟着文件走：小文件 128KiB，大文件最多 4MiB。
    // HTTP 请求仍由 `max_request_size` 封顶，不会因为片变小而变碎。
    let cfg = cfg.for_total(item.size);

    // 预分配：先把文件撑到最终大小，之后每个请求 seek 到自己的位置写。
    // 好处是没有碎片文件、不用合并，143MB 能省一轮完整读写。
    if let Err(err) = preallocate(part, item.size).await {
        debug!(error = %err, "预分配失败，回退单连接");
        return SplitOutcome::Fallback;
    }

    let Some(scheduler) = PieceScheduler::new(item.size, cfg) else {
        return SplitOutcome::Fallback;
    };
    let sched = Arc::new(Mutex::new(scheduler));

    let worker_limit = cfg.max_connections.max(1);
    let give_up_at = attempts.max(1).saturating_mul(3).saturating_add(3);

    let mut set: JoinSet<PieceResult> = JoinSet::new();
    let mut active = 0usize;
    // 速度驱动的窗口：起步就有 4 条，之后每 250ms 看一次本文件实测速度，
    // 单连接低于 256KB/s 就翻倍，直到 worker_limit。
    // 慢链路会在 0.5 秒内加到满，这正是按连接限速的源最需要的行为。
    let mut permitted = INITIAL_SPLIT_CONNECTIONS.min(worker_limit).max(1);
    let mut url_index = 0usize;
    let mut consecutive_failures = 0u32;
    let mut canceled = false;
    let mut unsupported = false;

    // 本文件累计收到的字节。进度是全局的，不能拿来算单文件速度。
    let meter = Arc::new(AtomicU64::new(0));
    // 本次尝试实际写盘的总字节。
    //
    // 进度是按「写入量」累加的（`fetcher` 收到一个 chunk 就 add 一次），所以回滚也必须
    // 按写入量撤，不能按 `confirmed_bytes()` 撤 —— 后者只算唯一已确认的前缀：
    // 失败重试时重复写过的字节、以及写了却没被 `commit` 认领的那一截都不在里面，
    // 按它回滚就会少撤，进度一路涨过 100%（实测 611 MB / 585 MB）。
    let written_total = Arc::new(AtomicU64::new(0));
    let mut last_meter = 0u64;
    let mut last_ramp = Instant::now();
    let mut ramp = tokio::time::interval(SPLIT_RAMP_INTERVAL);
    ramp.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    // `interval` 的第一次 tick 是立即就绪的，先消费掉，免得进循环立刻误判一次
    ramp.tick().await;

    'outer: loop {
        // 1. 派活：受两个上限约束 —— 本文件的 permitted、以及全局分片连接池
        while active < permitted {
            // 全局池满时要等，不能直接放弃：
            // 同时有 5 个大文件在分片时，第 5 个拿不到许可是常态，
            // 若在这里 break，它会被判成「分片走不通」而退回单连接。
            let global = match permits.clone().try_acquire_owned() {
                Ok(permit) => permit,
                Err(_) if active == 0 => {
                    // 第一条连接就抢不到全局许可：说明分片池被别的文件占满了。
                    // 这里不能无限排队 —— 一批 1–2MB 的文件会互相堵死；
                    // 等一小会儿还拿不到就退回单连接，用文件级并发先跑起来。
                    let acquired = tokio::select! {
                        biased;
                        _ = cancel.cancelled() => {
                            canceled = true;
                            break 'outer;
                        }
                        result = tokio::time::timeout(
                            SPLIT_POOL_WAIT,
                            permits.clone().acquire_owned(),
                        ) => result,
                    };
                    match acquired {
                        Ok(Ok(permit)) => permit,
                        Ok(Err(_)) => break 'outer,
                        Err(_) => {
                            debug!(name = %item.name, "分片连接池繁忙，退回单连接");
                            return SplitOutcome::Fallback;
                        }
                    }
                }
                Err(_) => {
                    tokio::select! {
                        biased;
                        _ = cancel.cancelled() => {
                            canceled = true;
                            break 'outer;
                        }
                        permit = permits.clone().acquire_owned() => match permit {
                            Ok(permit) => permit,
                            // 信号量被关闭只会在整体收尾时发生
                            Err(_) => break 'outer,
                        },
                    }
                }
            };
            let assignment = {
                let mut guard = sched.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                guard.assign(active, permitted - active)
            };
            let Some(assignment) = assignment else {
                drop(global);
                break;
            };

            let url = {
                let urls = &item.split_urls;
                if urls.is_empty() {
                    drop(global);
                    break;
                }
                urls[url_index % urls.len()].clone()
            };

            let fetcher = Arc::clone(fetcher);
            let ledger = Arc::clone(ledger);
            let meter = Arc::clone(&meter);
            let written_total = Arc::clone(&written_total);
            let cancel = cancel.clone();
            let sched = Arc::clone(&sched);
            let part = part.to_path_buf();

            set.spawn(async move {
                let _global = global;
                // 调度器用半开区间，HTTP Range 用闭区间，只在这一个地方转换
                let range = ByteRange::new(assignment.start, assignment.end.saturating_sub(1));
                let outcome = fetcher
                    .fetch_at(&url, &part, range, ledger, meter, cancel)
                    .await;

                // 先记「这次到底写了多少」再决定成败：成败两个分支都要记，
                // 失败时已经 add 进进度的字节同样是这部分。
                let written = match &outcome {
                    Ok(written) => *written,
                    Err(failure) => failure.written,
                };
                written_total.fetch_add(written, Ordering::Relaxed);

                let mut guard = sched.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                match outcome {
                    Ok(written) => {
                        guard.commit(&assignment.leases, written);
                        PieceResult::Done
                    }
                    Err(failure) => {
                        // 关键：已写的字节照样记账，只把缺口还回去
                        guard.commit(&assignment.leases, failure.written);
                        match failure.error.code {
                            ErrorCode::Canceled => PieceResult::Canceled,
                            ErrorCode::Unsupported => PieceResult::Unsupported,
                            _ => PieceResult::Retry,
                        }
                    }
                }
            });
            active += 1;
        }

        // 2. 没有在飞的、也派不出活 → 结束
        if active == 0 {
            break;
        }

        // 3. 等一个回来，或等扩容检查 —— 两件事都要能打断对方
        let joined = tokio::select! {
            biased;
            _ = cancel.cancelled() => {
                canceled = true;
                sched
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .stop();
                break;
            }
            joined = set.join_next() => joined,
            _ = ramp.tick() => {
                let now = Instant::now();
                let elapsed = now.duration_since(last_ramp).as_secs_f64();
                let current = meter.load(Ordering::Relaxed);
                let delta = current.saturating_sub(last_meter);
                last_meter = current;
                last_ramp = now;

                let remaining = sched
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .remaining_bytes();
                let per_connection = if elapsed > 0.0 {
                    delta as f64 / elapsed / active.max(1) as f64
                } else {
                    0.0
                };
                if remaining > 0
                    && permitted < worker_limit
                    && per_connection < SPLIT_TARGET_PER_CONNECTION_BPS as f64
                {
                    let next = (permitted * 2).min(worker_limit);
                    debug!(
                        name = %item.name,
                        active,
                        from = permitted,
                        to = next,
                        bytes_per_sec = delta as f64 / elapsed.max(0.001),
                        "分片速度不足，扩容"
                    );
                    permitted = next;
                }
                continue;
            }
        };
        let Some(joined) = joined else {
            break;
        };
        active -= 1;
        match joined {
            Ok(PieceResult::Done) => {
                consecutive_failures = 0;
            }
            Ok(PieceResult::Canceled) => {
                canceled = true;
                sched
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .stop();
            }
            Ok(PieceResult::Unsupported) => {
                unsupported = true;
                sched
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .stop();
            }
            Ok(PieceResult::Retry) | Err(_) => {
                consecutive_failures += 1;
                // 一条连接碰到瞬时错误，不该把另外十五条一起停掉。
                // 连续两次失败才收缩一半，交给扩容循环重新爬。
                if consecutive_failures >= 2 {
                    permitted = (permitted / 2).max(1);
                }
                url_index = url_index.wrapping_add(1); // 换下一个源
                if consecutive_failures >= give_up_at {
                    sched
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .stop();
                }
            }
        }
    }

    // 等在飞的请求自然结束。不能 abort —— 中途打断会让「进度加了、账没记」
    // 两边对不上，那正是之前进度条超过 100% 的根因。
    while set.join_next().await.is_some() {}

    let done = {
        let guard = sched.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        guard.is_done()
    };
    // 回滚一律按「实际写盘量」撤：进度是按写入量加的，销账就得按同一个口径。
    let written = written_total.load(Ordering::Relaxed);

    if canceled {
        ledger.sub(written);
        cleanup(part).await;
        return SplitOutcome::Canceled;
    }

    if !done {
        // 没走通：把这些字节从进度里撤掉（单连接会重下一遍），然后降级
        ledger.sub(written);
        cleanup(part).await;
        debug!(
            name = %item.name,
            unsupported,
            written,
            "分片未完成，回退单连接"
        );
        return SplitOutcome::Fallback;
    }

    // 把各句柄写下的内容刷到磁盘，再验长度和 sha1
    if let Err(err) = sync_file(part).await {
        debug!(error = %err, "分片结果刷盘失败，回退单连接");
        ledger.sub(written);
        cleanup(part).await;
        return SplitOutcome::Fallback;
    }

    if item.size > 0 {
        match tokio::fs::metadata(part).await {
            Ok(meta) if meta.len() == item.size => {}
            Ok(meta) => {
                debug!(
                    expected = item.size,
                    actual = meta.len(),
                    "分片结果长度不符，回退单连接"
                );
                ledger.sub(written);
                cleanup(part).await;
                return SplitOutcome::Fallback;
            }
            Err(err) => {
                debug!(error = %err, "读取分片结果失败");
                ledger.sub(written);
                cleanup(part).await;
                return SplitOutcome::Fallback;
            }
        }
    }

    // 校验失败同样退回单连接：宁可慢一次，也别把坏文件交出去。
    // 分片逻辑本身若有 bug，这条兜底能保证功能不坏。
    if !checksum_ok(part, item.sha1.as_deref()).await {
        debug!(name = %item.name, "分片结果校验失败，回退单连接");
        ledger.sub(written);
        cleanup(part).await;
        return SplitOutcome::Fallback;
    }

    if promote(part, &item.dest).await.is_err() {
        // 字节已经写出来了，但文件归不了位：这个文件算失败，进度里那部分也要撤回，
        // 否则一个失败的大文件就能把全局进度顶到 100% 以上。
        ledger.sub(written);
        cleanup(part).await;
        return SplitOutcome::Failed;
    }
    SplitOutcome::Done
}

/// 把临时文件预先撑到最终大小，供多个请求按偏移写。
async fn preallocate(path: &Path, size: u64) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    // truncate 是必须的：上一轮残留的 `.xclpart` 内容必须作废，
    // 否则「跳过语义」会把旧字节和新字节混在一起
    let file = tokio::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(path)
        .await?;
    file.set_len(size).await?;
    file.sync_all().await?;
    Ok(())
}

async fn sync_file(path: &Path) -> std::io::Result<()> {
    let file = tokio::fs::OpenOptions::new().write(true).open(path).await?;
    file.sync_all().await
}


/// 把 `.xclpart` 挪到正式位置。
async fn promote(part: &Path, dest: &Path) -> std::io::Result<()> {
    if let Some(parent) = dest.parent() {
        let _ = tokio::fs::create_dir_all(parent).await;
    }
    tokio::fs::rename(part, dest).await
}

/// 文件是否已经是最新的。
///
/// 语义对齐参考实现：有 sha1 就比对内容；`sha1` 缺省且 size 已知时比大小；
/// 两者都没有时，文件存在即认为可用。
async fn is_up_to_date(item: &DownloadItem) -> bool {
    let Ok(meta) = tokio::fs::metadata(&item.dest).await else {
        return false;
    };
    if !meta.is_file() {
        return false;
    }

    match item.sha1.as_deref() {
        Some(expected) => sha1_file(&item.dest)
            .await
            .map(|actual| actual.eq_ignore_ascii_case(expected))
            .unwrap_or(false),
        None if item.size > 0 => meta.len() == item.size,
        None => true,
    }
}

async fn checksum_ok(path: &Path, expected: Option<&str>) -> bool {
    let Some(expected) = expected else {
        return true;
    };
    sha1_file(path)
        .await
        .map(|actual| actual.eq_ignore_ascii_case(expected))
        .unwrap_or(false)
}

/// 同时最多算几份 sha1。
///
/// 哈希是纯 CPU 活：多文件下载时每条连接收尾都要算一整份，并发一高就把 CPU
/// 占满，WebView 的合成与动画跟着掉帧（实测「单文件没事、文件一多就卡」）。
/// 限制成 2 份并发：既不把 CPU 打满，也不会让校验拖慢整体下载。
const HASH_CONCURRENCY: usize = 2;

fn hash_gate() -> Arc<Semaphore> {
    static GATE: OnceLock<Arc<Semaphore>> = OnceLock::new();
    GATE.get_or_init(|| Arc::new(Semaphore::new(HASH_CONCURRENCY)))
        .clone()
}

async fn sha1_file(path: &Path) -> std::io::Result<String> {
    use std::io::Read;

    let gate = hash_gate();
    let permit = gate
        .acquire_owned()
        .await
        .map_err(|_| std::io::Error::other("哈希限流器已关闭"))?;
    let path = path.to_path_buf();

    // 读 + 哈希都放阻塞线程：不占 tokio 的异步 worker，也不和 IPC / 事件推送抢
    let joined = tokio::task::spawn_blocking(move || -> std::io::Result<String> {
        let _permit = permit; // 阻塞线程里持有直到算完
        let mut file = std::fs::File::open(&path)?;
        let mut hasher = Sha1::new();
        let mut buffer = vec![0u8; 64 * 1024];
        loop {
            let read = file.read(&mut buffer)?;
            if read == 0 {
                break;
            }
            hasher.update(&buffer[..read]);
        }
        Ok(hasher
            .finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect())
    })
    .await;

    match joined {
        Ok(result) => result,
        Err(err) => Err(std::io::Error::other(err)),
    }
}

fn part_path(dest: &Path) -> PathBuf {
    let mut raw = dest.as_os_str().to_os_string();
    raw.push(PART_SUFFIX);
    PathBuf::from(raw)
}

async fn cleanup(part: &Path) {
    let _ = tokio::fs::remove_file(part).await;
}

// ---------- 陈旧半截文件的清理 ----------

/// 进程内「正在写」的半截文件计数表。
///
/// 为什么需要：同一个 root 下不同版本会共享 `libraries/` 与 `assets/`，
/// 两个安装任务完全可能同时命中同一个目标文件。A 正在写它的 `.xclpart` 时，
/// B 启动清理不能把它当垃圾删掉 —— 那就成了自己人打自己人。
///
/// 用计数而不是集合：A、B 都登记了同一路径时，先结束的那个不能把
/// 还在写的另一个的保护撤掉。
fn active_parts() -> &'static Mutex<HashMap<PathBuf, u32>> {
    static ACTIVE: OnceLock<Mutex<HashMap<PathBuf, u32>>> = OnceLock::new();
    ACTIVE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// 半截文件的占位凭据，离开作用域自动注销。
struct ActivePart(PathBuf);

impl ActivePart {
    fn register(part: PathBuf) -> Self {
        if let Ok(mut map) = active_parts().lock() {
            *map.entry(part.clone()).or_insert(0) += 1;
        }
        Self(part)
    }
}

impl Drop for ActivePart {
    fn drop(&mut self) {
        if let Ok(mut map) = active_parts().lock() {
            if let Some(count) = map.get_mut(&self.0) {
                *count -= 1;
                if *count == 0 {
                    map.remove(&self.0);
                }
            }
        }
    }
}

/// 清理一批计划文件旁边的陈旧半截文件，返回实际删掉的数量。
///
/// 下载任务开始时清一次缓存，
/// 但比「整个目录全删」精确：
/// 我们只清理本次计划涉及的那些路径，不会碰到别人的文件。
///
/// 为什么非清不可：`.xclpart` 刻意写在目标文件同目录（跨目录 rename 不是
/// 原子操作，可能退化成拷贝），代价就是进程被强杀、窗口被关、dev 热重载
/// 这些「来不及执行 cleanup」的场景会在数据目录里永久留下半截文件。
/// 这些残留不会被任何逻辑当成正式文件，但会白占空间，而且容易让人误以为
/// 下载卡住了。
///
/// 只删 `.xclpart`，不碰正式文件；删不掉（被占用、权限）就跳过，不报错。
pub async fn purge_stale_parts(items: &[DownloadItem]) -> usize {
    // 先取一份快照就放锁：持有 std Mutex 跨越 await 是不允许的（也不是好习惯）
    let busy: HashSet<PathBuf> = match active_parts().lock() {
        Ok(map) => map.keys().cloned().collect(),
        Err(poisoned) => poisoned.into_inner().keys().cloned().collect(),
    };

    let candidates: Vec<PathBuf> = items
        .iter()
        .map(|item| part_path(&item.dest))
        .filter(|part| !busy.contains(part))
        .collect();

    let removed: Vec<usize> = stream::iter(candidates)
        .map(|part| async move {
            match tokio::fs::remove_file(&part).await {
                Ok(()) => 1,
                // 绝大多数情况是「压根不存在」，不是错误
                Err(_) => 0,
            }
        })
        .buffer_unordered(PURGE_CONCURRENCY)
        .collect()
        .await;

    removed.into_iter().sum()
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::future::BoxFuture;
    use std::sync::atomic::AtomicBool;
    use std::time::Duration;
    use xcl_core::AppError;

    /// 假传输层：把固定内容写进目标文件，可配置延迟、失败、Range 支持。
    struct FakeFetcher {
        payload: Vec<u8>,
        delay_ms: u64,
        always_fail: AtomicBool,
        /// 前 N 次调用失败，用来验证「失败重试后进度不许超 100%」
        fail_first: AtomicUsize,
        /// 是否支持分段（false 时对 Range 请求返回 Unsupported）
        supports_range: AtomicBool,
        calls: AtomicUsize,
        active: AtomicUsize,
        max_active: AtomicUsize,
    }

    impl FakeFetcher {
        fn new(payload: &[u8]) -> Arc<Self> {
            Self::build(payload, 0)
        }

        fn with_delay(payload: &[u8], delay_ms: u64) -> Arc<Self> {
            Self::build(payload, delay_ms)
        }

        fn build(payload: &[u8], delay_ms: u64) -> Arc<Self> {
            Arc::new(Self {
                payload: payload.to_vec(),
                delay_ms,
                always_fail: AtomicBool::new(false),
                fail_first: AtomicUsize::new(0),
                supports_range: AtomicBool::new(true),
                calls: AtomicUsize::new(0),
                active: AtomicUsize::new(0),
                max_active: AtomicUsize::new(0),
            })
        }

        fn without_range(payload: &[u8]) -> Arc<Self> {
            let fetcher = Self::build(payload, 0);
            fetcher.supports_range.store(false, Ordering::SeqCst);
            fetcher
        }

        fn failing_first(payload: &[u8], times: usize) -> Arc<Self> {
            let fetcher = Self::build(payload, 0);
            fetcher.fail_first.store(times, Ordering::SeqCst);
            fetcher
        }
    }

    impl Fetcher for FakeFetcher {
        fn fetch_to<'a>(
            &'a self,
            _url: &'a str,
            dest: &'a Path,
            _expected_total: u64,
            ledger: Arc<FileLedger>,
            _cancel: CancellationToken,
        ) -> BoxFuture<'a, Result<u64, AppError>> {
            Box::pin(async move {
                let current = self.active.fetch_add(1, Ordering::SeqCst) + 1;
                self.max_active.fetch_max(current, Ordering::SeqCst);
                self.calls.fetch_add(1, Ordering::SeqCst);

                if self.delay_ms > 0 {
                    tokio::time::sleep(Duration::from_millis(self.delay_ms)).await;
                }

                // fetch_update 返回 Ok 表示之前还有剩余的失败次数
                let forced_fail = self
                    .fail_first
                    .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1))
                    .is_ok();

                let result = if forced_fail || self.always_fail.load(Ordering::SeqCst) {
                    Err(AppError::network("模拟失败", "boom"))
                } else {
                    // 模拟真实 fetcher 的续传：盘上有多少就从那之后接着写，
                    // 只把新写的字节记进进度，返回盘上的总长度。
                    let existing = tokio::fs::metadata(dest)
                        .await
                        .map(|meta| meta.len() as usize)
                        .unwrap_or(0)
                        .min(self.payload.len());
                    let tail = &self.payload[existing..];
                    let mut file = tokio::fs::OpenOptions::new()
                        .create(true)
                        .write(true)
                        .truncate(existing == 0)
                        .open(dest)
                        .await;
                    match &mut file {
                        Ok(file) => {
                            if existing > 0 {
                                let _ = tokio::io::AsyncSeekExt::seek(
                                    file,
                                    std::io::SeekFrom::Start(existing as u64),
                                )
                                .await;
                            }
                            match tokio::io::AsyncWriteExt::write_all(file, tail).await {
                                Ok(()) => {
                                    ledger.add(tail.len() as u64);
                                    Ok(self.payload.len() as u64)
                                }
                                Err(e) => Err(AppError::io(&e, "写文件")),
                            }
                        }
                        Err(e) => Err(AppError::io(e, "写文件")),
                    }
                };

                self.active.fetch_sub(1, Ordering::SeqCst);
                result
            })
        }

        fn fetch_at<'a>(
            &'a self,
            _url: &'a str,
            part: &'a Path,
            range: ByteRange,
            ledger: Arc<FileLedger>,
            meter: Arc<AtomicU64>,
            _cancel: CancellationToken,
        ) -> BoxFuture<'a, Result<u64, PieceFailure>> {
            Box::pin(async move {
                let current = self.active.fetch_add(1, Ordering::SeqCst) + 1;
                self.max_active.fetch_max(current, Ordering::SeqCst);
                self.calls.fetch_add(1, Ordering::SeqCst);

                if self.delay_ms > 0 {
                    tokio::time::sleep(Duration::from_millis(self.delay_ms)).await;
                }
                self.active.fetch_sub(1, Ordering::SeqCst);

                let forced_fail = self
                    .fail_first
                    .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1))
                    .is_ok();
                if forced_fail || self.always_fail.load(Ordering::SeqCst) {
                    return Err(PieceFailure::untouched(AppError::network(
                        "模拟失败",
                        "boom",
                    )));
                }
                if !self.supports_range.load(Ordering::SeqCst) {
                    return Err(PieceFailure::untouched(AppError::unsupported(
                        "模拟不支持分段",
                        "ignore range",
                    )));
                }

                let start = (range.start as usize).min(self.payload.len());
                let end = ((range.end as usize) + 1).min(self.payload.len());
                if start >= end {
                    return Err(PieceFailure::untouched(AppError::unsupported(
                        "模拟区间越界",
                        "416",
                    )));
                }
                let slice = &self.payload[start..end];

                let mut file = match tokio::fs::OpenOptions::new().write(true).open(part).await {
                    Ok(file) => file,
                    Err(e) => {
                        return Err(PieceFailure::untouched(AppError::io(&e, "打开临时文件")))
                    }
                };
                use tokio::io::{AsyncSeekExt, AsyncWriteExt};
                if let Err(e) = file.seek(std::io::SeekFrom::Start(range.start)).await {
                    return Err(PieceFailure::untouched(AppError::io(&e, "定位写入偏移")));
                }
                if let Err(e) = file.write_all(slice).await {
                    return Err(PieceFailure::untouched(AppError::io(&e, "写入文件")));
                }
                ledger.add(slice.len() as u64);
                meter.fetch_add(slice.len() as u64, Ordering::SeqCst);
                Ok(slice.len() as u64)
            })
        }
    }

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "xcl-dl-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn sha1_hex(data: &[u8]) -> String {
        let mut hasher = Sha1::new();
        hasher.update(data);
        hasher
            .finalize()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect()
    }

    #[tokio::test]
    async fn downloads_and_reports() {
        let dir = temp_dir("ok");
        let payload = b"hello minecraft";
        let fetcher = FakeFetcher::new(payload);
        let dest = dir.join("a.txt");

        let items = vec![DownloadItem {
            name: "a.txt".into(),
            urls: vec!["https://example.com/a".into()],
            split_urls: Vec::new(),
            dest: dest.clone(),
            sha1: Some(sha1_hex(payload)),
            size: payload.len() as u64,
        }];

        let progress = Progress::new(payload.len() as u64, 1);
        let summary = run(
            items,
            fetcher,
            Arc::clone(&progress),
            DownloadOptions::default(),
            CancellationToken::new(),
        )
        .await;

        assert_eq!(summary.downloaded, 1);
        assert_eq!(tokio::fs::read(&dest).await.unwrap(), payload);
        assert_eq!(progress.snapshot().fraction(), 1.0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn skips_when_checksum_matches() {
        let dir = temp_dir("skip");
        let payload = b"already here";
        let dest = dir.join("b.txt");
        tokio::fs::write(&dest, payload).await.unwrap();

        let fetcher = FakeFetcher::new(payload);
        let items = vec![DownloadItem {
            name: "b.txt".into(),
            urls: vec!["https://example.com/b".into()],
            split_urls: Vec::new(),
            dest: dest.clone(),
            sha1: Some(sha1_hex(payload)),
            size: payload.len() as u64,
        }];

        let summary = run(
            items,
            fetcher.clone(),
            Progress::new(payload.len() as u64, 1),
            DownloadOptions::default(),
            CancellationToken::new(),
        )
        .await;

        assert_eq!(summary.skipped, 1);
        assert_eq!(
            fetcher.calls.load(Ordering::SeqCst),
            0,
            "命中校验应完全不发请求"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn checksum_mismatch_fails_and_cleans_part_file() {
        let dir = temp_dir("bad");
        let payload = b"corrupted";
        let dest = dir.join("c.txt");
        let fetcher = FakeFetcher::new(payload);

        let items = vec![DownloadItem {
            name: "c.txt".into(),
            urls: vec!["https://example.com/c".into()],
            split_urls: Vec::new(),
            dest: dest.clone(),
            sha1: Some("0000000000000000000000000000000000000000".into()),
            size: payload.len() as u64,
        }];

        let summary = run(
            items,
            fetcher,
            Progress::new(payload.len() as u64, 1),
            DownloadOptions::default(),
            CancellationToken::new(),
        )
        .await;

        assert_eq!(summary.failed, 1);
        assert!(!dest.exists(), "校验失败不能留下成品文件");
        assert!(!part_path(&dest).exists(), "临时文件必须清掉");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn respects_concurrency_limit() {
        let dir = temp_dir("conc");
        let payload = vec![7u8; 1024];
        let fetcher = FakeFetcher::with_delay(&payload, 20);

        let items: Vec<DownloadItem> = (0..12)
            .map(|i| DownloadItem {
                name: format!("f{i}"),
                urls: vec![format!("https://example.com/{i}")],
                split_urls: Vec::new(),
                dest: dir.join(format!("f{i}.bin")),
                sha1: None,
                size: payload.len() as u64,
            })
            .collect();

        let summary = run(
            items,
            fetcher.clone(),
            Progress::new(1024 * 12, 12),
            DownloadOptions {
                concurrency: 3,
                attempts_per_url: 1,
                split: SplitConfig::default(),
            },
            CancellationToken::new(),
        )
        .await;

        assert_eq!(summary.downloaded, 12);
        let max = fetcher.max_active.load(Ordering::SeqCst);
        assert!(max <= 3, "并发不应超过限制，实际 {max}");
        assert!(max > 1, "应当真的并发执行，实际最大并发 {max}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn cancellation_stops_early() {
        let dir = temp_dir("cancel");
        let payload = vec![1u8; 10];
        let fetcher = FakeFetcher::with_delay(&payload, 30);

        let items: Vec<DownloadItem> = (0..20)
            .map(|i| DownloadItem {
                name: format!("c{i}"),
                urls: vec![format!("https://example.com/{i}")],
                split_urls: Vec::new(),
                dest: dir.join(format!("c{i}.bin")),
                sha1: None,
                size: payload.len() as u64,
            })
            .collect();

        let cancel = CancellationToken::new();
        cancel.cancel();

        let summary = run(
            items,
            fetcher,
            Progress::new(200, 20),
            DownloadOptions::default(),
            cancel,
        )
        .await;

        assert!(summary.cancelled);
        assert_eq!(summary.downloaded, 0, "取消后不应再下载");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn tries_every_candidate_before_giving_up() {
        let dir = temp_dir("fallback");
        let payload = b"from second source";
        let fetcher = FakeFetcher::new(payload);
        fetcher.always_fail.store(true, Ordering::SeqCst);

        let items = vec![DownloadItem {
            name: "d.txt".into(),
            urls: vec!["https://bad/one".into(), "https://bad/two".into()],
            split_urls: Vec::new(),
            dest: dir.join("d.txt"),
            sha1: Some(sha1_hex(payload)),
            size: payload.len() as u64,
        }];

        let summary = run(
            items,
            fetcher.clone(),
            Progress::new(payload.len() as u64, 1),
            DownloadOptions {
                concurrency: 1,
                attempts_per_url: 1,
                split: SplitConfig::default(),
            },
            CancellationToken::new(),
        )
        .await;

        assert_eq!(summary.failed, 1);
        // 两个候选各试一次
        assert_eq!(fetcher.calls.load(Ordering::SeqCst), 2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ---------- 陈旧半截文件的清理 ----------

    fn item_at(dest: PathBuf) -> DownloadItem {
        DownloadItem {
            name: dest
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default(),
            urls: vec!["https://example.com/x".into()],
            split_urls: vec!["https://example.com/x".into()],
            dest,
            sha1: None,
            size: 0,
        }
    }

    #[tokio::test]
    async fn purge_removes_stale_part_but_keeps_real_file() {
        let dir = temp_dir("purge-stale");
        let dest = dir.join("lib.jar");
        tokio::fs::write(&dest, b"finished").await.unwrap();
        // 上次中断留下的半截文件
        tokio::fs::write(part_path(&dest), b"half").await.unwrap();

        let removed = purge_stale_parts(&[item_at(dest.clone())]).await;

        assert_eq!(removed, 1, "应当删掉 1 个半截文件");
        assert!(!part_path(&dest).exists(), "半截文件应已删除");
        assert_eq!(
            tokio::fs::read(&dest).await.unwrap(),
            b"finished",
            "正式文件绝不能被动"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn purge_is_quiet_when_nothing_to_clean() {
        let dir = temp_dir("purge-empty");
        let dest = dir.join("nothing.jar");

        let removed = purge_stale_parts(&[item_at(dest)]).await;

        assert_eq!(removed, 0, "没有残留时不能报删了东西");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn purge_skips_parts_being_written() {
        let dir = temp_dir("purge-active");
        let dest = dir.join("shared.jar");
        let part = part_path(&dest);
        tokio::fs::write(&part, b"in flight").await.unwrap();

        {
            // 模拟另一个安装任务正在写这个文件
            let _guard = ActivePart::register(part.clone());
            let removed = purge_stale_parts(&[item_at(dest.clone())]).await;
            assert_eq!(removed, 0, "正在写的半截文件不能被当成垃圾");
            assert!(part.exists(), "在写的文件必须还在");
        }

        // 凭据释放后就可以清了
        let removed = purge_stale_parts(&[item_at(dest.clone())]).await;
        assert_eq!(removed, 1);
        assert!(!part.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn run_cleans_leftover_part_before_downloading() {
        let dir = temp_dir("purge-before");
        let payload = b"fresh content";
        let dest = dir.join("game.jar");
        // 上次中断：只剩 6 字节的半截文件，没有正式文件
        tokio::fs::write(part_path(&dest), b"stale!").await.unwrap();

        let summary = run(
            vec![item_at(dest.clone())],
            FakeFetcher::new(payload),
            Progress::new(payload.len() as u64, 1),
            DownloadOptions::default(),
            CancellationToken::new(),
        )
        .await;

        assert_eq!(summary.downloaded, 1);
        assert!(!part_path(&dest).exists(), "下载完成后不该留半截文件");
        assert_eq!(
            tokio::fs::read(&dest).await.unwrap(),
            payload,
            "应当从零开始下载，不能把残留内容接上去"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ---------- 分片下载 ----------

    /// 测试专用选项：任何大小都能分片（不等 1MB），否则测试要造几十 MB 数据。
    fn options_at(_dir: &Path) -> DownloadOptions {
        DownloadOptions {
            concurrency: 8,
            attempts_per_url: 1,
            split: SplitConfig {
                piece_size: 256 * 1024,
                min_split_size: 256 * 1024,
                min_segment_size: 256 * 1024,
                max_request_size: 1024 * 1024,
                max_connections: 4,
                total_connections: 8,
            },
        }
    }

    /// 递增模式的 payload：偏移写错了，逐字节比对立刻能发现。
    fn patterned(len: usize) -> Vec<u8> {
        (0..len).map(|i| (i % 251) as u8).collect()
    }

    #[tokio::test]
    async fn large_file_is_downloaded_in_parallel_pieces() {
        let dir = temp_dir("split-ok");
        let payload = patterned(4_500_000);
        let dest = dir.join("big.jar");

        let fetcher = FakeFetcher::with_delay(&payload, 10);
        let mut item = item_at(dest.clone());
        item.sha1 = Some(sha1_hex(&payload));
        item.size = payload.len() as u64;

        let progress = Progress::new(payload.len() as u64, 1);
        let summary = run(
            vec![item],
            Arc::clone(&fetcher) as Arc<dyn Fetcher>,
            Arc::clone(&progress),
            options_at(&dir),
            CancellationToken::new(),
        )
        .await;

        assert_eq!(summary.downloaded, 1);
        assert_eq!(
            tokio::fs::read(&dest).await.unwrap(),
            payload,
            "分片合并后必须逐字节正确"
        );
        assert!(
            fetcher.calls.load(Ordering::SeqCst) >= 5,
            "4.5MB 应切成至少 5 片，实际 {} 次请求",
            fetcher.calls.load(Ordering::SeqCst)
        );
        assert!(
            fetcher.max_active.load(Ordering::SeqCst) >= 2,
            "分片必须真的并发，实际峰值 {}",
            fetcher.max_active.load(Ordering::SeqCst)
        );
        assert_eq!(
            progress.snapshot().done_bytes,
            payload.len() as u64,
            "进度必须正好等于文件大小"
        );

        // 分片不再产生碎片文件，数据目录里只该有最终的成品
        let leftovers = std::fs::read_dir(&dir)
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

    /// 源不支持 Range 时，分片必须整体退让，改用单连接把文件下完。
    /// 这是最重要的兜底：自定义镜像里不支持 Range 的并不罕见。
    #[tokio::test]
    async fn falls_back_to_single_stream_without_range_support() {
        let dir = temp_dir("split-norange");
        let payload = patterned(4_200_000);
        let dest = dir.join("norange.jar");

        let fetcher = FakeFetcher::without_range(&payload);
        let mut item = item_at(dest.clone());
        item.sha1 = Some(sha1_hex(&payload));
        item.size = payload.len() as u64;

        let progress = Progress::new(payload.len() as u64, 1);
        let summary = run(
            vec![item],
            Arc::clone(&fetcher) as Arc<dyn Fetcher>,
            Arc::clone(&progress),
            options_at(&dir),
            CancellationToken::new(),
        )
        .await;

        assert_eq!(summary.downloaded, 1, "不支持分段也必须能下下来");
        assert_eq!(tokio::fs::read(&dest).await.unwrap(), payload);
        assert_eq!(
            progress.snapshot().done_bytes,
            payload.len() as u64,
            "降级路径也不能把进度算多"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 分片内容对不上 sha1 时必须回退单连接重下，而不是把坏文件交出去。
    #[tokio::test]
    async fn bad_piece_checksum_falls_back_and_still_succeeds() {
        let dir = temp_dir("split-bad");
        let payload = patterned(4_100_000);
        let dest = dir.join("bad.jar");

        let fetcher = FakeFetcher::new(&payload);
        let mut item = item_at(dest.clone());
        // 故意写错的 sha1：分片路线必然校验失败
        item.sha1 = Some(sha1_hex(b"something else"));
        item.size = payload.len() as u64;

        let summary = run(
            vec![item],
            Arc::clone(&fetcher) as Arc<dyn Fetcher>,
            Progress::new(payload.len() as u64, 1),
            options_at(&dir),
            CancellationToken::new(),
        )
        .await;

        // 分片和单连接都过不了校验，最终任务是失败的 —— 但不能留下正式文件
        assert_eq!(summary.failed, 1, "校验不过的任务必须算失败");
        assert!(!dest.exists(), "校验失败绝不能产出正式文件");
        assert!(!part_path(&dest).exists(), "半截文件也要清掉");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 失败重试、换源、分片重发都可能把同一段字节写两遍。
    /// 不管中间过程多乱，收尾时总账必须等于「成功文件的大小之和」：
    /// 失败的文件一分不占，成功的按清单声明的大小结算。
    #[tokio::test]
    async fn failed_files_leave_no_bytes_in_progress() {
        let dir = temp_dir("ledger-failed");
        let payload = patterned(2_500_000);
        let digest = sha1_hex(&payload);
        let size = payload.len() as u64;

        let ok_dest = dir.join("ok.bin");
        let retry_dest = dir.join("retry.bin");
        let bad_dest = dir.join("bad.bin");

        let mut ok = item_at(ok_dest.clone());
        ok.sha1 = Some(digest.clone());
        ok.size = size;

        let mut retry = item_at(retry_dest.clone());
        retry.sha1 = Some(digest.clone());
        retry.size = size;

        // 这个永远过不了校验：它写进去的字节最后必须一分不剩地退回来
        let mut bad = item_at(bad_dest.clone());
        bad.sha1 = Some(sha1_hex(b"never matches"));
        bad.size = size;

        // 前两次请求直接失败，逼出「失败后重试 / 换源」这条路径
        let fetcher = FakeFetcher::failing_first(&payload, 2);
        let progress = Progress::new(size * 3, 3);
        let summary = run(
            vec![ok, retry, bad],
            Arc::clone(&fetcher) as Arc<dyn Fetcher>,
            Arc::clone(&progress),
            options_at(&dir),
            CancellationToken::new(),
        )
        .await;

        let snapshot = progress.snapshot();
        assert_eq!(summary.downloaded, 2, "两个文件应当下成功");
        assert_eq!(summary.failed, 1, "校验不过的那个应当失败");
        assert!(
            snapshot.done_bytes <= snapshot.total_bytes,
            "进度绝不能超过总量：{} / {}",
            snapshot.done_bytes,
            snapshot.total_bytes
        );
        assert_eq!(
            snapshot.done_bytes,
            size * 2,
            "失败文件的字节必须全部退回，只留两个成功文件的大小"
        );
        assert!(!bad_dest.exists(), "失败的文件不能落正式文件");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 清单没给大小的文件（`size == 0`）也得自洽：实际下来的量要同时补进总量，
    /// 否则这类文件只涨已下载、不涨总量。
    #[tokio::test]
    async fn unknown_size_files_add_to_total() {
        let dir = temp_dir("ledger-unknown");
        let payload = patterned(1_200_000);
        let dest = dir.join("unknown.bin");

        let mut item = item_at(dest.clone());
        item.sha1 = None;
        item.size = 0;

        let progress = Progress::new(0, 1);
        let fetcher = FakeFetcher::new(&payload);
        let summary = run(
            vec![item],
            Arc::clone(&fetcher) as Arc<dyn Fetcher>,
            Arc::clone(&progress),
            options_at(&dir),
            CancellationToken::new(),
        )
        .await;

        let snapshot = progress.snapshot();
        assert_eq!(summary.downloaded, 1);
        assert_eq!(snapshot.done_bytes, payload.len() as u64);
        assert_eq!(
            snapshot.total_bytes,
            payload.len() as u64,
            "未知大小必须在结算时补进总量"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn sub_bytes_saturates_at_zero() {
        let progress = Progress::new(100, 1);
        progress.add_bytes(30);
        progress.sub_bytes(50);
        assert_eq!(
            progress.snapshot().done_bytes,
            0,
            "撤销量大于已加量时不能回绕成天文数字"
        );
    }

    /// 失败重试之后，进度必须不多不少。
    ///
    /// 校验失败会删掉临时文件、换下一个源整份重下，所以这一轮写下的字节
    /// 必须从进度里退掉。不退就是每换一次源多算一整个文件 ——
    /// 线上出现「611 MB / 585 MB、进度卡在 100%」就是这条账没销。
    #[tokio::test]
    async fn checksum_mismatch_refunds_progress() {
        let dir = temp_dir("progress-bad-checksum");
        let payload = patterned(150_000);
        let dest = dir.join("bad.bin");
        let fetcher = FakeFetcher::new(&payload);

        let mut item = item_at(dest.clone());
        item.size = payload.len() as u64;
        item.sha1 = Some("0".repeat(40));

        let progress = Progress::new(payload.len() as u64, 1);
        let summary = run(
            vec![item],
            fetcher,
            Arc::clone(&progress),
            DownloadOptions::default(),
            CancellationToken::new(),
        )
        .await;

        assert_eq!(summary.failed, 1);
        let snapshot = progress.snapshot();
        assert_eq!(snapshot.done_bytes, 0, "作废的一整份字节必须退回进度");
        assert!(
            snapshot.done_bytes <= snapshot.total_bytes,
            "已下载字节不允许超过总量：{} / {}",
            snapshot.done_bytes,
            snapshot.total_bytes
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 之前真实的 bug：一次下载报了 250.8MB / 143.2MB，就是失败重试的字节没回退。
    #[tokio::test]
    async fn progress_stays_exact_after_retries() {
        let dir = temp_dir("progress-retry");
        let payload = patterned(200_000);
        let dest = dir.join("retry.bin");

        let fetcher = FakeFetcher::failing_first(&payload, 2);
        let mut item = item_at(dest.clone());
        item.size = payload.len() as u64;
        item.sha1 = Some(sha1_hex(&payload));

        let progress = Progress::new(payload.len() as u64, 1);
        let summary = run(
            vec![item],
            Arc::clone(&fetcher) as Arc<dyn Fetcher>,
            Arc::clone(&progress),
            DownloadOptions {
                concurrency: 1,
                attempts_per_url: 3,
                split: SplitConfig::default(),
            },
            CancellationToken::new(),
        )
        .await;

        assert_eq!(summary.downloaded, 1, "前两次失败，第三次应成功");
        assert_eq!(fetcher.calls.load(Ordering::SeqCst), 3);

        let snapshot = progress.snapshot();
        assert_eq!(
            snapshot.done_bytes, snapshot.total_bytes,
            "重试后进度必须正好是 100%，多了说明失败字节没回退"
        );
        assert_eq!(snapshot.fraction(), 1.0);

        let _ = std::fs::remove_dir_all(&dir);
    }
}
