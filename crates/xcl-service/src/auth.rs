//! 账号管理：持久化、登录、切换、刷新。
//!
//! 存储位置是 `<root>/accounts.toml` —— 跟 config.toml 分开，因为它含
//! accessToken，语义上是「账号数据」而不是「偏好设置」。同样用
//! 「临时文件 + rename」防写一半。
//!
//! 登录方式分两类（微软登录不在范围内，见 `xcl-auth` 文档）：
//! - 离线：只有名字，UUID 由名字派生
//! - 第三方：Yggdrasil 协议，连自己填的认证服务器

use std::fs;
use std::path::PathBuf;
use std::time::Duration;

use base64::Engine;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tracing::{info, warn};
use xcl_auth::yggdrasil::{AuthlibLoginResult, AuthlibProfile, AuthlibSession, YggdrasilClient};
use xcl_auth::{Account, AccountKind, AccountStore, SkinModel};
use xcl_core::{AppError, PathLayout};
use xcl_net::RaceWinner;

use crate::AppState;

/// authlib-injector 官方 artifact 清单：版本 / 下载地址 / SHA-256。
/// 与主流启动器用的是同一个入口。
const AUTHLIB_META_OFFICIAL: &str = "https://authlib-injector.yushi.moe/artifact/latest.json";
/// BMCLAPI 镜像清单；官方不可达时用它。
const AUTHLIB_META_MIRROR: &str =
    "https://bmclapi2.bangbang93.com/mirrors/authlib-injector/artifact/latest.json";
const AUTHLIB_OFFICIAL_HOST: &str = "authlib-injector.yushi.moe";
const AUTHLIB_MIRROR_HOST: &str = "bmclapi2.bangbang93.com/mirrors/authlib-injector";
/// 官方先跑 1 秒，镜像随后加入。两边都是很小的 JSON，不用等满 5 秒。
const AUTHLIB_META_HEDGE: Duration = Duration::from_secs(1);
/// 必须下载时的清单总超时。
const AUTHLIB_META_TIMEOUT: Duration = Duration::from_secs(20);
/// 单个 jar 地址的超时。文件只有几百 KB，8 秒足够。
const AUTHLIB_DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(8);

/// 落盘的账号表。字段与 `AccountStore` 一致，单独一个类型是为了
/// 文件格式（加 `schema` 版本号）将来能演进。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AccountsFile {
    #[serde(default)]
    pub accounts: Vec<Account>,
    #[serde(default)]
    pub active_id: Option<String>,
}

impl AccountsFile {
    pub fn into_store(self) -> AccountStore {
        let mut store = AccountStore {
            accounts: self.accounts,
            active_id: self.active_id,
        };
        // 读进来就排成创建序：store 本体的顺序 = 列表顺序，
        // 「删当前账号后让位给最早的」才和列表显示一致
        store.sort_by_created_at();
        store
    }
}

/// 给前端的账号视图。
///
/// 脱敏：不带 password / accessToken。前端只需要知道「有没有存密码」
/// （决定刷新按钮亮不亮），不需要知道密码本身。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountView {
    pub id: String,
    pub kind: AccountKind,
    pub username: String,
    pub uuid: String,
    pub server_url: Option<String>,
    pub email: Option<String>,
    /// 是否保存了密码（能用来刷新 token）
    pub has_password: bool,
    /// 是否是当前账号
    pub active: bool,
    /// 创建时间（Unix 毫秒）。后端已按它排好序，前端原样渲染。
    pub created_at_ms: i64,
    /// 离线账号手动选的默认皮肤模型；`None` = 按 UUID 自动
    pub skin_model: Option<SkinModel>,
}

/// 登录第三方账号的结果。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
#[serde(tag = "outcome")]
pub enum AuthlibLoginOutcome {
    /// 登录成功，账号已入库并置为当前
    LoggedIn(AccountView),
    /// 账号下有多个角色，要选一个
    NeedProfile {
        profiles: Vec<AuthlibProfile>,
        /// 回显，让前端确认连的是哪台服务器
        server_url: String,
    },
}

/// base64 编码（标准字母表 + padding）。authlib-injector 的 prefetched
/// 参数要的就是这个。
fn base64_encode(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

/// 读账号表。缺文件 = 空表（不是错误）。
pub fn load(layout: &PathLayout) -> Result<AccountStore, AppError> {
    let path = layout.authlib_accounts_file();
    if !path.exists() {
        return Ok(AccountStore::default());
    }
    let text = fs::read_to_string(&path).map_err(|e| AppError::io(&e, "读取账号文件"))?;
    let file: AccountsFile = toml::from_str(&text).map_err(|e| {
        AppError::parse(
            "账号文件格式错误",
            format!("{}：{e}", path.display()),
        )
        .with_hint("可以删除该文件，所有账号需要重新添加")
    })?;
    Ok(file.into_store())
}

/// 写账号表（临时文件 + rename）。
pub fn save(layout: &PathLayout, store: &AccountStore) -> Result<(), AppError> {
    let path = layout.authlib_accounts_file();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| AppError::io(&e, "创建账号目录"))?;
    }
    let file = AccountsFile {
        accounts: store.accounts.clone(),
        active_id: store.active_id.clone(),
    };
    let text = toml::to_string_pretty(&file)
        .map_err(|e| AppError::parse("账号序列化失败", e.to_string()))?;
    let tmp = path.with_extension("toml.tmp");
    fs::write(&tmp, text).map_err(|e| AppError::io(&e, "写入账号文件"))?;
    fs::rename(&tmp, &path).map_err(|e| AppError::io(&e, "替换账号文件"))?;
    Ok(())
}

fn view(account: &Account, active_id: Option<&str>) -> AccountView {
    AccountView {
        id: account.id.clone(),
        kind: account.kind,
        username: account.username.clone(),
        uuid: account.uuid.clone(),
        server_url: account.server_url.clone(),
        email: account.email.clone(),
        has_password: account.password.as_ref().is_some_and(|p| !p.is_empty()),
        active: Some(account.id.as_str()) == active_id,
        created_at_ms: account.created_at_ms,
        skin_model: account.skin_model,
    }
}

impl AppState {
    // ---------- 查询 ----------

    /// 所有账号，按创建时间升序（当前账号不置顶）。
    ///
    /// 顺序稳定是需求：按添加顺序找账号，切换账号不该让
    /// 列表跳位。新建账号排最后，删掉再建会跑到末尾——符合直觉。
    pub fn accounts(&self) -> Vec<AccountView> {
        let store = self.lock_accounts();
        let active = store.active_id.clone();
        let mut sorted = store.accounts.clone();
        sorted.sort_by_key(|a| a.created_at_ms);
        sorted.iter().map(|a| view(a, active.as_deref())).collect()
    }

    /// 当前账号。空表时（理论不上会发生，init 时已补）返回 `None`。
    pub fn active_account(&self) -> Option<AccountView> {
        let store = self.lock_accounts();
        let active = store.active_id.clone();
        store.active().map(|a| view(a, active.as_deref()))
    }

    // ---------- 离线登录 ----------

    /// 添加一个离线账号并切为当前。
    ///
    /// 同名账号只留一个：离线 UUID 由名字唯一决定，重复条目没有意义。
    /// 重加同名账号时沿用原创建时间——看起来还是原来那个账号，
    /// 不该因为重新点了一次「添加」就蹦到列表末尾。
    pub fn add_offline_account(&self, name: &str) -> Result<AccountView, AppError> {
        // 校验放在 xcl-auth，前端拿到的错误文案可以直接显示
        let account = Account::offline(name)?;
        let mut store = self.lock_accounts();
        // 同 UUID（同名字）的旧条目直接顶掉，id 与创建时间沿用，前端引用不会断
        let existing = store
            .accounts
            .iter()
            .find(|a| a.uuid == account.uuid && a.kind == AccountKind::Offline)
            .map(|a| (a.id.clone(), a.created_at_ms));
        let account = match existing {
            Some((id, created_at_ms)) => Account { id, created_at_ms, ..account },
            None => account,
        };
        store.upsert(account.clone());
        store.set_active(&account.id);
        save(&self.layout, &store)?;
        Ok(view(&account, Some(&account.id)))
    }

    // ---------- 第三方登录 ----------

    /// 用邮箱 + 密码登录第三方服务器。
    ///
    /// `profile_id`：账号有多个角色时，前端选一个再带回来；
    /// `None` 表示「服务端给哪个用哪个」（单角色时就是它）。
    /// 多角色且没选时返回 `NeedProfile`，由前端弹选择。
    pub async fn add_authlib_account(
        &self,
        server_url: &str,
        email: &str,
        password: &str,
        profile_id: Option<&str>,
        remember_password: bool,
    ) -> Result<AuthlibLoginOutcome, AppError> {
        let server_url = xcl_auth::normalize_server_url(server_url)?;
        if email.trim().is_empty() {
            return Err(AppError::validation("邮箱不能为空", "填写登录用的邮箱")
                .with_hint("第三方登录用的是你在那个网站上注册的账号"));
        }
        if password.is_empty() {
            return Err(AppError::validation("密码不能为空", "填写账号密码")
                .with_hint("密码只用于向认证服务器换取登录凭据"));
        }

        let client = YggdrasilClient::new(&server_url)?;
        // 已存同服务器的账号就复用它 的 clientToken，保持「同一台设备」的身份
        let existing_token = {
            let store = self.lock_accounts();
            store
                .accounts
                .iter()
                .find(|a| a.is_authlib() && a.server_url.as_deref() == Some(server_url.as_str()))
                .and_then(|a| a.client_token.clone())
        };
        let session = match client
            .authenticate(email, password, existing_token.as_deref())
            .await?
        {
            AuthlibLoginResult::LoggedIn(session) => session,
            // 多角色：带了 profile_id 就从这一次的响应里定；没带就让他选
            AuthlibLoginResult::NeedProfile { session, profiles } => {
                if profile_id.is_none() {
                    return Ok(AuthlibLoginOutcome::NeedProfile {
                        profiles,
                        server_url,
                    });
                }
                session
            }
        };

        let profile = pick_profile(&session, profile_id)?;
        let account = build_account(
            &server_url,
            email,
            password,
            remember_password,
            &session,
            profile,
            existing_token,
        );
        self.store_authlib_account(account)
    }

    /// 刷新第三方账号的 token。密码是登录时存的（选了「记住密码」）。
    pub async fn refresh_authlib_account(&self, id: &str) -> Result<AccountView, AppError> {
        let account = {
            let store = self.lock_accounts();
            store.find(id).cloned()
        }
        .ok_or_else(|| AppError::not_found("账号不存在", "它可能已经被删掉了"))?;

        if !account.is_authlib() {
            return Err(AppError::unsupported(
                "离线账号不需要刷新",
                "离线登录不依赖任何服务器，随时可用",
            ));
        }
        let (Some(server_url), Some(access_token), Some(client_token)) =
            (account.server_url.clone(), account.access_token.clone(), account.client_token.clone())
        else {
            return Err(AppError::validation(
                "这个账号没有登录凭据",
                "删掉它重新登录一次",
            ));
        };

        let profile = AuthlibProfile {
            id: account.uuid.clone(),
            name: account.username.clone(),
        };
        let client = YggdrasilClient::new(&server_url)?;
        let session = match client
            .refresh(&access_token, &client_token, &profile)
            .await?
        {
            AuthlibLoginResult::LoggedIn(s) => s,
            AuthlibLoginResult::NeedProfile { .. } => {
                return Err(AppError::unsupported(
                    "token 已失效",
                    "刷新时服务器要求重新选择角色，删掉账号重新登录吧",
                ))
            }
        };

        let mut updated = account.clone();
        updated.access_token = Some(session.access_token);
        updated.client_token = Some(session.client_token);
        let mut store = self.lock_accounts();
        store.upsert(updated.clone());
        save(&self.layout, &store)?;
        Ok(view(&updated, store.active_id.as_deref()))
    }

    /// 让一个第三方账号的 token 失效（服务器侧登出）。
    ///
    /// 失败不阻塞删除 —— 本地删掉才是要的结果。
    pub async fn signout_authlib_account(&self, id: &str) -> Result<(), AppError> {
        let account = {
            let store = self.lock_accounts();
            store.find(id).cloned()
        };
        let Some(account) = account else {
            return Ok(());
        };
        if !account.is_authlib() {
            return Ok(());
        }
        let (Some(server_url), Some(access_token), Some(client_token)) = (
            account.server_url.clone(),
            account.access_token.clone(),
            account.client_token.clone(),
        ) else {
            return Ok(());
        };
        let client = YggdrasilClient::new(&server_url)?;
        // 服务端不在线也照样删本地账号
        let _ = client.invalidate(&access_token, &client_token).await;
        Ok(())
    }

    // ---------- 管理 ----------

    /// 删除账号（含服务器侧登出，失败也删）。
    pub async fn remove_account(&self, id: &str) -> Result<(), AppError> {
        // 服务端不在线也照样删本地账号 —— 本地删掉才是要的结果
        let _ = self.signout_authlib_account(id).await;
        let mut store = self.lock_accounts();
        if !store.remove(id) {
            return Err(AppError::not_found("账号不存在", "它可能已经被删掉了"));
        }
        save(&self.layout, &store)?;
        Ok(())
    }

    /// 设置离线账号的默认皮肤模型（Steve / Alex）。
    ///
    /// 第三方账号的皮肤由服务器 textures metadata 决定，不提供这个开关。
    /// 改的是账号偏好，不改 `uuid` 本身 —— 启动和预览时再按模型调整末 5 位。
    pub fn set_offline_skin_model(
        &self,
        id: &str,
        model: SkinModel,
    ) -> Result<AccountView, AppError> {
        let mut store = self.lock_accounts();
        let index = store
            .accounts
            .iter()
            .position(|a| a.id == id)
            .ok_or_else(|| AppError::not_found("账号不存在", "它可能已经被删掉了"))?;
        if store.accounts[index].is_authlib() {
            return Err(AppError::unsupported(
                "第三方账号不能选默认皮肤",
                "皮肤和模型由认证服务器决定",
            ));
        }
        store.accounts[index].skin_model = Some(model);
        save(&self.layout, &store)?;
        let account = store.accounts[index].clone();
        Ok(view(&account, store.active_id.as_deref()))
    }

    /// 切换当前账号。
    pub fn set_active_account(&self, id: &str) -> Result<AccountView, AppError> {
        let mut store = self.lock_accounts();
        if !store.set_active(id) {
            return Err(AppError::not_found("账号不存在", "它可能已经被删掉了"));
        }
        save(&self.layout, &store)?;
        let account = store.find(id).cloned().expect("刚 set_active 成功");
        Ok(view(&account, Some(id)))
    }

    // ---------- 启动时用 ----------

    /// 当前账号的玩家档案。给 `xcl_core::launch::PlayerProfile` 用。
    ///
    /// 启动前确认第三方账号的登录状态还有效。
    ///
    /// 每次启动都走一遍登录（「登录」子任务），
    /// 只是它拿存下来的密码重新认证，所以看起来「怎么那么快」。
    /// 我们做得更省：先问一句，能不重登就不重登。
    ///
    /// 1. `/authserver/validate` —— 一次 POST，几十毫秒，问 token 还有效吗；
    /// 2. 失效才 `/authserver/refresh` —— 用 accessToken + clientToken 换新的，写回账号文件；
    /// 3. refresh 也不行、但存了密码 —— 才 `/authserver/authenticate` 整份重登。
    ///
    /// 离线账号不联网，直接返回。网络不通不拦启动：宁可带旧凭据让游戏自己报错，
    /// 也别因为一次抖动把人挡在门外；只有服务器明确说「这个 token 不认」才算过期。
    pub(crate) async fn ensure_authlib_session(&self) -> Result<(), AppError> {
        let account = {
            let store = self.lock_accounts();
            store.active().cloned()
        };
        let Some(account) = account else {
            return Ok(());
        };
        if !account.is_authlib() {
            return Ok(());
        }
        let (Some(server_url), Some(access_token), Some(client_token)) = (
            account.server_url.clone(),
            account.access_token.clone(),
            account.client_token.clone(),
        ) else {
            // 缺凭据的情况交给 `player_profile` 报，那里的文案更具体
            return Ok(());
        };

        let client = YggdrasilClient::new(&server_url)?;
        match client.validate(&access_token, &client_token).await {
            Ok(true) => return Ok(()),
            Ok(false) => {}
            Err(err) => {
                // 服务器连不上 / 返回了怪东西：先按「还有效」处理
                warn!(error = %err, "校验登录状态失败，先带旧凭据启动");
                return Ok(());
            }
        }

        // token 过期：先试着刷新（不需要密码）
        let profile = AuthlibProfile {
            id: account.uuid.clone(),
            name: account.username.clone(),
        };
        if let Ok(AuthlibLoginResult::LoggedIn(session)) = client
            .refresh(&access_token, &client_token, &profile)
            .await
        {
            return self.store_session(&account.id, session);
        }

        // 刷新也不行，只有「记住了密码」才能整份重登
        if let (Some(email), Some(password)) = (account.email.clone(), account.password.clone()) {
            if let Ok(result) = client
                .authenticate(&email, &password, Some(&client_token))
                .await
            {
                // NeedProfile 也照用：角色已经定过（`player_profile` 用的是存下的 uuid）
                let session = match result {
                    AuthlibLoginResult::LoggedIn(session) => session,
                    AuthlibLoginResult::NeedProfile { session, .. } => session,
                };
                return self.store_session(&account.id, session);
            }
        }

        Err(AppError::validation(
            "登录状态已过期",
            format!("{} 的登录凭据已经失效了", account.username),
        )
        .with_hint("在启动页点账号卡片重新登录一次；勾上「记住密码」以后就能自动续期"))
    }

    /// 把一次会话的新 token 写回账号文件。
    fn store_session(&self, id: &str, session: AuthlibSession) -> Result<(), AppError> {
        let mut store = self.lock_accounts();
        let Some(mut account) = store.find(id).cloned() else {
            return Ok(());
        };
        account.access_token = Some(session.access_token);
        account.client_token = Some(session.client_token);
        store.upsert(account);
        save(&self.layout, &store)
    }

    /// `version_id` 用来决定离线皮肤的 UUID 规则：1.19.3 起游戏换了默认
    /// 皮肤的选择算法，必须按版本分别调整 `--uuid`。
    ///
    /// 两种情况直接报错，不带病启动：
    /// - 第三方账号缺 token —— 带着空 token 启动，游戏里只会弹一句
    ///   「无法登录」，不知道去哪修；
    /// - 一个账号都没有 —— 约定：没有账号就不许启动
    ///   （启动按钮也会禁用），这里是后端的最后一道门。
    pub(crate) fn player_profile(
        &self,
        version_id: &str,
    ) -> Result<xcl_core::launch::PlayerProfile, AppError> {
        use xcl_core::launch::PlayerProfile;
        let store = self.lock_accounts();
        match store.active() {
            Some(account) if account.is_authlib() => {
                let token = account.access_token.clone().ok_or_else(|| {
                    AppError::validation(
                        "这个账号还没有登录凭据",
                        format!("{} 的登录状态已失效", account.username),
                    )
                    .with_hint("在启动页点账号卡片，对这个账号点「刷新登录」；不行就删掉重新登录")
                })?;
                Ok(PlayerProfile {
                    name: account.username.clone(),
                    uuid: account.uuid.clone(),
                    access_token: token,
                    // authlib 兼容 Mojang 协议，userType 用 mojang，
                    // 第三方服务器统计上按 mojang 归类更老实
                    user_type: "mojang".to_string(),
                })
            }
            Some(account) => {
                // 选了 Steve / Alex 就按版本规则调整末 5 位，再作为 --uuid 传给游戏。
                // 游戏没有单独的 skinModel 参数，离线默认皮肤只认 UUID。
                let uuid = crate::skin::launch_uuid(&account.uuid, account.skin_model, version_id);
                Ok(PlayerProfile::offline_with_uuid(
                    account.username.clone(),
                    uuid,
                ))
            }
            None => Err(AppError::validation(
                "还没有添加任何账号",
                "启动游戏前需要先添加一个账号",
            )
            .with_hint("在启动页点「管理账号」，添加离线账号或用第三方登录")),
        }
    }

    /// 当前账号需要的额外 JVM 参数（authlib-injector）。
    ///
    /// 离线账号返回空。第三方账号按主流做法：
    /// `-javaagent` 挂上 injector + 服务器地址，`prefetched` 带上首页内容
    /// 省一次 JVM 内请求，再信任系统根证书（部分第三方站点证书链不全）。
    pub(crate) async fn authlib_jvm_args(&self) -> Result<Vec<String>, AppError> {
        let account = {
            let store = self.lock_accounts();
            store.active().cloned()
        };
        let Some(account) = account else {
            return Ok(Vec::new());
        };
        if !account.is_authlib() {
            return Ok(Vec::new());
        }
        let Some(server_url) = account.server_url.clone() else {
            return Ok(Vec::new());
        };

        let jar = self.ensure_authlib_jar().await?;
        // authlib-injector 是 AGPL-3.0，但其例外条款明确允许「以未经修改的
        // 二进制形式打包、并作为 javaagent 加载」—— 我们正是这么用的。
        // 参数格式照协议文档：
        // javaagent 后面跟 `=<服务器 URL>`，prefetched 是省一次 JVM 内请求的
        // 可选优化（内容为 GET 服务器首页的原始响应）。
        let mut args = vec![
            format!("-javaagent:{}={}", jar.display(), server_url),
            "-Dauthlibinjector.side=client".to_string(),
        ];
        // 信任 Windows 系统根证书：部分第三方站点的证书链不完整。
        // 只在 Windows 上加 —— 这个 trustStore 类型是 Windows 特有的。
        if cfg!(windows) {
            args.push("-Djavax.net.ssl.trustStoreType=WINDOWS-ROOT".to_string());
        }

        // 预取首页内容。失败不拦着启动 —— injector 自己会再拉一次
        if let Ok(client) = YggdrasilClient::new(&server_url) {
            if let Some(body) = client.prefetch().await {
                args.push(format!(
                    "-Dauthlibinjector.yggdrasil.prefetched={}",
                    base64_encode(&body)
                ));
            }
        }
        Ok(args)
    }

    /// 数据根下有没有可用的 authlib-injector.jar。
    ///
    /// 只看文件存在且不是 0 字节；不联网、不校验版本。
    pub fn authlib_jar_present(&self) -> bool {
        authlib_jar_usable(&self.layout.authlib_jar())
    }

    /// 后台预取：启动时和登录成功后调用。
    ///
    /// 有 jar 就立即返回，没有才下载；失败不影响账号，真正启动第三方账号前
    /// `authlib_jvm_args` 还会再保证一次。
    pub async fn prefetch_authlib_jar(&self) -> Result<PathBuf, AppError> {
        self.ensure_authlib_jar().await
    }

    /// 保证数据根下有可用的 authlib-injector.jar。
    ///
    /// 有就不管：只要文件存在且不是 0 字节，就直接用，不联网、不校验版本。
    /// 没有才从官方 artifact 清单取下载地址（官方优先、BMCLAPI 兜底），
    /// 下载后做 SHA-256 校验。这样正常启动完全不碰网络，只有缺文件时才下。
    async fn ensure_authlib_jar(&self) -> Result<PathBuf, AppError> {
        // 启动预取、登录预取和启动前保证可能并发；串行化，后到的那个
        // 拿到锁时文件已经就位，会直接返回。
        let _guard = self.authlib_jar_lock.lock().await;
        let target = self.layout.authlib_jar();
        if authlib_jar_usable(&target) {
            return Ok(target);
        }

        let (artifact, from_mirror) = self
            .fetch_authlib_artifact(AUTHLIB_META_TIMEOUT)
            .await
            .map_err(|err| {
                AppError::network("获取第三方登录组件失败", err.to_string())
                    .with_hint("检查网络后重试；离线账号不受影响")
            })?;

        let mut urls = authlib_download_urls(&artifact.download_url);
        if urls.is_empty() {
            return Err(AppError::network(
                "第三方登录组件地址无效",
                "服务器没有返回下载地址",
            ));
        }
        // 清单是从镜像拿到的，下载也先走镜像，避免官方地址再挂 8 秒。
        if from_mirror {
            urls.reverse();
        }

        // 单独建一个短超时客户端：默认 60s × 3 次对几百 KB 的小文件太久了。
        let client = xcl_net::HttpClient::new(self.http().policy())?
            .with_retry_policy(xcl_net::RetryPolicy {
                max_retries: 0,
                base_delay: Duration::from_millis(300),
            })
            .with_attempt_timeout(AUTHLIB_DOWNLOAD_TIMEOUT);
        let bytes = client.get_from_candidates(&urls).await.map_err(|err| {
            AppError::network("下载第三方登录组件失败", err.to_string())
                .with_hint("检查网络后重试；离线账号不受影响")
        })?;

        let actual = sha256_hex(&bytes);
        if !actual.eq_ignore_ascii_case(&artifact.checksums.sha256) {
            return Err(AppError::validation(
                "第三方登录组件校验失败",
                "下载的文件和服务器给出的 SHA-256 不一致",
            )
            .with_hint("请重试；如果反复失败，检查网络是否被劫持"));
        }

        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).map_err(|e| AppError::io(&e, "创建数据目录"))?;
        }
        let tmp = target.with_extension("jar.tmp");
        fs::write(&tmp, &bytes).map_err(|e| AppError::io(&e, "写入 authlib-injector.jar"))?;
        if let Err(err) = fs::rename(&tmp, &target) {
            let _ = fs::remove_file(&tmp);
            return Err(AppError::io(&err, "保存 authlib-injector.jar"));
        }
        info!(version = %artifact.version, "已下载 authlib-injector");
        Ok(target)
    }

    /// 拉 artifact 清单，返回 `(清单, 是否来自 BMCLAPI)`。
    async fn fetch_authlib_artifact(
        &self,
        overall: Duration,
    ) -> Result<(AuthlibArtifact, bool), AppError> {
        let outcome = self
            .http()
            .race_text(
                AUTHLIB_META_OFFICIAL,
                AUTHLIB_META_MIRROR,
                AUTHLIB_META_HEDGE,
                overall,
            )
            .await?;
        let artifact = serde_json::from_slice(&outcome.body).map_err(|e| {
            AppError::parse("第三方登录组件信息格式错误", e.to_string())
                .with_hint("稍后重试；已安装的旧组件仍会使用")
        })?;
        Ok((artifact, outcome.winner == RaceWinner::Secondary))
    }

    // ---- 内部 ----

    fn lock_accounts(&self) -> std::sync::RwLockWriteGuard<'_, AccountStore> {
        self.accounts
            .write()
            .unwrap_or_else(|e| e.into_inner())
    }

    fn store_authlib_account(
        &self,
        account: Account,
    ) -> Result<AuthlibLoginOutcome, AppError> {
        let mut store = self.lock_accounts();
        store.upsert(account.clone());
        store.set_active(&account.id);
        save(&self.layout, &store)?;
        Ok(AuthlibLoginOutcome::LoggedIn(view(&account, Some(&account.id))))
    }
}

/// artifact 清单。字段名照官方 API 的用法。
#[derive(Debug, Deserialize)]
struct AuthlibArtifact {
    #[serde(default)]
    version: String,
    download_url: String,
    checksums: AuthlibChecksums,
}

#[derive(Debug, Deserialize)]
struct AuthlibChecksums {
    sha256: String,
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

/// 文件存在且不是 0 字节。0 字节通常是上次写入失败留下的，按缺失处理。
fn authlib_jar_usable(path: &std::path::Path) -> bool {
    path.is_file() && fs::metadata(path).map(|m| m.len() > 0).unwrap_or(false)
}

/// 官方优先、BMCLAPI 兜底。BMCLAPI 镜像的路径前缀与官方域名一一对应，
/// 所以同一个 artifact 地址可以互相改写。
fn authlib_download_urls(download_url: &str) -> Vec<String> {
    let official = download_url.replace(AUTHLIB_MIRROR_HOST, AUTHLIB_OFFICIAL_HOST);
    let mirror = official.replace(AUTHLIB_OFFICIAL_HOST, AUTHLIB_MIRROR_HOST);
    let mut urls = Vec::new();
    for url in [official, mirror] {
        if !urls.contains(&url) {
            urls.push(url);
        }
    }
    urls
}

/// 从登录结果里定下用哪个角色。
fn pick_profile(
    session: &AuthlibSession,
    profile_id: Option<&str>,
) -> Result<AuthlibProfile, AppError> {
    if let Some(id) = profile_id {
        return session
            .available_profiles
            .iter()
            .find(|p| p.id == id)
            .cloned()
            .ok_or_else(|| {
                AppError::validation("没有这个角色", format!("账号下找不到 id 为 {id} 的角色"))
            });
    }
    session
        .selected_profile
        .clone()
        .or_else(|| session.available_profiles.first().cloned())
        .ok_or_else(|| {
            AppError::validation(
                "这个账号下没有角色",
                "先去服务器网站创建一个角色再登录",
            )
            .with_hint("例如 LittleSkin 需要在用户中心新建角色")
        })
}

fn build_account(
    server_url: &str,
    email: &str,
    password: &str,
    remember_password: bool,
    session: &AuthlibSession,
    profile: AuthlibProfile,
    existing_token: Option<String>,
) -> Account {
    Account {
        id: xcl_auth::new_id(),
        kind: AccountKind::Authlib,
        username: profile.name,
        uuid: profile.id,
        created_at_ms: xcl_auth::now_ms(),
        skin_model: None,
        server_url: Some(server_url.to_string()),
        email: Some(email.to_string()),
        // 记住密码才落盘；否则刷新 token 时会要求重新登录
        password: remember_password.then(|| password.to_string()),
        access_token: Some(session.access_token.clone()),
        client_token: Some(
            existing_token.unwrap_or_else(|| session.client_token.clone()),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use xcl_core::path::PathLayout;

    fn temp_layout(tag: &str) -> PathLayout {
        let dir = std::env::temp_dir().join(format!("xcl-auth-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        PathLayout::new(dir.clone(), dir.join("cache"))
    }

    #[test]
    fn save_and_load_roundtrip() {
        let layout = temp_layout("roundtrip");
        let mut store = AccountStore::default();
        let mut notch = Account::offline("Notch").unwrap();
        // 固定创建时间，断言排序时可复现
        notch.created_at_ms = 100;
        let notch_id = notch.id.clone();
        store.upsert(notch);
        store.set_active(&notch_id);
        let authlib = Account {
            id: "a1".into(),
            kind: AccountKind::Authlib,
            username: "Hero".into(),
            uuid: "b".repeat(32),
            created_at_ms: 500,
            skin_model: None,
            server_url: Some("https://example.com/api/yggdrasil".into()),
            email: Some("a@b.c".into()),
            password: Some("secret".into()),
            access_token: Some("tok".into()),
            client_token: Some("ct".into()),
        };
        store.upsert(authlib);
        save(&layout, &store).unwrap();

        let loaded = load(&layout).unwrap();
        assert_eq!(loaded.accounts.len(), 2);
        // 读进来按创建时间排：先 Notch 后 Hero，不因为谁是当前账号而变
        assert_eq!(loaded.accounts[0].username, "Notch");
        assert_eq!(loaded.accounts[1].username, "Hero");
        let found = loaded.find("a1").unwrap();
        assert_eq!(found.username, "Hero");
        assert_eq!(found.access_token.as_deref(), Some("tok"));
        assert_eq!(loaded.active().unwrap().username, "Notch");
    }

    #[test]
    fn missing_file_is_empty_not_error() {
        let layout = temp_layout("missing");
        let store = load(&layout).unwrap();
        assert!(store.accounts.is_empty());
        assert!(store.active().is_none());
    }

    #[test]
    fn view_masks_secrets() {
        let account = Account {
            id: "a1".into(),
            kind: AccountKind::Authlib,
            username: "Hero".into(),
            uuid: "b".repeat(32),
            created_at_ms: 1,
            skin_model: None,
            server_url: Some("https://example.com".into()),
            email: Some("a@b.c".into()),
            password: Some("secret".into()),
            access_token: Some("tok".into()),
            client_token: Some("ct".into()),
        };
        let v = view(&account, Some("a1"));
        assert!(v.active);
        assert!(v.has_password);
        assert_eq!(v.kind, AccountKind::Authlib);
        // 序列化结果里不能出现密码与 token
        let json = serde_json::to_string(&v).unwrap();
        assert!(!json.contains("secret"));
        assert!(!json.contains("tok"));
    }

    #[test]
    fn pick_profile_prefers_explicit_then_selected_then_first() {
        let session = AuthlibSession {
            access_token: "t".into(),
            client_token: "c".into(),
            available_profiles: vec![
                AuthlibProfile {
                    id: "id1".into(),
                    name: "A".into(),
                },
                AuthlibProfile {
                    id: "id2".into(),
                    name: "B".into(),
                },
            ],
            selected_profile: Some(AuthlibProfile {
                id: "id2".into(),
                name: "B".into(),
            }),
        };
        assert_eq!(pick_profile(&session, Some("id1")).unwrap().name, "A");
        assert_eq!(pick_profile(&session, None).unwrap().name, "B");

        let empty = AuthlibSession {
            access_token: "t".into(),
            client_token: "c".into(),
            available_profiles: Vec::new(),
            selected_profile: None,
        };
        assert!(pick_profile(&empty, None).is_err(), "没有角色要报错");
    }

    #[test]
    fn sha256_hex_matches_known_value() {
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn zero_byte_jar_is_treated_as_missing() {
        let layout = temp_layout("jar-usable");
        fs::create_dir_all(layout.root()).unwrap();
        let path = layout.authlib_jar();
        fs::write(&path, b"").unwrap();
        assert!(!authlib_jar_usable(&path));
        fs::write(&path, b"x").unwrap();
        assert!(authlib_jar_usable(&path));
    }

    #[test]
    fn authlib_download_urls_prefers_official_then_mirror() {
        let urls = authlib_download_urls(
            "https://bmclapi2.bangbang93.com/mirrors/authlib-injector/artifact/1.2.8/authlib-injector-1.2.8.jar",
        );
        assert_eq!(urls.len(), 2);
        assert_eq!(
            urls[0],
            "https://authlib-injector.yushi.moe/artifact/1.2.8/authlib-injector-1.2.8.jar"
        );
        assert_eq!(
            urls[1],
            "https://bmclapi2.bangbang93.com/mirrors/authlib-injector/artifact/1.2.8/authlib-injector-1.2.8.jar"
        );
    }

    #[test]
    fn authlib_download_urls_keeps_official_first() {
        let urls = authlib_download_urls(
            "https://authlib-injector.yushi.moe/artifact/1.2.8/authlib-injector-1.2.8.jar",
        );
        assert_eq!(urls.len(), 2);
        assert!(urls[0].starts_with("https://authlib-injector.yushi.moe/"));
        assert!(urls[1].starts_with("https://bmclapi2.bangbang93.com/mirrors/authlib-injector/"));
    }
}
