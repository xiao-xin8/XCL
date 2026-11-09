//! 微软账号认证（MSA → Xbox Live → XSTS → Minecraft 服务）。
//!
//! 设备代码流（OAuth 2.0 Device Authorization Grant）：启动器弹一个短
//! 验证码，玩家去浏览器输码登录，启动器这边轮询等结果。不用嵌浏览器、
//! 不用注册回调地址，桌面程序最省事的一条路。
//!
//! 完整链路六步：
//!
//! | 步骤 | 端点 | 用途 |
//! |---|---|---|
//! | 1 | `POST …/consumers/oauth2/v2.0/devicecode` | 取验证码 |
//! | 2 | `POST …/consumers/oauth2/v2.0/token` | 轮询换微软令牌（或 refresh 续期） |
//! | 3 | `POST user.auth.xboxlive.com/user/authenticate` | 微软令牌 → Xbox 令牌 |
//! | 4 | `POST xsts.auth.xboxlive.com/xsts/authorize` | Xbox 令牌 → XSTS 令牌 |
//! | 5 | `POST api.minecraftservices.com/authentication/login_with_xbox` | XSTS → 游戏令牌 |
//! | 5b | `GET …/entitlements/mcstore` | 验证买了游戏 |
//! | 6 | `GET …/minecraft/profile` | 取 UUID 与角色名 |
//!
//! 游戏令牌约 24 小时过期，微软 refresh token 是长期的 —— 平时续期只用
//! refresh token。⚠️ 微软的 refresh token 是轮换制：用一次旧的作废、
//! 响应带新的。所以续期必须「先落盘新 token，再跑 Xbox 链」（`refresh_full`
//! 的 `on_rotated` 回调就是为此存在），链中途失败也不能把新 token 丢掉。

use serde::{Deserialize, Serialize};
use xcl_core::AppError;

// ---------- 端点（测试可注入本地 mock） ----------

/// 链路上的全部 URL。默认是生产地址；单测/集成测试注入本地 mock 用。
#[derive(Debug, Clone)]
pub struct MsaEndpoints {
    pub device_code: String,
    pub token: String,
    pub xbl: String,
    pub xsts: String,
    pub mc_login: String,
    pub mc_entitlements: String,
    pub mc_profile: String,
}

impl Default for MsaEndpoints {
    fn default() -> Self {
        Self {
            device_code: "https://login.microsoftonline.com/consumers/oauth2/v2.0/devicecode"
                .to_string(),
            token: "https://login.microsoftonline.com/consumers/oauth2/v2.0/token".to_string(),
            xbl: "https://user.auth.xboxlive.com/user/authenticate".to_string(),
            xsts: "https://xsts.auth.xboxlive.com/xsts/authorize".to_string(),
            mc_login: "https://api.minecraftservices.com/authentication/login_with_xbox"
                .to_string(),
            mc_entitlements: "https://api.minecraftservices.com/entitlements/mcstore".to_string(),
            mc_profile: "https://api.minecraftservices.com/minecraft/profile".to_string(),
        }
    }
}

/// 要的授权范围。`offline_access` 才给 refresh token（续期全靠它）。
const SCOPE: &str = "XboxLive.signin offline_access";
/// 验证码页面要中文提示，设备码接口认这个查询参数。
const DEVICE_CODE_MKT: &str = "zh-Hans";

// ---------- 对外数据 ----------

/// 设备代码流第一步的返回：给玩家看的验证码与登录地址。
#[derive(Debug, Clone)]
pub struct DeviceCode {
    /// 轮询用的凭据，不给玩家看。
    pub device_code: String,
    /// 玩家要在网页里输入的短码，如 `A1B2C3D4`。
    pub user_code: String,
    /// 输码的网址（一般是 https://www.microsoft.com/link）。
    pub verification_uri: String,
    /// 服务端给的整句指引（已本地化），直接显示即可。
    pub message: String,
    /// 验证码有效期（秒）。
    pub expires_in: u64,
    /// 建议的轮询间隔（秒）。
    pub interval: u64,
}

/// 微软层的令牌。游戏层的令牌在 `McSession` 里。
#[derive(Debug, Clone)]
pub struct MsaTokens {
    pub access_token: String,
    pub refresh_token: String,
    /// 微软令牌有效期（秒），约 1 小时。续期链不看它，记着备用。
    pub expires_in: u64,
}

/// 链路走完后的游戏会话：启动要的就是它。
#[derive(Debug, Clone)]
pub struct McSession {
    /// Minecraft 服务的 access token（`--accessToken` 传它），约 24 小时。
    pub mc_access_token: String,
    /// 游戏令牌有效期（秒），通常 86400。
    pub expires_in: u64,
    /// 32 位十六进制、无横杠。
    pub uuid: String,
    /// 角色名。
    pub name: String,
}

/// 刷新后的完整结果：轮换过的微软令牌 + 新游戏会话。
#[derive(Debug, Clone)]
pub struct RefreshedSession {
    /// 轮换后的微软令牌（旧的已作废）。
    pub tokens: MsaTokens,
    pub session: McSession,
}

/// 轮询一次设备码的结果。
#[derive(Debug, Clone)]
pub enum DevicePoll {
    /// 玩家还没在网页上输码，继续等。
    Pending,
    /// 微软让我们慢点问（问太勤了），下一次间隔要加长（RFC 8628：永久 +5s）。
    SlowDown,
    /// 玩家登录成功，拿到微软令牌。
    Done(MsaTokens),
}

// ---------- 客户端 ----------

/// 微软认证客户端。`client_id` 是 Azure 应用注册的 Application (client) ID，
/// 构建期注入（`bake.toml`）—— 这是应用自己的身份，不是配置项。
pub struct MsaClient {
    http: reqwest::Client,
    client_id: String,
    endpoints: MsaEndpoints,
}

impl MsaClient {
    /// 没配 client id 时明确报错，而不是带着空 id 打微软接口换来一头雾水。
    pub fn new(client_id: &str) -> Result<Self, AppError> {
        Self::with_endpoints(client_id, MsaEndpoints::default())
    }

    /// 指定端点（测试指向本地 mock 用）。
    pub fn with_endpoints(client_id: &str, endpoints: MsaEndpoints) -> Result<Self, AppError> {
        let client_id = client_id.trim();
        if client_id.is_empty() {
            return Err(AppError::unsupported(
                "这个版本没有配置微软登录",
                "打包时缺少微软应用的 client id，先在 bake.toml 里配置再重新编译",
            ));
        }
        let http = reqwest::Client::builder()
            .user_agent(format!(
                "XCL/{} (Minecraft Launcher)",
                env!("CARGO_PKG_VERSION")
            ))
            .connect_timeout(std::time::Duration::from_secs(10))
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .map_err(|e| AppError::network("初始化登录组件失败", e.to_string()))?;
        Ok(Self {
            http,
            client_id: client_id.to_string(),
            endpoints,
        })
    }

    /// 第一步：取设备码（验证码 + 登录地址）。
    pub async fn start_device_code(&self) -> Result<DeviceCode, AppError> {
        let url = format!("{}?mkt={DEVICE_CODE_MKT}", self.endpoints.device_code);
        let resp = self
            .http
            .post(&url)
            .form(&[("client_id", self.client_id.as_str()), ("scope", SCOPE)])
            .send()
            .await
            .map_err(|e| AppError::network("连不上微软登录服务", e.to_string()))?;

        let status = resp.status().as_u16();
        let text = resp.text().await.unwrap_or_default();
        if !(200..300).contains(&status) {
            return Err(token_error_body(&text, status));
        }
        let raw: DeviceCodeResponse = serde_json::from_str(&text)
            .map_err(|e| AppError::parse("登录服务响应无法解析", e.to_string()))?;
        Ok(DeviceCode {
            device_code: raw.device_code,
            user_code: raw.user_code,
            verification_uri: raw.verification_uri,
            message: raw.message,
            expires_in: raw.expires_in,
            interval: raw.interval.max(1),
        })
    }

    /// 轮询一次：玩家在网页上输码了吗。
    ///
    /// 调用方负责按 `interval` 控制节奏 —— 这里只忠实翻译服务端的回答。
    /// `authorization_pending` / `slow_down` 是「还没好」（注意它们是 HTTP 400
    /// 但属于答案不是故障，所以先读 body 再判 status）；
    /// 玩家点了取消、验证码过期等终止性错误直接抛。
    pub async fn poll_device_token(&self, device_code: &str) -> Result<DevicePoll, AppError> {
        let resp = self
            .http
            .post(&self.endpoints.token)
            .form(&[
                ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
                ("client_id", self.client_id.as_str()),
                ("device_code", device_code),
                ("scope", SCOPE),
            ])
            .send()
            .await
            .map_err(|e| AppError::network("连不上微软登录服务", e.to_string()))?;

        let status = resp.status().as_u16();
        let text = resp.text().await.unwrap_or_default();
        if status == 200 {
            let raw: TokenResponse = serde_json::from_str(&text)
                .map_err(|e| AppError::parse("登录服务响应无法解析", e.to_string()))?;
            return Ok(DevicePoll::Done(MsaTokens {
                access_token: raw.access_token,
                refresh_token: raw.refresh_token.unwrap_or_default(),
                expires_in: raw.expires_in.unwrap_or(3600),
            }));
        }
        // 先看 body 里的 OAuth error —— pending/slow_down 都是「答案」
        if let Some(err) = parse_oauth_error(&text) {
            return match err.error.as_str() {
                "authorization_pending" => Ok(DevicePoll::Pending),
                "slow_down" => Ok(DevicePoll::SlowDown),
                "authorization_declined" | "access_denied" => Err(AppError::validation(
                    "登录被取消",
                    "你在网页上取消了这次登录",
                )
                .with_hint("想继续就重新点一次登录")),
                "expired_token" => Err(AppError::validation(
                    "验证码过期了",
                    "登录用时太长，验证码已经失效",
                )
                .with_hint("回到启动器重新点一次登录")),
                "bad_verification_code" => Err(AppError::validation(
                    "验证码不正确",
                    "网页里输入的验证码不对",
                )
                .with_hint("核对一下启动器显示的验证码，或回启动器重新取一个")),
                _ => Err(token_error_body(&text, status)),
            };
        }
        Err(token_error_body(&text, status))
    }

    /// 用 refresh token 静默换新的微软令牌。玩家无感，启动前续期用。
    ///
    /// ⚠️ 返回的 `refresh_token` 是轮换后的新值（旧的已作废），
    /// 调用方必须立刻落盘，否则账号会被踢下线。要跑完整链就用
    /// [`Self::refresh_full`]，它自带「先落盘再跑链」的顺序保证。
    pub async fn refresh(&self, refresh_token: &str) -> Result<MsaTokens, AppError> {
        let resp = self
            .http
            .post(&self.endpoints.token)
            .form(&[
                ("grant_type", "refresh_token"),
                ("client_id", self.client_id.as_str()),
                ("refresh_token", refresh_token),
                ("scope", SCOPE),
            ])
            .send()
            .await
            .map_err(|e| AppError::network("连不上微软登录服务", e.to_string()))?;

        let status = resp.status().as_u16();
        let text = resp.text().await.unwrap_or_default();
        if status == 200 {
            let raw: TokenResponse = serde_json::from_str(&text)
                .map_err(|e| AppError::parse("登录服务响应无法解析", e.to_string()))?;
            return Ok(MsaTokens {
                access_token: raw.access_token,
                // 有的应答不带新 refresh token，沿用旧的
                refresh_token: raw
                    .refresh_token
                    .filter(|t| !t.is_empty())
                    .unwrap_or_else(|| refresh_token.to_string()),
                expires_in: raw.expires_in.unwrap_or(3600),
            });
        }
        // refresh 失效的几种说法：
        // must sign in again / password expired / refresh_token is not valid / expired
        let lower = text.to_lowercase();
        let must_relogin = lower.contains("must sign in again")
            || lower.contains("password expired")
            || (lower.contains("refresh_token") && lower.contains("is not valid"))
            || parse_oauth_error(&text).is_some_and(|e| {
                matches!(
                    e.error.as_str(),
                    "invalid_grant" | "interaction_required" | "expired_token"
                )
            });
        if must_relogin {
            return Err(AppError::validation(
                "登录状态已过期",
                "微软登录的续期凭据失效了，需要重新登录一次",
            )
            .with_hint("在账号页对这个微软账号重新登录，以后就又能自动续期了"));
        }
        Err(token_error_body(&text, status))
    }

    /// 续期 + 跑完整链，且保证「轮换后的 refresh token 先落盘」。
    ///
    /// 顺序是硬性的：拿到新 refresh token 立刻调 `on_rotated`（调用方负责
    /// 写盘），然后再跑 Xbox/XSTS/MC 链。链失败时新 token 已经存好，
    /// 重试续期不会失效，顺序不能反。
    pub async fn refresh_full(
        &self,
        refresh_token: &str,
        on_rotated: impl FnOnce(&str) -> Result<(), AppError>,
    ) -> Result<RefreshedSession, AppError> {
        let tokens = self.refresh(refresh_token).await?;
        // 落盘失败要立刻停：带着没存的 token 继续跑链，链一失败 token 就丢了
        on_rotated(&tokens.refresh_token)?;
        let session = self.minecraft_chain(&tokens.access_token).await?;
        Ok(RefreshedSession { tokens, session })
    }

    /// 第三～六步：微软令牌 → Xbox → 游戏令牌 → 验证购买 → 角色档案。
    ///
    /// 登录完成和静默续期走的是同一条链，只是入口令牌来源不同。
    pub async fn minecraft_chain(&self, ms_access_token: &str) -> Result<McSession, AppError> {
        let xbl = self.xbl_authenticate(ms_access_token).await?;
        let xsts = self.xsts_authorize(&xbl.token).await?;
        let identity = format!("XBL3.0 x={};{}", xbl.uhs, xsts);
        let mc = self.minecraft_login(&identity).await?;
        self.minecraft_entitlements(&mc.access_token).await?;
        let (uuid, name) = self.minecraft_profile(&mc.access_token).await?;
        Ok(McSession {
            mc_access_token: mc.access_token,
            expires_in: mc.expires_in.unwrap_or(86400),
            uuid,
            name,
        })
    }

    // ---- Xbox 链路 ----

    async fn xbl_authenticate(&self, ms_access_token: &str) -> Result<XblTicket, AppError> {
        #[derive(Serialize)]
        struct Props {
            #[serde(rename = "AuthMethod")]
            auth_method: &'static str,
            #[serde(rename = "SiteName")]
            site_name: &'static str,
            #[serde(rename = "RpsTicket")]
            rps_ticket: String,
        }
        #[derive(Serialize)]
        struct Body {
            #[serde(rename = "Properties")]
            properties: Props,
            #[serde(rename = "RelyingParty")]
            relying_party: &'static str,
            #[serde(rename = "TokenType")]
            token_type: &'static str,
        }
        // `d=` 前缀是 Xbox Live 的老规矩：RPS 票据必须带（已带则不重复加）
        let ticket = if ms_access_token.starts_with("d=") {
            ms_access_token.to_string()
        } else {
            format!("d={ms_access_token}")
        };
        let body = Body {
            properties: Props {
                auth_method: "RPS",
                site_name: "user.auth.xboxlive.com",
                rps_ticket: ticket,
            },
            relying_party: "http://auth.xboxlive.com",
            token_type: "JWT",
        };
        let resp = self
            .http
            .post(&self.endpoints.xbl)
            .json(&body)
            .send()
            .await
            .map_err(|e| AppError::network("连不上 Xbox 服务", e.to_string()))?;
        let status = resp.status().as_u16();
        let text = resp.text().await.unwrap_or_default();
        if !(200..300).contains(&status) {
            return Err(service_error("登录 Xbox 失败", status, &text));
        }
        let parsed: XblResponse = serde_json::from_str(&text)
            .map_err(|e| AppError::parse("Xbox 服务响应无法解析", e.to_string()))?;
        let uhs = parsed
            .display_claims
            .xui
            .first()
            .map(|x| x.uhs.clone())
            .unwrap_or_default();
        if uhs.is_empty() {
            return Err(AppError::parse(
                "Xbox 服务响应缺少用户标识",
                "DisplayClaims.xui 里没有 uhs",
            ));
        }
        Ok(XblTicket {
            token: parsed.token,
            uhs,
        })
    }

    async fn xsts_authorize(&self, xbl_token: &str) -> Result<String, AppError> {
        #[derive(Serialize)]
        struct Props<'a> {
            #[serde(rename = "SandboxId")]
            sandbox_id: &'static str,
            #[serde(rename = "UserTokens")]
            user_tokens: Vec<&'a str>,
        }
        #[derive(Serialize)]
        struct Body<'a> {
            #[serde(rename = "Properties")]
            properties: Props<'a>,
            #[serde(rename = "RelyingParty")]
            relying_party: &'static str,
            #[serde(rename = "TokenType")]
            token_type: &'static str,
        }
        let body = Body {
            properties: Props {
                sandbox_id: "RETAIL",
                user_tokens: vec![xbl_token],
            },
            relying_party: "rp://api.minecraftservices.com/",
            token_type: "JWT",
        };
        let resp = self
            .http
            .post(&self.endpoints.xsts)
            .json(&body)
            .send()
            .await
            .map_err(|e| AppError::network("连不上 Xbox 服务", e.to_string()))?;
        let status = resp.status().as_u16();
        let text = resp.text().await.unwrap_or_default();
        if status != 200 {
            // XSTS 的拒绝理由在 XErr 里（JSON）；body 挂了也可能是纯文本带码
            if let Some(code) = parse_xerr(&text) {
                return Err(xerr_error(code));
            }
            return Err(service_error("登录 Xbox 失败", status, &text));
        }
        let parsed: XstsResponse = serde_json::from_str(&text)
            .map_err(|e| AppError::parse("Xbox 服务响应无法解析", e.to_string()))?;
        Ok(parsed.token)
    }

    async fn minecraft_login(&self, identity_token: &str) -> Result<McLoginResponse, AppError> {
        #[derive(Serialize)]
        struct Body<'a> {
            #[serde(rename = "identityToken")]
            identity_token: &'a str,
        }
        let resp = self
            .http
            .post(&self.endpoints.mc_login)
            .json(&Body { identity_token })
            .send()
            .await
            .map_err(|e| AppError::network("连不上 Minecraft 服务", e.to_string()))?;
        let status = resp.status().as_u16();
        let text = resp.text().await.unwrap_or_default();
        if status == 200 {
            return serde_json::from_str(&text)
                .map_err(|e| AppError::parse("Minecraft 服务响应无法解析", e.to_string()));
        }
        match status {
            429 => Err(AppError::network(
                "登录尝试太过频繁",
                "Mojang 限制了短时间内的登录次数",
            )
            .with_hint("等几分钟后再试")),
            // 新注册的应用没过 Mojang 审核就是这一条 —— 单独给话术，
            // 否则玩家只会看到一句没头没脑的 403
            403 if text.contains("Invalid app registration") => Err(AppError::network(
                "这个启动器的微软登录还没通过 Mojang 审核",
                "Mojang 需要逐个批准第三方启动器的应用，未批准前换不到游戏令牌",
            )
            .with_hint("审核通过后就能正常登录，通常需要几天")),
            // 其余 403 按 IP 风控处理
            403 => Err(AppError::network(
                "当前 IP 的登录尝试异常",
                "Mojang 拒绝了这个网络位置的登录请求",
            )
            .with_hint("如果你使用了 VPN 或加速器，请把它们关掉或更换节点后再试")),
            503 => Err(AppError::network(
                "Mojang 服务器出现问题",
                "Mojang 的登录服务暂时不可用",
            )
            .with_hint("稍后重试，这不是启动器或你的网络的问题")),
            _ => {
                if text.contains("ACCOUNT_SUSPENDED") {
                    return Err(AppError::validation(
                        "该账号已被微软封禁",
                        "这个微软账号无法用于登录",
                    ));
                }
                Err(service_error("登录 Minecraft 服务失败", status, &text))
            }
        }
    }

    /// 验证账号持有 Minecraft（也会刷新 XGP 状态）。
    /// `items` 为空 = 没买游戏或订阅到期。这一步能把「没买游戏」和
    /// 「买了没建角色」（profile 404）分开，缺了它两种情况会混成一种。
    async fn minecraft_entitlements(&self, mc_access_token: &str) -> Result<(), AppError> {
        let resp = self
            .http
            .get(&self.endpoints.mc_entitlements)
            .bearer_auth(mc_access_token)
            .send()
            .await
            .map_err(|e| AppError::network("连不上 Minecraft 服务", e.to_string()))?;
        let status = resp.status().as_u16();
        let text = resp.text().await.unwrap_or_default();
        if !(200..300).contains(&status) {
            return Err(service_error("验证游戏拥有权失败", status, &text));
        }
        let parsed: EntitlementsResponse = serde_json::from_str(&text)
            .map_err(|e| AppError::parse("Minecraft 服务响应无法解析", e.to_string()))?;
        if parsed.items.is_empty() {
            return Err(AppError::validation(
                "这个账号没有 Minecraft",
                "你尚未购买正版 Minecraft，或者 Xbox Game Pass 已到期",
            )
            .with_hint("确认这个账号买过游戏（或订阅有效）；要换号就在网页上换个账号登录"));
        }
        Ok(())
    }

    async fn minecraft_profile(
        &self,
        mc_access_token: &str,
    ) -> Result<(String, String), AppError> {
        let resp = self
            .http
            .get(&self.endpoints.mc_profile)
            .bearer_auth(mc_access_token)
            .send()
            .await
            .map_err(|e| AppError::network("连不上 Minecraft 服务", e.to_string()))?;
        let status = resp.status().as_u16();
        let text = resp.text().await.unwrap_or_default();
        match status {
            200 => {
                let parsed: McProfileResponse = serde_json::from_str(&text)
                    .map_err(|e| AppError::parse("Minecraft 服务响应无法解析", e.to_string()))?;
                let uuid = parsed.id.replace('-', "").to_lowercase();
                if uuid.len() != 32 || parsed.name.is_empty() {
                    return Err(AppError::parse(
                        "Minecraft 档案数据不完整",
                        "服务端返回的 UUID 或角色名是空的",
                    ));
                }
                Ok((uuid, parsed.name))
            }
            429 => Err(AppError::network(
                "登录尝试太过频繁",
                "Mojang 限制了短时间内的登录次数",
            )
            .with_hint("等几分钟后再试")),
            // 买了游戏但还没在官网创建角色（404）
            404 => Err(AppError::validation(
                "还没有 Minecraft 角色",
                "请先创建 Minecraft 玩家档案，然后再重新登录",
            )
            .with_hint("去 minecraft.net 登录一次，创建角色名后再回来登录")),
            403 => Err(AppError::validation(
                "这个账号没有 Minecraft",
                "登录成功了，但这个微软账号下没有 Minecraft: Java Edition",
            )
            .with_hint("确认这个账号买过游戏（或订阅有效）；要换号就在网页上换个账号登录")),
            _ => Err(service_error("读取 Minecraft 角色失败", status, &text)),
        }
    }
}

// ---------- 响应体 ----------

#[derive(Debug, Deserialize)]
struct DeviceCodeResponse {
    device_code: String,
    user_code: String,
    verification_uri: String,
    #[serde(default)]
    message: String,
    expires_in: u64,
    interval: u64,
}

#[derive(Debug, Deserialize)]
struct TokenResponse {
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    expires_in: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct XblResponse {
    #[serde(rename = "Token")]
    token: String,
    #[serde(rename = "DisplayClaims")]
    display_claims: DisplayClaims,
}

#[derive(Debug, Deserialize)]
struct DisplayClaims {
    xui: Vec<XuiEntry>,
}

#[derive(Debug, Deserialize)]
struct XuiEntry {
    uhs: String,
}

#[derive(Debug, Deserialize)]
struct XstsResponse {
    #[serde(rename = "Token")]
    token: String,
}

#[derive(Debug, Deserialize)]
struct McLoginResponse {
    access_token: String,
    #[serde(default)]
    expires_in: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct EntitlementsResponse {
    #[serde(default)]
    items: Vec<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct McProfileResponse {
    id: String,
    name: String,
}

#[derive(Debug, Deserialize)]
struct OAuthError {
    error: String,
    #[serde(default, rename = "error_description")]
    description: String,
}

#[derive(Debug, Deserialize)]
struct XErrResponse {
    #[serde(rename = "XErr")]
    code: u64,
}

/// 一个已经换到的 Xbox 票据（token + 账号哈希）。
struct XblTicket {
    token: String,
    uhs: String,
}

// ---------- 翻译辅助 ----------

fn parse_oauth_error(body: &str) -> Option<OAuthError> {
    serde_json::from_str(body).ok()
}

/// XErr 可能是 JSON 字段，也可能 body 直接是纯文本带码（代理/网关形态）。
/// JSON 优先，回落文本包含匹配。
fn parse_xerr(body: &str) -> Option<u64> {
    if let Ok(parsed) = serde_json::from_str::<XErrResponse>(body) {
        return Some(parsed.code);
    }
    [
        2148916227u64,
        2148916233,
        2148916235,
        2148916236,
        2148916237,
        2148916238,
    ]
    .into_iter()
    .find(|code| body.contains(&code.to_string()))
}

/// XSTS 拒绝的固定码表；
/// 未收录的码带原始码给通用提示，不赌死映射（不同实现说法不一）。
fn xerr_error(code: u64) -> AppError {
    match code {
        2148916227 => AppError::validation(
            "该账号已被微软封禁",
            "这个微软账号无法登录",
        ),
        2148916233 => AppError::validation(
            "你尚未注册 Xbox 账户",
            "Xbox Live 需要一个基本档案才能登录游戏",
        )
        .with_hint("去 xbox.com 用这个微软账号创建一次档案（起个 Xbox ID）再回来登录"),
        2148916235 => AppError::validation(
            "所在国家或地区无法登录微软账号",
            "Xbox Live 在这个账号所属的地区不可用",
        )
        .with_hint("使用加速器或 VPN 改善网络环境后再试"),
        2148916238 => AppError::validation(
            "该账号年龄不足",
            "需要先修改出生日期或由家庭组的大人放行，然后才能登录",
        )
        .with_hint("把出生日期改成 18 岁以上，等一分钟再回来登录"),
        // 2148916236/37 等：说法不一（成年验证/地区），不赌死映射
        _ => AppError::network(
            "Xbox 登录被拒绝",
            format!("Xbox 服务返回了错误码 {code}"),
        )
        .with_hint("稍后重试；持续失败去 xbox.com 检查账号状态"),
    }
}

/// token 端点的错误体翻译。先认几个微软专用的说法，
/// 其余走 OAuth error 字段。
fn token_error_body(body: &str, status: u16) -> AppError {
    if body.contains("AADSTS70000") {
        return AppError::validation(
            "需要使用密码登录",
            "这个账号不能只靠网页确认身份",
        )
        .with_hint(
            "登录时选「其他登录方法」→「使用我的密码」；没有该选项就先去 \
             account.live.com「设置密码」，设置完再登录",
        );
    }
    if body.contains("Account security interrupt") {
        return AppError::validation(
            "该账号由于安全问题无法登录",
            "微软要求先处理账号的安全验证",
        )
        .with_hint("前往微软账户页查看并完成验证后再试");
    }
    if body.contains("service abuse") {
        return AppError::validation("该账号已被微软封禁", "这个微软账号无法登录");
    }
    if let Some(err) = parse_oauth_error(body) {
        return AppError::network(
            "微软登录失败",
            format!("{}（{}）", err.error, err.description),
        )
        .with_hint("稍后重试；持续失败的话检查网络后重新登录");
    }
    AppError::network(
        "微软登录服务响应异常",
        format!("HTTP {status}，返回的不是预期的登录结果"),
    )
    .with_hint("稍后重试；持续失败可能是网络拦截了微软的登录域名")
}

/// 通用非 OAuth 服务错误（Xbox / Minecraft 层）。
fn service_error(title: &str, status: u16, body: &str) -> AppError {
    // 服务端偶尔会带一句 errorMessage，比裸状态码有用；body 可能是整页
    // HTML（代理），截短再展示
    let detail = serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|v| {
            v.get("errorMessage")
                .or_else(|| v.get("Message"))
                .or_else(|| v.get("error"))
                .and_then(|m| m.as_str())
                .map(str::to_string)
        })
        .unwrap_or_else(|| {
            let trimmed: String = body.chars().take(200).collect();
            if trimmed.trim().is_empty() {
                format!("HTTP {status}")
            } else {
                format!("HTTP {status}：{}", trimmed.trim())
            }
        });
    AppError::network(title, detail).with_hint("稍后重试；持续失败的话检查网络后重新登录")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_code_response_parses() {
        let text = r#"{
            "device_code": "abc",
            "user_code": "A1B2C3D4",
            "verification_uri": "https://www.microsoft.com/link",
            "message": "要登录，请访问 …",
            "expires_in": 900,
            "interval": 5
        }"#;
        let raw: DeviceCodeResponse = serde_json::from_str(text).unwrap();
        assert_eq!(raw.user_code, "A1B2C3D4");
        assert_eq!(raw.interval, 5);
    }

    #[test]
    fn token_response_tolerates_missing_refresh() {
        // 有的应答不带 refresh_token（续期时），解析不能炸
        let text = r#"{"access_token":"t","expires_in":3600}"#;
        let raw: TokenResponse = serde_json::from_str(text).unwrap();
        assert_eq!(raw.access_token, "t");
        assert!(raw.refresh_token.is_none());
    }

    #[test]
    fn xbl_response_extracts_uhs() {
        let text = r#"{
            "Token": "xbl-token",
            "DisplayClaims": {"xui": [{"uhs": "hash123"}]}
        }"#;
        let raw: XblResponse = serde_json::from_str(text).unwrap();
        assert_eq!(raw.token, "xbl-token");
        assert_eq!(raw.display_claims.xui[0].uhs, "hash123");
    }

    #[test]
    fn xerr_json_and_text_fallback_both_work() {
        // JSON 形态
        assert_eq!(
            parse_xerr(r#"{"XErr":2148916233,"Message":""}"#),
            Some(2148916233)
        );
        // 纯文本形态
        assert_eq!(
            parse_xerr("error 2148916238: child account"),
            Some(2148916238)
        );
        assert_eq!(parse_xerr("totally unrelated"), None);
    }

    #[test]
    fn xerr_mapping_follows_pcl() {
        // 收录的四个码必须有指路话术；未收录的不赌死映射、带原始码
        for code in [2148916227u64, 2148916233, 2148916235, 2148916238] {
            let err = xerr_error(code);
            let text = format!("{err:?}");
            assert!(!text.contains(&code.to_string()), "{code} 没翻译");
        }
        // 2148916236/37 说法不一 → 通用提示 + 原始码
        for code in [2148916236u64, 2148916237, 999] {
            let err = xerr_error(code);
            let text = format!("{err:?}");
            assert!(text.contains(&code.to_string()), "{code} 要带原始码");
        }
    }

    #[test]
    fn token_error_recognizes_microsoft_specials() {
        // AADSTS70000：需要密码登录
        let err = token_error_body("AADSTS70000: something", 400);
        let text = format!("{err:?}");
        assert!(text.contains("密码"), "AADSTS70000 要提示用密码登录");
        // 安全中断
        let err = token_error_body("Account security interrupt", 400);
        assert!(format!("{err:?}").contains("安全"));
        // 封禁
        let err = token_error_body("service abuse detected", 400);
        assert!(format!("{err:?}").contains("封禁"));
        // 普通 OAuth error
        let err = token_error_body(
            r#"{"error":"invalid_client","error_description":"bad client"}"#,
            400,
        );
        assert!(format!("{err:?}").contains("invalid_client"));
    }

    #[test]
    fn oauth_error_parses_standard_shape() {
        let text = r#"{"error":"authorization_pending","error_description":"AADSTS70016"}"#;
        let err = parse_oauth_error(text).unwrap();
        assert_eq!(err.error, "authorization_pending");
    }

    #[test]
    fn empty_client_id_is_rejected_early() {
        let err = match MsaClient::new("") {
            Err(e) => e,
            Ok(_) => panic!("空 client id 不该建得起来"),
        };
        assert!(format!("{err:?}").contains("client id") || format!("{err:?}").contains("微软"));
        assert!(MsaClient::new(" 00000000-0000-0000-0000-000000000000 ").is_ok());
    }

    #[test]
    fn mc_profile_uuid_is_normalized() {
        // 带横杠的 UUID 要归一成 32 位小写无横杠
        let text = r#"{"id":"069A79F4-44E9-4726-A5EB-5722BA6CC974","name":"Notch"}"#;
        let raw: McProfileResponse = serde_json::from_str(text).unwrap();
        let uuid = raw.id.replace('-', "").to_lowercase();
        assert_eq!(uuid, "069a79f444e94726a5eb5722ba6cc974");
        assert_eq!(uuid.len(), 32);
    }

    #[test]
    fn entitlements_empty_means_not_owned() {
        let text = r#"{"items":[]}"#;
        let raw: EntitlementsResponse = serde_json::from_str(text).unwrap();
        assert!(raw.items.is_empty());
        let text = r#"{"items":[{"name":"game_minecraft","signature":"x"}]}"#;
        let raw: EntitlementsResponse = serde_json::from_str(text).unwrap();
        assert_eq!(raw.items.len(), 1);
        // 字段整个缺失也按「没买」处理
        let raw: EntitlementsResponse = serde_json::from_str("{}").unwrap();
        assert!(raw.items.is_empty());
    }
}
