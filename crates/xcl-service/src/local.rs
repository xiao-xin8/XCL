//! 本地已安装版本的扫描。
//!
//! 这是「安装」的另一半：`install.rs` 说「这个版本需要什么」，这里说
//! 「本机到底装了什么」。没有它，界面只能显示「可以下载的版本」，
//! 玩家装完了也看不出来 —— 更没法启动。
//!
//! 全程只读本地磁盘，不联网。版本清单拉不到的时候它照样能用。

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use serde::Serialize;
use xcl_core::{PathLayout, VersionDetail, VersionKind};

/// 一个本地已安装的版本。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalVersion {
    pub id: String,
    /// `release` / `snapshot` / 模组加载器自报的类型
    pub version_type: String,
    /// 图标分类：从库名 / 主类识别出装了哪个加载器
    pub kind: VersionKind,
    /// 这个版本能不能装 Mod（Forge / Fabric / …）。纯净版为 `false`
    pub modable: bool,
    /// 老版本（1.5 及更早）只有材质包，没有资源包
    pub texture_packs: bool,
    /// 1.7+ 才会把日志写进 `logs/`；更老的版本只输出到控制台
    pub has_logs: bool,
    /// 继承自哪个版本（Fabric / Forge 生成的 JSON 都有）
    pub inherits_from: Option<String>,
    /// 这个版本对应的 MC 版本号（`1.20.1`）。用来给下载页的「版本筛选」做选项。
    ///
    /// 取值顺序（越靠前越权威）：`XCL/instance.toml`（安装/改名时落盘）→ `inheritsFrom` →
    /// 规范命名的加载器 id 尾段 → 客户端 jar 里的 `version.json` → 从 id 里猜。
    pub game_version: Option<String>,
    /// 装了哪个加载器的哪个版本（`net.fabricmc:fabric-loader:0.20.1` → `0.20.1`）。
    ///
    /// 从 `libraries` 反推，和目录名无关 —— 实例改成任意名字都还能认出来。
    /// 只有加载器实例有值。
    pub loader_version: Option<String>,
    /// 自己目录里有没有客户端 jar。继承版本没有自己的 jar 是正常的
    pub has_client_jar: bool,
    /// 沿 `inheritsFrom` 找到 jar 了（自己或父版本），可以启动
    pub launchable: bool,
    pub java_major: Option<u32>,
    pub main_class: Option<String>,
    /// 目录最后修改时间（毫秒时间戳），用来排序
    pub modified_ms: u64,
    /// JSON 能否正常解析
    pub valid: bool,
    /// `valid = false` 时的原因
    pub error: Option<String>,
    /// 玩家自己写的说明（`XCL/instance.toml`）
    pub description: Option<String>,
    /// 收藏：列表里排最前
    pub favorite: bool,
    /// 自定义图标的 data URL（`XCL/icon.png`），没有就 `None`
    pub icon: Option<String>,
}

/// 扫描 `<root>/versions/` 下的所有版本，按最近修改倒序。
pub fn scan(layout: &PathLayout) -> Vec<LocalVersion> {
    let dir = layout.versions_dir();
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };

    let mut found: Vec<LocalVersion> = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let Some(id) = path.file_name().and_then(|n| n.to_str()).map(str::to_string) else {
            continue;
        };

        found.push(inspect(&id, &path));
    }

    // 「有没有 jar」可能落在父版本身上，所以要等全部扫完再算一遍
    let with_jar: HashSet<String> = found
        .iter()
        .filter(|version| version.has_client_jar)
        .map(|version| version.id.clone())
        .collect();

    for version in &mut found {
        version.launchable = version.valid
            && (version.has_client_jar
                || version
                    .inherits_from
                    .as_ref()
                    .is_some_and(|parent| with_jar.contains(parent)));
    }

    found.sort_by(|a, b| {
        // 收藏的永远排最前，其余按最近改动
        b.favorite
            .cmp(&a.favorite)
            .then_with(|| b.modified_ms.cmp(&a.modified_ms))
            .then_with(|| a.id.cmp(&b.id))
    });
    found
}

/// 判断某个版本是不是已安装（给版本列表打标记用）。
pub fn installed_ids(layout: &PathLayout) -> HashSet<String> {
    scan(layout)
        .into_iter()
        .filter(|version| version.valid)
        .map(|version| version.id)
        .collect()
}

/// 资源包机制从 1.6 起；更老的版本只有材质包（`texturepacks`）。
fn uses_texture_packs(id: &str) -> bool {
    let lower = id.to_ascii_lowercase();
    // Alpha / Beta / Infdev 的 id 里也含 "1.x"，必须先拦掉
    if lower.starts_with('b')
        || lower.starts_with('a')
        || lower.starts_with("rd-")
        || lower.starts_with("inf-")
    {
        return true;
    }
    // 周快照和新年份版本号都解析不出 "1.x"，一律按现代版本（资源包）算
    matches!(crate::launch::minecraft_major(id), Some(major) if major < 6)
}

/// 1.7.2 起版本 JSON 带 `logging`，游戏才会写 `logs/latest.log`。
/// 更老的版本只把输出打到控制台，不会生成日志目录。
fn uses_log_files(id: &str) -> bool {
    let lower = id.to_ascii_lowercase();
    if lower.starts_with('b')
        || lower.starts_with('a')
        || lower.starts_with("rd-")
        || lower.starts_with("inf-")
    {
        return false;
    }
    // 周快照和新年份版本号都解析不出 "1.x"，都按现代版本算
    match crate::launch::minecraft_major(id) {
        Some(major) => major >= 7,
        None => true,
    }
}

/// 猜这个版本对应的 MC 版本号，只认「看起来像」的，猜不出就 `None`。
pub(crate) fn guess_game_version(id: &str, inherits_from: Option<&str>) -> Option<String> {
    // 继承式（Forge / 老式 Fabric）：父版本 id 一般就是 MC 版本
    if let Some(parent) = inherits_from {
        if let Some(found) = mc_version_like(parent) {
            return Some(found.to_string());
        }
        if let Some(found) = parent.rsplit('-').next().and_then(mc_version_like) {
            return Some(found.to_string());
        }
    }
    if let Some(found) = mc_version_like(id) {
        return Some(found.to_string());
    }
    // 自包含的加载器实例：MC 版本永远在最后一段
    // （`fabric-loader-0.20.1-1.21.10` / `forge-61.2.1-1.21.11` /
    //   `neoforge-21.10.64-1.21.10`）。
    // 不能只认 `fabric-loader-`：Forge / NeoForge 装出来也是自包含的，
    // 写死前缀会让这两个加载器的版本在「下载 Mod」页整个消失。
    if let Some((_, last)) = id.rsplit_once('-')
        && mc_version_like(last).is_some()
    {
        return Some(last.to_string());
    }
    None
}

/// `1.20.1` / `1.7.10` / `1.20` / `24w14a` / `a1.2.6` 这类才算 MC 版本。
fn mc_version_like(value: &str) -> Option<&str> {
    let bytes = value.as_bytes();

    // 快照：24w14a
    if bytes.len() == 6
        && bytes[0].is_ascii_digit()
        && bytes[1].is_ascii_digit()
        && bytes[2] == b'w'
        && bytes[3].is_ascii_digit()
        && bytes[4].is_ascii_digit()
        && bytes[5].is_ascii_alphabetic()
    {
        return Some(value);
    }

    // 远古版本：a1.2.6 / b1.7.3
    if bytes.len() >= 4
        && matches!(bytes[0], b'a' | b'b')
        && bytes[1].is_ascii_digit()
        && bytes[2] == b'.'
    {
        return Some(value);
    }

    // 正式版：1 / 1.20 / 1.20.1
    let parts: Vec<&str> = value.split('.').collect();
    if !(2..=3).contains(&parts.len()) {
        return None;
    }
    if parts
        .iter()
        .all(|part| !part.is_empty() && part.len() <= 3 && part.bytes().all(|b| b.is_ascii_digit()))
    {
        return Some(value);
    }
    None
}

/// 决定「这个版本属于哪个原版」，越靠前越权威。
///
/// 1. `XCL/instance.toml`：安装 / 改名时落盘的权威值；
/// 2. `inheritsFrom`：继承式实例的父版本一般就是原版号；
/// 3. 规范命名的加载器 id 尾段（`fabric-loader-0.20.1-26.2`）：纯字符串，快；
/// 4. 目录名本身就是一个干净的版本号（`26.2`、`1.20.1`）：纯字符串，快；
/// 5. 客户端 jar 里的 `version.json`：连名字都改过的老实例只能靠它；
/// 6. 最后的 id 猜测。
///
/// ⚠️ 第 5 步要打开客户端 jar（几十 MB），是整条链里唯一的磁盘重活。
/// 它必须排在所有纯字符串判据之后 —— 版本列表每次刷新都会把所有版本扫一遍，
/// 一个原版目录就为了确认「它自己叫 26.2」去开一次 zip，会被杀毒软件的
/// 实时扫描放大成肉眼可见的卡顿。别把它往前挪。
fn resolve_game_version(
    id: &str,
    json_id: Option<&str>,
    inherits_from: Option<&str>,
    meta: Option<String>,
    dir: &Path,
) -> Option<String> {
    if let Some(game) = meta {
        return Some(game);
    }
    if let Some(parent) = inherits_from
        && let Some(found) = game_from_parent(parent)
    {
        return Some(found);
    }
    if xcl_core::kind_from_id(id).is_some()
        && let Some(found) = guess_game_version(id, None)
    {
        return Some(found);
    }
    // 原版目录名就是 MC 版本号，没有任何歧义，不用碰 jar
    if let Some(found) = mc_version_like(id) {
        return Some(found.to_string());
    }
    // 版本 JSON 里记的 id：目录被改过名（整合包实例叫「我的整合」）时，
    // JSON 的 id 往往还是规范命名（`fabric-loader-0.20.1-26.2`），尾段就是原版号。
    // 纯字符串，比下面开 jar 便宜几个数量级，而且 JSON 已经读进内存了。
    if let Some(json_id) = json_id
        && let Some(found) = guess_game_version(json_id, None)
    {
        return Some(found);
    }
    read_jar_game_version(dir, id).or_else(|| guess_game_version(id, None))
}

fn game_from_parent(parent: &str) -> Option<String> {
    if let Some(found) = mc_version_like(parent) {
        return Some(found.to_string());
    }
    parent
        .rsplit('-')
        .next()
        .and_then(mc_version_like)
        .map(str::to_string)
}

/// 从客户端 jar 里的 `version.json` 读原版号（1.14 起官方 jar 都带）。
///
/// 这是唯一不依赖任何命名的来源：目录、JSON 里的 `id`、
/// `instance.toml` 全改/全丢，jar 里的版本号还是对的。
fn read_jar_game_version(dir: &Path, id: &str) -> Option<String> {
    let jar = find_jar(dir, id)?;
    let file = std::fs::File::open(jar).ok()?;
    let mut archive = zip::ZipArchive::new(file).ok()?;
    let mut entry = archive.by_name("version.json").ok()?;
    let mut text = String::new();
    std::io::Read::read_to_string(&mut entry, &mut text).ok()?;
    let value: serde_json::Value = serde_json::from_str(&text).ok()?;
    let described = value.get("id")?.as_str()?.trim();
    mc_version_like(described).map(str::to_string)
}

/// 这个实例装的是加载器的哪个版本号。
///
/// 优先看 `libraries`（权威、与名字无关），认不出来才从规范命名的 id 里切。
/// 返回的版本号已经剥掉原版前缀：`1.21.11-61.2.1` → `61.2.1`。
fn resolve_loader_version(
    id: &str,
    libraries: &[xcl_core::Library],
    game: Option<&str>,
) -> Option<String> {
    if let Some((_, raw)) = xcl_core::loader_stamp(libraries) {
        return Some(strip_game_prefix(&raw, game));
    }
    // 认不出「规范命名」就不当它是加载器实例
    xcl_core::kind_from_id(id)?;
    let rest = id.split_once('-')?.1;
    // `fabric-loader-0.20.1-26.2` 多一段 `loader-`，Forge / NeoForge 没有
    let rest = if id.to_ascii_lowercase().starts_with("fabric-loader-") {
        rest.split_once('-')?.1
    } else {
        rest
    };
    let (version, _) = rest.rsplit_once('-')?;
    Some(version.to_string())
}

/// `1.21.11-61.2.1` + 原版 `1.21.11` → `61.2.1`；剥不掉就原样返回。
fn strip_game_prefix(raw: &str, game: Option<&str>) -> String {
    if let Some(game) = game
        && let Some(rest) = raw.strip_prefix(&format!("{game}-"))
    {
        return rest.to_string();
    }
    if let Some((head, tail)) = raw.split_once('-')
        && mc_version_like(head).is_some()
    {
        return tail.to_string();
    }
    raw.to_string()
}

/// 一个已装版本的身份：与目录名无关的三个稳定维度。
///
/// 目录名是给人看的，随时会改（`fabric-loader-0.20.1-26.2` → `我的整合`）。
/// 判断「装没装」「能不能装 Mod」一律看这里，别再拿 id 去拼字符串。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VersionIdentity {
    /// 目录名。只用于显示和定位，不参与任何判定。
    pub id: String,
    pub kind: VersionKind,
    pub game_version: Option<String>,
    pub loader_version: Option<String>,
    pub valid: bool,
}

impl LocalVersion {
    pub fn identity(&self) -> VersionIdentity {
        VersionIdentity {
            id: self.id.clone(),
            kind: self.kind,
            game_version: self.game_version.clone(),
            loader_version: self.loader_version.clone(),
            valid: self.valid,
        }
    }
}

/// 「这个加载器 + 这个加载器版本 + 这个原版」装过了吗。
pub fn has_loader(versions: &[LocalVersion], kind: VersionKind, loader: &str, game: &str) -> bool {
    versions.iter().any(|version| {
        version.valid
            && version.kind == kind
            && version.game_version.as_deref() == Some(game)
            && version.loader_version.as_deref() == Some(loader)
    })
}

/// 这个原版的纯净实例装过了吗（装了加载器的不算）。
pub fn has_vanilla(versions: &[LocalVersion], game: &str) -> bool {
    versions.iter().any(|version| {
        version.valid && !version.kind.is_modable() && version.game_version.as_deref() == Some(game)
    })
}

/// 装在某个原版上的加载器实例：`(显示名, 实例 id)`。给版本清单打「已安装 Forge」用。
pub fn loader_instances(versions: &[LocalVersion], game: &str) -> Vec<(&'static str, String)> {
    versions
        .iter()
        .filter(|version| version.valid && version.game_version.as_deref() == Some(game))
        .filter_map(|version| loader_label(version.kind).map(|label| (label, version.id.clone())))
        .collect()
}

fn loader_label(kind: VersionKind) -> Option<&'static str> {
    match kind {
        VersionKind::Fabric => Some("Fabric"),
        VersionKind::Forge => Some("Forge"),
        VersionKind::NeoForge => Some("NeoForge"),
        _ => None,
    }
}

fn inspect(id: &str, dir: &Path) -> LocalVersion {
    let modified_ms = dir
        .metadata()
        .and_then(|meta| meta.modified())
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|dur| dur.as_millis() as u64)
        .unwrap_or(0);

    let meta = read_version_meta(dir);
    // 安装时记下的值最权威（改名也不丢）；没有才按目录名猜
    let meta_game = meta.game_version.clone();

    let has_client_jar = find_jar(dir, id).is_some();
    let Some(json_path) = find_json(dir, id) else {
        return LocalVersion {
            id: id.to_string(),
            game_version: resolve_game_version(id, None, None, meta_game, dir),
            version_type: "unknown".to_string(),
            kind: xcl_core::detect_kind(id, None, None, &[]),
            loader_version: None,
            modable: false,
            texture_packs: uses_texture_packs(id),
            has_logs: uses_log_files(id),
            inherits_from: None,
            has_client_jar,
            launchable: false,
            java_major: None,
            main_class: None,
            modified_ms,
            valid: false,
            error: Some("目录里没有版本描述文件（.json）".to_string()),
            description: meta.description,
            favorite: meta.favorite,
            icon: meta.icon,
        };
    };

    match read_detail(&json_path) {
        Ok(detail) => {
            let kind = xcl_core::detect_kind(
                id,
                detail.version_type.as_deref(),
                detail.main_class.as_deref(),
                &detail.libraries,
            );
            let game_version = resolve_game_version(
                id,
                Some(&detail.id),
                detail.inherits_from.as_deref(),
                meta_game,
                dir,
            );
            LocalVersion {
                id: id.to_string(),
                loader_version: resolve_loader_version(id, &detail.libraries, game_version.as_deref()),
                game_version,
                version_type: detail.version_type.unwrap_or_else(|| "unknown".to_string()),
                modable: kind.is_modable(),
                texture_packs: uses_texture_packs(id),
                has_logs: uses_log_files(id),
                kind,
                inherits_from: detail.inherits_from,
                has_client_jar,
                launchable: false, // 稍后统一算
                java_major: detail.java_version.as_ref().map(|v| v.major_version),
                main_class: detail.main_class,
                modified_ms,
                valid: true,
                error: None,
                description: meta.description,
                favorite: meta.favorite,
                icon: meta.icon,
            }
        }
        Err(err) => LocalVersion {
            id: id.to_string(),
            game_version: resolve_game_version(id, None, None, meta_game, dir),
            version_type: "unknown".to_string(),
            kind: xcl_core::detect_kind(id, None, None, &[]),
            loader_version: None,
            modable: false,
            texture_packs: uses_texture_packs(id),
            has_logs: uses_log_files(id),
            inherits_from: None,
            has_client_jar,
            launchable: false,
            java_major: None,
            main_class: None,
            modified_ms,
            valid: false,
            error: Some(err.message),
            description: meta.description,
            favorite: meta.favorite,
            icon: meta.icon,
        },
    }
}

/// 版本自定义元数据：描述、收藏、图标。
struct VersionMeta {
    description: Option<String>,
    favorite: bool,
    icon: Option<String>,
    /// 安装时记下的「属于哪个原版」（改名也不会丢）
    game_version: Option<String>,
}

/// 图标文件上限 512 KiB：前端会先缩到 128×128，这里只是兜底防手滑。
const MAX_ICON_BYTES: u64 = 512 * 1024;

/// 按文件头判断图片类型。
///
/// 不能看扩展名，也不能信 HTTP 的 content-type：Modrinth 的项目图标是 `.webp`，
/// 存到本地还叫 `icon.png`，镜像回的 content-type 也写着 `image/png`。
pub(crate) fn image_mime(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A]) {
        return Some("image/png");
    }
    if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return Some("image/jpeg");
    }
    if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        return Some("image/webp");
    }
    if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        return Some("image/gif");
    }
    None
}

fn read_version_meta(dir: &Path) -> VersionMeta {
    let settings = crate::isolation::settings_at(&dir.join("XCL").join("instance.toml"));

    // 图标直接转 data URL：列表一次要画几十个版本，走文件路径还得再开一次
    // 自定义协议，不如在这里一次性编码好。
    //
    // MIME 按魔数判断，不能写死 png：Modrinth 的项目图标实际是 `.webp`
    // （文件名字段还叫 `icon.png`，服务器连 content-type 都错标成 png），
    // 按 png 编码进去浏览器直接解不出来，图标就是一片空白。
    let icon_path = dir.join("XCL").join("icon.png");
    let icon = std::fs::metadata(&icon_path)
        .ok()
        .filter(|meta| meta.is_file() && meta.len() <= MAX_ICON_BYTES)
        .and_then(|_| std::fs::read(&icon_path).ok())
        .and_then(|bytes| {
            use base64::Engine as _;
            let mime = image_mime(&bytes)?;
            Some(format!(
                "data:{mime};base64,{}",
                base64::engine::general_purpose::STANDARD.encode(bytes)
            ))
        });

    VersionMeta {
        description: settings
            .description
            .map(|text| text.trim().to_string())
            .filter(|text| !text.is_empty()),
        favorite: settings.favorite.unwrap_or(false),
        icon,
        game_version: settings.game_version,
    }
}

/// 读并解析版本 JSON。
pub fn read_detail(json_path: &Path) -> Result<VersionDetail, xcl_core::AppError> {
    let text = std::fs::read_to_string(json_path)
        .map_err(|e| xcl_core::AppError::io(&e, "读取版本描述文件"))?;
    VersionDetail::parse(&text)
}

/// 按版本 id 从本地读出详情。不联网。
pub fn load(layout: &PathLayout, id: &str) -> Result<VersionDetail, xcl_core::AppError> {
    let dir = layout.version_dir(id);
    let Some(json_path) = find_json(&dir, id) else {
        return Err(xcl_core::AppError::not_found(
            "版本未安装",
            format!("{} 不在本地，找不到 {}", id, dir.display()),
        )
        .with_hint("先在下载页安装这个版本"));
    };
    read_detail(&json_path)
}

/// 找客户端 jar（自己目录里的）。
pub fn client_jar(layout: &PathLayout, id: &str) -> Option<PathBuf> {
    find_jar(&layout.version_dir(id), id)
}

/// 找版本 JSON。规范是 `<id>.json`，但手动改目录名的情况也兜一下。
fn find_json(dir: &Path, id: &str) -> Option<PathBuf> {
    let direct = dir.join(format!("{id}.json"));
    if direct.is_file() {
        return Some(direct);
    }
    first_with_ext(dir, "json")
}

/// 找客户端 jar。同理。
fn find_jar(dir: &Path, id: &str) -> Option<PathBuf> {
    let direct = dir.join(format!("{id}.jar"));
    if direct.is_file() {
        return Some(direct);
    }
    first_with_ext(dir, "jar")
}

fn first_with_ext(dir: &Path, ext: &str) -> Option<PathBuf> {
    let entries = std::fs::read_dir(dir).ok()?;
    let mut candidates: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.is_file()
                && path
                    .extension()
                    .and_then(|e| e.to_str())
                    .is_some_and(|e| e.eq_ignore_ascii_case(ext))
        })
        .collect();
    candidates.sort();
    candidates.into_iter().next()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 图标格式只能按文件头认：Modrinth 的图标是 webp，落地却叫 `icon.png`，
    /// 连镜像回的 content-type 都错写成 `image/png`。
    #[test]
    fn image_mime_sniffs_by_header_not_by_name() {
        assert_eq!(
            image_mime(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]),
            Some("image/png")
        );
        assert_eq!(image_mime(&[0xFF, 0xD8, 0xFF, 0xE0]), Some("image/jpeg"));

        let mut webp = b"RIFF".to_vec();
        webp.extend_from_slice(&[0x1A, 0, 0, 0]);
        webp.extend_from_slice(b"WEBP");
        assert_eq!(image_mime(&webp), Some("image/webp"));

        assert_eq!(image_mime(b"GIF89a..."), Some("image/gif"));
        assert_eq!(image_mime(b"<html>not an image</html>"), None);
        assert_eq!(image_mime(&[]), None);
    }

    /// 目录名被改成「我的整合」时，靠版本 JSON 里记的 id 也要认得出原版号。
    ///
    /// 传一个不存在的目录：只要实现去开 jar，这里必然返回 `None`，测试就红。
    /// 这条盯的是「版本列表每次刷新都把每个实例的 jar 开一遍」那个老毛病。
    #[test]
    fn renamed_instance_resolves_from_json_id_without_opening_the_jar() {
        let missing = std::path::Path::new("C:/definitely/not/here");
        let got = resolve_game_version(
            "我的整合",
            Some("fabric-loader-0.20.1-26.2"),
            None,
            None,
            missing,
        );
        assert_eq!(got.as_deref(), Some("26.2"));
    }

    #[test]
    fn game_version_is_guessed_for_every_shape() {
        assert_eq!(guess_game_version("1.20.1", None).as_deref(), Some("1.20.1"));
        assert_eq!(guess_game_version("1.7.10", None).as_deref(), Some("1.7.10"));
        assert_eq!(guess_game_version("24w14a", None).as_deref(), Some("24w14a"));
        // 自包含的加载器实例：MC 版本在最后一段（三种加载器一视同仁）
        assert_eq!(
            guess_game_version("fabric-loader-0.16.5-1.20.1", None).as_deref(),
            Some("1.20.1")
        );
        assert_eq!(
            guess_game_version("forge-61.2.1-1.21.11", None).as_deref(),
            Some("1.21.11")
        );
        assert_eq!(
            guess_game_version("neoforge-21.10.64-1.21.10", None).as_deref(),
            Some("1.21.10")
        );
        // 猜不出来的不乱猜
        assert_eq!(guess_game_version("my-pack", None), None);
        // 继承式：看父版本
        assert_eq!(
            guess_game_version("1.12.2-forge-14.23.5.2860", Some("1.12.2")).as_deref(),
            Some("1.12.2")
        );
        // 认不出来的别瞎猜
        assert_eq!(guess_game_version("my-custom-pack", None), None);
    }

    fn temp_layout(tag: &str) -> PathLayout {
        let root = std::env::temp_dir().join(format!(
            "xcl-local-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("versions")).unwrap();
        PathLayout::new(&root, root.join("cache"))
    }

    fn write_version(layout: &PathLayout, id: &str, json: &str, with_jar: bool) {
        let dir = layout.version_dir(id);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(format!("{id}.json")), json).unwrap();
        if with_jar {
            std::fs::write(dir.join(format!("{id}.jar")), b"jar").unwrap();
        }
    }

    #[test]
    fn scans_installed_versions() {
        let layout = temp_layout("scan");
        write_version(
            &layout,
            "1.20.1",
            r#"{"id":"1.20.1","type":"release","mainClass":"net.minecraft.client.main.Main","javaVersion":{"majorVersion":17}}"#,
            true,
        );
        write_version(
            &layout,
            "1.20.1-forge",
            r#"{"id":"1.20.1-forge","inheritsFrom":"1.20.1","mainClass":"cpw.mods.bootstraplauncher.BootstrapLauncher","libraries":[{"name":"net.minecraftforge:forge:1.20.1-47.2.0"}]}"#,
            false,
        );

        let list = scan(&layout);
        assert_eq!(list.len(), 2);

        let vanilla = list.iter().find(|v| v.id == "1.20.1").unwrap();
        assert!(vanilla.valid);
        assert!(vanilla.has_client_jar);
        assert!(vanilla.launchable);
        assert_eq!(vanilla.java_major, Some(17));
        assert_eq!(vanilla.kind, VersionKind::Vanilla);
        assert!(!vanilla.modable, "纯净版不能装 Mod");
        assert!(!vanilla.texture_packs, "1.20 用的是资源包");
        assert!(vanilla.has_logs, "1.20 会写 logs 目录");

        // 加载器版本没有自己的 jar，但父版本有 → 可启动
        let forge = list.iter().find(|v| v.id == "1.20.1-forge").unwrap();
        assert!(!forge.has_client_jar);
        assert!(forge.launchable, "jar 在父版本里，应当判为可启动");
        assert_eq!(forge.inherits_from.as_deref(), Some("1.20.1"));
        assert_eq!(forge.kind, VersionKind::Forge);
        assert!(forge.modable, "Forge 版本要显示模组入口");

        let _ = std::fs::remove_dir_all(layout.root());
    }

    /// 版本目录改成任意名字之后，「它属于哪个原版」必须还能认出来
    /// （靠安装时写进 `XCL/instance.toml` 的 gameVersion，而不是猜目录名）。
    #[test]
    fn renamed_version_keeps_game_version_from_settings() {
        let layout = temp_layout("renamed");
        write_version(
            &layout,
            "我的整合",
            r#"{"id":"我的整合","mainClass":"net.minecraftforge.bootstrap.ForgeBootstrap","libraries":[{"name":"net.minecraftforge:fmlloader:1.21.11-61.2.1"}]}"#,
            true,
        );
        let dir = layout.version_dir("我的整合");
        std::fs::create_dir_all(dir.join("XCL")).unwrap();
        std::fs::write(
            dir.join("XCL").join("instance.toml"),
            "gameVersion = \"1.21.11\"\n",
        )
        .unwrap();

        let list = scan(&layout);
        let item = list.iter().find(|v| v.id == "我的整合").unwrap();
        assert_eq!(item.game_version.as_deref(), Some("1.21.11"));
        assert_eq!(item.kind, VersionKind::Forge);
        assert!(item.modable);
        // 关键：能进「下载 Mod」页的候选列表
        assert!(crate::resources::targets::candidate_of(item).is_some());

        let _ = std::fs::remove_dir_all(layout.root());
    }

    /// 写一份带真 zip 客户端 jar 的版本目录，jar 里放 `version.json`。
    fn write_version_with_jar_descriptor(layout: &PathLayout, id: &str, json: &str, mc: &str) {
        use std::io::Write as _;

        let dir = layout.version_dir(id);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(format!("{id}.json")), json).unwrap();

        let file = std::fs::File::create(dir.join(format!("{id}.jar"))).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        zip.start_file("version.json", zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(format!(r#"{{"id":"{mc}","name":"{mc}"}}"#).as_bytes())
            .unwrap();
        zip.finish().unwrap();
    }

    /// 改名不能改身份：客户端 jar 里的 `version.json` 是最后一层保险。
    ///
    /// 场景：把 `fabric-loader-0.20.1-26.2` 改成「我的整合」，
    /// 顺手删了 `XCL/instance.toml`（或者这实例根本是从别处拷来的）。
    /// 目录名、JSON 里的 id 全都指不出原版号，只能靠 jar 里的描述。
    #[test]
    fn renamed_instance_keeps_identity_from_jar() {
        let layout = temp_layout("renamed-jar");
        write_version_with_jar_descriptor(
            &layout,
            "我的整合",
            r#"{"id":"我的整合","mainClass":"net.fabricmc.loader.impl.launch.knot.KnotClient","libraries":[{"name":"net.fabricmc:fabric-loader:0.20.1"}]}"#,
            "26.2",
        );

        let list = scan(&layout);
        let item = list.iter().find(|v| v.id == "我的整合").unwrap();
        assert_eq!(item.game_version.as_deref(), Some("26.2"));
        assert_eq!(item.loader_version.as_deref(), Some("0.20.1"));
        assert_eq!(item.kind, VersionKind::Fabric);

        // 「加载器已安装」判定要认得出来（这就是那个「改了名就说没装」的 bug）
        assert!(has_loader(&list, VersionKind::Fabric, "0.20.1", "26.2"));
        assert!(!has_loader(&list, VersionKind::Fabric, "0.19.0", "26.2"));
        // 装了加载器的不算「原版已安装」
        assert!(!has_vanilla(&list, "26.2"));
        assert_eq!(
            loader_instances(&list, "26.2"),
            vec![("Fabric", "我的整合".to_string())]
        );

        let _ = std::fs::remove_dir_all(layout.root());
    }

    /// 原版改名之后，版本清单里的「已安装」也得还认得出来。
    #[test]
    fn renamed_vanilla_still_counts_as_installed() {
        let layout = temp_layout("renamed-vanilla");
        write_version_with_jar_descriptor(
            &layout,
            "我的纯净版",
            r#"{"id":"我的纯净版","type":"release","mainClass":"net.minecraft.client.main.Main"}"#,
            "1.20.1",
        );

        let list = scan(&layout);
        let item = list.iter().find(|v| v.id == "我的纯净版").unwrap();
        assert_eq!(item.game_version.as_deref(), Some("1.20.1"));
        assert_eq!(item.kind, VersionKind::Vanilla);
        assert!(has_vanilla(&list, "1.20.1"));
        assert!(loader_instances(&list, "1.20.1").is_empty());
        // 「1.20.1 能装 Mod 吗」的候选列表里不该混进纯净版
        assert!(crate::resources::targets::candidate_of(item).is_none());

        let _ = std::fs::remove_dir_all(layout.root());
    }

    /// 把原版改叫「1.20.1-准备装forge」：名字骗不了人，它还是原版。
    #[test]
    fn name_never_decides_the_loader() {
        let layout = temp_layout("name-scare");
        write_version_with_jar_descriptor(
            &layout,
            "1.20.1-准备装forge",
            r#"{"id":"1.20.1-准备装forge","type":"release","mainClass":"net.minecraft.client.main.Main"}"#,
            "1.20.1",
        );

        let list = scan(&layout);
        let item = list.iter().find(|v| v.id == "1.20.1-准备装forge").unwrap();
        assert_eq!(item.kind, VersionKind::Vanilla, "不该被名字带偏");
        assert!(!item.modable);

        let _ = std::fs::remove_dir_all(layout.root());
    }

    #[test]
    fn texture_pack_era_is_detected() {
        for (id, expected) in [
            ("1.0", true),
            ("1.5.2", true),
            ("b1.7.3", true),
            ("1.6.1", false),
            ("1.12.2", false),
            ("1.20.1", false),
            ("26.2", false),
            ("24w14a", false),
        ] {
            assert_eq!(uses_texture_packs(id), expected, "{id}");
        }
    }

    #[test]
    fn log_folder_era_is_detected() {
        for (id, expected) in [
            ("1.0", false),
            ("1.6.4", false),
            ("b1.7.3", false),
            ("1.7.2", true),
            ("1.12.2", true),
            ("1.20.1-forge-47.2.0", true),
            ("26.2", true),
            ("24w14a", true),
        ] {
            assert_eq!(uses_log_files(id), expected, "{id}");
        }
    }

    #[test]
    fn missing_jar_is_not_launchable() {
        let layout = temp_layout("nojar");
        write_version(&layout, "1.19", r#"{"id":"1.19"}"#, false);
        let list = scan(&layout);
        assert!(list[0].valid);
        assert!(!list[0].launchable, "没有 jar 不能启动");
        let _ = std::fs::remove_dir_all(layout.root());
    }

    #[test]
    fn broken_json_is_reported_not_panicked() {
        let layout = temp_layout("broken");
        write_version(&layout, "bad", "{ this is not json", true);
        let list = scan(&layout);
        assert_eq!(list.len(), 1);
        assert!(!list[0].valid);
        assert!(list[0].error.is_some());
        assert!(!list[0].launchable);
        let _ = std::fs::remove_dir_all(layout.root());
    }

    #[test]
    fn directory_without_json_is_flagged() {
        let layout = temp_layout("empty");
        std::fs::create_dir_all(layout.version_dir("hollow")).unwrap();
        let list = scan(&layout);
        assert_eq!(list.len(), 1);
        assert!(!list[0].valid);
        assert!(list[0].error.as_deref().unwrap().contains("没有版本描述"));
        let _ = std::fs::remove_dir_all(layout.root());
    }

    #[test]
    fn installed_ids_skips_broken_entries() {
        let layout = temp_layout("ids");
        write_version(&layout, "good", r#"{"id":"good"}"#, true);
        write_version(&layout, "bad", "not json", true);
        let ids = installed_ids(&layout);
        assert!(ids.contains("good"));
        assert!(!ids.contains("bad"), "坏掉的版本不算已安装");
        let _ = std::fs::remove_dir_all(layout.root());
    }

    #[test]
    fn missing_versions_dir_is_empty_not_error() {
        let root = std::env::temp_dir().join(format!("xcl-local-none-{}", std::process::id()));
        let layout = PathLayout::new(&root, root.join("cache"));
        assert!(scan(&layout).is_empty());
    }
}
