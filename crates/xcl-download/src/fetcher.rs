//! 传输抽象：把「如何拿到字节」和「如何调度」分开。
//!
//! 生产实现是 reqwest 流式下载；测试实现是脚本化的假数据，
//! 这样并发、校验、跳过、取消、分片等逻辑都能离线验证。
//!
//! 这里只负责「一次请求」，有两种落盘方式：
//!
//! - [`Fetcher::fetch_to`] —— 整文件。`dest` 会被截断重写，失败即作废。
//! - [`Fetcher::fetch_at`] —— 指定区间写到文件的指定偏移。文件由调用方预分配，
//!   多个请求可以同时写同一个文件的不同区域。失败时也回报「写了多少」，
//!   让调度器把已写部分记账、只重下缺口。
//!
//! 切多少片、怎么并发、怎么合并，都在上层。

use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures::future::BoxFuture;
use futures::StreamExt;
use tokio::io::{AsyncSeekExt, AsyncWriteExt};
use tokio_util::sync::CancellationToken;
use xcl_core::{AppError, ErrorCode};

use crate::FileLedger;

/// 请求的字节区间，闭区间（含 `end`，与 HTTP `Range` 一致）。
///
/// 注意：`split` 模块内部用的是半开区间 `[start, end)`，
/// 两者只在 IO 交界处转换，不要混用。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ByteRange {
    pub start: u64,
    /// 含端偏移
    pub end: u64,
}

impl ByteRange {
    pub fn new(start: u64, end: u64) -> Self {
        Self { start, end }
    }

    /// 区间长度。闭区间，所以 +1。
    pub fn len(&self) -> u64 {
        self.end.saturating_sub(self.start) + 1
    }

    fn header_value(&self) -> String {
        format!("bytes={}-{}", self.start, self.end)
    }
}

/// 区间下载失败，但已经写下去了多少字节必须交代清楚。
///
/// 为什么不能只返回 `AppError`：一次 4MiB 的请求断在 3.2MiB 处，
/// 这 3.2MiB 已经在磁盘上了，如果丢掉这个信息，重试就会把 3.2MiB 再下一遍
/// （也就是老实现里「失败整个文件重来」的那个坑）。
pub struct PieceFailure {
    pub error: AppError,
    pub written: u64,
}

impl PieceFailure {
    /// 连一个字节都没写下去
    pub fn untouched(error: AppError) -> Self {
        Self { error, written: 0 }
    }
}

impl From<PieceFailure> for AppError {
    fn from(failure: PieceFailure) -> Self {
        failure.error
    }
}

pub trait Fetcher: Send + Sync {
    /// 整文件下载到 `dest`（临时文件，由调用方负责改名）。
    ///
    /// `dest` 若已存在且比 `expected_total` 短，会从现有长度续传
    /// （发 `Range: bytes=N-`，服务器回 206 就接着写）。服务器忽略 Range
    /// （回 200，正文是整个文件）或 `Content-Range` 的总长对不上时，自动把
    /// 半截文件作废、从头重写。
    ///
    /// `expected_total` 期望的完整大小，`0` 表示未知；未知时只用服务器给的
    /// `Content-Length` 兜底校验长度。
    ///
    /// 失败时不删文件、也不撤已写字节 —— 它们留在盘上，下一次尝试接着用。
    /// 因此返回值是这个文件当前在盘上的总字节数（不是本次写入量）。
    fn fetch_to<'a>(
        &'a self,
        url: &'a str,
        dest: &'a Path,
        expected_total: u64,
        ledger: Arc<FileLedger>,
        cancel: CancellationToken,
    ) -> BoxFuture<'a, Result<u64, AppError>>;

    /// 把 `range` 这段字节写到 `part` 的对应偏移处。
    ///
    /// `part` 必须已存在且已预分配（长度 = 文件总大小），本方法不截断，
    /// 只 `seek(range.start)` 后顺序写。多个请求写同一文件的不同区域是安全的。
    ///
    /// 服务器若不支持 Range（返回 200 或长度不符），错误码为
    /// [`ErrorCode::Unsupported`]，上层据此整体降级为单连接。
    ///
    /// 失败时返回 [`PieceFailure`]，其中的 `written` 是真实写盘的字节数，
    /// 进度里也保留这部分（它确实已经在磁盘上了）。
    fn fetch_at<'a>(
        &'a self,
        url: &'a str,
        part: &'a Path,
        range: ByteRange,
        ledger: Arc<FileLedger>,
        // 本文件已收到的字节数。调度器用它算单文件实时速度，据此决定加不加连接。
        meter: Arc<AtomicU64>,
        cancel: CancellationToken,
    ) -> BoxFuture<'a, Result<u64, PieceFailure>>;

    /// 设置总带宽上限（字节/秒），`0` = 不限速。
    ///
    /// 默认空实现：测试用的假 Fetcher 不需要。
    /// 这是全局速率，多个并发请求共享同一个桶 —— 和「总带宽」的语义一致。
    fn set_speed_limit(&self, _bps: u64) {}
}

/// 令牌桶限速。`bps == 0` 时完全不介入。
///
/// 桶最多攒 1 秒的量：不限的话，空闲很久之后的第一个块会全速冲出去，
/// 界面上就会看到「明明限了速却一下跳一大截」。
pub struct Throttle {
    bps: AtomicU64,
    state: Mutex<Bucket>,
}

struct Bucket {
    tokens: f64,
    last: Instant,
}

impl Default for Throttle {
    fn default() -> Self {
        Self::new()
    }
}

impl Throttle {
    pub fn new() -> Self {
        Self {
            bps: AtomicU64::new(0),
            state: Mutex::new(Bucket {
                tokens: 0.0,
                last: Instant::now(),
            }),
        }
    }

    pub fn set(&self, bps: u64) {
        self.bps.store(bps, Ordering::Relaxed);
        // 换速率时清桶，别沿用旧速率的余额
        let mut bucket = self.state.lock().unwrap_or_else(|e| e.into_inner());
        bucket.tokens = 0.0;
        bucket.last = Instant::now();
    }

    pub fn limit(&self) -> u64 {
        self.bps.load(Ordering::Relaxed)
    }

    /// 记下这些字节，超速就等一会儿。返回实际等待时长。
    pub async fn consume(&self, bytes: u64) -> Duration {
        let bps = self.bps.load(Ordering::Relaxed);
        if bps == 0 || bytes == 0 {
            return Duration::ZERO;
        }

        let wait = {
            let mut bucket = self.state.lock().unwrap_or_else(|e| e.into_inner());
            let now = Instant::now();
            let elapsed = now.duration_since(bucket.last).as_secs_f64();
            bucket.last = now;
            bucket.tokens = (bucket.tokens + bps as f64 * elapsed).min(bps as f64);
            bucket.tokens -= bytes as f64;
            if bucket.tokens < 0.0 {
                Duration::from_secs_f64(-bucket.tokens / bps as f64)
            } else {
                Duration::ZERO
            }
        };

        if !wait.is_zero() {
            tokio::time::sleep(wait).await;
        }
        wait
    }
}

/// 基于 reqwest 的流式下载。
pub struct HttpFetcher {
    client: reqwest::Client,
    /// 总带宽上限。多个并发请求共用同一个桶。
    throttle: Throttle,
}

impl HttpFetcher {
    pub fn new() -> Result<Self, AppError> {
        Ok(Self {
            // 下载走强制 HTTP/1.1 的客户端：h2 会把并发请求复用进一条 TCP 连接，
            // 而多连接下载的前提就是「连接真的有多条」。见 `xcl_net::build_download_client`。
            client: xcl_net::build_download_client()?,
            throttle: Throttle::new(),
        })
    }
}

impl Fetcher for HttpFetcher {
    fn set_speed_limit(&self, bps: u64) {
        self.throttle.set(bps);
    }

    fn fetch_to<'a>(
        &'a self,
        url: &'a str,
        dest: &'a Path,
        expected_total: u64,
        ledger: Arc<FileLedger>,
        cancel: CancellationToken,
    ) -> BoxFuture<'a, Result<u64, AppError>> {
        Box::pin(async move {
            // 断点续传的起点：只有「盘上已有、又还没到最终大小」才值得接着下。
            // 相等或更长都说明这半截没用了（预分配出来的稀疏文件走这里 → 从头写）。
            let resume_from = match (expected_total, file_len(dest).await) {
                (size, len) if size > 0 && len > 0 && len < size => len,
                _ => 0,
            };
            let (result, _) = transfer(
                &self.client,
                url,
                dest,
                None,
                true,
                resume_from,
                expected_total,
                &ledger,
                None,
                &cancel,
                &self.throttle,
            )
            .await;
            // 失败不退款、不删盘：字节还在硬盘上，下一次续传接着用。
            // 退款只发生在调用方决定「这份作废」的时候（校验失败 / 彻底放弃）。
            result
        })
    }

    fn fetch_at<'a>(
        &'a self,
        url: &'a str,
        part: &'a Path,
        range: ByteRange,
        ledger: Arc<FileLedger>,
        meter: Arc<AtomicU64>,
        cancel: CancellationToken,
    ) -> BoxFuture<'a, Result<u64, PieceFailure>> {
        Box::pin(async move {
            let (result, written) = transfer(
                &self.client,
                url,
                part,
                Some(range),
                false,
                0,
                0,
                &ledger,
                Some(&meter),
                &cancel,
                &self.throttle,
            )
            .await;
            match result {
                Ok(bytes) => Ok(bytes),
                Err(error) => Err(PieceFailure { error, written }),
            }
        })
    }
}

/// 目标文件的当前长度，不存在就是 0。
async fn file_len(path: &Path) -> u64 {
    tokio::fs::metadata(path)
        .await
        .map(|meta| meta.len())
        .unwrap_or(0)
}

/// 从 `Content-Range: bytes 0-99/12345` 里取出总长度。
///
/// 续传时它是唯一能确认「服务器上还是同一个文件」的证据：总长变了就说明文件
/// 换过，接着往下写只会拼出一个坏文件。
fn content_range_total(response: &reqwest::Response) -> Option<u64> {
    let raw = response
        .headers()
        .get(reqwest::header::CONTENT_RANGE)?
        .to_str()
        .ok()?;
    raw.rsplit('/').next()?.trim().parse().ok()
}

/// 等响应头的上限。连接本身另有 `connect_timeout`。
const HEADER_TIMEOUT: Duration = Duration::from_secs(30);
/// 两个数据块之间的空闲上限。不是整个下载的总时长 ——
/// 慢速大文件只要还在持续出数据，就不该被掐断。
const IDLE_TIMEOUT: Duration = Duration::from_secs(30);

/// 一次 HTTP 传输。
///
/// 返回 `(结果, 字节数)`：即使失败也返回已写字节数，由调用方决定这部分算不算数。
/// 字节数的口径分两种：
/// - 整文件模式（`truncate = true`）：这个文件当前在盘上的总字节数；
/// - 区间模式：本次请求实际写盘的量（调度器按它记账）。
async fn transfer(
    client: &reqwest::Client,
    url: &str,
    dest: &Path,
    range: Option<ByteRange>,
    truncate: bool,
    // 整文件模式的续传起点（0 = 从头，会截断重写）
    resume_from: u64,
    // 期望的完整大小（0 = 未知），用于长度校验和续传的总长核对
    expected_total: u64,
    ledger: &FileLedger,
    // 分片模式下用来算单文件实时速度；整文件模式不需要
    meter: Option<&AtomicU64>,
    cancel: &CancellationToken,
    throttle: &Throttle,
) -> (Result<u64, AppError>, u64) {
    // 取消必须在三个位置都能立刻生效，否则点了取消，界面停了、
    // 后端还在收数据（keep-alive 连接不会自己断）：
    //   1. 等响应头时（连接已建立但还没回包，这一段最容易被忽略）
    //   2. 等每个数据块时
    //   3. 写盘前
    // 任何一处漏掉，表现都是「点了取消还在下载」。
    let mut request = client.get(url);
    if let Some(r) = range {
        request = request.header(reqwest::header::RANGE, r.header_value());
    } else if resume_from > 0 {
        // 断点续传：只要 N 之后的部分。服务器支持就回 206，不支持会回 200（整份）
        request = request.header(
            reqwest::header::RANGE,
            format!("bytes={resume_from}-"),
        );
    }

    let response = tokio::select! {
        biased;
        _ = cancel.cancelled() => return (Err(AppError::canceled()), 0),
        resp = tokio::time::timeout(HEADER_TIMEOUT, request.send()) => match resp {
            Ok(Ok(resp)) => resp,
            Ok(Err(e)) => {
                let error = if e.is_timeout() {
                    AppError::network("下载超时", format!("{url}"))
                } else if e.is_connect() {
                    AppError::network("无法连接下载源", format!("{url}"))
                } else {
                    AppError::network("下载失败", e.to_string())
                };
                return (Err(error), 0);
            }
            Err(_) => {
                return (
                    Err(AppError::network(
                        "下载源响应超时",
                        format!("{url} → {} 秒内没有返回响应头", HEADER_TIMEOUT.as_secs()),
                    )),
                    0,
                );
            }
        },
    };

    let status = response.status();

    // Range 的三道校验，缺一不可：
    //   1. 请求了区间却回 200 —— 服务器忽略了 Range，正文是整个文件。
    //      这时候如果照写，多个分片会各存一份完整文件，拼出来就是垃圾。
    //   2. 416 —— 区间越界（文件在服务器上变了大小）。
    //   3. Content-Length 必须精确等于区间长度。
    if range.is_some() {
        if status.as_u16() == 200 {
            return (
                Err(AppError::unsupported(
                    "下载源不支持分段下载",
                    format!("{url} → 请求了分段，却返回 200（完整文件）"),
                )),
                0,
            );
        }
        if status.as_u16() == 416 {
            return (
                Err(AppError::unsupported(
                    "分段区间无效",
                    format!("{url} → HTTP 416，文件大小可能已变化"),
                )),
                0,
            );
        }
    }

    // 续传的响应怎么读（对 200/206/416 的处理）：
    //   206 → 正常，从续传点接着写；但 `Content-Range` 的总长必须和预期一致，
    //         否则服务器上的文件换过了，拼下去只会得到一个坏文件
    //   200 → 服务器忽略了 Range，正文是整份文件：已下的那截作废，从头写
    //   416 → 这个位置越界（服务器上文件变小了）：清空半截文件，下一轮从头来
    let mut start_at = range.map(|r| r.start).unwrap_or(0);
    if range.is_none() && resume_from > 0 {
        start_at = resume_from;
        match status.as_u16() {
            206 => {
                if let Some(total) = content_range_total(&response) {
                    if expected_total > 0 && total != expected_total {
                        start_at = 0;
                    }
                }
            }
            200 => start_at = 0,
            416 => {
                let _ = tokio::fs::File::create(dest).await;
                ledger.sub(resume_from);
                return (
                    Err(AppError::unsupported(
                        "续传位置无效",
                        format!("{url} → HTTP 416，服务器上的文件可能变小了"),
                    )),
                    0,
                );
            }
            _ => {}
        }
        if start_at == 0 {
            // 续传作废、接下来要整份重写：已经记过的字节必须退回，否则会算两遍
            ledger.sub(resume_from);
        }
    }

    if !status.is_success() {
        return (
            Err(AppError::network(
                "下载源返回错误",
                format!("{url} → HTTP {}", status.as_u16()),
            )),
            0,
        );
    }

    // 长度校验的口径：
    // - 区间模式：必须精确等于区间长度；
    // - 整文件模式：优先用服务器声明的 `Content-Length`（续传时它天然就是剩余长度），
    //   声明缺失时退到「期望总长 - 起点」。
    // 整文件也必须校验：连接提前断掉时流会正常结束，不校验就会被当成成功，
    // 等 sha1 打回再整份重下 —— 小文件反复失败就是这么来的。
    let expected = range.map(|r| r.len()).or_else(|| {
        response
            .content_length()
            .or_else(|| (expected_total > start_at).then(|| expected_total - start_at))
    });
    if let (Some(exp), Some(declared)) = (expected, response.content_length()) {
        if declared != exp {
            return (
                Err(AppError::unsupported(
                    "分段大小不一致",
                    format!("{url} → 期望 {exp} 字节，实际 {declared} 字节"),
                )),
                0,
            );
        }
    }

    if let Some(parent) = dest.parent() {
        if let Err(e) = tokio::fs::create_dir_all(parent).await {
            return (Err(AppError::io(&e, "创建下载目录")), 0);
        }
    }

    let mut file = if truncate && start_at == 0 {
        match tokio::fs::File::create(dest).await {
            Ok(file) => file,
            Err(e) => return (Err(AppError::io(&e, "创建临时文件")), 0),
        }
    } else {
        // 续传 / 区间写入：文件已有内容，不能截断
        match tokio::fs::OpenOptions::new().write(true).open(dest).await {
            Ok(file) => file,
            Err(e) => return (Err(AppError::io(&e, "打开临时文件")), 0),
        }
    };

    if start_at > 0 {
        if let Err(e) = file.seek(std::io::SeekFrom::Start(start_at)).await {
            return (Err(AppError::io(&e, "定位写入偏移")), 0);
        }
    }

    let mut written = 0u64;
    // 回报口径：整文件模式报「盘上总字节」，区间模式报「本次写入量」。
    // 整文件模式必须把续传已有的那截算进去，否则调用方退款会少退。
    let on_disk = |w: u64| if truncate { start_at + w } else { w };
    let mut stream = response.bytes_stream();
    loop {
        // `biased` 保证取消优先于数据块被处理
        let chunk = tokio::select! {
            biased;
            _ = cancel.cancelled() => {
                let _ = file.flush().await;
                return (Err(AppError::canceled()), on_disk(written));
            }
            next = tokio::time::timeout(IDLE_TIMEOUT, stream.next()) => match next {
                Ok(Some(chunk)) => chunk,
                Ok(None) => break,
                Err(_) => {
                    let _ = file.flush().await;
                    return (
                        Err(AppError::network(
                            "下载超时",
                            format!(
                                "{url} → {} 秒内没有收到新数据",
                                IDLE_TIMEOUT.as_secs()
                            ),
                        )),
                        on_disk(written),
                    );
                }
            },
        };

        let chunk = match chunk {
            Ok(chunk) => chunk,
            Err(e) => {
                let _ = file.flush().await;
                return (
                    Err(AppError::network("下载中断", e.to_string())),
                    on_disk(written),
                );
            }
        };

        // 写盘前再确认一次，避免取消后还把这最后一块写进去
        if cancel.is_cancelled() {
            let _ = file.flush().await;
            return (Err(AppError::canceled()), on_disk(written));
        }

        if let Err(e) = file.write_all(&chunk).await {
            let _ = file.flush().await;
            return (Err(AppError::io(&e, "写入文件")), on_disk(written));
        }
        written += chunk.len() as u64;
        ledger.add(chunk.len() as u64);
        if let Some(meter) = meter {
            meter.fetch_add(chunk.len() as u64, Ordering::Relaxed);
        }

        // 限速放在写完盘之后：数据已经落好了再等，取消时不会丢已写的部分。
        // `bps == 0` 时 `consume` 第一行就返回，等于没有这段代码。
        throttle.consume(chunk.len() as u64).await;
    }

    if let Err(e) = file.flush().await {
        return (Err(AppError::io(&e, "刷新文件")), on_disk(written));
    }

    // 服务器说了长度，就必须一字不差地收到。少了说明连接提前断了，
    // 这种「看似成功」的残缺文件最危险 —— 校验会兜住，但没必要拖到那一步。
    // 整文件模式同样走这条：残缺要当场判失败，好让上层接着续传。
    if let Some(exp) = expected {
        if written != exp {
            return (Err(incomplete(exp, written)), on_disk(written));
        }
    }

    (Ok(on_disk(written)), on_disk(written))
}

fn incomplete(expected: u64, actual: u64) -> AppError {
    AppError::new(
        ErrorCode::Network,
        "数据不完整",
        format!("期望 {expected} 字节，实际收到 {actual} 字节"),
    )
    .with_retryable(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn byte_range_len_is_inclusive() {
        assert_eq!(ByteRange::new(0, 0).len(), 1);
        assert_eq!(ByteRange::new(0, 1023).len(), 1024);
        assert_eq!(ByteRange::new(1024, 2047).len(), 1024);
    }

    #[test]
    fn byte_range_header_format() {
        assert_eq!(ByteRange::new(0, 99).header_value(), "bytes=0-99");
        assert_eq!(ByteRange::new(500, 999).header_value(), "bytes=500-999");
    }

    #[test]
    fn throttle_is_transparent_when_unlimited() {
        let throttle = Throttle::new();
        assert_eq!(throttle.limit(), 0);
        // 不限速时不该产生任何等待
        let wait = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap()
            .block_on(throttle.consume(10 * 1024 * 1024));
        assert_eq!(wait, Duration::ZERO);
    }

    #[test]
    fn throttle_waits_when_over_budget() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap();
        runtime.block_on(async {
            let throttle = Throttle::new();
            // 1 MB/s：一次要 100 KB，桶里只攒了不到 1 秒的量，必然要等
            throttle.set(1024 * 1024);
            let wait = throttle.consume(100 * 1024).await;
            assert!(wait > Duration::ZERO, "超速时必须等待");
            assert!(wait <= Duration::from_millis(200), "等待时长不该离谱：{wait:?}");
        });
    }

    #[test]
    fn changing_limit_resets_the_bucket() {
        let throttle = Throttle::new();
        throttle.set(1000);
        throttle.set(2048);
        assert_eq!(throttle.limit(), 2048);
        // 改速率后桶被清空，不会带着旧速率的余额冲一下
        assert_eq!(throttle.state.lock().unwrap().tokens, 0.0);
    }

    #[test]
    fn piece_failure_keeps_written_bytes() {
        let failure = PieceFailure {
            error: AppError::network("断了", "connection reset"),
            written: 3 * 1024 * 1024,
        };
        assert_eq!(failure.written, 3 * 1024 * 1024);
        // 转成 AppError 时只丢结构，不丢语义
        let error: AppError = failure.into();
        assert_eq!(error.title, "断了");
    }

    #[test]
    fn untouched_failure_reports_zero() {
        let failure = PieceFailure::untouched(AppError::canceled());
        assert_eq!(failure.written, 0);
    }
}
