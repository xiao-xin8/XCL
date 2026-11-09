//! mock 集成测试：本地起一个最小 HTTP 服务模拟微软/Xbox/Minecraft 六个端点，
//! 把 `MsaClient` 的完整链路跑通。
//!
//! 为什么要有它：微软登录没法在 CI 里真机验证（要真账号 + Mojang 过审），
//! 所以协议层的行为 —— 请求形状（`d=` 前缀、`XBL3.0 x=…;…`）、响应解析、
//! 错误分支、轮换落盘顺序 —— 全部在这里锁死。改坏任何一条这些测试会先红。
//!
//! mock 用裸 TCP 手写（测试代码不引第三方 mock 库）：HTTP/1.1 +
//! `Connection: close`，一行一个应答，按路径排队。

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};

use xcl_auth::msa::{DevicePoll, MsaClient, MsaEndpoints};

// ---------- 裸 TCP mock ----------

/// 请求日志：每条形如 `POST /xbl auth=- body={…}`。
type MockLog = Arc<Mutex<Vec<String>>>;
/// 每个路径被打了几次（决定取应答队列的第几项）。
type MockHits = Arc<Mutex<HashMap<String, usize>>>;

struct MockResponse {
    status: u16,
    body: String,
}

fn ok(body: &str) -> MockResponse {
    MockResponse {
        status: 200,
        body: body.to_string(),
    }
}

fn err(status: u16, body: &str) -> MockResponse {
    MockResponse {
        status,
        body: body.to_string(),
    }
}

/// 起 mock。`routes`：路径 → 应答队列（按调用次数出队，用完停在最后一个）。
/// 返回 `(base_url, 请求日志, 路径命中计数)`。
fn spawn_mock(routes: HashMap<String, Vec<MockResponse>>) -> (String, MockLog, MockHits) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("绑定测试端口");
    let base = format!("http://{}", listener.local_addr().unwrap());
    let log: MockLog = Arc::new(Mutex::new(Vec::new()));
    let hits: MockHits = Arc::new(Mutex::new(HashMap::new()));
    let log2 = Arc::clone(&log);
    let hits2 = Arc::clone(&hits);
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            // 读到头结束，再按 Content-Length 补齐 body
            let mut buf = Vec::new();
            let mut tmp = [0u8; 2048];
            loop {
                match stream.read(&mut tmp) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => buf.extend_from_slice(&tmp[..n]),
                }
                if buf.windows(4).any(|w| w == b"\r\n\r\n") || buf.len() > 65536 {
                    break;
                }
            }
            let header_end = buf
                .windows(4)
                .position(|w| w == b"\r\n\r\n")
                .map(|p| p + 4)
                .unwrap_or(buf.len());
            let text = String::from_utf8_lossy(&buf[..header_end.min(buf.len())]).to_string();
            let mut content_length = 0usize;
            let mut auth = String::from("-");
            for line in text.lines() {
                let lower = line.to_ascii_lowercase();
                if let Some(v) = lower.strip_prefix("content-length:") {
                    content_length = v.trim().parse().unwrap_or(0);
                }
                if lower.starts_with("authorization:") {
                    auth = line[14..].trim().to_string();
                }
            }
            let mut body_bytes = buf.get(header_end..).unwrap_or(&[]).to_vec();
            while body_bytes.len() < content_length {
                match stream.read(&mut tmp) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => body_bytes.extend_from_slice(&tmp[..n]),
                }
            }
            let body = String::from_utf8_lossy(&body_bytes).to_string();

            let first = text.lines().next().unwrap_or("").to_string();
            let mut parts = first.split_whitespace();
            let method = parts.next().unwrap_or("").to_string();
            let path_raw = parts.next().unwrap_or("").to_string();
            let path = path_raw.split('?').next().unwrap_or("").to_string();

            let index = {
                let mut h = hits2.lock().unwrap();
                let e = h.entry(path.clone()).or_insert(0);
                let i = *e;
                *e += 1;
                i
            };
            log2.lock().unwrap().push(format!(
                "{method} {path} auth={auth} body={body}"
            ));

            let resp = routes
                .get(&path)
                .and_then(|q| q.get(index).or_else(|| q.last()))
                .map(|r| MockResponse {
                    status: r.status,
                    body: r.body.clone(),
                })
                .unwrap_or(err(404, r#"{"error":"no route"}"#));
            let out = format!(
                "HTTP/1.1 {} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                resp.status,
                resp.body.len(),
                resp.body
            );
            let _ = stream.write_all(out.as_bytes());
            let _ = stream.flush();
        }
    });
    (base, log, hits)
}

fn endpoints(base: &str) -> MsaEndpoints {
    MsaEndpoints {
        device_code: format!("{base}/devicecode"),
        token: format!("{base}/token"),
        xbl: format!("{base}/xbl"),
        xsts: format!("{base}/xsts"),
        mc_login: format!("{base}/mclogin"),
        mc_entitlements: format!("{base}/entitlements"),
        mc_profile: format!("{base}/profile"),
    }
}

fn route(map: &mut HashMap<String, Vec<MockResponse>>, path: &str, responses: Vec<MockResponse>) {
    map.insert(path.to_string(), responses);
}

fn logs(log: &MockLog) -> Vec<String> {
    log.lock().unwrap().clone()
}

/// 断言日志里有一条同时包含这些片段。
fn assert_log_has(log: &[String], path: &str, fragments: &[&str]) {
    let found = log.iter().any(|line| {
        line.contains(path) && fragments.iter().all(|f| line.contains(f))
    });
    assert!(
        found,
        "日志里没有同时包含 {path} 与 {fragments:?} 的请求：\n{}",
        log.join("\n")
    );
}

// ---------- 测试 ----------

/// 全链成功：设备码 → pending → slow_down → 成功 → XBL → XSTS → MC → 购买验证 → 档案。
#[tokio::test]
async fn full_device_code_chain_succeeds() {
    let mut routes = HashMap::new();
    route(
        &mut routes,
        "/devicecode",
        vec![ok(
            r#"{"device_code":"dev-1","user_code":"ABCD1234","verification_uri":"https://www.microsoft.com/link","message":"去输码","expires_in":900,"interval":1}"#,
        )],
    );
    route(
        &mut routes,
        "/token",
        vec![
            err(400, r#"{"error":"authorization_pending","error_description":"waiting"}"#),
            err(400, r#"{"error":"slow_down","error_description":"slow"}"#),
            ok(r#"{"access_token":"ms-token","refresh_token":"rt-1","expires_in":3600}"#),
        ],
    );
    route(
        &mut routes,
        "/xbl",
        vec![ok(r#"{"Token":"xbl-t","DisplayClaims":{"xui":[{"uhs":"hash9"}]}}"#)],
    );
    route(&mut routes, "/xsts", vec![ok(r#"{"Token":"xsts-t"}"#)]);
    route(
        &mut routes,
        "/mclogin",
        vec![ok(r#"{"access_token":"mc-t","expires_in":86400}"#)],
    );
    route(
        &mut routes,
        "/entitlements",
        vec![ok(r#"{"items":[{"name":"game_minecraft","signature":"s"}]}"#)],
    );
    route(
        &mut routes,
        "/profile",
        vec![ok(r#"{"id":"069A79F4-44E9-4726-A5EB-5722BA6CC974","name":"Notch"}"#)],
    );
    let (base, log, _hits) = spawn_mock(routes);
    let client = MsaClient::with_endpoints("client-id", endpoints(&base)).unwrap();

    // 1) 设备码
    let code = client.start_device_code().await.unwrap();
    assert_eq!(code.user_code, "ABCD1234");
    assert_eq!(code.interval, 1);

    // 2) 轮询：pending → slow_down → done
    assert!(matches!(
        client.poll_device_token(&code.device_code).await.unwrap(),
        DevicePoll::Pending
    ));
    assert!(matches!(
        client.poll_device_token(&code.device_code).await.unwrap(),
        DevicePoll::SlowDown
    ));
    let tokens = match client.poll_device_token(&code.device_code).await.unwrap() {
        DevicePoll::Done(t) => t,
        other => panic!("第三查该成功了，得到 {other:?}"),
    };
    assert_eq!(tokens.access_token, "ms-token");
    assert_eq!(tokens.refresh_token, "rt-1");

    // 3) 整条链
    let session = client.minecraft_chain(&tokens.access_token).await.unwrap();
    assert_eq!(session.mc_access_token, "mc-t");
    assert_eq!(session.name, "Notch");
    assert_eq!(session.uuid, "069a79f444e94726a5eb5722ba6cc974");
    assert_eq!(session.expires_in, 86400);

    // 请求形状逐条锁死
    let log = logs(&log);
    assert_log_has(&log, "/devicecode", &["client-id", "XboxLive.signin"]);
    // 轮询要带设备码与 grant_type（form 编码后冒号会变 %3A）
    assert_log_has(
        &log,
        "/token",
        &["grant_type=urn", "device_code=dev-1"],
    );
    // XBL：RpsTicket 带 d= 前缀、RelyingParty 是 http://
    assert_log_has(
        &log,
        "/xbl",
        &["d=ms-token", "user.auth.xboxlive.com", "http://auth.xboxlive.com"],
    );
    // XSTS：RETAIL + rp://
    assert_log_has(
        &log,
        "/xsts",
        &["RETAIL", "rp://api.minecraftservices.com/", "xbl-t"],
    );
    // MC 登录：identityToken 形态（无空格）
    assert_log_has(&log, "/mclogin", &["XBL3.0 x=hash9;xsts-t"]);
    // 购买验证与档案都带 Bearer
    assert_log_has(&log, "/entitlements", &["auth=Bearer mc-t"]);
    assert_log_has(&log, "/profile", &["auth=Bearer mc-t"]);
}

/// 轮询遇到「玩家点了取消」要报可读错误，不是裸协议错。
#[tokio::test]
async fn poll_declined_gives_friendly_error() {
    let mut routes = HashMap::new();
    route(
        &mut routes,
        "/token",
        vec![err(
            400,
            r#"{"error":"authorization_declined","error_description":"user said no"}"#,
        )],
    );
    let (base, _log, _hits) = spawn_mock(routes);
    let client = MsaClient::with_endpoints("client-id", endpoints(&base)).unwrap();
    let err = client.poll_device_token("dev").await.unwrap_err();
    let text = format!("{err:?}");
    assert!(text.contains("取消"), "要提示是玩家取消的：{text}");
}

/// XSTS 拒绝：固定码表的话术（没注册 Xbox 账户 / 地区 / 年龄 / 封禁）。
#[tokio::test]
async fn xsts_xerr_maps_to_pcl_messages() {
    for (code, expect) in [
        (2148916227u64, "封禁"),
        (2148916233, "Xbox"),
        (2148916235, "地区"),
        (2148916238, "年龄"),
    ] {
        let mut routes = HashMap::new();
        route(
            &mut routes,
            "/xbl",
            vec![ok(r#"{"Token":"xbl-t","DisplayClaims":{"xui":[{"uhs":"h"}]}}"#)],
        );
        route(
            &mut routes,
            "/xsts",
            vec![err(
                401,
                &format!(r#"{{"XErr":{code},"Message":"no","Redirect":""}}"#),
            )],
        );
        let (base, _log, _hits) = spawn_mock(routes);
        let client = MsaClient::with_endpoints("client-id", endpoints(&base)).unwrap();
        let err = client.minecraft_chain("ms").await.unwrap_err();
        let text = format!("{err:?}");
        assert!(text.contains(expect), "XErr {code} 应提示「{expect}」，实际：{text}");
    }
}

/// MC 登录 403 两分支：未过审 vs IP 风控，文案必须不同。
#[tokio::test]
async fn mc_login_403_distinguishes_unapproved_app_and_ip_risk() {
    // 分支一：Invalid app registration → 提到审核
    let mut routes = HashMap::new();
    route(
        &mut routes,
        "/xbl",
        vec![ok(r#"{"Token":"t","DisplayClaims":{"xui":[{"uhs":"h"}]}}"#)],
    );
    route(&mut routes, "/xsts", vec![ok(r#"{"Token":"s"}"#)]);
    route(
        &mut routes,
        "/mclogin",
        vec![err(
            403,
            r#"{"error":"forbidden","errorMessage":"Invalid app registration, see https://aka.ms/AppRegInfo"}"#,
        )],
    );
    let (base, _log, _hits) = spawn_mock(routes);
    let client = MsaClient::with_endpoints("client-id", endpoints(&base)).unwrap();
    let failure = client.minecraft_chain("ms").await.unwrap_err();
    assert!(
        format!("{failure:?}").contains("审核"),
        "未过审要提到审核：{failure:?}"
    );

    // 分支二：其余 403 → IP 风控
    let mut routes = HashMap::new();
    route(
        &mut routes,
        "/xbl",
        vec![ok(r#"{"Token":"t","DisplayClaims":{"xui":[{"uhs":"h"}]}}"#)],
    );
    route(&mut routes, "/xsts", vec![ok(r#"{"Token":"s"}"#)]);
    route(
        &mut routes,
        "/mclogin",
        vec![err(403, r#"{"error":"forbidden","errorMessage":"nope"}"#)],
    );
    let (base, _log, _hits) = spawn_mock(routes);
    let client = MsaClient::with_endpoints("client-id", endpoints(&base)).unwrap();
    let failure = client.minecraft_chain("ms").await.unwrap_err();
    let text = format!("{failure:?}");
    assert!(text.contains("IP") || text.contains("VPN"), "403 要提示 IP/VPN：{text}");
}

/// 购买验证：items 空 = 没买游戏，且这错误发生在取档案之前。
#[tokio::test]
async fn empty_entitlements_reports_not_owned() {
    let mut routes = HashMap::new();
    route(
        &mut routes,
        "/xbl",
        vec![ok(r#"{"Token":"t","DisplayClaims":{"xui":[{"uhs":"h"}]}}"#)],
    );
    route(&mut routes, "/xsts", vec![ok(r#"{"Token":"s"}"#)]);
    route(
        &mut routes,
        "/mclogin",
        vec![ok(r#"{"access_token":"mc","expires_in":100}"#)],
    );
    route(&mut routes, "/entitlements", vec![ok(r#"{"items":[]}"#)]);
    let (base, log, _hits) = spawn_mock(routes);
    let client = MsaClient::with_endpoints("client-id", endpoints(&base)).unwrap();
    let err = client.minecraft_chain("ms").await.unwrap_err();
    let text = format!("{err:?}");
    assert!(text.contains("没有 Minecraft") || text.contains("购买"), "{text}");
    // 没走到 profile
    assert!(
        !logs(&log).iter().any(|l| l.contains("/profile")),
        "购买验证失败不该继续取档案"
    );
}

/// profile 404 = 买了没建角色（与没买游戏分开）。
#[tokio::test]
async fn profile_404_reports_missing_profile() {
    let mut routes = HashMap::new();
    route(
        &mut routes,
        "/xbl",
        vec![ok(r#"{"Token":"t","DisplayClaims":{"xui":[{"uhs":"h"}]}}"#)],
    );
    route(&mut routes, "/xsts", vec![ok(r#"{"Token":"s"}"#)]);
    route(
        &mut routes,
        "/mclogin",
        vec![ok(r#"{"access_token":"mc","expires_in":100}"#)],
    );
    route(
        &mut routes,
        "/entitlements",
        vec![ok(r#"{"items":[{"name":"game_minecraft"}]}"#)],
    );
    route(&mut routes, "/profile", vec![err(404, r#"{"error":"NOT_FOUND"}"#)]);
    let (base, _log, _hits) = spawn_mock(routes);
    let client = MsaClient::with_endpoints("client-id", endpoints(&base)).unwrap();
    let err = client.minecraft_chain("ms").await.unwrap_err();
    assert!(format!("{err:?}").contains("角色"), "{err:?}");
}

/// ⚠️ 轮换落盘顺序：refresh 拿到新 refresh token 后必须先回调落盘，
/// 再跑 Xbox 链。链失败也不能把轮换后的 token 丢了（丢了 = 账号会被踢下线）。
#[tokio::test]
async fn rotated_refresh_token_is_saved_before_chain_even_on_chain_failure() {
    let mut routes = HashMap::new();
    route(
        &mut routes,
        "/token",
        vec![ok(r#"{"access_token":"ms-2","refresh_token":"rt-2","expires_in":3600}"#)],
    );
    route(&mut routes, "/xbl", vec![err(500, r#"{"error":"boom"}"#)]);
    let (base, log, _hits) = spawn_mock(routes);
    let client = MsaClient::with_endpoints("client-id", endpoints(&base)).unwrap();

    let saved: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    let saved2 = Arc::clone(&saved);
    let result = client
        .refresh_full("rt-old", move |rotated| {
            *saved2.lock().unwrap() = Some(rotated.to_string());
            Ok(())
        })
        .await;

    assert!(result.is_err(), "链失败整体要报错");
    assert_eq!(
        saved.lock().unwrap().as_deref(),
        Some("rt-2"),
        "轮换后的 refresh token 必须在链失败前就已落盘"
    );
    // 顺序断言：token（refresh）请求在 xbl 之前
    let log = logs(&log);
    let token_pos = log.iter().position(|l| l.contains("POST /token")).unwrap();
    let xbl_pos = log.iter().position(|l| l.contains("POST /xbl")).unwrap();
    assert!(token_pos < xbl_pos);
    // refresh 请求的 grant_type
    assert_log_has(&log, "/token", &["grant_type=refresh_token", "rt-old"]);
}

/// refresh_full 成功路径：新 token 落盘 + 完整会话返回。
#[tokio::test]
async fn refresh_full_success_returns_rotated_tokens_and_session() {
    let mut routes = HashMap::new();
    route(
        &mut routes,
        "/token",
        vec![ok(r#"{"access_token":"ms-2","refresh_token":"rt-2","expires_in":3600}"#)],
    );
    route(
        &mut routes,
        "/xbl",
        vec![ok(r#"{"Token":"x","DisplayClaims":{"xui":[{"uhs":"h2"}]}}"#)],
    );
    route(&mut routes, "/xsts", vec![ok(r#"{"Token":"s2"}"#)]);
    route(
        &mut routes,
        "/mclogin",
        vec![ok(r#"{"access_token":"mc-2","expires_in":86400}"#)],
    );
    route(
        &mut routes,
        "/entitlements",
        vec![ok(r#"{"items":[{"name":"game_minecraft"}]}"#)],
    );
    route(
        &mut routes,
        "/profile",
        vec![ok(r#"{"id":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","name":"Alex"}"#)],
    );
    let (base, log, _hits) = spawn_mock(routes);
    let client = MsaClient::with_endpoints("client-id", endpoints(&base)).unwrap();

    let saved: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    let saved2 = Arc::clone(&saved);
    let out = client
        .refresh_full("rt-old", move |rotated| {
            *saved2.lock().unwrap() = Some(rotated.to_string());
            Ok(())
        })
        .await
        .unwrap();

    assert_eq!(saved.lock().unwrap().as_deref(), Some("rt-2"));
    assert_eq!(out.tokens.refresh_token, "rt-2");
    assert_eq!(out.session.mc_access_token, "mc-2");
    assert_eq!(out.session.name, "Alex");
    assert_log_has(&logs(&log), "/mclogin", &["XBL3.0 x=h2;s2"]);
}

/// refresh 时服务端说「必须重新登录」→ 明确的过期错误。
#[tokio::test]
async fn refresh_invalid_grant_requires_relogin() {
    let mut routes = HashMap::new();
    route(
        &mut routes,
        "/token",
        vec![err(
            400,
            r#"{"error":"invalid_grant","error_description":"AADSTS70000: refresh_token is not valid"}"#,
        )],
    );
    let (base, _log, _hits) = spawn_mock(routes);
    let client = MsaClient::with_endpoints("client-id", endpoints(&base)).unwrap();
    let err = client.refresh("rt-dead").await.unwrap_err();
    let text = format!("{err:?}");
    assert!(text.contains("过期") || text.contains("重新登录"), "{text}");
}

/// 设备码接口本身挂了（代理 HTML）→ 报可读错误而不是解析 panic。
#[tokio::test]
async fn device_code_html_error_is_readable() {
    let mut routes = HashMap::new();
    route(
        &mut routes,
        "/devicecode",
        vec![err(502, "<html><body>Bad Gateway</body></html>")],
    );
    let (base, _log, _hits) = spawn_mock(routes);
    let client = MsaClient::with_endpoints("client-id", endpoints(&base)).unwrap();
    let err = client.start_device_code().await.unwrap_err();
    let text = format!("{err:?}");
    assert!(text.contains("微软") || text.contains("登录"), "{text}");
}
