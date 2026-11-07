//! 启动参数拼装。
//!
//! 纯逻辑：零 IO、零 async，输入是已解析的版本 JSON + 路径 + 玩家信息，
//! 输出一条可以直接喂给 `Command` 的参数表。这样最难的规则（去重、占位符、
//! rules 过滤）能 100% 单测，而不用真起一个 JVM 去试。
//!
//! 参数拼装分几步：
//! - `DeduplicateJavaArguments`（`ModLaunch.vb:1617-1655`）—— 去重语义
//! - `McLaunchArgumentsReplace`（`1506-1579`）—— 占位符表
//! - `ModJava.vb:128-198` —— 版本号 → Java 需求推断
//!
//! 坑点（都是实测踩出来的）：
//! - 同名游戏参数会让两个都失效：`--width 800 --width 1280` 时游戏两个都不认，
//!   所以必须「后者覆盖前者」。唯一例外是 `--tweakClass`，老 Forge 靠它挂多个
//!   tweaker，覆盖等于砍掉 mod。
//! - 负数长得像参数：`-xPos 23 -xPos -50`，`-50` 要是被当成键，参数结构就错了。
//! - 老版本没有 `arguments` 对象，只有空格分隔的 `minecraftArguments` 字符串。

use std::collections::HashMap;
use std::path::PathBuf;

use crate::error::AppError;
use crate::rule::{rules_match, Env};
use crate::version::{Argument, ArgumentValue, VersionDetail};

// ---------- Java 需求 ----------

/// 某个版本对 Java 的要求。
///
/// 老版本有上界：1.5.2 及以前要求 Java 8 或更低，装 Java 17 反而起不来。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JavaRequirement {
    pub min: u32,
    pub max: Option<u32>,
}

impl JavaRequirement {
    pub fn accepts(&self, major: u32) -> bool {
        major >= self.min && self.max.map_or(true, |max| major <= max)
    }

    /// 给看的说明，例如「需要 Java 17 或更高」。
    pub fn describe(&self) -> String {
        match self.max {
            Some(max) if max == self.min => format!("需要 Java {}", self.min),
            Some(max) => format!("需要 Java {} ~ {}", self.min, max),
            None => format!("需要 Java {} 或更高", self.min),
        }
    }
}

/// 判定某个版本需要什么 Java。
///
/// `declared` 来自版本 JSON 的 `javaVersion.majorVersion`。注意它只说下限 ——
/// Mojang 后来给 1.12.2 这类老版本的 JSON 也补了 `majorVersion: 8`，
/// 但 1.12.2 照样跑不了 Java 17。所以上限始终按版本号自己推断：
///
/// ```text
/// 1.12.2  declared = 8  →  { min: 8, max: 8 }   用 Java 17 必崩
/// 1.20.1  declared = 17 →  { min: 17, max: None } 取推断的下限
/// ```
pub fn java_requirement(version_id: &str, declared: Option<u32>) -> JavaRequirement {
    let inferred = infer_from_version(version_id);
    match declared.filter(|major| *major > 0) {
        Some(major) => JavaRequirement {
            min: major.max(inferred.min),
            max: inferred.max,
        },
        None => inferred,
    }
}

/// 纯按版本号推断，不看 JSON。
fn infer_from_version(version_id: &str) -> JavaRequirement {
    let Some((1, minor, patch)) = parse_release(version_id) else {
        // 快照（24w14a）或非 1.x：无从推断，给个现代下限
        return JavaRequirement { min: 8, max: None };
    };

    // 1.20.5 起官方要求 Java 21
    if minor > 20 || (minor == 20 && patch >= 5) {
        return JavaRequirement { min: 21, max: None };
    }
    if minor >= 18 {
        return JavaRequirement { min: 17, max: None };
    }
    if minor >= 17 {
        return JavaRequirement { min: 16, max: None };
    }
    // 1.13 ~ 1.16：Java 8 起步，但不能太新 —— 这些版本还在用 lwjgl 2/3 的老 API，
    // Java 17 起反射限制变严，容易在启动阶段就崩
    if minor >= 13 {
        return JavaRequirement {
            min: 8,
            max: Some(16),
        };
    }
    // 1.12 及更早：只能用 Java 8。
    // Java 9 开始的模块化让反射访问 JDK 内部类失败，1.12.2 及以前的 Forge
    // 几乎必崩。这一条以前漏了，会把 Java 17 塞给 1.12.2，属于「能拼出命令行
    // 但游戏起不来」的那种最难查的错。
    JavaRequirement {
        min: 8,
        max: Some(8),
    }
}

/// 从 `1.20.1` / `1.18-pre2` / `1.7.10` 里取出数字部分。
fn parse_release(id: &str) -> Option<(u32, u32, u32)> {
    // 快照是 `24w14a` 这种，直接放弃
    let core = id.split(['-', '_']).next()?.trim();
    let mut parts = core.split('.');
    let major: u32 = parts.next()?.parse().ok()?;
    if major != 1 {
        return None;
    }
    let minor: u32 = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    let patch: u32 = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    Some((major, minor, patch))
}

// ---------- 参数去重 ----------

/// 按「同名参数后者覆盖前者」的语义去重。
///
/// - 单参数（`--demo`、`-Xmx4G` 这种后面不跟值的）：保留第一次出现，后面重复的丢掉
/// - 键值对：
///   - 游戏参数：同名键用新值覆盖旧值（`--tweakClass` 例外，它允许并列）
///   - JVM 参数：只有键和值都完全相同才丢，否则并列保留
pub fn dedupe_args(args: Vec<String>, is_jvm: bool) -> Vec<String> {
    let mut result: Vec<String> = Vec::with_capacity(args.len());
    let mut i = 0;

    while i < args.len() {
        let key = &args[i];

        // 判定「这是单参数还是键值对的键」
        let is_single = !key.starts_with('-')
            || i + 1 >= args.len()
            || (args[i + 1].starts_with('-') && val_after_dash(&args[i + 1]) == 0.0);

        if is_single {
            i += 1;
            if result.iter().any(|existing| existing == key) {
                continue;
            }
            result.push(key.clone());
            continue;
        }

        let value = args[i + 1].clone();
        i += 2;

        let mut merged = false;
        let mut j = 0;
        while j + 1 < result.len() {
            if result[j] != *key {
                j += 1;
                continue;
            }
            // 游戏参数：后者覆盖（--tweakClass 除外，多个 tweaker 要共存）
            if !is_jvm && key != "--tweakClass" {
                result[j + 1] = value.clone();
                merged = true;
                break;
            }
            // 键和值都一样，纯冗余
            if result[j + 1] == value {
                merged = true;
                break;
            }
            j += 1;
        }

        if !merged {
            result.push(key.clone());
            result.push(value);
        }
    }

    result
}

/// 模拟 VB 的 `Val(Token.AfterFirst("-"))`：只取开头的数字部分。
///
/// 用来把 `-50` 认成负数、把 `--width` 认成键。
fn val_after_dash(token: &str) -> f64 {
    let body = token.strip_prefix('-').unwrap_or(token);
    let head: String = body
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.' || *c == '-')
        .collect();
    head.parse().unwrap_or(0.0)
}

// ---------- 参数展开 ----------

/// 把版本 JSON 的 `arguments` 展开成扁平参数表，顺手做 rules 过滤。
pub fn expand_arguments(args: &[Argument], env: &Env) -> Vec<String> {
    let mut out = Vec::new();
    for arg in args {
        match arg {
            Argument::Plain(value) => out.push(value.clone()),
            Argument::Conditional { rules, value } => {
                // 注意语义：有 rules 就默认拒绝，匹配不上直接跳过
                if !rules_match(Some(rules), env) {
                    continue;
                }
                match value {
                    ArgumentValue::Single(single) => out.push(single.clone()),
                    ArgumentValue::Many(many) => out.extend(many.iter().cloned()),
                }
            }
        }
    }
    out
}

/// 老版本（1.12-）的 `minecraftArguments`：一个空格分隔的长字符串。
///
/// 正规 JSON 里这些值都不含空格（路径靠占位符替换，替换在切分之后做），
/// 所以按空白切就够；真遇到引号包裹的情况也顺手处理掉。
pub fn split_legacy_arguments(raw: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;

    for ch in raw.chars() {
        match ch {
            '"' => in_quotes = !in_quotes,
            c if c.is_whitespace() && !in_quotes => {
                if !current.is_empty() {
                    out.push(std::mem::take(&mut current));
                }
            }
            c => current.push(c),
        }
    }
    if !current.is_empty() {
        out.push(current);
    }
    out
}

// ---------- 占位符 ----------

/// 占位符变量表。键不含 `${}`，替换时统一拼。
pub type Vars = HashMap<String, String>;

/// classpath 分隔符。
pub fn classpath_separator() -> &'static str {
    if cfg!(windows) {
        ";"
    } else {
        ":"
    }
}

/// 把 `${{key}}` 换成值。未知占位符原样保留 —— 悄悄换成空串会让问题更难查。
pub fn substitute(token: &str, vars: &Vars) -> String {
    let mut out = token.to_string();
    for (key, value) in vars {
        if !out.contains(key.as_str()) {
            continue;
        }
        out = out.replace(key.as_str(), value);
    }
    out
}

fn substitute_all(tokens: Vec<String>, vars: &Vars) -> Vec<String> {
    tokens
        .into_iter()
        .map(|token| substitute(&token, vars))
        .collect()
}

// ---------- 组装 ----------

#[derive(Debug, Clone)]
pub struct PlayerProfile {
    pub name: String,
    /// 32 位十六进制，无横杠
    pub uuid: String,
    pub access_token: String,
    /// `msa` / `legacy` / `offline`
    pub user_type: String,
}

impl PlayerProfile {
    /// 离线档案：UUID 由用户名派生（与官方离线模式一致的算法）。
    pub fn offline(name: impl Into<String>) -> Self {
        let name = name.into();
        let uuid = offline_uuid(&name);
        Self::offline_with_uuid(name, uuid)
    }

    /// 离线档案，但 UUID 由调用方给 —— 玩家选了 Steve / Alex 时，UUID 末
    /// 5 位要按模型调整（见 `xcl-service::skin::launch_uuid`）。
    pub fn offline_with_uuid(name: impl Into<String>, uuid: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            uuid: uuid.into(),
            access_token: "0".to_string(),
            user_type: "legacy".to_string(),
        }
    }
}

/// 离线 UUID：`md5("OfflinePlayer:<name>")` 加版本位，取 32 位十六进制。
///
/// 这是 Java 版离线模式的通行算法，同一用户名在任何启动器里得到的 UUID 都相同 ——
/// 换了启动器存档里的玩家数据才不会丢。
pub fn offline_uuid(name: &str) -> String {
    use md5::{Digest, Md5};
    let mut bytes: [u8; 16] = Md5::digest(format!("OfflinePlayer:{name}").as_bytes()).into();
    bytes[6] = (bytes[6] & 0x0f) | 0x30; // version 3
    bytes[8] = (bytes[8] & 0x3f) | 0x80; // variant
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[derive(Debug, Clone)]
pub struct WindowOptions {
    pub width: u32,
    pub height: u32,
    pub fullscreen: bool,
}

impl Default for WindowOptions {
    fn default() -> Self {
        Self {
            // 官方默认，也是绝大多数玩家习惯的尺寸
            width: 854,
            height: 480,
            fullscreen: false,
        }
    }
}

#[derive(Debug, Clone)]
pub struct LaunchPaths {
    /// 游戏工作目录（存档、日志、config 都落在这里）
    pub game_dir: PathBuf,
    pub assets_dir: PathBuf,
    pub asset_index: String,
    pub natives_dir: PathBuf,
    pub libraries_dir: PathBuf,
    /// classpath 中的 jar，顺序有意义；客户端 jar 由调用方放在最后
    pub classpath: Vec<PathBuf>,
    pub client_jar: Option<PathBuf>,
}

#[derive(Debug, Clone)]
pub struct LaunchRequest {
    pub version_id: String,
    /// `release` / `snapshot` / `old_beta`，进 `${version_type}`
    pub version_type: String,
    pub player: PlayerProfile,
    pub paths: LaunchPaths,
    pub java_major: u32,
    pub memory_mb: u32,
    pub window: WindowOptions,
    pub extra_jvm: Vec<String>,
    pub extra_game: Vec<String>,
}

/// 可以直接交给进程启动器的参数表。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandLine {
    /// JVM 参数（含 `-cp`），放在主类之前
    pub jvm: Vec<String>,
    /// 主类
    pub main_class: String,
    /// 游戏参数，放在主类之后
    pub game: Vec<String>,
}

impl CommandLine {
    /// 拼成完整命令行（不含 java 自身路径），用于日志展示。
    pub fn preview(&self) -> Vec<String> {
        let mut all = self.jvm.clone();
        all.push(self.main_class.clone());
        all.extend(self.game.iter().cloned());
        all
    }
}

/// 组装完整启动参数。
pub fn build_command(
    detail: &VersionDetail,
    request: &LaunchRequest,
    env: &Env,
) -> Result<CommandLine, AppError> {
    let main_class = detail.main_class.clone().ok_or_else(|| {
        AppError::parse(
            "版本信息不完整",
            format!("{} 的 JSON 里没有 mainClass", request.version_id),
        )
        .with_hint("这个版本的描述文件可能已损坏，建议重新安装")
    })?;

    // 1. 展开参数
    let (raw_jvm, raw_game) = match &detail.arguments {
        Some(args) => (
            expand_arguments(&args.jvm, env),
            expand_arguments(&args.game, env),
        ),
        // 老版本：只有一个字符串
        None => (
            Vec::new(),
            split_legacy_arguments(detail.minecraft_arguments.as_deref().unwrap_or("")),
        ),
    };

    // 2. 变量表
    let vars = build_vars(detail, request);

    // 3. 替换 + 去重
    let mut jvm = substitute_all(raw_jvm, &vars);
    let mut game = substitute_all(raw_game, &vars);

    // 4. 补 JVM 骨架参数
    //    老版本 JSON 的 jvm 段是空的，natives 路径得自己加，否则 lwjgl 找不到 dll
    let has_library_path = jvm.iter().any(|a| a.starts_with("-Djava.library.path"));
    if !has_library_path {
        jvm.insert(
            0,
            format!("-Djava.library.path={}", request.paths.natives_dir.display()),
        );
    }
    let has_classpath = jvm.iter().any(|a| a == "-cp" || a == "-classpath");
    if !has_classpath {
        jvm.push("-cp".to_string());
        jvm.push(vars.get("${classpath}").cloned().unwrap_or_default());
    }

    // 内存：放在最前面，方便人肉看日志时一眼发现
    jvm.insert(0, format!("-Xmx{}M", request.memory_mb.max(512)));

    // 5. 补游戏参数里缺的关键项
    ensure_game_arg(&mut game, "--username", &request.player.name);
    ensure_game_arg(&mut game, "--version", &request.version_id);
    ensure_game_arg(
        &mut game,
        "--gameDir",
        &request.paths.game_dir.display().to_string(),
    );
    ensure_game_arg(
        &mut game,
        "--assetsDir",
        &request.paths.assets_dir.display().to_string(),
    );
    ensure_game_arg(&mut game, "--assetIndex", &request.paths.asset_index);
    ensure_game_arg(&mut game, "--uuid", &request.player.uuid);
    ensure_game_arg(&mut game, "--accessToken", &request.player.access_token);
    ensure_game_arg(&mut game, "--userType", &request.player.user_type);
    ensure_game_arg(&mut game, "--versionType", &request.version_type);

    // 分辨率：官方 JSON 靠 `has_custom_resolution` 规则展开，没有就自己补
    if request.window.width > 0 && request.window.height > 0 {
        ensure_game_arg(&mut game, "--width", &request.window.width.to_string());
        ensure_game_arg(&mut game, "--height", &request.window.height.to_string());
    }
    if request.window.fullscreen && !game.iter().any(|a| a == "--fullscreen") {
        game.push("--fullscreen".to_string());
    }

    // 6. 自定义参数：放最后，靠去重覆盖掉同名的默认值
    let mut extra_jvm = substitute_all(request.extra_jvm.clone(), &vars);
    jvm.append(&mut extra_jvm);
    let mut extra_game = substitute_all(request.extra_game.clone(), &vars);
    game.append(&mut extra_game);

    let jvm = dedupe_args(jvm, true);
    let game = dedupe_args(game, false);

    Ok(CommandLine {
        jvm,
        main_class,
        game,
    })
}

/// 只有完全不存在这个参数时才补，避免和版本 JSON 里的值打架。
fn ensure_game_arg(game: &mut Vec<String>, key: &str, value: &str) {
    if game.iter().any(|a| a == key) {
        return;
    }
    game.push(key.to_string());
    game.push(value.to_string());
}

fn build_vars(detail: &VersionDetail, request: &LaunchRequest) -> Vars {
    let mut vars = Vars::new();
    let paths = &request.paths;

    // 路径类
    vars.insert(
        "${natives_directory}".into(),
        paths.natives_dir.display().to_string(),
    );
    vars.insert(
        "${library_directory}".into(),
        paths.libraries_dir.display().to_string(),
    );
    // 有些加载器（Forge）读的是复数形式
    vars.insert(
        "${libraries_directory}".into(),
        paths.libraries_dir.display().to_string(),
    );
    vars.insert(
        "${game_directory}".into(),
        paths.game_dir.display().to_string(),
    );
    vars.insert(
        "${assets_root}".into(),
        paths.assets_dir.display().to_string(),
    );
    // 1.5.2 及更早的资源索引要从 virtual/legacy 读
    vars.insert(
        "${game_assets}".into(),
        paths.assets_dir.join("virtual").join("legacy").display().to_string(),
    );
    vars.insert("${assets_index_name}".into(), paths.asset_index.clone());
    vars.insert(
        "${classpath}".into(),
        paths
            .classpath
            .iter()
            .map(|p| p.display().to_string())
            .collect::<Vec<_>>()
            .join(classpath_separator()),
    );
    vars.insert("${classpath_separator}".into(), classpath_separator().into());
    if let Some(jar) = &paths.client_jar {
        vars.insert("${primary_jar}".into(), jar.display().to_string());
    }

    // 身份类
    vars.insert("${auth_player_name}".into(), request.player.name.clone());
    vars.insert("${auth_uuid}".into(), request.player.uuid.clone());
    vars.insert(
        "${auth_access_token}".into(),
        request.player.access_token.clone(),
    );
    // 微软登录用的 XSTS token / clientid，离线模式留空即可
    vars.insert("${auth_xuid}".into(), String::new());
    vars.insert("${clientid}".into(), String::new());
    vars.insert("${auth_session}".into(), request.player.access_token.clone());
    vars.insert("${access_token}".into(), request.player.access_token.clone());
    vars.insert("${user_type}".into(), request.player.user_type.clone());
    vars.insert("${user_properties}".into(), "{}".into());

    // 版本信息
    vars.insert("${version_name}".into(), request.version_id.clone());
    vars.insert("${version_type}".into(), request.version_type.clone());
    vars.insert("${launcher_name}".into(), "XCL".into());
    vars.insert(
        "${launcher_version}".into(),
        env!("CARGO_PKG_VERSION").into(),
    );

    // 窗口
    vars.insert("${resolution_width}".into(), request.window.width.to_string());
    vars.insert(
        "${resolution_height}".into(),
        request.window.height.to_string(),
    );

    // 自定义信息：`--versionType` 会显示在游戏标题栏。
    // 留空会让游戏显示 `${version_type}` 或空引号，干脆删掉这个参数。
    if detail.version_type.is_none() {
        // 没有自定义信息时，title 用版本类型兜底，别留占位符
        vars.insert("${version_type}".into(), request.version_type.clone());
    }

    vars
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rule::Rule;
    use std::collections::HashMap as Map;

    fn env() -> Env {
        Env {
            os_name: "windows".into(),
            os_version: String::new(),
            os_arch: "x86_64".into(),
            features: Map::new(),
        }
    }

    fn req() -> LaunchRequest {
        LaunchRequest {
            version_id: "1.20.1".into(),
            version_type: "release".into(),
            player: PlayerProfile {
                name: "Steve".into(),
                uuid: "0123456789abcdef0123456789abcdef".into(),
                access_token: "token".into(),
                user_type: "msa".into(),
            },
            paths: LaunchPaths {
                game_dir: PathBuf::from("C:/mc"),
                assets_dir: PathBuf::from("C:/mc/assets"),
                asset_index: "17".into(),
                natives_dir: PathBuf::from("C:/mc/natives"),
                libraries_dir: PathBuf::from("C:/mc/libraries"),
                classpath: vec![
                    PathBuf::from("C:/mc/libraries/a.jar"),
                    PathBuf::from("C:/mc/versions/1.20.1/1.20.1.jar"),
                ],
                client_jar: Some(PathBuf::from("C:/mc/versions/1.20.1/1.20.1.jar")),
            },
            java_major: 17,
            memory_mb: 4096,
            window: WindowOptions::default(),
            extra_jvm: Vec::new(),
            extra_game: Vec::new(),
        }
    }

    fn detail_with(arguments: Option<crate::version::Arguments>) -> VersionDetail {
        VersionDetail {
            id: "1.20.1".into(),
            version_type: Some("release".into()),
            inherits_from: None,
            main_class: Some("net.minecraft.client.main.Main".into()),
            assets: Some("17".into()),
            asset_index: None,
            downloads: None,
            libraries: Vec::new(),
            arguments,
            minecraft_arguments: None,
            java_version: Some(crate::version::JavaVersion {
                major_version: 17,
                component: Some("java-runtime-gamma".into()),
            }),
        }
    }

    // ---------- 去重 ----------

    #[test]
    fn game_arg_duplicate_overwrites_earlier() {
        // 同名游戏参数两个都失效，所以必须后者覆盖
        let args: Vec<String> = ["--width", "800", "--width", "1280"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(dedupe_args(args, false), vec!["--width", "1280"]);
    }

    #[test]
    fn tweak_class_is_exempt_from_overwrite() {
        // 老 Forge 靠多个 --tweakClass 挂 tweaker，覆盖等于砍 mod
        let args: Vec<String> = ["--tweakClass", "a.Tweaker", "--tweakClass", "b.Tweaker"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(
            dedupe_args(args, false),
            vec!["--tweakClass", "a.Tweaker", "--tweakClass", "b.Tweaker"]
        );
    }

    #[test]
    fn jvm_duplicate_only_dropped_when_identical() {
        let args: Vec<String> = ["-Dfoo=1", "-Dfoo=1"].iter().map(|s| s.to_string()).collect();
        assert_eq!(dedupe_args(args, true), vec!["-Dfoo=1"]);

        // 键相同值不同 → 并列保留（JVM 参数不像游戏参数会互相失效）
        let args: Vec<String> = ["-Dfoo", "1", "-Dfoo", "2"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(
            dedupe_args(args, true),
            vec!["-Dfoo", "1", "-Dfoo", "2"]
        );
    }

    #[test]
    fn negative_number_is_value_not_key() {
        // -xPos 23 -xPos -50：后面的 -50 是值，不能被当成键
        let args: Vec<String> = ["-xPos", "23", "-xPos", "-50"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(dedupe_args(args, false), vec!["-xPos", "-50"]);
    }

    #[test]
    fn single_flag_duplicate_kept_once() {
        let args: Vec<String> = ["--demo", "--demo"].iter().map(|s| s.to_string()).collect();
        assert_eq!(dedupe_args(args, false), vec!["--demo"]);
    }

    #[test]
    fn unknown_number_prefix_helper() {
        assert_eq!(val_after_dash("-50"), 50.0);
        assert_eq!(val_after_dash("--width"), 0.0);
        assert_eq!(val_after_dash("-1.5"), 1.5);
    }

    // ---------- 占位符 ----------

    #[test]
    fn substitutes_known_placeholders() {
        let mut vars = Vars::new();
        vars.insert("${auth_player_name}".into(), "Alex".into());
        assert_eq!(substitute("${auth_player_name}", &vars), "Alex");
        // 未知占位符原样保留，方便排错
        assert_eq!(substitute("${nope}", &vars), "${nope}");
    }

    #[test]
    fn access_token_placeholder_is_not_confused() {
        let mut vars = Vars::new();
        vars.insert("${auth_access_token}".into(), "AAA".into());
        vars.insert("${access_token}".into(), "AAA".into());
        assert_eq!(substitute("${access_token}", &vars), "AAA");
        assert_eq!(substitute("${auth_access_token}", &vars), "AAA");
    }

    // ---------- rules 过滤 ----------

    #[test]
    fn conditional_argument_respects_rules() {
        let allow_windows = Argument::Conditional {
            rules: vec![Rule {
                action: crate::rule::RuleAction::Allow,
                os: Some(crate::rule::OsRule {
                    name: Some("windows".into()),
                    version: None,
                    arch: None,
                }),
                features: None,
            }],
            value: ArgumentValue::Single("--windows-only".into()),
        };
        let allow_linux = Argument::Conditional {
            rules: vec![Rule {
                action: crate::rule::RuleAction::Allow,
                os: Some(crate::rule::OsRule {
                    name: Some("linux".into()),
                    version: None,
                    arch: None,
                }),
                features: None,
            }],
            value: ArgumentValue::Single("--linux-only".into()),
        };
        let args = vec![
            Argument::Plain("--always".into()),
            allow_windows,
            allow_linux,
        ];
        let got = expand_arguments(&args, &env());
        assert_eq!(got, vec!["--always", "--windows-only"]);
    }

    #[test]
    fn many_value_argument_expands_flat() {
        let arg = Argument::Conditional {
            rules: vec![Rule {
                action: crate::rule::RuleAction::Allow,
                os: None,
                features: None,
            }],
            value: ArgumentValue::Many(vec!["--width".into(), "854".into()]),
        };
        assert_eq!(expand_arguments(&[arg], &env()), vec!["--width", "854"]);
    }

    // ---------- 老版本字符串 ----------

    #[test]
    fn splits_legacy_argument_string() {
        let got = split_legacy_arguments("--username  Steve   --gameDir C:/mc");
        assert_eq!(got, vec!["--username", "Steve", "--gameDir", "C:/mc"]);
    }

    #[test]
    fn keeps_quoted_segment_together() {
        let got = split_legacy_arguments("--title \"hello world\" --demo");
        assert_eq!(got, vec!["--title", "hello world", "--demo"]);
    }

    // ---------- Java 需求 ----------

    #[test]
    fn declared_java_is_a_floor_not_a_ceiling() {
        // 1.20.1 的 JSON 声明 17，正好等于推断值
        let req = java_requirement("1.20.1", Some(17));
        assert_eq!(req.min, 17);
        assert_eq!(req.max, None);
        assert!(req.accepts(17));
        assert!(!req.accepts(16));

        // 关键：1.12.2 的 JSON 也声明了 8，但不能因此就允许 Java 17
        let req = java_requirement("1.12.2", Some(8));
        assert_eq!(req.min, 8);
        assert_eq!(req.max, Some(8), "上限必须来自版本号推断，不是 JSON");
        assert!(!req.accepts(17));
    }

    #[test]
    fn infers_from_version_number() {
        assert_eq!(java_requirement("1.20.5", None).min, 21);
        assert_eq!(java_requirement("1.21", None).min, 21);
        assert_eq!(java_requirement("1.18.2", None).min, 17);
        assert_eq!(java_requirement("1.17.1", None).min, 16);
        assert_eq!(java_requirement("1.16.5", None).min, 8);
        assert_eq!(java_requirement("1.12.2", None).min, 8);
        assert_eq!(java_requirement("1.7.10", None).min, 8);
    }

    #[test]
    fn old_versions_reject_modern_java() {
        // 1.12 及更早只能用 Java 8，给 Java 17 会在启动阶段崩
        let req = java_requirement("1.12.2", None);
        assert!(req.accepts(8));
        assert!(!req.accepts(17), "1.12.2 不能跑在 Java 17 上");
        assert!(!req.accepts(21));

        // 1.13~1.16 能给到 Java 16，但不能给 17
        let req = java_requirement("1.16.5", None);
        assert!(req.accepts(8));
        assert!(req.accepts(16));
        assert!(!req.accepts(17));
    }

    #[test]
    fn ancient_versions_have_upper_bound() {
        let req = java_requirement("1.5.2", None);
        assert_eq!(req.max, Some(8));
        assert!(req.accepts(8));
        assert!(!req.accepts(17), "Java 17 起不来 1.5.2");
    }

    #[test]
    fn snapshot_falls_back_to_eight() {
        assert_eq!(java_requirement("24w14a", None).min, 8);
    }

    // ---------- 组装 ----------

    #[test]
    fn builds_command_from_structured_arguments() {
        let args = crate::version::Arguments {
            jvm: vec![
                Argument::Plain("-Djava.library.path=${natives_directory}".into()),
                Argument::Plain("-cp".into()),
                Argument::Plain("${classpath}".into()),
            ],
            game: vec![
                Argument::Plain("--username".into()),
                Argument::Plain("${auth_player_name}".into()),
                Argument::Plain("--version".into()),
                Argument::Plain("${version_name}".into()),
                Argument::Plain("--gameDir".into()),
                Argument::Plain("${game_directory}".into()),
            ],
        };
        let detail = detail_with(Some(args));
        let cmd = build_command(&detail, &req(), &env()).unwrap();

        assert_eq!(cmd.main_class, "net.minecraft.client.main.Main");
        assert!(cmd.jvm.iter().any(|a| a == "-Xmx4096M"));
        // 已经有 -cp 就不再补
        assert_eq!(cmd.jvm.iter().filter(|a| *a == "-cp").count(), 1);
        assert!(cmd
            .jvm
            .iter()
            .any(|a| a.contains("libraries/a.jar") && a.contains("1.20.1.jar")));

        assert_eq!(
            cmd.game,
            vec![
                "--username", "Steve", "--version", "1.20.1", "--gameDir", "C:/mc",
                "--assetsDir", "C:/mc/assets", "--assetIndex", "17",
                "--uuid", "0123456789abcdef0123456789abcdef",
                "--accessToken", "token", "--userType", "msa",
                "--versionType", "release",
                "--width", "854", "--height", "480",
            ]
        );
    }

    #[test]
    fn legacy_version_gets_natives_and_classpath_injected() {
        let mut detail = detail_with(None);
        detail.minecraft_arguments =
            Some("--username ${auth_player_name} --gameDir ${game_directory}".into());
        let cmd = build_command(&detail, &req(), &env()).unwrap();

        assert!(
            cmd.jvm.iter().any(|a| a.starts_with("-Djava.library.path")),
            "老版本 JSON 没有 jvm 段，natives 路径必须自己补"
        );
        assert!(cmd.jvm.iter().any(|a| a == "-cp"));
    }

    #[test]
    fn missing_main_class_is_reported() {
        let mut detail = detail_with(Some(crate::version::Arguments {
            jvm: vec![],
            game: vec![],
        }));
        detail.main_class = None;
        let err = build_command(&detail, &req(), &env()).unwrap_err();
        assert_eq!(err.code, crate::error::ErrorCode::Parse);
    }

    #[test]
    fn extra_game_args_override_defaults() {
        let mut request = req();
        request.extra_game = vec!["--width".into(), "1920".into()];
        let detail = detail_with(Some(crate::version::Arguments {
            jvm: vec![],
            game: vec![],
        }));
        let cmd = build_command(&detail, &request, &env()).unwrap();
        let width = cmd.game.iter().position(|a| a == "--width").unwrap();
        assert_eq!(cmd.game[width + 1], "1920");
        // 覆盖后只应剩一个 --width
        assert_eq!(cmd.game.iter().filter(|a| *a == "--width").count(), 1);
    }

    #[test]
    fn offline_uuid_is_stable_and_shaped() {
        let a = offline_uuid("Steve");
        let b = offline_uuid("Steve");
        let c = offline_uuid("Alex");
        assert_eq!(a, b, "同一用户名必须得到同一个 UUID");
        assert_ne!(a, c);
        assert_eq!(a.len(), 32);
        // 版本位 3、变体位 8~b（RFC 4122 的离线 UUID 约定）
        assert_eq!(&a[12..13], "3");
        assert!(matches!(&a[16..17], "8" | "9" | "a" | "b"));
    }

    #[test]
    fn offline_profile_fills_token() {
        let profile = PlayerProfile::offline("Steve");
        assert_eq!(profile.name, "Steve");
        assert_eq!(profile.user_type, "legacy");
        assert_eq!(profile.access_token, "0");
        assert_eq!(profile.uuid.len(), 32);
    }
}
