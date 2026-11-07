//! XCL 联机账号：我们自己的账号，用来「进节点」。
//!
//! 与 `crate::auth` 的区别要说清楚，两者的名字都很像但完全不是一回事：
//!
//! | 模块 | 管什么 | 用来干什么 | 存在哪 |
//! |---|---|---|---|
//! | `auth` | Minecraft 账号（离线 / Yggdrasil / 第三方） | 进游戏、拿皮肤 | `<root>/accounts.toml` |
//! | 本模块 | 联机服务账号（用户名 + 密码） | 进节点、开房、加入房间 | `<root>/connect/account.json` |
//!
//! 节点桥接只认本模块发出的令牌 —— 没登录就拿不到令牌，连不上节点。
//! 所以界面上它是联机功能的前置：没登录就没有「创建 / 加入房间」。
//!
//! 令牌是服务端用 Ed25519 签的，客户端只负责保存和出示，不解析、不校验——
//! 校验在节点上（离线验签，不回调 HTTP）。客户端要是自己判一遍，等于多一份
//! 可能过期的逻辑，还给了「本地改个文件就以为登录了」的错觉。

use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use xcl_core::AppError;

use crate::AppState;

/// 账号服务地址由部署方在 `config.toml` 的 `connect.accountServer` 里填。
///
/// 不进代码：仓库里没有默认值，自己编译的人填自己的服务端。
/// 启动时 `AppState::new` 把配置灌进 [`CONFIGURED_SERVER`]，下面那些
/// 不接 `AppState` 的小函数才拿得到（它们全是内部工具，传一遍参数纯噪音）。
static CONFIGURED_SERVER: std::sync::OnceLock<String> = std::sync::OnceLock::new();

/// 把配置里的账号服务地址交给本模块。只有第一次调用生效。
pub(crate) fn configure_server(url: &str) {
    let _ = CONFIGURED_SERVER.set(url.trim().trim_end_matches('/').to_string());
}

/// 单次请求超时。登录、注册都是几 KB 的小请求，15 秒足够跨洋打个来回。
const TIMEOUT: Duration = Duration::from_secs(15);

/// 注册时算工作量证明的上限。20 bit 正常 0.3~1 秒；给到 30 秒是防老机器，
/// 真超了说明服务端难度被人调过，直接报错比无限转圈强。
const POW_DEADLINE: Duration = Duration::from_secs(30);

/// 账号文件的相对位置（相对数据根）。
const ACCOUNT_FILE: &str = "connect/account.json";

/// 前端看到的账号状态。
///
/// 故意只有「有没有登录、叫什么、什么时候过期」——不暴露 token。
/// 前端不需要 token，也绝不该有机会把它写进日志或界面。
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AccountView {
    pub signed_in: bool,
    pub username: Option<String>,
    pub user_id: Option<i64>,
    /// 令牌过期时间（Unix 秒）。0 表示没登录。
    pub expires_at: i64,
    /// 当前连的服务地址，界面上显示出来，省得对着本地/线上猜。
    pub server: String,
}

impl AccountView {
    fn signed_out(server: String) -> Self {
        Self {
            signed_in: false,
            username: None,
            user_id: None,
            expires_at: 0,
            server,
        }
    }
}

/// 落盘的账号。只有这几个字段，别往里加密码之类的东西——
/// 密码只在登录那一次过内存，本地一个字节都不留。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct Stored {
    token: String,
    expires_at: i64,
    user_id: i64,
    username: String,
    /// 服务端下发的桥接地址（`host:port`）。
    ///
    /// 账号接口挂在 Cloudflare 后面，桥接是裸 TCP —— 两者地址不同，
    /// 所以这个值得由服务端在登录时告诉我们，客户端不能自己拿账号域名加端口去猜。
    /// 老版本的账号文件没有这一项，`Option` 会让它自然变成 `None`。
    bridge: Option<String>,
}

/// 服务端的会话响应（`/v1/auth/login` 与 `/v1/auth/register` 同构）。
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SessionResponse {
    token: String,
    expires_at: i64,
    user: UserResponse,
    /// 桥接地址。空/缺失时回退到「账号服务主机名 + 11010」。
    #[serde(default)]
    bridge: Option<String>,
}

#[derive(Debug, Deserialize)]
struct UserResponse {
    id: i64,
    username: String,
}

/// 服务端的错误体。
#[derive(Debug, Deserialize)]
struct ErrorResponse {
    #[allow(dead_code)]
    code: String,
    message: String,
}

/// 挑战接口的响应。
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CaptchaChallenge {
    provider: String,
    challenge: Option<String>,
    difficulty: Option<u32>,
}

// ---------- 状态 ----------

/// 现在登录着没有。
pub fn state(state: &AppState) -> AccountView {
    let server = server();
    let Some(stored) = read_stored(state) else {
        return AccountView::signed_out(server);
    };
    // 过期就算没登录：本地先用时间判，省一次网络请求。
    // 真正的权威判定在节点上（服务端会验签 + 查会话），这里只是别让界面骗人。
    if stored.expires_at <= now() {
        return AccountView::signed_out(server);
    }
    AccountView {
        signed_in: true,
        username: Some(stored.username),
        user_id: Some(stored.user_id),
        expires_at: stored.expires_at,
        server,
    }
}

/// 当前令牌。没登录或已过期返回 `None`。
///
/// 联机会话建立前要用它——但仅用于本地代理出示，不传给 easytier。
pub fn token(state: &AppState) -> Option<String> {
    let stored = read_stored(state)?;
    (stored.expires_at > now()).then_some(stored.token)
}

// ---------- 登录 / 注册 / 登出 ----------

/// 登录。
pub async fn login(state: &AppState, username: &str, password: &str) -> Result<AccountView, AppError> {
    let (username, password) = check_credentials(username, password, false)?;
    let body = serde_json::json!({
        "username": username,
        "password": password,
        "deviceName": device_name(),
    });
    let resp = post("auth/login", body, None).await?;
    finish_login(state, resp)
}

/// 注册。会先领一道人机验证题、算完再提交。
pub async fn register(
    state: &AppState,
    username: &str,
    password: &str,
) -> Result<AccountView, AppError> {
    let (username, password) = check_credentials(username, password, true)?;

    let challenge = get_challenge().await?;
    let proof = match challenge.provider.as_str() {
        "pow" => {
            let text = challenge.challenge.clone().ok_or_else(|| {
                AppError::parse("人机验证异常", "服务端没有下发挑战，请稍后重试")
            })?;
            let bits = challenge.difficulty.unwrap_or(20);
            // 算题是纯 CPU 循环，必须挪出 async 线程，否则整个后端在这一秒里没法响应别的命令
            let nonce = tokio::task::spawn_blocking(move || solve_pow(&text, bits))
                .await
                .map_err(|err| AppError::parse("人机验证失败", err.to_string()))??;
            serde_json::json!({ "provider": "pow", "challenge": challenge.challenge, "nonce": nonce })
        }
        "turnstile" => {
            // 服务端配了 Cloudflare 人机验证。WebView 里能不能加载 widget 还没验过，
            // 现在明确提示，而不是转圈到超时。
            return Err(AppError::unsupported(
                "需要完成人机验证",
                "当前服务器要求 Cloudflare 人机验证，客户端还没接这一步",
            )
            .with_hint("请让管理员先关闭 Turnstile，或改用网页完成注册"));
        }
        other => {
            return Err(AppError::parse(
                "人机验证异常",
                format!("服务端返回了不认识的验证方式「{other}」"),
            ));
        }
    };

    let body = serde_json::json!({
        "username": username,
        "password": password,
        "deviceName": device_name(),
        "captcha": proof,
    });
    let resp = post("auth/register", body, None).await?;
    finish_login(state, resp)
}

/// 登出。即使服务端拒绝也要把本地清掉 —— 意图是「不再登录」，
/// 服务端那一步只是尽量把会话作废，失败不影响本地。
pub async fn logout(state: &AppState) -> Result<(), AppError> {
    let stored = read_stored(state);
    if let Some(stored) = stored.as_ref()
        && !stored.token.is_empty()
    {
        let _ = post_empty("auth/logout", &stored.token).await;
    }
    remove_file(state);
    Ok(())
}

/// 向服务端确认令牌还有效，顺便刷新一次过期时间。
///
/// 不做定时轮询：只在打开联机页时调一次，失败就按「未登录」展示。
pub async fn refresh(state: &AppState) -> Result<AccountView, AppError> {
    let Some(stored) = read_stored(state) else {
        return Ok(AccountView::signed_out(server()));
    };
    let resp = post("auth/refresh", serde_json::json!({}), Some(&stored.token)).await;
    match resp {
        Ok(resp) => finish_login(state, resp),
        Err(err) => {
            // 401 说明会话真没了（被封、别处退出、过期），本地必须跟着清；
            // 网络失败则保留本地令牌，下次还能用。
            if err.code == xcl_core::ErrorCode::Unauthorized {
                remove_file(state);
                return Ok(AccountView::signed_out(server()));
            }
            Err(err)
        }
    }
}

/// 提前多久续期。剩不到七天就用一次 `/auth/refresh`，免得玩家玩到一半令牌过期。
const RENEW_BEFORE: i64 = 7 * 24 * 3600;

/// 向服务端确认登录态是否还有效（只读，不延长有效期）。
///
/// 为什么必须有这一步：桥接对未授权的连接一个字节都不回 —— 令牌过期、被封、
/// 在别处退出之后，玩家能看到的只有「连不上」，永远不知道是登录没了。
/// 所以进联机页时主动问一次：401 就是真失效，本地清掉、界面切回登录卡。
///
/// 用 `/auth/me` 而不是 `/auth/refresh`：refresh 会把有效期往后推 30 天，
/// 每进一次页面就推一次，等于令牌永不过期 —— 那不是我们想要的。
pub async fn check(state: &AppState) -> Result<AccountView, AppError> {
    let Some(stored) = read_stored(state) else {
        return Ok(AccountView::signed_out(server()));
    };
    // 本地已经过期就不必出网，直接当作没登录
    if stored.expires_at <= now() {
        remove_file(state);
        return Ok(AccountView::signed_out(server()));
    }

    let url = format!("{}/v1/auth/me", require_server()?);
    let resp = client()
        .get(&url)
        .timeout(TIMEOUT)
        .bearer_auth(&stored.token)
        .send()
        .await
        .map_err(|err| network_error("确认登录态", err))?;

    if resp.status().as_u16() == 401 {
        remove_file(state);
        return Ok(AccountView::signed_out(server()));
    }

    let user: UserResponse = parse_json(resp).await?;

    // 快到期就顺手续一次（服务端会把有效期推后 30 天）。
    // 只在「进联机页」这个时机做，不做定时轮询 —— 桌面应用没必要常驻心跳。
    // 续期失败（网络抖动）就当没事发生，原来的令牌还能用几天。
    if stored.expires_at - now() < RENEW_BEFORE {
        if let Ok(fresh) = refresh(state).await {
            return Ok(fresh);
        }
    }

    Ok(AccountView {
        signed_in: true,
        username: Some(user.username),
        user_id: Some(user.id),
        expires_at: stored.expires_at,
        server: server(),
    })
}

// ---------- 内部 ----------
fn finish_login(state: &AppState, resp: SessionResponse) -> Result<AccountView, AppError> {
    let stored = Stored {
        token: resp.token,
        expires_at: resp.expires_at,
        user_id: resp.user.id,
        username: resp.user.username.clone(),
        bridge: resp.bridge,
    };
    write_stored(state, &stored)?;
    Ok(AccountView {
        signed_in: true,
        username: Some(stored.username),
        user_id: Some(stored.user_id),
        expires_at: stored.expires_at,
        server: server(),
    })
}

/// 用户名 / 密码的本地预检查。
///
/// 服务端当然也会检，这里是为了不让一次明显无效的请求出网：注册还要先算
/// 二十万次 sha256 呢，用户名带空格就别白算了。
fn check_credentials(
    username: &str,
    password: &str,
    registering: bool,
) -> Result<(String, String), AppError> {
    let username = username.trim();
    if username.len() < 3 || username.len() > 24 {
        return Err(AppError::validation("用户名长度不对", "用户名要 3 到 24 个字符"));
    }
    if !username.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-') {
        return Err(AppError::validation(
            "用户名只能用字母和数字",
            "支持 a-z A-Z 0-9 以及下划线、短横线",
        ));
    }
    if registering {
        if password.len() < 8 {
            return Err(AppError::validation(
                "密码太短",
                "至少 8 位。这个密码决定谁能进你的房间，别用 12345678",
            ));
        }
        if password.len() > 128 {
            return Err(AppError::validation("密码太长", "最多 128 个字符"));
        }
    } else if password.is_empty() {
        return Err(AppError::validation("请输入密码", "密码不能为空"));
    }
    Ok((username.to_string(), password.to_string()))
}

/// 算工作量证明：找最小的计数器，让 `sha256(challenge|nonce)` 前 `bits` 位全为 0。
///
/// `nonce` 的格式是「计数器按大端序编成 8 字节，再转 16 位小写 hex」，
/// 与服务端 `captcha.HexNonce` 一致。理论上服务端只比较字符串、任何编码都能过，
/// 但两端约定同一套写法，别人才可能照着文档写出能互操作的实现。
fn solve_pow(challenge: &str, bits: u32) -> Result<String, AppError> {
    use sha2::{Digest, Sha256};
    let started = std::time::Instant::now();
    let full = (bits / 8) as usize;
    let rem = (bits % 8) as u32;
    let mut counter: u64 = 0;
    loop {
        let nonce = format!("{counter:016x}");
        let mut hasher = Sha256::new();
        hasher.update(challenge.as_bytes());
        hasher.update(b"|");
        hasher.update(nonce.as_bytes());
        let sum = hasher.finalize();
        let head_ok = sum[..full].iter().all(|b| *b == 0);
        if head_ok && (rem == 0 || (sum[full] >> (8 - rem)) == 0) {
            return Ok(nonce);
        }
        counter += 1;
        // 每 6 万次看一眼表：正常情况根本走不到这里，纯粹是防难度被人调高后死转
        if counter % 65_536 == 0 && started.elapsed() > POW_DEADLINE {
            return Err(AppError::network(
                "人机验证超时",
                "算题时间超过 30 秒，可能是服务器把难度调高了",
            ));
        }
    }
}

async fn get_challenge() -> Result<CaptchaChallenge, AppError> {
    let url = format!("{}/v1/captcha", require_server()?);
    let resp = client()
        .get(&url)
        .timeout(TIMEOUT)
        .send()
        .await
        .map_err(|err| network_error("领取人机验证题", err))?;
    parse_json(resp).await
}

async fn post(
    path: &str,
    body: serde_json::Value,
    token: Option<&str>,
) -> Result<SessionResponse, AppError> {
    let url = format!("{}/v1/{}", require_server()?, path);
    let mut req = client().post(&url).timeout(TIMEOUT).json(&body);
    if let Some(token) = token {
        req = req.bearer_auth(token);
    }
    let resp = req
        .send()
        .await
        .map_err(|err| network_error("连接账号服务器", err))?;
    parse_json(resp).await
}

async fn post_empty(path: &str, token: &str) {
    // 退出登录是「尽力而为」，没配服务端就没什么可退的
    let Ok(server) = require_server() else {
        return;
    };
    let url = format!("{}/v1/{}", server, path);
    let _ = client()
        .post(&url)
        .timeout(TIMEOUT)
        .bearer_auth(token)
        .send()
        .await;
}

/// 统一解析响应：2xx 解目标结构，其余把服务端的 `message` 抬成能读的错误。
async fn parse_json<T: serde::de::DeserializeOwned>(resp: reqwest::Response) -> Result<T, AppError> {
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if status.is_success() {
        return serde_json::from_str(&text)
            .map_err(|err| AppError::parse("服务器返回了看不懂的内容", err.to_string()));
    }

    let detail = serde_json::from_str::<ErrorResponse>(&text)
        .map(|err| err.message)
        .unwrap_or_else(|_| format!("HTTP {}", status.as_u16()));

    Err(match status.as_u16() {
        401 => AppError::new(xcl_core::ErrorCode::Unauthorized, "登录已失效", detail),
        403 => AppError::validation("账号不可用", detail),
        409 => AppError::validation("用户名已被占用", detail),
        429 => AppError::network("操作太频繁", detail),
        400 => AppError::validation("请求被拒绝", detail),
        _ => AppError::network("账号服务器出错了", detail),
    })
}

fn network_error(action: &str, err: reqwest::Error) -> AppError {
    let message = if err.is_timeout() {
        "连接超时，检查网络或服务器是否在运行".to_string()
    } else if err.is_connect() {
        format!("{action}失败：连不上账号服务器")
    } else {
        format!("{action}失败：{err}")
    };
    AppError::network("账号服务器连不上", message).with_hint(format!("当前地址：{}", server()))
}

fn client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .timeout(TIMEOUT)
            // 账号接口是几 KB 的 JSON，不需要 HTTP/2 多路复用；
            // 但也不需要强制 1.1，交给 reqwest 默认即可（与下载客户端的取舍不同）。
            .user_agent(format!("XCL/{}", env!("CARGO_PKG_VERSION")))
            .build()
            .unwrap_or_default()
    })
}

fn server() -> String {
    std::env::var("XCL_ACCOUNT_SERVER")
        .ok()
        .map(|value| value.trim().trim_end_matches('/').to_string())
        .filter(|value| !value.is_empty())
        .or_else(|| CONFIGURED_SERVER.get().cloned())
        .filter(|value| !value.is_empty())
        .unwrap_or_default()
}

/// 真正要出网时用这个：没有地址就别拼出一个 `/v1/xxx` 的半成品 URL。
fn require_server() -> Result<String, AppError> {
    let url = server();
    if url.is_empty() {
        return Err(AppError::unsupported(
            "没有配置账号服务",
            "这个构建没有内置账号服务地址",
        )
        .with_hint("在config.toml的connect.accountServer里填上"));
    }
    Ok(url)
}

/// 节点桥接的地址（`host:port`）。
///
/// 优先用服务端下发的那个：账号接口挂在 Cloudflare 后面，域名解析到的是
/// CF 的边缘节点；桥接是裸 TCP，必须直连源站。客户端要是拿账号域名加个 11010
/// 去连，连到的是 CF，永远连不上 —— 所以这个地址只能由服务端告诉我们。
///
/// 没有下发时（本地调试、老版本服务端）回退到「账号服务主机名 + 11010」。
pub fn bridge_addr(state: &AppState) -> Option<String> {
    let stored = read_stored(state);
    pick_bridge(stored.and_then(|item| item.bridge).as_deref(), fallback_bridge_addr())
}

/// 服务端下发的优先，空/缺失才回退。抽成纯函数是为了能单测这层选择逻辑。
fn pick_bridge(stored: Option<&str>, fallback: Option<String>) -> Option<String> {
    match stored.map(str::trim).filter(|value| !value.is_empty()) {
        Some(value) => Some(value.to_string()),
        None => fallback,
    }
}

/// 服务端没下发时的兜底：账号服务的主机名 + 固定端口。
///
/// 只在本机调试（账号服务和桥接是同一个 127.0.0.1）时可靠。
fn fallback_bridge_addr() -> Option<String> {
    let raw = server();
    if raw.is_empty() {
        return None;
    }
    let without_scheme = raw.split("://").nth(1).unwrap_or(&raw);
    let host = without_scheme.split('/').next()?;
    // 账号服务是 HTTP 的端口，桥接不是同一个，只留主机部分
    let host = host.rsplit_once(':').map(|(head, _)| head).unwrap_or(host);
    Some(format!("{host}:11010"))
}

fn device_name() -> String {
    std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .unwrap_or_else(|_| "XCL".to_string())
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|dur| dur.as_secs() as i64)
        .unwrap_or(0)
}

fn account_path(state: &AppState) -> PathBuf {
    state.layout.root().join(ACCOUNT_FILE)
}

fn read_stored(state: &AppState) -> Option<Stored> {
    let text = std::fs::read_to_string(account_path(state)).ok()?;
    serde_json::from_str(&text).ok()
}

fn write_stored(state: &AppState, stored: &Stored) -> Result<(), AppError> {
    let path = account_path(state);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|err| AppError::io(&err, "创建账号目录"))?;
    }
    let text = serde_json::to_string_pretty(stored)
        .map_err(|err| AppError::parse("保存登录状态", err.to_string()))?;
    // 临时文件 + rename：写到一半断电也不会留下半份 JSON（配置写入也是同一套做法）
    let temp = path.with_extension("json.tmp");
    std::fs::write(&temp, text).map_err(|err| AppError::io(&err, "写入登录状态"))?;
    std::fs::rename(&temp, &path).map_err(|err| AppError::io(&err, "保存登录状态"))?;
    Ok(())
}

fn remove_file(state: &AppState) {
    let _ = std::fs::remove_file(account_path(state));
}

#[cfg(test)]
mod tests {
    use super::*;

    // 和服务端 captcha_test 用的是同一组期望：两边算法一旦漂移，这条会先炸。
    #[test]
    fn pow_matches_server_algorithm() {
        let nonce = solve_pow("hello", 8).expect("8 bit 应当立刻算出");
        let mut hasher = sha2::Sha256::new();
        use sha2::Digest;
        hasher.update(b"hello|");
        hasher.update(nonce.as_bytes());
        let sum = hasher.finalize();
        assert_eq!(sum[0], 0, "第一位必须是 0");
    }

    #[test]
    fn pow_solution_is_deterministic() {
        assert_eq!(solve_pow("abc", 10).unwrap(), solve_pow("abc", 10).unwrap());
    }

    /// nonce 必须是 16 位小写 hex（8 字节大端计数器），和服务端 `HexNonce` 对齐。
    #[test]
    fn pow_nonce_is_hex_counter() {
        let nonce = solve_pow("hello", 4).unwrap();
        assert_eq!(nonce.len(), 16, "8 字节 → 16 个 hex 字符：{nonce}");
        assert!(
            nonce.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
            "只允许小写 hex：{nonce}"
        );
    }

    #[test]
    fn credentials_are_checked_before_going_out() {
        assert!(check_credentials("ab", "password123", true).is_err());
        assert!(check_credentials("用中文名", "password123", true).is_err());
        assert!(check_credentials("xiaoxin", "short", true).is_err());
        assert!(check_credentials("xiaoxin", "password123", true).is_ok());
        // 登录不检查密码长度（老账号可能保持旧规则），只要求非空
        assert!(check_credentials("xiaoxin", "x", false).is_ok());
        assert!(check_credentials("xiaoxin", "", false).is_err());
    }

    #[test]
    fn server_address_trailing_slash_is_trimmed() {
        // 环境变量带不带尾斜杠都要能拼出正确的 URL
        // SAFETY: 单线程测试里改环境变量，且只在本用例内使用
        unsafe { std::env::set_var("XCL_ACCOUNT_SERVER", "http://127.0.0.1:1/") };
        assert_eq!(server(), "http://127.0.0.1:1");
        unsafe { std::env::remove_var("XCL_ACCOUNT_SERVER") };
        // 没有环境变量时回到配置里的地址（没配置就是空串）
        assert_eq!(server(), CONFIGURED_SERVER.get().cloned().unwrap_or_default());
    }

    /// 服务端下发了就以它为准 —— 账号域名走 Cloudflare，桥接必须直连源站，
    /// 两个地址不能混用。
    #[test]
    fn bridge_prefers_server_value() {
        assert_eq!(
            pick_bridge(Some("bridge.example.com:11010"), Some("api.example.com:11010".into())),
            Some("bridge.example.com:11010".to_string())
        );
        // 空串、纯空白都当作「没下发」
        assert_eq!(
            pick_bridge(Some("   "), Some("api.example.com:11010".into())),
            Some("api.example.com:11010".to_string())
        );
        assert_eq!(pick_bridge(None, Some("api.example.com:11010".into())), Some("api.example.com:11010".to_string()));
    }
}
