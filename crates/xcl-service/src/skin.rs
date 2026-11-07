//! 皮肤：把账号变成前端能直接渲染的 3D 皮肤数据。
//!
//! 数据链路照 Yggdrasil / 皮肤协议走：
//!
//! 1. 第三方账号：`GET {服务器}/sessionserver/session/minecraft/profile/{uuid}`
//!    → `properties` 里找 `textures` → base64 解出 `{SKIN:{url,metadata},CAPE:{url}}`
//! 2. 离线账号：没有服务器皮肤，用内置默认皮肤。1.19.2 及更早按 UUID
//!    奇偶选 Steve / Alex；1.19.3+ 游戏改成
//!    按 `floorMod(UUID.hashCode(), 18)` 从 9 套皮肤 × 宽/细里选，启动时
//!    调整 UUID 末 5 位命中目标下标。
//!
//! 缓存分两层，都在 `cache/skin/` 下（可再生数据，不进便携目录）：
//! - `index/{uuid}.json`：解析出的 URL + 时间戳，TTL 6 小时
//! - `blob/{sha1(url)}.png`：下载到的位图，URL 不变就复用
//!
//! 任何失败都静默回落到默认皮肤：皮肤是装饰，不该因为拉不到它就弹错
//! 拦住启动。

use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use base64::Engine;
use serde::{Deserialize, Serialize};
use sha1::{Digest, Sha1};

pub use xcl_auth::SkinModel;

use xcl_core::PathLayout;

use crate::AppState;

/// 皮肤地址索引的缓存时长。皮肤不会经常换，6 小时足够。
const SKIN_INDEX_TTL_SECS: u64 = 6 * 60 * 60;

/// 内置默认皮肤（官方 64×64 的 Steve / Alex 贴图）。
///
/// 编译期内嵌而不是走 Tauri resources：两个文件各几 KB，内嵌后
/// dev / 打包 / 绿色版行为完全一致，也不存在「资源解析不到就没有预览」
/// 的失败模式。
const STEVE_PNG: &[u8] = include_bytes!("../../../apps/desktop/src-tauri/resources/skins/Steve.png");
const ALEX_PNG: &[u8] = include_bytes!("../../../apps/desktop/src-tauri/resources/skins/Alex.png");

/// 皮肤数据来源：服务器拉来的，还是内置默认皮肤。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum SkinSource {
    Server,
    OfflineDefault,
}

/// 给前端的皮肤渲染数据。
///
/// 位图一律 base64（PNG，不带 `data:` 前缀）—— 前端直接拼 dataURL 喂给
/// skinview3d，绕开 WebView2 的 CORS 与 canvas 污染，也不用配 fs 插件权限。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkinRender {
    pub skin: String,
    pub cape: Option<String>,
    pub model: SkinModel,
    pub source: SkinSource,
}

// ---------- sessionserver profile 的返回体 ----------

/// `GET /sessionserver/session/minecraft/profile/{uuid}` 的返回体。
#[derive(Debug, Deserialize)]
struct ProfileResponse {
    properties: Vec<ProfileProperty>,
}

#[derive(Debug, Deserialize)]
struct ProfileProperty {
    name: String,
    value: String,
}

/// `textures` 属性 base64 解开后的 JSON。顶层字段（timestamp / profileId 等）
/// 用不到，只取 `textures`；它可能整个缺失（没设过皮肤）。
#[derive(Debug, Deserialize, Default)]
struct TexturesDoc {
    #[serde(default)]
    textures: Option<TexturesSet>,
}

#[derive(Debug, Deserialize, Default)]
struct TexturesSet {
    /// 协议里是大写 `SKIN` / `CAPE`（有个别实现是先把整个 JSON 转小写再查的，
    /// 见 `ModMinecraft.vb:1440`）；`alias` 同时兼容大写与小写。
    #[serde(default, alias = "SKIN")]
    skin: Option<TextureEntry>,
    #[serde(default, alias = "CAPE")]
    cape: Option<TextureEntry>,
}

#[derive(Debug, Deserialize)]
struct TextureEntry {
    url: String,
    #[serde(default)]
    metadata: Option<TextureMetadata>,
}

#[derive(Debug, Deserialize)]
struct TextureMetadata {
    #[serde(default)]
    model: Option<String>,
}

/// 从 textures JSON 里解出来的有用信息。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct ResolvedTextures {
    skin_url: Option<String>,
    cape_url: Option<String>,
    model: SkinModel,
}

/// 落盘的皮肤地址索引。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct SkinIndex {
    #[serde(default)]
    skin_url: Option<String>,
    #[serde(default)]
    cape_url: Option<String>,
    #[serde(default)]
    model: Option<String>,
    /// 写入时的 Unix 时间戳（秒）。
    fetched_at: u64,
}

impl SkinIndex {
    fn is_fresh(&self, now: u64) -> bool {
        // saturating_sub：本机时钟往回跳时不会 panic，只是当成过期
        now.saturating_sub(self.fetched_at) < SKIN_INDEX_TTL_SECS
    }
}

/// 离线 UUID → Steve / Alex。
///
/// UUID 第
/// 7/15/23/31 位（十六进制数字）异或，奇数 → Alex（slim），偶数 → Steve。
/// 离线 UUID 由名字派生，所以同一个名字每次结果一致。
pub fn offline_model(uuid: &str) -> SkinModel {
    let digit = |i: usize| {
        uuid.as_bytes()
            .get(i)
            .and_then(|b| char::from(*b).to_digit(16))
    };
    let parity = match (digit(7), digit(15), digit(23), digit(31)) {
        (Some(a), Some(b), Some(c), Some(d)) => a ^ b ^ c ^ d,
        // 不是合法 UUID：按 Steve 算，不报错
        _ => 0,
    };
    if parity % 2 == 1 {
        SkinModel::Slim
    } else {
        SkinModel::Default
    }
}

/// 按玩家选的模型调整离线 UUID。
///
/// 离线默认皮肤在游戏里分两代规则，都要靠 `--uuid` 控制（没有
/// `--skinModel` 之类的启动参数）：
/// - 1.19.2 及更早：按 UUID 第 7/15/23/31 位异或的奇偶选 Steve / Alex
///   （奇偶判定）。前 27 位不动，只递增末 5 位直到奇偶匹配。
/// - 1.19.3 及更新：Mojang 改成 9 套默认皮肤 × 宽/细两版，共 18 个，
///   用 `floorMod(UUID.hashCode(), 18)` 选。想指定 Steve 就命中下标 15
///   （wide/steve），Alex 命中下标 0（slim/alex）—— 26.2 的
///   `DefaultPlayerSkin` 字节码即如此。
///
/// `None` 表示自动，原样返回。
pub fn launch_uuid(uuid: &str, preferred: Option<SkinModel>, version_id: &str) -> String {
    let Some(target) = preferred else {
        return uuid.to_string();
    };
    let valid = uuid.len() == 32 && uuid.bytes().all(|b| b.is_ascii_hexdigit());
    if !valid {
        return uuid.to_string();
    }
    if uses_modern_default_skins(version_id) {
        let want = match target {
            SkinModel::Default => MODERN_WIDE_STEVE,
            SkinModel::Slim => MODERN_SLIM_ALEX,
        };
        return search_uuid(uuid, |candidate| modern_skin_index(candidate) == Some(want));
    }
    search_uuid(uuid, |candidate| offline_model(candidate) == target)
}

/// 固定前 27 位、递增末 5 位，找到第一个满足条件的 UUID。
/// 末 5 位只有 2^20 种取值，目标一定存在；找不到就原样返回，不报错。
fn search_uuid(uuid: &str, matches: impl Fn(&str) -> bool) -> String {
    if matches(uuid) {
        return uuid.to_string();
    }
    let Some(mut suffix) = u32::from_str_radix(&uuid[27..32], 16).ok() else {
        return uuid.to_string();
    };
    for _ in 0..=0xFFFFF {
        suffix = (suffix + 1) & 0xFFFFF;
        let candidate = format!("{}{:05x}", &uuid[..27], suffix);
        if matches(&candidate) {
            return candidate;
        }
    }
    uuid.to_string()
}

// ---------- 1.19.3+ 的默认皮肤选择 ----------

/// 默认皮肤数组长度：9 套皮肤 × 宽/细两版。
const MODERN_SKIN_COUNT: i32 = 18;
/// `wide/steve` 在数组里的下标。
const MODERN_WIDE_STEVE: usize = 15;
/// `slim/alex` 在数组里的下标。
const MODERN_SLIM_ALEX: usize = 0;

/// 复刻 Java `UUID.hashCode()`：
/// `(most ^ least) >>> 32 ^ (int)(most ^ least)`。
fn java_uuid_hash(uuid: &str) -> Option<i32> {
    if uuid.len() != 32 {
        return None;
    }
    let most = u64::from_str_radix(&uuid[..16], 16).ok()?;
    let least = u64::from_str_radix(&uuid[16..], 16).ok()?;
    let hilo = most ^ least;
    Some(((hilo >> 32) as u32 ^ hilo as u32) as i32)
}

/// 1.19.3+ 里这个 UUID 会被游戏选中的默认皮肤下标。
fn modern_skin_index(uuid: &str) -> Option<usize> {
    java_uuid_hash(uuid).map(|hash| hash.rem_euclid(MODERN_SKIN_COUNT) as usize)
}

fn minecraft_release_in(id: &str) -> Option<(u32, Option<u32>)> {
    let bytes = id.as_bytes();
    let mut i = 0;
    while i + 2 < bytes.len() {
        let independent = i == 0 || !(bytes[i - 1].is_ascii_digit() || bytes[i - 1] == b'.');
        if bytes[i] == b'1' && bytes[i + 1] == b'.' && independent && bytes[i + 2].is_ascii_digit()
        {
            let start = i + 2;
            let mut end = start;
            while end < bytes.len() && bytes[end].is_ascii_digit() {
                end += 1;
            }
            let minor = id[start..end].parse().ok()?;
            let patch = if end + 1 < bytes.len()
                && bytes[end] == b'.'
                && bytes[end + 1].is_ascii_digit()
            {
                let pstart = end + 1;
                let mut pend = pstart;
                while pend < bytes.len() && bytes[pend].is_ascii_digit() {
                    pend += 1;
                }
                id[pstart..pend].parse().ok()
            } else {
                None
            };
            return Some((minor, patch));
        }
        i += 1;
    }
    None
}

/// 这个版本用的是不是 1.19.3 之后的「18 个默认皮肤」规则。
///
/// 1.19.3 的第一个快照是 22w42a；更早的 Alpha/Beta 直接排除。
fn uses_modern_default_skins(version_id: &str) -> bool {
    let id = version_id.trim();
    let lower = id.to_ascii_lowercase();
    if lower.starts_with('b')
        || lower.starts_with('a')
        || lower.starts_with("rd-")
        || lower.starts_with("inf-")
    {
        return false;
    }
    // 周快照
    if id.len() >= 3 && id.as_bytes()[2] == b'w' {
        if let Ok(year) = id[..2].parse::<u32>() {
            if year != 22 {
                return year > 22;
            }
            let week: String = id[3..].chars().take_while(|c| c.is_ascii_digit()).collect();
            return week.parse::<u32>().is_ok_and(|w| w >= 42);
        }
    }
    // 正式版（含加载器目录名里夹着的原版号）
    if let Some((minor, patch)) = minecraft_release_in(id) {
        return minor > 19 || (minor == 19 && patch.unwrap_or(0) >= 3);
    }
    // "26.2" 这种新年份版本号
    let major: String = id.chars().take_while(|c| c.is_ascii_digit()).collect();
    major.parse::<u32>().is_ok_and(|m| m >= 20)
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn base64_encode(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

/// 解析 base64 的 textures 属性。
///
/// 返回 `None` 表示解析失败（坏 base64 / 坏 JSON）；返回 `Some` 表示
/// 解析成功 —— 这时 `skin_url` / `cape_url` 仍可能为 `None`（没设
/// 皮肤），由调用方决定怎么回落。纯函数，可单测。
fn parse_textures(value: &str) -> Option<ResolvedTextures> {
    let bytes = base64::engine::general_purpose::STANDARD.decode(value).ok()?;
    let doc: TexturesDoc = serde_json::from_slice(&bytes).ok()?;
    // textures 整个缺失 = 没设过任何皮肤，不是错误
    let set = doc.textures.unwrap_or_default();
    // 先取模型（借用地），再移动 url 字段
    let model = set
        .skin
        .as_ref()
        .and_then(|s| s.metadata.as_ref())
        .and_then(|m| m.model.clone());
    Some(ResolvedTextures {
        skin_url: set.skin.map(|s| s.url),
        cape_url: set.cape.map(|c| c.url),
        model: SkinModel::from_metadata(model.as_deref()),
    })
}

/// 皮肤位图的缓存文件名：URL 的 sha1。同一 URL 永远同一文件。
fn blob_path(layout: &PathLayout, url: &str) -> PathBuf {
    let mut hasher = Sha1::new();
    hasher.update(url.as_bytes());
    let mut hash = String::with_capacity(40);
    for byte in hasher.finalize() {
        hash.push_str(&format!("{byte:02x}"));
    }
    layout.skin_blob_dir().join(format!("{hash}.png"))
}

fn read_index(layout: &PathLayout, uuid: &str, now: u64) -> Option<SkinIndex> {
    let text = fs::read_to_string(layout.skin_index_file(uuid)).ok()?;
    let index: SkinIndex = serde_json::from_str(&text).ok()?;
    index.is_fresh(now).then_some(index)
}

fn write_index(layout: &PathLayout, uuid: &str, index: &SkinIndex) {
    let path = layout.skin_index_file(uuid);
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let Ok(text) = serde_json::to_string_pretty(index) else {
        return;
    };
    // 临时文件 + rename：写一半的索引比没有索引更糟
    let tmp = path.with_extension("json.tmp");
    if fs::write(&tmp, text).is_ok() {
        let _ = fs::rename(&tmp, &path);
    }
}

/// 拉一个 UUID 的 profile 并解出 textures。失败返回 `None`（调用方回落默认皮肤）。
async fn fetch_textures(server_url: &str, uuid: &str) -> Option<ResolvedTextures> {
    let http = reqwest::Client::builder()
        .user_agent(format!("XCL/{} (skin)", env!("CARGO_PKG_VERSION")))
        .connect_timeout(std::time::Duration::from_secs(10))
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .ok()?;
    let url = format!(
        "{}/sessionserver/session/minecraft/profile/{uuid}",
        server_url.trim_end_matches('/')
    );
    let resp = http.get(&url).send().await.ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let body: ProfileResponse = resp.json().await.ok()?;
    let value = body
        .properties
        .into_iter()
        .find(|p| p.name == "textures")
        .map(|p| p.value)?;
    parse_textures(&value)
}

/// 下载皮肤位图，带磁盘缓存。失败返回 `None`。
async fn cached_download(layout: &PathLayout, url: &str) -> Option<Vec<u8>> {
    let path = blob_path(layout, url);
    if let Ok(bytes) = fs::read(&path) {
        return Some(bytes);
    }
    let http = reqwest::Client::builder()
        .user_agent(format!("XCL/{} (skin)", env!("CARGO_PKG_VERSION")))
        .connect_timeout(std::time::Duration::from_secs(10))
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .ok()?;
    let resp = http.get(url).send().await.ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let bytes = resp.bytes().await.ok()?;
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let tmp = path.with_extension("png.part");
    if fs::write(&tmp, &bytes).is_ok() {
        let _ = fs::rename(&tmp, &path);
    }
    Some(bytes.to_vec())
}

/// 内置默认皮肤的渲染数据。`preferred` 是离线账号手动选的模型；`None` 按 UUID 自动。
fn default_render(uuid: Option<&str>, preferred: Option<SkinModel>) -> SkinRender {
    let model = preferred.unwrap_or_else(|| uuid.map(offline_model).unwrap_or_default());
    let png = match model {
        SkinModel::Slim => ALEX_PNG,
        SkinModel::Default => STEVE_PNG,
    };
    SkinRender {
        skin: base64_encode(png),
        cape: None,
        model,
        source: SkinSource::OfflineDefault,
    }
}

impl AppState {
    /// 拿一个账号的皮肤渲染数据。
    ///
    /// `account_id` 为 `None` 时用当前账号。永不失败：任何一步出问题
    /// （没配服务器、profile 端点没实现、下载超时……）都回落到内置默认
    /// 皮肤 —— 皮肤是装饰，不能因为它弹错挡住启动。
    pub async fn skin_render(&self, account_id: Option<&str>) -> SkinRender {
        let account = {
            let store = self
                .accounts
                .read()
                .unwrap_or_else(|e| e.into_inner());
            match account_id {
                Some(id) => store.find(id).cloned(),
                None => store.active().cloned(),
            }
        };
        let Some(account) = account else {
            return default_render(None, None);
        };
        // 离线账号没有服务器皮肤，直接发玩家选的默认皮肤
        if !account.is_authlib() {
            return default_render(Some(&account.uuid), account.skin_model);
        }
        let Some(server_url) = account.server_url.clone() else {
            return default_render(Some(&account.uuid), None);
        };

        // 1. 解析皮肤地址（TTL 缓存；过期或没有才请求服务器）
        let now = now_secs();
        let textures = match read_index(&self.layout, &account.uuid, now) {
            Some(index) => ResolvedTextures {
                skin_url: index.skin_url,
                cape_url: index.cape_url,
                model: SkinModel::from_metadata(index.model.as_deref()),
            },
            None => match fetch_textures(&server_url, &account.uuid).await {
                Some(resolved) => {
                    write_index(
                        &self.layout,
                        &account.uuid,
                        &SkinIndex {
                            skin_url: resolved.skin_url.clone(),
                            cape_url: resolved.cape_url.clone(),
                            model: Some(resolved.model.as_str().to_string()),
                            fetched_at: now,
                        },
                    );
                    resolved
                }
                None => return default_render(Some(&account.uuid), None),
            },
        };

        // 2. 下载位图（磁盘缓存）。没设皮肤也回落默认
        let Some(skin_url) = textures.skin_url else {
            return default_render(Some(&account.uuid), None);
        };
        let Some(skin_bytes) = cached_download(&self.layout, &skin_url).await else {
            return default_render(Some(&account.uuid), None);
        };
        let cape = match textures.cape_url {
            Some(url) => cached_download(&self.layout, &url).await,
            None => None,
        };
        SkinRender {
            skin: base64_encode(&skin_bytes),
            cape: cape.as_deref().map(base64_encode),
            model: textures.model,
            source: SkinSource::Server,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn textures_base64(skin_url: Option<&str>, cape_url: Option<&str>, model: Option<&str>) -> String {
        let mut tex = serde_json::Map::new();
        if let Some(url) = skin_url {
            let mut entry = serde_json::Map::new();
            entry.insert("url".into(), url.into());
            if let Some(model) = model {
                entry.insert(
                    "metadata".into(),
                    serde_json::json!({ "model": model }),
                );
            }
            tex.insert("SKIN".into(), serde_json::Value::Object(entry));
        }
        if let Some(url) = cape_url {
            tex.insert("CAPE".into(), serde_json::json!({ "url": url }));
        }
        let doc = serde_json::json!({
            "timestamp": 1700000000000i64,
            "profileId": "abc",
            "profileName": "Hero",
            "textures": tex,
        });
        base64::engine::general_purpose::STANDARD
            .encode(serde_json::to_vec(&doc).unwrap())
    }

    #[test]
    fn parses_skin_and_cape() {
        let value = textures_base64(
            Some("https://textures.minecraft.net/texture/abc"),
            Some("https://textures.minecraft.net/texture/def"),
            None,
        );
        let resolved = parse_textures(&value).unwrap();
        assert_eq!(
            resolved.skin_url.as_deref(),
            Some("https://textures.minecraft.net/texture/abc")
        );
        assert_eq!(
            resolved.cape_url.as_deref(),
            Some("https://textures.minecraft.net/texture/def")
        );
        assert_eq!(resolved.model, SkinModel::Default);
    }

    #[test]
    fn parses_slim_model() {
        let value = textures_base64(Some("https://x/s.png"), None, Some("slim"));
        let resolved = parse_textures(&value).unwrap();
        assert_eq!(resolved.model, SkinModel::Slim);
        assert!(resolved.cape_url.is_none());
    }

    #[test]
    fn missing_skin_is_not_an_error() {
        // 只有披风、或 textures 整个缺失：解得出信息但 skin_url 为 None
        let value = textures_base64(None, Some("https://x/c.png"), None);
        let resolved = parse_textures(&value).unwrap();
        assert!(resolved.skin_url.is_none());
        assert!(resolved.cape_url.is_some());

        let doc = serde_json::json!({ "timestamp": 1i64, "profileId": "a", "profileName": "b" });
        let value = base64::engine::general_purpose::STANDARD
            .encode(serde_json::to_vec(&doc).unwrap());
        assert!(parse_textures(&value).unwrap().skin_url.is_none());
    }

    #[test]
    fn garbage_returns_none() {
        assert!(parse_textures("不是 base64!!!").is_none());
        assert!(parse_textures("").is_none());
        let junk = base64::engine::general_purpose::STANDARD.encode("{ 不是 json".as_bytes());
        assert!(parse_textures(&junk).is_none());
    }

    #[test]
    fn offline_model_parity_rule() {
        // 全零：0^0^0^0 = 0，偶数 → Steve
        assert_eq!(offline_model(&"0".repeat(32)), SkinModel::Default);
        // 只有第 31 位非零：奇数 → Alex
        let mut uuid = "0".repeat(32);
        uuid.replace_range(31..32, "f");
        assert_eq!(offline_model(&uuid), SkinModel::Slim);
        // 两个非零位相等：1^1 = 0，偶数 → Steve（证明是异或不是计数）
        let mut uuid = "0".repeat(32);
        uuid.replace_range(7..8, "1");
        uuid.replace_range(23..24, "1");
        assert_eq!(offline_model(&uuid), SkinModel::Default);
        // 两个非零位不等：1^2 = 3，奇数 → Alex
        let mut uuid = "0".repeat(32);
        uuid.replace_range(7..8, "1");
        uuid.replace_range(23..24, "2");
        assert_eq!(offline_model(&uuid), SkinModel::Slim);
        // 不是 32 位：不 panic，按 Steve
        assert_eq!(offline_model("abc"), SkinModel::Default);
        assert_eq!(offline_model(""), SkinModel::Default);
    }

    #[test]
    fn launch_uuid_matches_requested_model_legacy() {
        // 1.19.2 及更早：奇偶规则
        let base = "0".repeat(32); // 偶数 → Steve
        // 自动：原样返回
        assert_eq!(launch_uuid(&base, None, "1.12.2"), base);
        // 已经是 Steve：不做无意义的改动
        assert_eq!(launch_uuid(&base, Some(SkinModel::Default), "1.12.2"), base);

        let alex = launch_uuid(&base, Some(SkinModel::Slim), "1.12.2");
        assert_eq!(offline_model(&alex), SkinModel::Slim);
        // 只动末 5 位，前缀保持
        assert_eq!(&alex[..27], &base[..27]);

        // 再切回 Steve：能找回一个 Steve UUID
        let steve = launch_uuid(&alex, Some(SkinModel::Default), "1.12.2");
        assert_eq!(offline_model(&steve), SkinModel::Default);
        assert_eq!(&steve[..27], &base[..27]);

        // 非法 UUID 不 panic、不篡改
        assert_eq!(launch_uuid("abc", Some(SkinModel::Slim), "1.12.2"), "abc");
        assert_eq!(launch_uuid("", Some(SkinModel::Slim), "1.12.2"), "");
    }

    #[test]
    fn java_hash_matches_java_uuid() {
        // 用 JDK 跑 UUID.hashCode() 得到的真值
        assert_eq!(java_uuid_hash(&"0".repeat(32)), Some(0));
        assert_eq!(
            java_uuid_hash("a6017a8759153549bd2261a4a9b71fbb"),
            Some(-343_854_639)
        );
        assert_eq!(java_uuid_hash("abc"), None);
    }

    #[test]
    fn modern_skin_index_matches_game_rule() {
        // floorMod(hashCode, 18)：0 = slim/alex，15 = wide/steve
        assert_eq!(modern_skin_index(&"0".repeat(32)), Some(0));
        // 这个账号原本会被游戏选成 wide/alex（下标 9）
        assert_eq!(
            modern_skin_index("a6017a8759153549bd2261a4a9b71fbb"),
            Some(9)
        );
    }

    #[test]
    fn launch_uuid_targets_modern_default_skins() {
        let base = "a6017a8759153549bd2261a4a9b71fbb"; // 下标 9，宽臂 Alex
        // 自动：原样返回
        assert_eq!(launch_uuid(base, None, "26.2"), base);

        let steve = launch_uuid(base, Some(SkinModel::Default), "26.2");
        assert_eq!(modern_skin_index(&steve), Some(MODERN_WIDE_STEVE));
        assert_eq!(&steve[..27], &base[..27]);

        let alex = launch_uuid(base, Some(SkinModel::Slim), "26.2");
        assert_eq!(modern_skin_index(&alex), Some(MODERN_SLIM_ALEX));
        assert_eq!(&alex[..27], &base[..27]);

        // 加载器目录名也要认出版本
        let steve_fabric = launch_uuid(base, Some(SkinModel::Default), "fabric-loader-0.15.11-1.20.1");
        assert_eq!(modern_skin_index(&steve_fabric), Some(MODERN_WIDE_STEVE));
    }

    #[test]
    fn modern_default_skin_era() {
        assert!(!uses_modern_default_skins("1.12.2"));
        assert!(!uses_modern_default_skins("1.19"));
        assert!(!uses_modern_default_skins("1.19.2"));
        assert!(uses_modern_default_skins("1.19.3"));
        assert!(uses_modern_default_skins("1.20.1"));
        assert!(uses_modern_default_skins("1.21.11"));
        assert!(!uses_modern_default_skins("22w41a"));
        assert!(uses_modern_default_skins("22w42a"));
        assert!(uses_modern_default_skins("24w14a"));
        assert!(uses_modern_default_skins("26.2"));
        assert!(uses_modern_default_skins("26.3"));
        assert!(!uses_modern_default_skins("b1.7.3"));
        // 加载器目录名
        assert!(uses_modern_default_skins("fabric-loader-0.15.11-1.20.1"));
        assert!(!uses_modern_default_skins("1.12.2-forge14.23.5.2859"));
    }

    #[test]
    fn preferred_model_overrides_uuid_parity() {
        let base = "0".repeat(32); // 自动是 Steve
        let render = default_render(Some(&base), Some(SkinModel::Slim));
        assert_eq!(render.model, SkinModel::Slim);
        let auto = default_render(Some(&base), None);
        assert_eq!(auto.model, SkinModel::Default);
        // Alex 贴图和 Steve 不是同一张
        assert_ne!(render.skin, auto.skin);
    }

    #[test]
    fn index_ttl_boundary() {
        let now = 1_000_000u64;
        let fresh = SkinIndex {
            fetched_at: now - SKIN_INDEX_TTL_SECS + 1,
            ..Default::default()
        };
        assert!(fresh.is_fresh(now));
        let expired = SkinIndex {
            fetched_at: now - SKIN_INDEX_TTL_SECS,
            ..Default::default()
        };
        assert!(!expired.is_fresh(now));
        // 时钟往回跳：saturating_sub 不会 panic，当成刚写过
        let future = SkinIndex {
            fetched_at: now + 9999,
            ..Default::default()
        };
        assert!(future.is_fresh(now));
    }

    #[test]
    fn default_render_embeds_valid_png() {
        let zero = "0".repeat(32);
        for uuid in [None, Some(zero.as_str()), Some("abc")] {
            let render = default_render(uuid, None);
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(&render.skin)
                .unwrap();
            assert!(bytes.starts_with(b"\x89PNG\r\n\x1a\n"), "默认皮肤应是合法 PNG");
            assert!(render.cape.is_none());
            assert_eq!(render.source, SkinSource::OfflineDefault);
        }
        // slim 账号发的是 Alex（两个文件不同）
        let steve = default_render(Some(&"0".repeat(32)), None);
        let mut alex_uuid = "0".repeat(32);
        alex_uuid.replace_range(31..32, "f");
        let alex = default_render(Some(&alex_uuid), None);
        assert_eq!(steve.model, SkinModel::Default);
        assert_eq!(alex.model, SkinModel::Slim);
        assert_ne!(steve.skin, alex.skin);
    }

    #[test]
    fn blob_path_is_stable_and_distinct() {
        let layout = PathLayout::new("C:/mc", "C:/cache");
        let a = blob_path(&layout, "https://x/skin.png");
        let b = blob_path(&layout, "https://x/skin.png");
        let c = blob_path(&layout, "https://x/other.png");
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert!(a.starts_with("C:/cache/skin/blob"));
        assert_eq!(a.extension().unwrap(), "png");
    }
}
