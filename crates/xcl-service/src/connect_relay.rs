//! 本地中继代理：给 easytier 用的「带令牌的出口」。
//!
//! # 为什么需要这一层
//!
//! 节点不再让人直连：连上之后必须先出示登录令牌，否则一个字节都不回。
//! 而 easytier 只会说自己的协议，不会在首帧里塞我们的令牌 —— 它把对端当成
//! 另一个 easytier。所以中间必须有一个极小的 TCP 代理：
//!
//! ```text
//! easytier core --(普通 TCP)--> 本代理 --(XCL1 + 令牌)--> 节点桥接 --> 节点 core
//! ```
//!
//! 代理不做任何协议解析，验完令牌收到 `0x00` 确认之后就是纯字节透传。
//!
//! # 两个不能动的细节
//!
//! - 必须绑物理网卡的 IP，不能绑回环：easytier 在 `--use-smoltcp` 下连不了
//!   `127.0.0.1`（实测 `code 10049`），所以代理要监听在能出网的那块网卡上，
//!   `-p` 也指向那个地址。
//! - 令牌每条连接都要现取：easytier 会开 3~6 条连接，长会话可能跨过令牌过期。
//!   取不到（被登出、过期）就直接断开，不要拿旧令牌硬连 —— 那是白送一次 RST。

use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_util::sync::CancellationToken;
use xcl_core::AppError;

/// 首帧 magic，必须和 `server/internal/bridge/bridge.go` 里的 `magic` 一致。
const MAGIC: &[u8; 4] = b"XCL1";

/// 令牌文本的长度上限，与桥接的 `maxTokenLen` 对齐。超了说明取到的不是令牌。
const MAX_TOKEN_LEN: usize = 512;

/// 连远端桥接的超时。跨洋时 3 秒不够，给到 10 秒；失败就当这条连接废了。
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// 等桥接回确认字节的超时。对方验签要查一次库，慢一点但不会超过 10 秒。
const ACK_TIMEOUT: Duration = Duration::from_secs(10);

/// 每次建连时现取令牌。取不到就说明这次连接不该发出去。
pub type TokenSource = Arc<dyn Fn() -> Option<String> + Send + Sync>;

/// 一个已经跑起来的本地代理。
///
/// `Drop` 即停：会话被顶掉、退出房间、启动器退出，都会走到这里，
/// 不需要调用方记得手动收。
#[derive(Debug)]
pub struct Relay {
    addr: SocketAddr,
    cancel: CancellationToken,
    task: tokio::task::JoinHandle<()>,
}

impl Relay {
    /// 代理监听的地址。`-p tcp://<ip>:<port>` 就用它。
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// 主动停掉（`Drop` 也会停，这里只是让调用点读起来明确）。
    pub fn stop(&self) {
        self.cancel.cancel();
    }
}

impl Drop for Relay {
    fn drop(&mut self) {
        self.cancel.cancel();
        self.task.abort();
    }
}

/// 起一个本地代理，把 `remote` 桥接地址包成「easytier 能直接用的出口」。
///
/// `remote` 是 `host:port` 形式的字符串（可能是域名，由服务端下发），
/// 这里做一次解析。`bind` 是监听用的本机网卡地址（由调用方用 `detect_local_ip()` 提供），
/// 端口交给系统分配 —— 同一个网络里可能同时跑着别的 easytier，固定端口会撞。
pub async fn start(
    bind: Ipv4Addr,
    remote: String,
    tokens: TokenSource,
) -> Result<Relay, AppError> {
    if bind.is_loopback() || bind.is_unspecified() {
        return Err(AppError::validation(
            "本地代理不能绑回环或 0.0.0.0",
            "easytier 在免 TUN 模式下连不了 127.0.0.1",
        )
        .with_hint("应当绑本机网卡的 IPv4 地址"));
    }

    let remote = resolve(&remote).await?;

    let listener = TcpListener::bind(SocketAddr::new(bind.into(), 0))
        .await
        .map_err(|err| {
            AppError::io(&err, "启动本地联机代理")
                .with_hint("可能是防火墙拦了监听，或者网卡刚断开")
        })?;
    let addr = listener
        .local_addr()
        .map_err(|err| AppError::io(&err, "读取本地代理端口"))?;

    let cancel = CancellationToken::new();
    let task = tokio::spawn(accept_loop(listener, remote, tokens, cancel.clone()));
    tracing::info!(%addr, %remote, "本地联机代理已启动");
    Ok(Relay {
        addr,
        cancel,
        task,
    })
}

/// `host:port` → `SocketAddr`。域名要走 DNS，所以是异步的。
async fn resolve(remote: &str) -> Result<SocketAddr, AppError> {
    let trimmed = remote.trim().trim_start_matches("tcp://");
    let mut found = tokio::net::lookup_host(trimmed)
        .await
        .map_err(|err| {
            AppError::network("解析联机节点地址失败", format!("{trimmed}：{err}"))
                .with_hint("确认网络正常，或者让管理员检查节点地址")
        })?;
    found.next().ok_or_else(|| {
        AppError::network(
            "解析联机节点地址失败",
            format!("{trimmed} 没有解析出任何地址"),
        )
    })
}

async fn accept_loop(
    listener: TcpListener,
    remote: SocketAddr,
    tokens: TokenSource,
    cancel: CancellationToken,
) {
    loop {
        tokio::select! {
            _ = cancel.cancelled() => break,
            accepted = listener.accept() => {
                let Ok((socket, peer)) = accepted else { continue };
                let tokens = Arc::clone(&tokens);
                tokio::spawn(async move {
                    if let Err(err) = tunnel(socket, remote, tokens).await {
                        // 连不上、被拒、对端中途断开都在这里，属于正常噪声，记 debug
                        tracing::debug!(%peer, error = %err, "本地代理转发结束");
                    }
                });
            }
        }
    }
}

/// 一条连接的完整流程：取令牌 → 连桥接 → 出示 → 等确认 → 双向透传。
async fn tunnel(
    mut local: TcpStream,
    remote: SocketAddr,
    tokens: TokenSource,
) -> Result<(), std::io::Error> {
    let Some(token) = tokens() else {
        // 没登录 / 令牌过期：直接断，让 easytier 自己重试
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "没有可用的登录令牌",
        ));
    };
    if token.len() > MAX_TOKEN_LEN {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "令牌长度异常",
        ));
    }

    let mut upstream = tokio::time::timeout(CONNECT_TIMEOUT, TcpStream::connect(remote))
        .await
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::TimedOut, "连桥接超时"))??;

    let mut frame = Vec::with_capacity(6 + token.len());
    frame.extend_from_slice(MAGIC);
    frame.extend_from_slice(&(token.len() as u16).to_be_bytes());
    frame.extend_from_slice(token.as_bytes());
    upstream.write_all(&frame).await?;

    // 通过才回一个 0x00；读不到、读到别的、或者直接被关，都算被拒。
    let mut ack = [0u8; 1];
    let read = tokio::time::timeout(ACK_TIMEOUT, upstream.read_exact(&mut ack)).await;
    match read {
        Ok(Ok(_)) if ack[0] == 0 => {}
        Ok(Ok(_)) => {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "桥接返回了非预期确认字节",
            ));
        }
        Ok(Err(err)) => return Err(err),
        Err(_) => {
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "等桥接确认超时",
            ));
        }
    }

    tokio::io::copy_bidirectional(&mut local, &mut upstream).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 首帧编码必须和 Go 侧逐字节一致，否则线上表现是「所有连接都被拒」，
    /// 且日志上只会看到一句「令牌不通过」，很难往编码上想。
    #[test]
    fn frame_matches_bridge_layout() {
        let token = "abcXYZ";
        let mut frame = Vec::new();
        frame.extend_from_slice(MAGIC);
        frame.extend_from_slice(&(token.len() as u16).to_be_bytes());
        frame.extend_from_slice(token.as_bytes());
        assert_eq!(&frame[..4], b"XCL1");
        assert_eq!(&frame[4..6], &[0x00, 0x06], "长度是大端 u16");
        assert_eq!(&frame[6..], b"abcXYZ");
    }

    #[tokio::test]
    async fn refuses_loopback_bind() {
        let tokens: TokenSource = Arc::new(|| None);
        let err = start(Ipv4Addr::LOCALHOST, "127.0.0.1:1".to_string(), tokens)
            .await
            .expect_err("回环必须被拒");
        assert!(err.message.contains("127.0.0.1") || err.hint.is_some());
    }

    /// 带不带 `tcp://`、带不带空格都要能解析 —— 服务端下发的格式不一定统一。
    #[tokio::test]
    async fn remote_accepts_scheme_and_spaces() {
        let addr = resolve("  tcp://127.0.0.1:11010 ").await.expect("应能解析");
        assert_eq!(addr.port(), 11010);
        assert!(addr.ip().is_loopback());
    }
}
