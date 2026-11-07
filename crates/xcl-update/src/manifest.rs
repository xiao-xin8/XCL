//! 更新清单 `update.json`。
//!
//! 清单是唯一的外部输入，所以这里既要宽松（缺字段不能让启动器崩），
//! 又要严格（字段形状不对、URL 不是 https、哈希格式不对，一律拒绝）。
//!
//! 字段名跟项目其它 JSON 一致，走 camelCase。

use semver::Version;
use serde::{Deserialize, Serialize};
use xcl_core::AppError;

use crate::verify::is_sha256_hex;

/// 清单格式版本。改结构时 +1，并在 [`Manifest::validate`] 里拒绝旧格式。
///
/// 2：从「直接下 exe」改成「下 zip 再解压」（`archive` + `exe.name`）。
pub const SCHEMA: u32 = 2;

/// 更新说明的字符上限。太长说明是被人塞了东西，不是正常发版。
pub const MAX_NOTES_CHARS: usize = 8_000;
/// 更新条目最多几条。
pub const MAX_CHANGELOG_LINES: usize = 50;
/// 单条更新内容的字符上限。
pub const MAX_CHANGELOG_LINE_CHARS: usize = 500;
/// 版本标题的字符上限。
pub const MAX_TITLE_CHARS: usize = 120;
/// URL 长度上限，同上。
pub const MAX_URL_LEN: usize = 2_048;
/// 压缩包里那个文件名的长度上限。
pub const MAX_EXE_NAME_LEN: usize = 120;

/// 压缩包里的文件名：只认「纯文件名 + .exe」。
///
/// 挡两件事：路径穿越（`../../windows/x.exe`）和奇怪的条目名。
/// 解压时不会按这个字符串拼路径以外的东西，但名字本身也要先干净。
fn is_packed_exe_name(name: &str) -> bool {
    if name.is_empty() || name.len() > MAX_EXE_NAME_LEN {
        return false;
    }
    if name.contains(['/', '\\']) || name.contains("..") {
        return false;
    }
    if name.chars().any(|c| c.is_control()) {
        return false;
    }
    name.to_ascii_lowercase().ends_with(".exe")
}

/// 本机地址：开发期用 `python -m http.server` 托管清单时放行 http。
///
/// 只认 loopback 三个写法，其他 `http://` 一律拒绝 —— 不能因为要调试就把
/// 校验放宽成「http 也行」。
fn is_loopback_url(url: &str) -> bool {
    let Some(rest) = url.strip_prefix("http://") else {
        return false;
    };
    let host = rest
        .split(['/', ':', '?', '#'])
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    matches!(host.as_str(), "127.0.0.1" | "localhost" | "[::1]")
}

/// 发布频道。和配置文件里的 `[update].channel` 是同一个概念，
/// 所以直接复用 `xcl-core` 里的定义，避免两处枚举各自演化。
pub use xcl_core::config::UpdateChannel as Channel;

/// 更新紧急程度。v1 只影响文案，不做强制升级。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Severity {
    #[default]
    Normal,
    /// 「建议尽快更新」的措辞，仍然允许稍后
    Critical,
}

/// 解压出来的那一个启动器文件。
///
/// `name` 是压缩包里的文件名（不含目录）。解压时只认这个名字，
/// 绝不按压缩包里的路径落盘 —— 那是 zip slip 的入口。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ExeArtifact {
    pub name: String,
    pub size: u64,
    pub sha256: String,
}

/// 要下载的压缩包。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ArchiveArtifact {
    pub url: String,
    pub size: u64,
    pub sha256: String,
}

/// 更新清单。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Manifest {
    /// 清单格式版本
    pub schema: u32,
    /// 最新版本号，允许 `v` 前缀
    pub version: String,
    pub channel: Channel,
    pub published_at: Option<String>,
    pub severity: Severity,
    /// 当前版本低于它时提示「太旧」，但不阻止继续用
    pub min_supported: Option<String>,
    /// 更新说明，纯文本，前端不渲染 Markdown
    pub notes: String,
    /// 逐条更新内容。比一整段 `notes` 好显示，界面按行渲染即可
    pub changelog: Vec<String>,
    pub notes_url: Option<String>,
    /// 强制更新：为真时启动器只允许更新，其他功能全部禁用。
    ///
    /// 一旦读到就会被持久化，之后每次启动都拦，直到版本号追上为止 ——
    /// 不能靠「下次联网没读到」绕过去。
    pub force: bool,
    /// 版本标题，界面上当副标题用；空则显示「新版本 x.y.z」
    pub title: String,
    /// 要下载的压缩包
    pub archive: ArchiveArtifact,
    /// 解压出来的启动器
    pub exe: ExeArtifact,
}

impl Manifest {
    /// 解析并校验。任何一种「形状不对」都在这里被挡住，
    /// 后面的流程只处理结构完整的数据。
    pub fn parse(text: &str) -> Result<Self, AppError> {
        let manifest: Manifest = serde_json::from_str(text).map_err(|err| {
            AppError::parse("更新清单格式错误", format!("无法解析更新清单：{err}"))
                .with_hint("稍后重试；若持续失败，可能是服务器上的清单有问题")
                .with_retryable(true)
        })?;
        manifest.validate()?;
        Ok(manifest)
    }

    /// 字段级校验。
    pub fn validate(&self) -> Result<(), AppError> {
        if self.schema != SCHEMA {
            return Err(AppError::unsupported(
                "更新清单版本不支持",
                format!("清单格式版本为 {}，本启动器只认识 {SCHEMA}", self.schema),
            )
            .with_hint("请手动下载新版启动器"));
        }
        if parse_version(&self.version).is_none() {
            return Err(AppError::parse(
                "更新清单格式错误",
                format!("版本号「{}」不是合法的 semver", self.version),
            ));
        }
        if !self.archive.url.starts_with("https://") && !is_loopback_url(&self.archive.url) {
            return Err(AppError::parse(
                "更新清单格式错误",
                "更新地址必须是 https://",
            )
            .with_hint("为安全起见，不会从非加密地址下载启动器"));
        }
        if self.archive.url.len() > MAX_URL_LEN {
            return Err(AppError::parse(
                "更新清单格式错误",
                "更新地址过长，已拒绝",
            ));
        }
        if !is_sha256_hex(&self.archive.sha256) {
            return Err(AppError::parse(
                "更新清单格式错误",
                "压缩包缺少合法的 SHA-256",
            )
            .with_hint("没有哈希就无法校验文件，本次更新已中止"));
        }
        if self.archive.size == 0 {
            return Err(AppError::parse(
                "更新清单格式错误",
                "压缩包大小不能为 0",
            )
            .with_hint("没有大小就算不出进度，本次更新已中止"));
        }
        if !is_packed_exe_name(&self.exe.name) {
            return Err(AppError::parse(
                "更新清单格式错误",
                format!("压缩包里的文件名「{}」不可用", self.exe.name),
            )
            .with_hint("只接受不带路径的 .exe 文件名"));
        }
        if !is_sha256_hex(&self.exe.sha256) {
            return Err(AppError::parse(
                "更新清单格式错误",
                "启动器缺少合法的 SHA-256",
            )
            .with_hint("没有哈希就无法校验文件，本次更新已中止"));
        }
        if self.exe.size == 0 {
            return Err(AppError::parse(
                "更新清单格式错误",
                "启动器大小不能为 0",
            ));
        }
        if self.notes.chars().count() > MAX_NOTES_CHARS {
            return Err(AppError::parse(
                "更新清单格式错误",
                "更新说明过长，已拒绝",
            ));
        }
        if self.changelog.len() > MAX_CHANGELOG_LINES {
            return Err(AppError::parse(
                "更新清单格式错误",
                "更新条目过多，已拒绝",
            ));
        }
        if self
            .changelog
            .iter()
            .any(|line| line.chars().count() > MAX_CHANGELOG_LINE_CHARS)
        {
            return Err(AppError::parse(
                "更新清单格式错误",
                "单条更新内容过长，已拒绝",
            ));
        }
        if self.title.chars().count() > MAX_TITLE_CHARS {
            return Err(AppError::parse(
                "更新清单格式错误",
                "更新标题过长，已拒绝",
            ));
        }
        Ok(())
    }

    /// 新 exe 在安装目录里的临时文件名，例如
    /// `.Xin Craft Launcher-new-0.2.0.exe`。
    ///
    /// `binary_stem` 传当前程序的文件名（不含扩展名）—— 不要硬编码，
    /// 包名随时可能改，写死一个名字会让替换在改名那一版直接失败。
    ///
    /// 版本号用规范化后的 `Version` 拼：`version` 来自网络，原样拼路径有
    /// 穿越风险；`Version` 的 `Display` 只产生 semver 字符集。
    pub fn update_file_name(&self, binary_stem: &str) -> Result<String, AppError> {
        let version = parse_version(&self.version).ok_or_else(|| {
            AppError::parse("更新清单格式错误", "版本号不是合法的 semver")
        })?;
        Ok(format!(".{binary_stem}-new-{version}.exe"))
    }

    /// 压缩包的临时文件名，例如 `.Xin Craft Launcher-new-0.2.0.zip`。
    ///
    /// 和 exe 用同一个前缀：`cleanup` 扫到 `-new-` 就一起收走，
    /// 不会因为扩展名不同漏掉半截下载。
    pub fn archive_file_name(&self, binary_stem: &str) -> Result<String, AppError> {
        let version = parse_version(&self.version).ok_or_else(|| {
            AppError::parse("更新清单格式错误", "版本号不是合法的 semver")
        })?;
        Ok(format!(".{binary_stem}-new-{version}.zip"))
    }

    /// 和当前版本比较。
    ///
    /// 解析不了、降级、同版本、稳定频道遇到预发布 —— 一律当作「没有更新」。
    /// 宁可漏报，也不能让启动器陷入「更新完还提示更新」的死循环。
    pub fn check(&self, current: &str, channel: Channel) -> CheckResult {
        let latest_text = self.version.clone();
        let up_to_date = |state_current: String| CheckResult {
            state: CheckState::UpToDate,
            current: state_current,
            latest: latest_text.clone(),
            severity: self.severity,
            below_min_supported: false,
            notes: self.notes.clone(),
            changelog: self.changelog.clone(),
            notes_url: self.notes_url.clone(),
            title: self.title.clone(),
            // 没更新就谈不上强制：版本已经追上了，清单怎么喊都该放行
            force: false,
        };

        let Some(current_version) = parse_version(current) else {
            return up_to_date(current.trim().to_string());
        };
        let Some(latest_version) = parse_version(&self.version) else {
            return up_to_date(current_version.to_string());
        };

        // 稳定频道不把预发布当更新
        if !channel.accepts_prerelease() && !latest_version.pre.is_empty() {
            return up_to_date(current_version.to_string());
        }
        // 同版本或降级（手工装了旧版）都不提示
        if latest_version <= current_version {
            return up_to_date(current_version.to_string());
        }

        let below_min_supported = self
            .min_supported
            .as_deref()
            .and_then(parse_version)
            .is_some_and(|min| current_version < min);

        CheckResult {
            state: CheckState::Available,
            current: current_version.to_string(),
            latest: latest_version.to_string(),
            severity: self.severity,
            below_min_supported,
            notes: self.notes.clone(),
            changelog: self.changelog.clone(),
            notes_url: self.notes_url.clone(),
            title: self.title.clone(),
            force: self.force,
        }
    }
}

/// 版本比较结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckState {
    /// 已是最新（或无法判断，保守当作最新）
    UpToDate,
    /// 有可用更新
    Available,
}

/// 一次检查的结论。字段都是前端直接要显示的东西，不用再加工。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckResult {
    pub state: CheckState,
    pub current: String,
    pub latest: String,
    pub severity: Severity,
    /// 当前版本低于清单的 `minSupported`
    pub below_min_supported: bool,
    pub notes: String,
    /// 逐条更新内容
    pub changelog: Vec<String>,
    pub notes_url: Option<String>,
    pub title: String,
    /// 这次更新是否必须装。只可能在「真的有更新」时为真。
    pub force: bool,
}

impl CheckResult {
    pub fn has_update(&self) -> bool {
        self.state == CheckState::Available
    }
}

/// 解析版本号，容忍 `v` / `V` 前缀和首尾空白。
pub fn parse_version(text: &str) -> Option<Version> {
    let text = text.trim();
    let text = text.strip_prefix(['v', 'V']).unwrap_or(text);
    Version::parse(text).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use xcl_core::ErrorCode;

    const SAMPLE: &str = r#"{
        "schema": 2,
        "version": "0.2.0",
        "channel": "stable",
        "publishedAt": "2026-11-01T00:00:00Z",
        "severity": "normal",
        "minSupported": "0.1.0",
        "notes": "修了几个问题",
        "notesUrl": "https://example.com/releases/0.2.0",
        "archive": {
            "url": "https://example.com/releases/Xin-Craft-Launcher-0.2.0.zip",
            "size": 987654,
            "sha256": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
        },
        "exe": {
            "name": "Xin Craft Launcher.exe",
            "size": 21934592,
            "sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
        }
    }"#;

    fn sample() -> Manifest {
        Manifest::parse(SAMPLE).unwrap()
    }

    #[test]
    fn parses_full_manifest() {
        let m = sample();
        assert_eq!(m.version, "0.2.0");
        assert_eq!(m.channel, Channel::Stable);
        assert_eq!(m.severity, Severity::Normal);
        assert_eq!(m.exe.size, 21934592);
        assert_eq!(m.exe.name, "Xin Craft Launcher.exe");
        assert_eq!(m.archive.size, 987654);
        assert_eq!(
            m.update_file_name("Xin Craft Launcher").unwrap(),
            ".Xin Craft Launcher-new-0.2.0.exe"
        );
        assert_eq!(
            m.archive_file_name("Xin Craft Launcher").unwrap(),
            ".Xin Craft Launcher-new-0.2.0.zip"
        );
    }

    #[test]
    fn rejects_wrong_schema() {
        // 结构变了就必须换 schema，否则旧客户端会拿新清单当老格式读
        let err = Manifest::parse(&SAMPLE.replace("\"schema\": 2", "\"schema\": 1")).unwrap_err();
        assert_eq!(err.code, ErrorCode::Unsupported);
    }

    #[test]
    fn rejects_non_https_url() {
        let err = Manifest::parse(&SAMPLE.replace(
            "https://example.com/releases/Xin",
            "http://example.com/releases/Xin",
        ))
        .unwrap_err();
        assert!(err.message.contains("https"));
    }

    #[test]
    fn rejects_packed_name_with_path() {
        // 压缩包里的条目名不能带路径 —— 那是 zip slip 的入口
        for bad in [
            "..\\\\evil.exe",
            "../../evil.exe",
            "sub/evil.exe",
            "evil.dll",
            "",
        ] {
            let text = SAMPLE.replace("Xin Craft Launcher.exe", bad);
            let err = Manifest::parse(&text).unwrap_err();
            assert!(
                err.message.contains("文件名"),
                "「{bad}」应该被拒绝，实际错误：{}",
                err.message
            );
        }
    }

    #[test]
    fn rejects_bad_hash() {
        let broken = SAMPLE.replace(
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "abc",
        );
        let err = Manifest::parse(&broken).unwrap_err();
        assert!(err.message.contains("SHA-256"));
    }

    #[test]
    fn rejects_bad_version() {
        let err = Manifest::parse(&SAMPLE.replace("\"0.2.0\"", "\"新版\"")).unwrap_err();
        assert!(err.message.contains("semver"));
    }

    #[test]
    fn broken_json_is_an_error_not_a_panic() {
        let err = Manifest::parse("{ not json").unwrap_err();
        assert_eq!(err.code, ErrorCode::Parse);
        assert!(err.retryable);
    }

    #[test]
    fn missing_fields_fall_back_to_defaults() {
        // schema 与版本号齐了，但 archive 整段缺失：靠 default 补成空串后被 validate 拒掉
        let err = Manifest::parse(r#"{"schema":2,"version":"0.2.0"}"#).unwrap_err();
        assert!(err.message.contains("https"));
    }

    #[test]
    fn check_reports_newer_version() {
        let r = sample().check("0.1.0", Channel::Stable);
        assert_eq!(r.state, CheckState::Available);
        assert!(r.has_update());
        assert_eq!(r.latest, "0.2.0");
        assert!(!r.below_min_supported);
    }

    #[test]
    fn check_hides_same_or_older_version() {
        assert!(!sample().check("0.2.0", Channel::Stable).has_update());
        // 手工装了更新版：不提示降级
        assert!(!sample().check("0.3.0", Channel::Stable).has_update());
        // 版本号带 v 前缀也能比
        assert!(sample().check("v0.1.9", Channel::Stable).has_update());
    }

    #[test]
    fn check_treats_unparsable_current_as_up_to_date() {
        // 关键是别让启动器卡在更新循环里
        assert!(!sample().check("dev-build", Channel::Stable).has_update());
        assert!(!sample().check("", Channel::Stable).has_update());
    }

    #[test]
    fn stable_channel_ignores_prerelease() {
        let beta = Manifest::parse(&SAMPLE.replace("\"0.2.0\"", "\"0.2.0-beta.1\"")).unwrap();
        assert!(!beta.check("0.1.0", Channel::Stable).has_update());
        assert!(beta.check("0.1.0", Channel::Preview).has_update());
    }

    #[test]
    fn below_min_supported_is_flagged() {
        let r = sample().check("0.0.9", Channel::Stable);
        assert!(r.has_update());
        assert!(r.below_min_supported);
    }

    #[test]
    fn min_supported_garbage_is_ignored() {
        let broken = SAMPLE.replace("\"minSupported\": \"0.1.0\"", "\"minSupported\": \"旧版\"");
        let manifest = Manifest::parse(&broken).unwrap();
        assert!(!manifest.check("0.0.1", Channel::Stable).below_min_supported);
    }

    #[test]
    fn parses_changelog_and_title() {
        let text = SAMPLE.replace(
            "\"notes\": \"修了几个问题\"",
            "\"notes\": \"修了几个问题\",\"title\": \"更稳的下载\",\"changelog\": [\"修复 A\",\"修复 B\"]",
        );
        let manifest = Manifest::parse(&text).unwrap();
        assert_eq!(manifest.title, "更稳的下载");
        assert_eq!(manifest.changelog, vec!["修复 A", "修复 B"]);
        assert!(!manifest.force, "没写 force 时默认不强制");

        let check = manifest.check("0.1.0", Channel::Stable);
        assert_eq!(check.title, "更稳的下载");
        assert_eq!(check.changelog.len(), 2);
    }

    #[test]
    fn force_flag_reaches_check_result() {
        let forced = Manifest::parse(&SAMPLE.replace("\"severity\"", "\"force\": true,\"severity\""))
            .unwrap();
        assert!(forced.force);
        let check = forced.check("0.1.0", Channel::Stable);
        assert!(check.force, "有更新且清单要求强制时，结论里必须是 force");

        // 版本已经追上了：清单再喊强制也不该拦人
        let check = forced.check("0.2.0", Channel::Stable);
        assert!(!check.force);
        assert!(!check.has_update());
    }

    #[test]
    fn too_many_changelog_lines_are_rejected() {
        let lines = (0..60).map(|i| format!("\"第 {i} 条\"")).collect::<Vec<_>>().join(",");
        let text = SAMPLE.replace(
            "\"notes\": \"修了几个问题\"",
            &format!("\"notes\": \"x\",\"changelog\": [{lines}]"),
        );
        let err = Manifest::parse(&text).unwrap_err();
        assert!(err.message.contains("条目"));
    }

    #[test]
    fn loopback_http_url_is_allowed_for_local_testing() {
        // 开发时后端清单就在本机 http 服务上，不能因为不是 https 就拒掉
        let local = SAMPLE.replace(
            "https://example.com/releases/Xin-Craft-Launcher-0.2.0.zip",
            "http://127.0.0.1:8765/Xin-Craft-Launcher-0.2.0.zip",
        );
        assert!(Manifest::parse(&local).is_ok());

        // 但别的主机的 http 一律不行
        let remote = SAMPLE.replace(
            "https://example.com/releases/Xin-Craft-Launcher-0.2.0.zip",
            "http://example.com/releases/Xin-Craft-Launcher-0.2.0.zip",
        );
        assert!(Manifest::parse(&remote).is_err());
    }

    #[test]
    fn missing_size_is_rejected() {
        // size 为 0 就算不出进度，直接拒
        let text = SAMPLE.replace("\"size\": 987654", "\"size\": 0");
        assert!(Manifest::parse(&text).is_err());
    }

    #[test]
    fn severity_survives_roundtrip() {
        let critical = Manifest::parse(&SAMPLE.replace("\"normal\"", "\"critical\"")).unwrap();
        assert_eq!(critical.severity, Severity::Critical);
        assert_eq!(critical.check("0.1.0", Channel::Stable).severity, Severity::Critical);
    }

    #[test]
    fn update_file_name_cannot_escape_directory() {
        // 版本号里带路径分隔符时，语义解析会失败 → 根本不进替换流程
        let evil = SAMPLE.replace("\"0.2.0\"", "\"../../evil\"");
        assert!(Manifest::parse(&evil).is_err());

        // 合法的预发布版本号只会产生安全字符
        let beta = Manifest::parse(&SAMPLE.replace("\"0.2.0\"", "\"0.2.0-rc.1\"")).unwrap();
        assert_eq!(
            beta.update_file_name("Xin Craft Launcher").unwrap(),
            ".Xin Craft Launcher-new-0.2.0-rc.1.exe"
        );
    }

    #[test]
    fn update_file_name_follows_the_binary_name() {
        // 程序名改了，临时文件跟着改 —— 不写死品牌名
        let manifest = sample();
        assert_eq!(
            manifest.update_file_name("Whatever").unwrap(),
            ".Whatever-new-0.2.0.exe"
        );
    }

    #[test]
    fn parse_version_accepts_prefix() {
        assert_eq!(parse_version("v1.2.3").unwrap(), Version::new(1, 2, 3));
        assert_eq!(parse_version(" 1.2.3 ").unwrap(), Version::new(1, 2, 3));
        assert!(parse_version("1.2").is_none());
    }
}
