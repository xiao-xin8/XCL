//! 账号。
//!
//! 支持两类登录（微软登录不在范围内 —— 需要正版账号才能验证，见项目决策）：
//!
//! 1. 离线（legacy）：只有用户名，UUID 由用户名派生，不联网。
//!    这是 Mojang 官方离线模式的通行算法，同一名字在任何启动器里 UUID 相同，
//!    换启动器存档不丢。
//! 2. 第三方登录（Yggdrasil / authlib-injector）：连一个外置认证服务器
//!    （LittleSkin、自建 Blessing Skin 等），走 Mojang 时代的
//!    `/authserver/authenticate` 系列接口，拿 accessToken + 角色档案。
//!    启动时通过 `-javaagent:authlib-injector.jar=<服务器>` 让游戏客户端
//!    把认证请求改道到这个服务器。
//!
//! 本 crate 只放纯数据结构与校验（零 IO），HTTP 在 `yggdrasil` 子模块。

pub mod yggdrasil;

use serde::{Deserialize, Serialize};
use xcl_core::AppError;

/// 登录方式。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AccountKind {
    /// 离线：只有名字，不联网
    Offline,
    /// 第三方认证服务器（Yggdrasil 协议）
    Authlib,
}

impl AccountKind {
    pub fn label(&self) -> &'static str {
        match self {
            AccountKind::Offline => "离线",
            AccountKind::Authlib => "第三方",
        }
    }
}

/// 默认皮肤模型。`Default` = 粗手臂（Steve），`Slim` = 细手臂（Alex）。
///
/// 只对离线账号有意义：第三方账号的模型由服务器 textures metadata 决定。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SkinModel {
    #[default]
    Default,
    Slim,
}

impl SkinModel {
    /// textures metadata 里的 `model` 字段：`"slim"` → 细手臂，其余 → 粗手臂。
    pub fn from_metadata(value: Option<&str>) -> Self {
        match value {
            Some("slim") => SkinModel::Slim,
            _ => SkinModel::Default,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            SkinModel::Default => "default",
            SkinModel::Slim => "slim",
        }
    }
}

/// 一个账号。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Account {
    /// 稳定 id。换名字、换 token 都不变，前端拿它定位。
    pub id: String,
    pub kind: AccountKind,
    /// 显示名。离线 = 玩家名；第三方 = 角色名（profile name）。
    pub username: String,
    /// 32 位十六进制、无横杠。
    ///
    /// 离线账号这里存名字派生的原始 UUID；玩家选 Steve / Alex 时不会
    /// 改它，启动/profile 时再按模型调整末 5 位（见 `skin::launch_uuid`）。
    pub uuid: String,
    /// 创建时间（Unix 毫秒）。账号列表只按它排序——顺序不随
    /// 「当前账号是谁」变化，按添加顺序找账号。
    /// 旧文件没有这个字段，反序列化补 0（排最前，当作最早建的）。
    #[serde(default)]
    pub created_at_ms: i64,
    /// 离线账号手动指定的默认皮肤；`None` = 按 UUID 自动。
    /// 旧账号文件没有这个字段，反序列化就是 `None`，行为和以前一致。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skin_model: Option<SkinModel>,

    // ---- 以下仅 Authlib ----
    /// 认证服务器根地址，如 `https://littleskin.cn/api/yggdrasil`。
    pub server_url: Option<String>,
    /// 登录邮箱（账号名）。
    pub email: Option<String>,
    /// 选了「记住密码」时才保存，用于刷新 token。
    pub password: Option<String>,
    pub access_token: Option<String>,
    /// 客户端标识。同一设备上保持稳定，服务端靠它判断「是不是同一个客户端」。
    pub client_token: Option<String>,
}

impl Account {
    pub fn offline(name: impl Into<String>) -> Result<Self, AppError> {
        let name = name.into();
        validate_player_name(&name)?;
        let uuid = xcl_core::launch::offline_uuid(&name);
        Ok(Self {
            id: new_id(),
            kind: AccountKind::Offline,
            username: name,
            uuid,
            created_at_ms: now_ms(),
            skin_model: None,
            server_url: None,
            email: None,
            password: None,
            access_token: None,
            client_token: None,
        })
    }

    pub fn is_authlib(&self) -> bool {
        self.kind == AccountKind::Authlib
    }
}

/// 账号表 + 当前账号。纯数据，不碰磁盘 —— 持久化在 `xcl-service::auth`。
#[derive(Debug, Clone, Default)]
pub struct AccountStore {
    pub accounts: Vec<Account>,
    /// 当前使用的账号 id。可能是 `None`（一个都没建）。
    pub active_id: Option<String>,
}

impl AccountStore {
    pub fn active(&self) -> Option<&Account> {
        self.active_id
            .as_ref()
            .and_then(|id| self.accounts.iter().find(|a| &a.id == id))
    }

    pub fn find(&self, id: &str) -> Option<&Account> {
        self.accounts.iter().find(|a| a.id == id)
    }

    /// 插入或更新（按 id）。
    pub fn upsert(&mut self, account: Account) {
        match self.accounts.iter_mut().find(|a| a.id == account.id) {
            Some(slot) => *slot = account,
            None => self.accounts.push(account),
        }
    }

    /// 删除账号。返回是否真的删掉了。
    ///
    /// 删的是当前账号时，把 active 让给剩下的第一个 —— 不允许「没有当前账号」，
    /// 否则启动时还要额外判断一次。
    pub fn remove(&mut self, id: &str) -> bool {
        let before = self.accounts.len();
        self.accounts.retain(|a| a.id != id);
        if self.accounts.len() == before {
            return false;
        }
        if self.active_id.as_deref() == Some(id) {
            self.active_id = self.accounts.first().map(|a| a.id.clone());
        }
        true
    }

    pub fn set_active(&mut self, id: &str) -> bool {
        if self.accounts.iter().any(|a| a.id == id) {
            self.active_id = Some(id.to_string());
            true
        } else {
            false
        }
    }

    /// 按创建时间升序排（同一毫秒保持插入序 —— `sort_by_key` 是稳定排序）。
    ///
    /// 这是账号列表的唯一顺序：不把当前账号提前，按添加顺序找账号，
    /// 切换账号不会让列表跳位（约定）。
    pub fn sort_by_created_at(&mut self) {
        self.accounts.sort_by_key(|a| a.created_at_ms);
    }
}

/// 玩家名校验。
///
/// 规则取 Mojang 正版的经典约束：3–16 位，仅字母 / 数字 / 下划线。
/// 离线模式技术上有不少启动器允许更花的名字，但空格和引号会渗进命令行参数、
/// 皮肤与统计接口也不认，从严把关省得后面各种怪问题。
/// （只校验非空 + 无引号 + 1.20.3 起 ≤16；
/// 我们从紧。）
pub fn validate_player_name(name: &str) -> Result<(), AppError> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err(AppError::validation("玩家名不能为空", "输入一个名字再登录")
            .with_hint("离线登录只需要一个名字，不用密码"));
    }
    if trimmed.len() < 3 || trimmed.len() > 16 {
        return Err(
            AppError::validation("玩家名长度不对", format!("「{trimmed}」是 {len} 个字符，需要 3–16 个", len = trimmed.len()))
                .with_hint("正版账号名的经典规则就是 3–16 个字符"),
        );
    }
    if !trimmed
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_')
    {
        return Err(AppError::validation(
            "玩家名含有无效字符",
            format!("「{trimmed}」只能包含字母、数字和下划线"),
        )
        .with_hint("含空格、中文或特殊字符的名字会让游戏和服务器出问题"));
    }
    Ok(())
}

/// 认证服务器地址规范化：去空白、去结尾斜杠、剥掉结尾的 `/authserver`、
/// 必须是 http(s)。
///
/// 存的永远是 API 根（形如 `https://littleskin.cn/api/yggdrasil`）——
/// authlib-injector 的 javaagent 参数、皮肤 `sessionserver` 端点都认这个形态；
/// 端点路径里的 `/authserver` 由 `YggdrasilClient` 拼。有些老教程让把
/// 地址填到 `/authserver` 一层，这里统一剥掉，两种填法都能用。
pub fn normalize_server_url(url: &str) -> Result<String, AppError> {
    let mut trimmed = url.trim().trim_end_matches('/').to_string();
    while let Some(stripped) = trimmed.strip_suffix("/authserver") {
        let candidate = stripped.trim_end_matches('/');
        // 别把域名恰好叫 authserver 的也剥了（`https://authserver` → `https:/`）
        if !candidate.contains("://") || candidate.ends_with(":/") {
            break;
        }
        trimmed = candidate.to_string();
    }
    if trimmed.is_empty() {
        return Err(AppError::validation("服务器地址不能为空", "填写第三方登录服务器的 API 地址")
            .with_hint("例如 https://littleskin.cn/api/yggdrasil"));
    }
    if !trimmed.starts_with("http://") && !trimmed.starts_with("https://") {
        return Err(AppError::validation(
            "服务器地址格式不对",
            "地址需要以 http:// 或 https:// 开头",
        )
        .with_hint("例如 https://littleskin.cn/api/yggdrasil"));
    }
    Ok(trimmed.to_string())
}

/// 随机 id（账号 id 与 clientToken 用）。
pub fn new_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

/// 当前 Unix 毫秒时间戳。取不到时钟（理论不存在）退 0。
pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offline_uuid_matches_standard_algorithm() {
        // 与 Mojang 官方离线模式一致的 UUID：md5("OfflinePlayer:<name>") 打版本位
        let account = Account::offline("Steve").unwrap();
        assert_eq!(account.uuid.len(), 32);
        assert_eq!(account.kind, AccountKind::Offline);
        assert_eq!(
            account.uuid,
            xcl_core::launch::offline_uuid("Steve"),
            "换启动器存档不丢的前提：同一名字 UUID 必须一致"
        );
    }

    #[test]
    fn name_rules() {
        assert!(validate_player_name("Steve").is_ok());
        assert!(validate_player_name("A_1").is_ok());
        assert!(validate_player_name("  Steve  ").is_ok(), "首尾空白由 trim 处理");
        assert!(validate_player_name("").is_err());
        assert!(validate_player_name("ab").is_err(), "少于 3 位");
        assert!(validate_player_name("a".repeat(17).as_str()).is_err(), "多于 16 位");
        assert!(validate_player_name("中文名").is_err());
        assert!(validate_player_name("has space").is_err());
        assert!(validate_player_name("quo\"te").is_err(), "引号会渗进命令行");
    }

    #[test]
    fn server_url_normalization() {
        assert_eq!(
            normalize_server_url(" https://example.com/api/yggdrasil/ ").unwrap(),
            "https://example.com/api/yggdrasil"
        );
        assert!(normalize_server_url("example.com").is_err());
        assert!(normalize_server_url("ftp://example.com").is_err());
        assert!(normalize_server_url("  ").is_err());
    }

    #[test]
    fn server_url_strips_authserver_suffix() {
        // 老教程让把地址填到 /authserver 一层 —— 剥掉，和填 API 根等价
        assert_eq!(
            normalize_server_url("https://example.com/api/yggdrasil/authserver").unwrap(),
            "https://example.com/api/yggdrasil"
        );
        assert_eq!(
            normalize_server_url("https://example.com/api/yggdrasil/authserver/").unwrap(),
            "https://example.com/api/yggdrasil"
        );
        // 幂等：剥过的再剥一次还是它自己
        assert_eq!(
            normalize_server_url("https://example.com/api/yggdrasil").unwrap(),
            "https://example.com/api/yggdrasil"
        );
        // 域名恰好叫 authserver 的不能误伤
        assert_eq!(
            normalize_server_url("https://authserver").unwrap(),
            "https://authserver"
        );
    }

    #[test]
    fn store_keeps_active_consistent() {
        let mut store = AccountStore::default();
        assert!(store.active().is_none());
        // 手工加一个离线账号 —— 「空表自动补账号」已被否决，
        // 没有账号就应该是什么都没有，启动链路自己会报错
        let notch = Account::offline("Notch").unwrap();
        let notch_id = notch.id.clone();
        store.upsert(notch);
        store.set_active(&notch_id);
        let active = store.active().unwrap();
        assert_eq!(active.username, "Notch");

        // 删当前账号 → active 让给别人
        let second = Account::offline("Alex").unwrap();
        let second_id = second.id.clone();
        store.upsert(second);
        store.set_active(&second_id);
        assert_eq!(store.active().unwrap().username, "Alex");
        assert!(store.remove(&second_id));
        assert_eq!(store.active().unwrap().username, "Notch");

        // 删不存在的返回 false
        assert!(!store.remove("nope"));
    }

    #[test]
    fn sorts_by_created_at_not_by_active() {
        let mut store = AccountStore::default();
        let first = Account::offline("First").unwrap();
        let second = Account::offline("Second").unwrap();
        let third = Account::offline("Third").unwrap();
        // 手工拨时间：simulate 先后创建（同毫秒会保持插入序，所以岔开值）
        let mut a = first;
        a.created_at_ms = 100;
        let mut b = second;
        b.created_at_ms = 200;
        let mut c = third;
        c.created_at_ms = 300;
        store.upsert(a);
        store.upsert(b);
        store.upsert(c);
        // 把最早的设为当前 —— 排序不能被它影响
        store.set_active(&store.accounts[0].id.clone());

        store.sort_by_created_at();
        let names: Vec<&str> = store.accounts.iter().map(|x| x.username.as_str()).collect();
        assert_eq!(names, vec!["First", "Second", "Third"]);
        // 当前账号仍然找得到，且没被排到别处
        assert_eq!(store.active().unwrap().username, "First");
    }

    #[test]
    fn offline_account_gets_created_at() {
        let account = Account::offline("Steve").unwrap();
        assert!(account.created_at_ms > 0, "新建账号必须带创建时间");
    }
}
