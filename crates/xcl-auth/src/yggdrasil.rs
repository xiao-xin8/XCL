//! Yggdrasil 认证协议客户端（authlib-injector 生态）。
//!
//! 协议就是 Mojang  legacy 的那套，第三方站点（LittleSkin、Blessing Skin、
//! 自建）都实现了它。`base_url` 存 API 根（形如
//! `https://littleskin.cn/api/yggdrasil`），本客户端负责拼 `/authserver/*`
//! 与 `/sessionserver/*`。接口只有五个：
//!
//! | 路径 | 用途 |
//! |---|---|
//! | `POST /authserver/authenticate` | 邮箱 + 密码换 token 与角色列表 |
//! | `POST /authserver/refresh` | 用旧 token 续期（换新 accessToken） |
//! | `POST /authserver/validate` | 问一句 token 还有效吗（无返回体） |
//! | `POST /authserver/signout` | 注销单个账号的登录态 |
//! | `POST /authserver/invalidate` | 让一个 token 失效（退出登录用） |
//!
//! ⚠️ `/authserver` 这层不能漏：直接打 `{根}/authenticate` 会 404。
//! 协议流程按 Yggdrasil 规范实现，
//! 请求字段与它逐字对齐 —— `agent.name = "Minecraft"`、`requestUser = true`
//! 这些老站点认得。

use serde::{Deserialize, Serialize};
use xcl_core::AppError;

/// 认证服务器上的一个角色（profile）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthlibProfile {
    /// 32 位十六进制、无横杠
    pub id: String,
    pub name: String,
}

/// `/authenticate` 与 `/refresh` 的返回体。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthlibSession {
    pub access_token: String,
    pub client_token: String,
    #[serde(default)]
    pub available_profiles: Vec<AuthlibProfile>,
    /// 单个角色时服务端直接给；多个角色时要选。
    pub selected_profile: Option<AuthlibProfile>,
}

/// 登录/刷新结果。`NeedProfile` 表示账号下有多个角色，要挑一个。
///
/// 注意 `NeedProfile` 里带着完整 session（含 accessToken）——
/// 选完角色直接接着用，不需要再向服务器认证一次。
#[derive(Debug, Clone)]
pub enum AuthlibLoginResult {
    LoggedIn(AuthlibSession),
    NeedProfile {
        session: AuthlibSession,
        profiles: Vec<AuthlibProfile>,
    },
}

/// Yggdrasil 错误响应体：`{ "error": "...", "errorMessage": "..." }`。
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct YggdrasilError {
    error: String,
    #[serde(default)]
    error_message: String,
}

#[derive(Debug, Serialize)]
struct Agent {
    name: &'static str,
    version: u32,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AuthenticateRequest {
    agent: Agent,
    username: String,
    password: String,
    request_user: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    client_token: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct TokenRequest {
    access_token: String,
    client_token: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    request_user: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    selected_profile: Option<AuthlibProfile>,
}

/// 一个认证服务器。
pub struct YggdrasilClient {
    http: reqwest::Client,
    base_url: String,
}

impl YggdrasilClient {
    pub fn new(base_url: impl Into<String>) -> Result<Self, AppError> {
        // 入口再规范化一次（幂等）：保证无论调用方传什么，base 一定是
        // 「无尾斜杠、无 /authserver 后缀」的 API 根
        let base_url = crate::normalize_server_url(&base_url.into())?;
        let http = reqwest::Client::builder()
            .user_agent(format!("XCL/{} (authlib)", env!("CARGO_PKG_VERSION")))
            .connect_timeout(std::time::Duration::from_secs(10))
            .timeout(std::time::Duration::from_secs(60))
            .build()
            .map_err(|e| AppError::network("初始化登录组件失败", e.to_string()))?;
        Ok(Self { http, base_url })
    }

    /// 邮箱 + 密码登录。
    ///
    /// `client_token` 传 `None` 时服务端会新建一个并随返回体给回来；
    /// 续登录时传上次的值，保持「同一台设备」的identity。
    pub async fn authenticate(
        &self,
        email: &str,
        password: &str,
        client_token: Option<&str>,
    ) -> Result<AuthlibLoginResult, AppError> {
        let body = AuthenticateRequest {
            agent: Agent {
                name: "Minecraft",
                version: 1,
            },
            username: email.to_string(),
            password: password.to_string(),
            request_user: true,
            client_token: client_token.map(str::to_string),
        };
        let session: AuthlibSession = self.post("/authserver/authenticate", &body).await?;
        Ok(classify(session))
    }

    /// 用 refresh 拿新 token。角色必须一起给（协议要求 selectedProfile）。
    pub async fn refresh(
        &self,
        access_token: &str,
        client_token: &str,
        profile: &AuthlibProfile,
    ) -> Result<AuthlibLoginResult, AppError> {
        let body = TokenRequest {
            access_token: access_token.to_string(),
            client_token: client_token.to_string(),
            request_user: Some(true),
            selected_profile: Some(profile.clone()),
        };
        let session: AuthlibSession = self.post("/authserver/refresh", &body).await?;
        Ok(classify(session))
    }

    /// token 是否还有效。`204` = 有效，`403` = 失效。
    pub async fn validate(&self, access_token: &str, client_token: &str) -> Result<bool, AppError> {
        let body = TokenRequest {
            access_token: access_token.to_string(),
            client_token: client_token.to_string(),
            request_user: None,
            selected_profile: None,
        };
        let url = format!("{}/authserver/validate", self.base_url);
        let resp = self
            .http
            .post(&url)
            .json(&body)
            .send()
            .await
            .map_err(|e| AppError::network("无法连接登录服务器", e.to_string()))?;
        match resp.status().as_u16() {
            204 => Ok(true),
            403 => Ok(false),
            status => Err(self.error_from(resp, status, &url).await),
        }
    }

    /// 注销登录态（signout 按邮箱，invalidate 按 token）。
    pub async fn signout(&self, email: &str, password: &str) -> Result<(), AppError> {
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct Signout {
            username: String,
            password: String,
        }
        self.post_no_content(
            "/authserver/signout",
            &Signout {
                username: email.to_string(),
                password: password.to_string(),
            },
        )
        .await
    }

    pub async fn invalidate(&self, access_token: &str, client_token: &str) -> Result<(), AppError> {
        let body = TokenRequest {
            access_token: access_token.to_string(),
            client_token: client_token.to_string(),
            request_user: None,
            selected_profile: None,
        };
        self.post_no_content("/authserver/invalidate", &body).await
    }

    /// 预取服务器首页内容，供启动参数
    /// `-Dauthlibinjector.yggdrasil.prefetched` 使用。
    ///
    /// authlib-injector 会在 JVM 启动时自己拉这个地址；预取只是省掉一次
    /// 往返（同样这么做）。
    /// 失败不影响登录 —— 返回 `None`，启动参数里就不带这一项。
    pub async fn prefetch(&self) -> Option<Vec<u8>> {
        let resp = self.http.get(&self.base_url).send().await.ok()?;
        if !resp.status().is_success() {
            return None;
        }
        resp.bytes().await.ok().map(|b| b.to_vec())
    }

    // ---- 内部 ----

    /// 拼端点 URL。`path` 必须带前导斜杠（如 `/authserver/authenticate`）。
    fn endpoint(&self, path: &str) -> String {
        format!("{}{path}", self.base_url)
    }

    async fn post<T: Serialize, R: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        body: &T,
    ) -> Result<R, AppError> {
        let url = self.endpoint(path);
        let resp = self
            .http
            .post(&url)
            .json(body)
            .send()
            .await
            .map_err(|e| AppError::network("无法连接登录服务器", e.to_string()))?;

        let status = resp.status().as_u16();
        if status == 204 {
            // validate 走不到这里（它单独处理）；其余接口 204 都是协议外的
            return Err(AppError::parse(
                "登录服务器返回了空响应",
                format!("{url} 返回 204，没有内容"),
            ));
        }
        if resp.status().is_success() {
            return resp
                .json::<R>()
                .await
                .map_err(|e| AppError::parse("登录服务器响应无法解析", e.to_string()));
        }
        Err(self.error_from(resp, status, &url).await)
    }

    async fn post_no_content<T: Serialize>(&self, path: &str, body: &T) -> Result<(), AppError> {
        let url = self.endpoint(path);
        let resp = self
            .http
            .post(&url)
            .json(body)
            .send()
            .await
            .map_err(|e| AppError::network("无法连接登录服务器", e.to_string()))?;
        let status = resp.status().as_u16();
        if resp.status().is_success() {
            return Ok(());
        }
        Err(self.error_from(resp, status, &url).await)
    }

    /// 把失败响应翻成能看懂的话。`url` 是刚才打过去的地址，404 排查要用。
    async fn error_from(&self, resp: reqwest::Response, status: u16, url: &str) -> AppError {
        let text = resp.text().await.unwrap_or_default();
        // 404：九成是地址填错了。authlib 的登录接口在 `{API 根}/authserver/*`，
        // 常见错法是漏填 `/api/yggdrasil` 这类中间层，或多填一层。
        if status == 404 {
            return AppError::network(
                "登录服务器地址不正确",
                format!("在 {url} 上没有找到登录接口（404 Not Found）"),
            )
            .with_hint(
                "第三方登录地址要填到 API 根，例如 https://littleskin.cn/api/yggdrasil；\
                 不要带 /authserver，也不要只填站点主页",
            );
        }
        let parsed: Option<YggdrasilError> = serde_json::from_str(&text).ok();
        match parsed {
            Some(err) => match err.error.as_str() {
                // 最常见的：密码错、账号被禁、站点关闭了登录
                "ForbiddenOperationException" => AppError::validation(
                    "登录被拒绝",
                    if err.error_message.is_empty() {
                        "邮箱或密码不正确".to_string()
                    } else {
                        err.error_message
                    },
                )
                .with_hint("检查邮箱和密码；如果刚改过密码，用新密码"),
                "IllegalArgumentException" => AppError::validation(
                    "登录信息不合法",
                    if err.error_message.is_empty() {
                        "服务器拒绝了这次登录请求".to_string()
                    } else {
                        err.error_message
                    },
                ),
                "ResourceException" => AppError::network(
                    "登录服务器暂时不可用",
                    err.error_message,
                ),
                _ => AppError::network(
                    "登录失败",
                    format!("{}（HTTP {status}）", err.error),
                )
                .with_hint("可以稍后重试；持续失败请联系服务器管理员"),
            },
            None => AppError::network(
                "登录服务器响应异常",
                format!("HTTP {status}，返回的不是预期的登录结果"),
            )
            .with_hint("确认地址是不是一个 Yggdrasil 认证服务器（如 LittleSkin）"),
        }
    }
}

/// 有多个角色 → 选一个；一个或零个 → 直接定。
fn classify(session: AuthlibSession) -> AuthlibLoginResult {
    if session.selected_profile.is_some() || session.available_profiles.len() <= 1 {
        return AuthlibLoginResult::LoggedIn(session);
    }
    let profiles = session.available_profiles.clone();
    AuthlibLoginResult::NeedProfile { session, profiles }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_profile_logs_straight_in() {
        let session = AuthlibSession {
            access_token: "t".into(),
            client_token: "c".into(),
            available_profiles: vec![AuthlibProfile {
                id: "a".repeat(32),
                name: "Notch".into(),
            }],
            selected_profile: None,
        };
        match classify(session) {
            AuthlibLoginResult::LoggedIn(s) => {
                // 单角色也要能定下来：selected_profile 缺省时取第一个
                assert_eq!(s.available_profiles.len(), 1);
            }
            _ => panic!("单角色不该要求选择"),
        }
    }

    #[test]
    fn multiple_profiles_require_choice() {
        let session = AuthlibSession {
            access_token: "t".into(),
            client_token: "c".into(),
            available_profiles: vec![
                AuthlibProfile {
                    id: "a".repeat(32),
                    name: "Notch".into(),
                },
                AuthlibProfile {
                    id: "b".repeat(32),
                    name: "Alex".into(),
                },
            ],
            selected_profile: None,
        };
        match classify(session) {
            AuthlibLoginResult::NeedProfile { session, profiles } => {
                assert_eq!(profiles.len(), 2);
                // token 必须还在，选完角色直接登录，不用再认证一次
                assert_eq!(session.access_token, "t");
            }
            _ => panic!("多角色必须先选"),
        }
    }

    #[test]
    fn explicit_selection_wins() {
        let session = AuthlibSession {
            access_token: "t".into(),
            client_token: "c".into(),
            available_profiles: vec![
                AuthlibProfile {
                    id: "a".repeat(32),
                    name: "Notch".into(),
                },
                AuthlibProfile {
                    id: "b".repeat(32),
                    name: "Alex".into(),
                },
            ],
            selected_profile: Some(AuthlibProfile {
                id: "b".repeat(32),
                name: "Alex".into(),
            }),
        };
        match classify(session) {
            AuthlibLoginResult::LoggedIn(s) => {
                assert_eq!(s.selected_profile.unwrap().name, "Alex")
            }
            _ => panic!("服务端已经选好了就不该再问"),
        }
    }

    #[test]
    fn authenticate_request_shape() {
        // 字段名必须是 camelCase，且带 agent / requestUser —— 老站点认这个
        let body = AuthenticateRequest {
            agent: Agent {
                name: "Minecraft",
                version: 1,
            },
            username: "a@b.c".into(),
            password: "pw".into(),
            request_user: true,
            client_token: None,
        };
        let json = serde_json::to_value(&body).unwrap();
        assert_eq!(json["agent"]["name"], "Minecraft");
        assert_eq!(json["agent"]["version"], 1);
        assert_eq!(json["username"], "a@b.c");
        assert_eq!(json["requestUser"], true);
        assert!(json.get("clientToken").is_none(), "None 时不该出现这个键");
    }

    #[test]
    fn error_body_parses() {
        let text = r#"{"error":"ForbiddenOperationException","errorMessage":"Invalid credentials. "}"#;
        let parsed: YggdrasilError = serde_json::from_str(text).unwrap();
        assert_eq!(parsed.error, "ForbiddenOperationException");
        assert_eq!(parsed.error_message, "Invalid credentials. ");
    }

    #[test]
    fn endpoints_carry_authserver_prefix() {
        // 回归：曾经漏掉 /authserver 这一层，authenticate 直接 404
        let client = YggdrasilClient::new("https://littleskin.cn/api/yggdrasil").unwrap();
        assert_eq!(
            client.endpoint("/authserver/authenticate"),
            "https://littleskin.cn/api/yggdrasil/authserver/authenticate"
        );
        assert_eq!(
            client.endpoint("/authserver/refresh"),
            "https://littleskin.cn/api/yggdrasil/authserver/refresh"
        );
    }

    #[test]
    fn base_with_authserver_suffix_still_works() {
        // 地址填到 /authserver 一层（老教程）也不能拼出 /authserver/authserver
        let client = YggdrasilClient::new("https://littleskin.cn/api/yggdrasil/authserver").unwrap();
        assert_eq!(
            client.endpoint("/authserver/invalidate"),
            "https://littleskin.cn/api/yggdrasil/authserver/invalidate"
        );
    }
}
