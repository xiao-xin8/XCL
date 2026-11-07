//! HTTP 层。
//!
//! 职责边界（不含下载引擎）：
//! - 单次请求：拿文本 / 拿字节
//! - 多候选源切换：按 `xcl-source` 算出的顺序逐个尝试
//! - 双源竞速：版本清单这类关键小接口，两个源同时开跑取先到者
//! - 分类重试：只重试值得重试的错误
//!
//! 不负责：并发下载、分片、续传、限速、进度。那些在 `xcl-download`。
//!
//! 可测性：底层 `Transport` 是 trait，测试用假实现验证重试与降级逻辑，
//! 不需要联网。

use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use futures::future::BoxFuture;
use futures::stream::{FuturesUnordered, StreamExt};
use tracing::debug;
use xcl_core::AppError;
use xcl_source::{SourcePolicy, UrlKind};

/// 请求结果（已读完的响应体）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fetched {
    pub status: u16,
    pub body: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TransportError {
    #[error("连接超时")]
    Timeout,
    #[error("无法建立连接")]
    Connect,
    #[error("传输失败：{0}")]
    Other(String),
}

/// 底层传输抽象。生产实现是 reqwest，测试实现是脚本化假数据。
///
/// 用 `BoxFuture` 而不是 `async fn`，保证可以 `dyn Transport` 装箱，
/// 这样 `AppState` 不必泛型化。
pub trait Transport: Send + Sync {
    fn get(&self, url: String) -> BoxFuture<'_, Result<Fetched, TransportError>>;

    /// POST 一个 JSON 请求体。
    ///
    /// 默认「不支持」：绝大多数请求是 GET，测试桩不需要为此多写一个方法。
    /// 真实实现（`ReqwestTransport`）覆写它。
    fn post_json(
        &self,
        _url: String,
        _body: Vec<u8>,
    ) -> BoxFuture<'_, Result<Fetched, TransportError>> {
        Box::pin(async { Err(TransportError::Other("这个传输层不支持 POST".to_string())) })
    }
}

// ---------- reqwest 实现 ----------

pub struct ReqwestTransport {
    client: reqwest::Client,
}

impl ReqwestTransport {
    pub fn new() -> Result<Self, AppError> {
        Ok(Self {
            client: build_api_client()?,
        })
    }
}

/// 构造统一配置的 HTTP 客户端。
///
/// 下载层也会复用它 —— 连接池参数必须和并发度匹配，否则 64 并发会不断重开连接。
/// API 客户端：拉版本清单、读 JSON 这类「请求小、次数多」的活。
///
/// 允许 HTTP/2 多路复用 —— 这种场景正是 h2 擅长的（省握手、单连接跑几十个小请求）。
pub fn build_api_client() -> Result<reqwest::Client, AppError> {
    build_client(true)
}

/// 下载客户端：传文件字节。
///
/// 强制 HTTP/1.1，这是关键的一条。原因：
/// h2 会把同一个 host 的所有并发请求多路复用进一条 TCP 连接。
/// 而下载要的恰恰相反 —— 「多条独立连接」才能叠加带宽：
/// 服务端按连接限速时，1 条连接上开 8 个流和只开 1 条连接没有区别，
/// 分片引擎等于白做；而且一条 TCP 丢包会阻塞里面所有流。
///
/// HTTP/1.1 是刻意的选择：分片引擎要的是多条独立的 TCP，
/// 单连接多路复用会让所有分片挤在同一条拥塞控制里。
pub fn build_download_client() -> Result<reqwest::Client, AppError> {
    build_client(false)
}

fn build_client(http2: bool) -> Result<reqwest::Client, AppError> {
    let mut builder = reqwest::Client::builder()
        .user_agent(user_agent())
        .connect_timeout(Duration::from_secs(10))
        .pool_idle_timeout(Duration::from_secs(90))
        // 下载并发 64 时，连接池必须留得下这么多空闲连接，
        // 否则每批都要重新 TLS 握手，速度会被吃回去。
        // HTTP/1.1 下「同时在用」的连接数由请求数决定，不受这个值限制。
        .pool_max_idle_per_host(64)
        // 关 Nagle：小包不攒着发，少一点几十毫秒级的延迟
        .tcp_nodelay(true);

    if http2 {
        // API 请求小且次数多，保留一个总时限，避免挂死的请求占住重试链。
        builder = builder.timeout(Duration::from_secs(300));
    } else {
        // 下载不设总时限：一个 143MB 的文件在 0.4MB/s 下要 6 分钟，
        // 任何固定总时限都会把「正常但慢」的下载误杀。改成由传输层看空闲时间
        // （见 `xcl-download::fetcher` 的 `IDLE_TIMEOUT`）。
        builder = builder.http1_only();
    }

    builder.build().map_err(|e| {
        AppError::network("初始化网络组件失败", e.to_string())
            .with_hint("请重启程序；若持续失败请反馈")
    })
}

impl Transport for ReqwestTransport {
    fn get(&self, url: String) -> BoxFuture<'_, Result<Fetched, TransportError>> {
        Box::pin(async move {
            let resp = self.client.get(&url).send().await.map_err(|e| {
                if e.is_timeout() {
                    TransportError::Timeout
                } else if e.is_connect() {
                    TransportError::Connect
                } else {
                    TransportError::Other(e.to_string())
                }
            })?;

            let status = resp.status().as_u16();
            let body = resp
                .bytes()
                .await
                .map_err(|e| TransportError::Other(e.to_string()))?
                .to_vec();
            Ok(Fetched { status, body })
        })
    }

    fn post_json(
        &self,
        url: String,
        body: Vec<u8>,
    ) -> BoxFuture<'_, Result<Fetched, TransportError>> {
        Box::pin(async move {
            let resp = self
                .client
                .post(&url)
                .header(reqwest::header::CONTENT_TYPE, "application/json")
                .body(body)
                .send()
                .await
                .map_err(|e| {
                    if e.is_timeout() {
                        TransportError::Timeout
                    } else if e.is_connect() {
                        TransportError::Connect
                    } else {
                        TransportError::Other(e.to_string())
                    }
                })?;

            let status = resp.status().as_u16();
            let body = resp
                .bytes()
                .await
                .map_err(|e| TransportError::Other(e.to_string()))?
                .to_vec();
            Ok(Fetched { status, body })
        })
    }
}

fn user_agent() -> String {
    format!("XCL/{} (Minecraft Launcher)", env!("CARGO_PKG_VERSION"))
}

// ---------- 重试策略 ----------

#[derive(Debug, Clone)]
pub struct RetryPolicy {
    /// 单个候选地址最多重试几次（不含首次尝试）
    pub max_retries: u32,
    /// 首次退避时长，之后每次翻倍
    pub base_delay: Duration,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_retries: 2,
            base_delay: Duration::from_millis(300),
        }
    }
}

/// 默认的单次尝试超时。
///
/// 客户端本身有 300s 的总超时，那是为下载大文件准备的；但元数据请求如果
/// 撞上一个挂起的镜像，等 300s 等于界面卡死。所以单次尝试给一个上限，
/// 超时按可重试错误处理，让候选切换能继续往下走。
pub const DEFAULT_ATTEMPT_TIMEOUT: Duration = Duration::from_secs(60);

/// 竞速时谁先拿到数据。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RaceWinner {
    Primary,
    Secondary,
}

/// 竞速结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RaceOutcome {
    pub body: Vec<u8>,
    pub winner: RaceWinner,
    /// 主源自己的耗时。主源没有成功时为 `None`。
    ///
    /// 上层用它判断「官方源够不够快」（见 `xcl_source::OFFICIAL_FAST_MS`）。
    pub primary_ms: Option<u64>,
}

/// 单次尝试的失败分类。
enum AttemptError {
    /// 值得重试（超时 / 连接失败 / 5xx / 429）
    Retryable(TransportError),
    /// 不该重试（404 / 403 / 其他 4xx、传输层致命错误）
    Fatal(String),
}

// ---------- 客户端 ----------

pub struct HttpClient {
    transport: Arc<dyn Transport>,
    /// 源策略。用 `RwLock` 是因为「自动选源」会在拉完版本清单后把探测结论
    /// 回填进来 —— 那一刻是 `&self` 上下文，改不了字段。
    policy: RwLock<SourcePolicy>,
    retry: RetryPolicy,
    attempt_timeout: Duration,
}

impl HttpClient {
    pub fn new(policy: SourcePolicy) -> Result<Self, AppError> {
        Ok(Self {
            transport: Arc::new(ReqwestTransport::new()?),
            policy: RwLock::new(policy),
            retry: RetryPolicy::default(),
            attempt_timeout: DEFAULT_ATTEMPT_TIMEOUT,
        })
    }

    /// 测试与特殊场景用：注入自定义 transport。
    pub fn with_transport(transport: Arc<dyn Transport>, policy: SourcePolicy) -> Self {
        Self {
            transport,
            policy: RwLock::new(policy),
            retry: RetryPolicy::default(),
            attempt_timeout: DEFAULT_ATTEMPT_TIMEOUT,
        }
    }

    pub fn with_retry_policy(mut self, retry: RetryPolicy) -> Self {
        self.retry = retry;
        self
    }

    pub fn with_attempt_timeout(mut self, timeout: Duration) -> Self {
        self.attempt_timeout = timeout;
        self
    }

    /// 当前源策略的快照。返回克隆而不是守卫，避免调用方跨 `await` 持有锁。
    pub fn policy(&self) -> SourcePolicy {
        self.policy
            .read()
            .map(|guard| guard.clone())
            .unwrap_or_else(|poisoned| poisoned.into_inner().clone())
    }

    /// 运行时换源策略（自动选源探测完成后调用）。
    pub fn set_policy(&self, policy: SourcePolicy) {
        match self.policy.write() {
            Ok(mut guard) => *guard = policy,
            Err(poisoned) => *poisoned.into_inner() = policy,
        }
    }

    /// 拉取文本。会自动按策略生成候选源并逐个尝试。
    pub async fn get_text(&self, kind: UrlKind, official: &str) -> Result<String, AppError> {
        let candidates = self.policy().candidates(kind, official);
        let bytes = self.get_from_candidates(&candidates).await?;
        String::from_utf8(bytes).map_err(|e| {
            AppError::parse("响应内容不是有效的 UTF-8", e.to_string())
                .with_hint("可能是下载源返回了错误页面")
        })
    }

    /// 拉取字节。为将来的下载层留的入口。
    pub async fn get_bytes(&self, kind: UrlKind, official: &str) -> Result<Vec<u8>, AppError> {
        let candidates = self.policy().candidates(kind, official);
        self.get_from_candidates(&candidates).await
    }

    /// 双源竞速拉文本。
    ///
    /// 语义：
    /// 1. 先给 `primary` 一个独占窗口（`hedge`）。窗口内成功就直接用，完全不惊动 `secondary`
    /// 2. 窗口内 `primary` 明确失败 → 立刻改用 `secondary`，不必等满窗口
    /// 3. 窗口到点 `primary` 还没回来 → 两个源并发，谁先成功用谁
    ///
    /// 第 3 步让「先回来的那个恰好失败」也不会浪费掉另一个已经在跑的请求，
    /// 这里用流实现，省掉了 10ms 轮询延迟。
    pub async fn race_text(
        &self,
        primary: &str,
        secondary: &str,
        hedge: Duration,
        overall: Duration,
    ) -> Result<RaceOutcome, AppError> {
        match tokio::time::timeout(overall, self.race_inner(primary, secondary, hedge)).await {
            Ok(result) => result,
            Err(_) => Err(AppError::network(
                "下载源响应超时",
                format!("{} 秒内两个源都没有返回数据", overall.as_secs()),
            )
            .with_hint("请检查网络连接，或在设置里切换下载源")),
        }
    }

    async fn race_inner(
        &self,
        primary: &str,
        secondary: &str,
        hedge: Duration,
    ) -> Result<RaceOutcome, AppError> {
        let started = Instant::now();
        let primary_fut = self.try_once(primary);
        tokio::pin!(primary_fut);
        let window = tokio::time::sleep(hedge);
        tokio::pin!(window);

        enum First {
            PrimaryDone(Result<Vec<u8>, String>),
            WindowClosed,
        }

        let first = tokio::select! {
            biased;
            res = &mut primary_fut => First::PrimaryDone(res),
            _ = &mut window => First::WindowClosed,
        };

        match first {
            First::PrimaryDone(Ok(body)) => Ok(RaceOutcome {
                body,
                winner: RaceWinner::Primary,
                primary_ms: Some(started.elapsed().as_millis() as u64),
            }),
            // 主源明确失败：直接上备源，别干等满一个窗口
            First::PrimaryDone(Err(primary_err)) => match self.try_once(secondary).await {
                Ok(body) => {
                    debug!(primary, primary_err, "主源失败，改用备源");
                    Ok(RaceOutcome {
                        body,
                        winner: RaceWinner::Secondary,
                        primary_ms: None,
                    })
                }
                Err(secondary_err) => Err(all_sources_failed(&[
                    (primary, primary_err),
                    (secondary, secondary_err),
                ])),
            },
            First::WindowClosed => {
                // 主源还在跑：并发备源，取第一个成功的
                let primary_task: BoxFuture<'_, (RaceWinner, Result<Vec<u8>, String>, u64)> =
                    Box::pin(async {
                        let result = primary_fut.as_mut().await;
                        (RaceWinner::Primary, result, started.elapsed().as_millis() as u64)
                    });
                let secondary_task: BoxFuture<'_, (RaceWinner, Result<Vec<u8>, String>, u64)> =
                    Box::pin(async {
                        let result = self.try_once(secondary).await;
                        (RaceWinner::Secondary, result, started.elapsed().as_millis() as u64)
                    });

                let mut inbox = FuturesUnordered::new();
                inbox.push(primary_task);
                inbox.push(secondary_task);

                let mut failures: Vec<(&str, String)> = Vec::new();
                while let Some((winner, result, elapsed_ms)) = inbox.next().await {
                    match result {
                        Ok(body) => {
                            let primary_ms = match winner {
                                // 主源在窗口内没回来就已经说明它慢，耗时值仍如实记录
                                RaceWinner::Primary => Some(elapsed_ms),
                                RaceWinner::Secondary => None,
                            };
                            return Ok(RaceOutcome {
                                body,
                                winner,
                                primary_ms,
                            });
                        }
                        Err(err) => failures.push(match winner {
                            RaceWinner::Primary => (primary, err),
                            RaceWinner::Secondary => (secondary, err),
                        }),
                    }
                }
                Err(all_sources_failed(&failures))
            }
        }
    }

    /// 单次尝试，不重试。
    ///
    /// 竞速场景专用：另一个源已经在跑了，重试只会拖长整体时间，
    /// 让调用方迟迟拿不到结果。
    async fn try_once(&self, url: &str) -> Result<Vec<u8>, String> {
        match self.attempt(url, None).await {
            Ok(fetched) => match classify_status(fetched.status) {
                None => Ok(fetched.body),
                Some(AttemptError::Fatal(reason)) => Err(reason),
                Some(AttemptError::Retryable(err)) => Err(err.to_string()),
            },
            Err(err) => Err(err.to_string()),
        }
    }

    /// 发一次请求，套上单次超时。`body` 有值时走 POST。
    async fn attempt(
        &self,
        url: &str,
        body: Option<&[u8]>,
    ) -> Result<Fetched, TransportError> {
        let future = match body {
            Some(body) => self.transport.post_json(url.to_string(), body.to_vec()),
            None => self.transport.get(url.to_string()),
        };
        match tokio::time::timeout(self.attempt_timeout, future).await {
            Ok(result) => result,
            Err(_) => Err(TransportError::Timeout),
        }
    }

    /// 按给定候选顺序尝试，返回第一个成功的内容。
    pub async fn get_from_candidates(&self, candidates: &[String]) -> Result<Vec<u8>, AppError> {
        self.candidates_body(candidates, None, "资源加载失败").await
    }

    /// 按给定候选顺序 POST 同一份 JSON 体，返回第一个成功的内容。
    ///
    /// CF 的「批量查文件」是 POST，镜像不一定实现 —— 所以这里也走候选链，
    /// 第一个 405/501 会当成致命错误直接换下一个候选（`classify_status`）。
    pub async fn post_json_from_candidates(
        &self,
        candidates: &[String],
        body: Vec<u8>,
    ) -> Result<Vec<u8>, AppError> {
        self.candidates_body(candidates, Some(&body), "请求资源平台失败")
            .await
    }

    async fn candidates_body(
        &self,
        candidates: &[String],
        body: Option<&[u8]>,
        title: &str,
    ) -> Result<Vec<u8>, AppError> {
        if candidates.is_empty() {
            return Err(AppError::network(
                "没有可用的下载源",
                "所有候选地址都被过滤掉了",
            ));
        }

        let mut failures: Vec<String> = Vec::new();
        for (index, url) in candidates.iter().enumerate() {
            match self.try_one(url, body).await {
                Ok(body) => {
                    if index > 0 {
                        debug!(url, index, "前面 {} 个源失败，已降级到当前源", index);
                    }
                    return Ok(body);
                }
                Err(reason) => {
                    debug!(url, reason, "下载源尝试失败");
                    failures.push(format!("{}：{}", short_host(url), reason));
                }
            }
        }

        // 标题只说明「东西没加载出来」；具体试了哪些源放在说明里
        Err(AppError::network(title, failures.join("；"))
            .with_hint("检查网络连接，或在设置里切换下载源"))
    }

    /// 对单个地址按重试策略尝试。
    async fn try_one(&self, url: &str, body: Option<&[u8]>) -> Result<Vec<u8>, String> {
        let mut delay = self.retry.base_delay;

        for attempt in 0..=self.retry.max_retries {
            match self.attempt(url, body).await {
                Ok(fetched) => match classify_status(fetched.status) {
                    None => return Ok(fetched.body),
                    Some(AttemptError::Fatal(reason)) => return Err(reason),
                    Some(AttemptError::Retryable(err)) => {
                        if attempt == self.retry.max_retries {
                            return Err(err.to_string());
                        }
                        debug!(url, attempt, "可重试的状态码，退避后重试");
                    }
                },
                Err(err @ (TransportError::Timeout | TransportError::Connect)) => {
                    if attempt == self.retry.max_retries {
                        return Err(err.to_string());
                    }
                    debug!(url, attempt, %err, "网络错误，退避后重试");
                }
                Err(TransportError::Other(msg)) => return Err(msg),
            }

            tokio::time::sleep(delay).await;
            delay = delay.saturating_mul(2);
        }

        Err("重试次数已用尽".to_string())
    }
}

/// 返回 `None` 表示成功；否则给出失败分类。
fn classify_status(status: u16) -> Option<AttemptError> {
    if (200..300).contains(&status) {
        return None;
    }
    if status == 429 || (500..600).contains(&status) {
        return Some(AttemptError::Retryable(TransportError::Other(format!(
            "HTTP {status}"
        ))));
    }
    Some(AttemptError::Fatal(format!("HTTP {status}")))
}

fn short_host(url: &str) -> String {
    let after_scheme = url.split_once("://").map(|(_, rest)| rest).unwrap_or(url);
    after_scheme
        .split('/')
        .next()
        .unwrap_or(after_scheme)
        .to_string()
}

/// 所有候选都失败时，把每个源的失败原因拼成一条可读文案。
fn all_sources_failed(entries: &[(&str, String)]) -> AppError {
    let detail = entries
        .iter()
        .map(|(url, reason)| format!("{}：{}", short_host(url), reason))
        .collect::<Vec<_>>()
        .join("；");
    AppError::network("资源加载失败", detail)
        .with_hint("检查网络连接，或在设置里切换下载源")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use xcl_source::{Mirror, Strategy};

    /// 按脚本依次返回结果，并记录被请求的 URL。
    struct FakeTransport {
        script: Mutex<Vec<Result<Fetched, TransportError>>>,
        calls: Mutex<Vec<String>>,
    }

    impl FakeTransport {
        fn new(script: Vec<Result<Fetched, TransportError>>) -> Self {
            Self {
                script: Mutex::new(script),
                calls: Mutex::new(Vec::new()),
            }
        }

        fn calls(&self) -> Vec<String> {
            self.calls.lock().unwrap().clone()
        }
    }

    impl Transport for FakeTransport {
        fn get(&self, url: String) -> BoxFuture<'_, Result<Fetched, TransportError>> {
            Box::pin(async move {
                self.calls.lock().unwrap().push(url);
                let mut script = self.script.lock().unwrap();
                if script.is_empty() {
                    Err(TransportError::Other("脚本已耗尽".to_string()))
                } else {
                    script.remove(0)
                }
            })
        }
    }

    fn ok(body: &str) -> Result<Fetched, TransportError> {
        Ok(Fetched {
            status: 200,
            body: body.as_bytes().to_vec(),
        })
    }

    fn status(code: u16) -> Result<Fetched, TransportError> {
        Ok(Fetched {
            status: code,
            body: Vec::new(),
        })
    }

    fn fast_client(transport: Arc<dyn Transport>) -> HttpClient {
        HttpClient::with_transport(
            transport,
            SourcePolicy::new(Mirror::Bmclapi, Strategy::MirrorFirst),
        )
        .with_retry_policy(RetryPolicy {
            max_retries: 2,
            base_delay: Duration::ZERO,
        })
    }

    const MANIFEST: &str = "https://piston-meta.mojang.com/mc/game/version_manifest_v2.json";

    #[tokio::test]
    async fn falls_back_to_next_candidate() {
        let fake = Arc::new(FakeTransport::new(vec![
            Err(TransportError::Connect),
            Err(TransportError::Connect),
            Err(TransportError::Connect),
            ok("{\"ok\":true}"),
        ]));
        let client = fast_client(fake.clone());

        let text = client.get_text(UrlKind::Meta, MANIFEST).await.unwrap();
        assert_eq!(text, "{\"ok\":true}");
        // 3 次重试首个源 + 1 次成功的镜像源
        assert_eq!(fake.calls().len(), 4);
        assert!(fake.calls()[0].contains("bmclapi"));
    }

    #[tokio::test]
    async fn not_found_switches_source_without_retry() {
        let fake = Arc::new(FakeTransport::new(vec![status(404), ok("second")]));
        let client = fast_client(fake.clone());

        let text = client.get_text(UrlKind::Meta, MANIFEST).await.unwrap();
        assert_eq!(text, "second");
        // 404 不重试，直接换下一个源 —— 总共只应该请求 2 次
        assert_eq!(fake.calls().len(), 2);
    }

    #[tokio::test]
    async fn server_error_is_retried_before_switching() {
        let fake = Arc::new(FakeTransport::new(vec![
            status(503),
            status(503),
            status(503),
            ok("recovered"),
        ]));
        let client = fast_client(fake.clone());

        let text = client.get_text(UrlKind::Meta, MANIFEST).await.unwrap();
        assert_eq!(text, "recovered");
        // 首个源重试到上限（3 次），再换源成功
        assert_eq!(fake.calls().len(), 4);
    }

    #[tokio::test]
    async fn all_sources_failing_reports_readable_error() {
        let fake = Arc::new(FakeTransport::new(vec![
            status(404),
            status(404),
        ]));
        let client = fast_client(fake);

        let err = client.get_text(UrlKind::Meta, MANIFEST).await.unwrap_err();
        assert_eq!(err.code, xcl_core::ErrorCode::Network);
        assert!(err.retryable);
        assert!(err.message.contains("HTTP 404"));
        assert!(err.hint.is_some());
    }

    #[tokio::test]
    async fn invalid_utf8_is_parse_error() {
        let fake = Arc::new(FakeTransport::new(vec![Ok(Fetched {
            status: 200,
            body: vec![0xFF, 0xFE, 0x00],
        })]));
        let client = fast_client(fake);

        let err = client.get_text(UrlKind::Meta, MANIFEST).await.unwrap_err();
        assert_eq!(err.code, xcl_core::ErrorCode::Parse);
    }

    #[test]
    fn short_host_extracts_host_only() {
        assert_eq!(
            short_host("https://bmclapi2.bangbang93.com/mc/game/x.json"),
            "bmclapi2.bangbang93.com"
        );
    }

    // ---------- 双源竞速 ----------

    /// 按 URL 关键字匹配行为：延迟 + 状态码。
    struct RuleTransport {
        rules: Vec<(&'static str, u64, u16)>,
        calls: Mutex<Vec<String>>,
    }

    impl RuleTransport {
        fn new(rules: Vec<(&'static str, u64, u16)>) -> Arc<Self> {
            Arc::new(Self {
                rules,
                calls: Mutex::new(Vec::new()),
            })
        }

        fn calls(&self) -> Vec<String> {
            self.calls.lock().unwrap().clone()
        }
    }

    impl Transport for RuleTransport {
        fn get(&self, url: String) -> BoxFuture<'_, Result<Fetched, TransportError>> {
            Box::pin(async move {
                self.calls.lock().unwrap().push(url.clone());
                let Some((_, delay_ms, status)) = self
                    .rules
                    .iter()
                    .find(|(keyword, _, _)| url.contains(keyword))
                else {
                    return Err(TransportError::Other(format!("没有匹配规则：{url}")));
                };
                if *delay_ms > 0 {
                    tokio::time::sleep(Duration::from_millis(*delay_ms)).await;
                }
                Ok(Fetched {
                    status: *status,
                    body: format!("from:{url}").into_bytes(),
                })
            })
        }
    }

    const OFFICIAL: &str = "https://piston-meta.mojang.com/manifest.json";
    const MIRROR: &str = "https://bmclapi2.bangbang93.com/manifest.json";

    fn race_client(transport: Arc<dyn Transport>) -> HttpClient {
        HttpClient::with_transport(
            transport,
            SourcePolicy::new(Mirror::Bmclapi, Strategy::Auto),
        )
    }

    #[tokio::test]
    async fn race_primary_wins_inside_window_without_touching_secondary() {
        // 官方 10ms 就回来了，镜像根本不该被请求
        let fake = RuleTransport::new(vec![("mojang", 10, 200), ("bmclapi", 0, 200)]);
        let outcome = race_client(fake.clone())
            .race_text(OFFICIAL, MIRROR, Duration::from_millis(200), Duration::from_secs(5))
            .await
            .unwrap();

        assert_eq!(outcome.winner, RaceWinner::Primary);
        assert!(String::from_utf8_lossy(&outcome.body).contains("mojang"));
        let primary_ms = outcome.primary_ms.expect("主源成功时必须回传耗时");
        assert!(primary_ms < 200, "耗时应在窗口内，实际 {primary_ms}ms");
        assert_eq!(fake.calls().len(), 1, "备源不应被请求");
    }

    #[tokio::test]
    async fn race_switches_to_secondary_when_primary_fails_fast() {
        // 主源 404：立刻上备源，不必等满窗口
        let fake = RuleTransport::new(vec![("mojang", 0, 404), ("bmclapi", 20, 200)]);
        let started = Instant::now();
        let outcome = race_client(fake.clone())
            .race_text(OFFICIAL, MIRROR, Duration::from_millis(500), Duration::from_secs(5))
            .await
            .unwrap();

        assert_eq!(outcome.winner, RaceWinner::Secondary);
        assert!(outcome.primary_ms.is_none(), "主源没成功就没有耗时");
        assert!(
            started.elapsed() < Duration::from_millis(500),
            "不该等满整个窗口"
        );
    }

    #[tokio::test]
    async fn race_runs_secondary_after_window_and_takes_first_success() {
        // 主源慢（120ms），窗口 20ms 后并发备源（10ms 就回）→ 备源赢
        let fake = RuleTransport::new(vec![("mojang", 120, 200), ("bmclapi", 10, 200)]);
        let outcome = race_client(fake.clone())
            .race_text(OFFICIAL, MIRROR, Duration::from_millis(20), Duration::from_secs(5))
            .await
            .unwrap();

        assert_eq!(outcome.winner, RaceWinner::Secondary);
        assert!(
            String::from_utf8_lossy(&outcome.body).contains("bmclapi"),
            "拿到的是备源内容"
        );
        assert_eq!(fake.calls().len(), 2, "两个源都应被请求");
    }

    #[tokio::test]
    async fn race_keeps_waiting_when_faster_source_happens_to_fail() {
        // 备源先完成但失败，主源后完成却成功 —— 仍然要拿到主源的内容
        let fake = RuleTransport::new(vec![("mojang", 120, 200), ("bmclapi", 10, 500)]);
        let outcome = race_client(fake.clone())
            .race_text(OFFICIAL, MIRROR, Duration::from_millis(20), Duration::from_secs(5))
            .await
            .unwrap();

        assert_eq!(outcome.winner, RaceWinner::Primary);
        assert!(String::from_utf8_lossy(&outcome.body).contains("mojang"));
        assert!(outcome.primary_ms.is_some());
    }

    #[tokio::test]
    async fn race_reports_both_sources_when_all_fail() {
        let fake = RuleTransport::new(vec![("mojang", 0, 502), ("bmclapi", 0, 404)]);
        let err = race_client(fake)
            .race_text(OFFICIAL, MIRROR, Duration::from_millis(20), Duration::from_secs(5))
            .await
            .unwrap_err();

        assert_eq!(err.code, xcl_core::ErrorCode::Network);
        assert!(err.retryable);
        assert!(err.message.contains("502") && err.message.contains("404"));
    }

    #[tokio::test]
    async fn attempt_timeout_is_reported_as_timeout() {
        // 单次尝试超时应当被归为可重试，由候选切换继续往下走
        let fake = RuleTransport::new(vec![("mojang", 200, 200), ("bmclapi", 0, 200)]);
        let client = HttpClient::with_transport(
            fake,
            SourcePolicy::new(Mirror::Bmclapi, Strategy::OfficialFirst),
        )
        .with_retry_policy(RetryPolicy {
            max_retries: 0,
            base_delay: Duration::ZERO,
        })
        .with_attempt_timeout(Duration::from_millis(30));

        let text = client.get_text(UrlKind::Meta, OFFICIAL).await.unwrap();
        assert!(text.contains("bmclapi"), "官方源超时后应换到镜像候选");
    }
}
