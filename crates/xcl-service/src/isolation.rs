//! 版本隔离。
//!
//! 判定顺序（从最权威到最弱）：
//!
//! ```text
//! 版本设置里有显式值 → 用它
//! 版本目录里有 mods 文件或 saves 子目录 → 自动隔离（兼容旧布局）
//! 否则 → 全局策略（默认隔离所有版本）
//! ```
//!
//! 首次判定会落盘（`versions/<id>/XCL/instance.toml`）：游戏数据现在直接
//! 写在版本目录下，若不留标记，下次就会把隔离目录里的 mods 当成旧布局
//! 的证据，版本被永久自动隔离。落盘只封存自动检测结果；跟随全局策略的版本
//! 仍然随全局策略变化。
//!
//! 隔离只影响游戏目录；versions / libraries / assets / Java 始终共享。

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tracing::warn;
use xcl_core::config::IsolationPolicy;
use xcl_core::{AppError, Config, PathLayout, VersionKind};

use crate::AppState;

/// 判定结果的来源，给界面解释「为什么隔离 / 为什么不隔离」。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum IsolationSource {
    /// 在版本设置里手动指定
    User,
    /// 版本目录里已有 mods / saves，自动开启（旧布局）
    Auto,
    /// 首次判定时采用全局策略
    Policy,
}

/// 给前端的成品视图，前端不做任何判断。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IsolationView {
    pub version_id: String,
    pub enabled: bool,
    pub source: IsolationSource,
    /// 来源的中文说明，可直接显示
    pub source_label: String,
    /// 隔离后的游戏目录（绝对路径）
    pub game_dir: String,
    /// 相对数据根的显示名，例如 `versions/1.20.1`
    pub game_dir_label: String,
    pub policy: IsolationPolicy,
    pub policy_label: String,
}

/// 启动阶段用的判定结果，直接带路径，省一次转换。
#[derive(Debug, Clone)]
pub struct Resolution {
    pub enabled: bool,
    pub source: IsolationSource,
    pub game_dir: PathBuf,
}

impl AppState {
    pub fn version_isolation(&self, version_id: &str) -> Result<IsolationView, AppError> {
        resolve_installed(&self.layout, &self.config(), version_id)
    }

    /// `isolate = None` 表示恢复「跟随全局」。
    pub fn set_version_isolation(
        &self,
        version_id: &str,
        isolate: Option<bool>,
    ) -> Result<IsolationView, AppError> {
        // 只允许已安装的版本，避免往不存在的地方写设置
        crate::local::load(&self.layout, version_id)?;
        write_override(&self.layout, version_id, isolate)?;
        self.version_isolation(version_id)
    }
}

/// 按版本 JSON 里的信息判定（给 IPC 用）。
pub fn resolve_installed(
    layout: &PathLayout,
    config: &Config,
    version_id: &str,
) -> Result<IsolationView, AppError> {
    let resolution = resolution_for(layout, config, version_id)?;
    Ok(to_view(config, version_id, &resolution))
}

/// 只拿路径的判定结果（启动、打开文件夹用）。
pub(crate) fn resolution_for(
    layout: &PathLayout,
    config: &Config,
    version_id: &str,
) -> Result<Resolution, AppError> {
    let detail = crate::local::load(layout, version_id)?;
    let kind = xcl_core::detect_kind(
        version_id,
        detail.version_type.as_deref(),
        detail.main_class.as_deref(),
        &detail.libraries,
    );
    Ok(resolve(
        layout,
        config,
        version_id,
        kind,
        detail.version_type.as_deref(),
    ))
}

/// 纯判定：不读版本 JSON，调用方把已经算好的 kind / type 传进来。
pub fn resolve(
    layout: &PathLayout,
    config: &Config,
    version_id: &str,
    kind: VersionKind,
    version_type: Option<&str>,
) -> Resolution {
    let policy = config.launch.isolation;
    let (enabled, source) = decide(layout, policy, version_id, kind, version_type);

    let game_dir = if enabled {
        layout.version_dir(version_id)
    } else {
        layout.root().to_path_buf()
    };

    Resolution {
        enabled,
        source,
        game_dir,
    }
}

/// 读取已有判定；没有就按规则算一次并写进版本设置。
fn decide(
    layout: &PathLayout,
    policy: IsolationPolicy,
    version_id: &str,
    kind: VersionKind,
    version_type: Option<&str>,
) -> (bool, IsolationSource) {
    if let Some(settings) = read_settings(layout, version_id) {
        match settings.source.unwrap_or(IsolationSource::User) {
            IsolationSource::User => {
                if let Some(isolate) = settings.isolate {
                    return (isolate, IsolationSource::User);
                }
            }
            IsolationSource::Auto => return (true, IsolationSource::Auto),
            IsolationSource::Policy => {
                return (policy.isolates(kind, version_type), IsolationSource::Policy);
            }
        }
    }

    let auto = version_dir_has_game_data(layout, version_id);
    let enabled = auto || policy.isolates(kind, version_type);
    let source = if auto {
        IsolationSource::Auto
    } else {
        IsolationSource::Policy
    };

    // 只封存自动检测结果；跟随策略的版本不写 isolate，以后仍按全局策略算。
    // 写失败只影响「下次还查不查旧布局」，不该拦住启动或打开文件夹。
    //
    // ⚠️ 必须读旧值再改，不能拿 `Default::default()` 重建：这个文件里还有
    // `gameVersion`（安装时写的权威原版号）、描述、收藏。整份重建会把它们一起
    // 抹掉 —— 整合包实例就这样丢过 `gameVersion`，于是每次扫描都认不出版本、
    // 退回「打开客户端 jar 读 version.json」，版本列表肉眼可见地卡。
    let mut settings = read_settings(layout, version_id).unwrap_or_default();
    settings.isolate = auto.then_some(true);
    settings.source = Some(source);
    if let Err(err) = write_settings(layout, version_id, Some(settings)) {
        warn!(version = version_id, error = %err, "保存版本隔离判定失败");
    }
    (enabled, source)
}

fn to_view(
    config: &Config,
    version_id: &str,
    resolution: &Resolution,
) -> IsolationView {
    let policy = config.launch.isolation;
    IsolationView {
        version_id: version_id.to_string(),
        enabled: resolution.enabled,
        source: resolution.source,
        source_label: source_label(resolution.source).to_string(),
        game_dir: resolution.game_dir.display().to_string(),
        game_dir_label: game_dir_label(version_id, resolution.enabled),
        policy,
        // 只是为了解释「跟随全局」当前意味着什么
        policy_label: policy_label(policy).to_string(),
    }
}

fn game_dir_label(version_id: &str, enabled: bool) -> String {
    if enabled {
        format!("versions/{version_id}")
    } else {
        "数据根目录（共享）".to_string()
    }
}

fn source_label(source: IsolationSource) -> &'static str {
    match source {
        IsolationSource::User => "手动设置",
        IsolationSource::Auto => "版本目录里有 mods 或存档，已自动隔离",
        IsolationSource::Policy => "跟随全局策略",
    }
}

fn policy_label(policy: IsolationPolicy) -> &'static str {
    match policy {
        IsolationPolicy::Off => "不隔离",
        IsolationPolicy::Modable => "仅隔离可装 Mod 的版本",
        IsolationPolicy::NonRelease => "仅隔离非正式版",
        IsolationPolicy::NonReleaseOrModable => "隔离非正式版和可装 Mod 的版本",
        IsolationPolicy::All => "隔离所有版本",
    }
}

// ---------- 版本设置文件 ----------

/// `versions/<id>/XCL/instance.toml`。以后版本级设置都放这里。
///
/// 除隔离值外还记下判定来源：冻结的「首次按策略判定」和手动选的
/// 「手动设置」要能区分，否则界面解释不出为什么改全局策略没反应。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub(crate) struct InstanceSettings {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) isolate: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) source: Option<IsolationSource>,
    /// 玩家自己写的一句话说明
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) description: Option<String>,
    /// 收藏：列表里排最前
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) favorite: Option<bool>,
    /// 上次启动的时间戳（毫秒）。启动成功时写一次。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) last_played_ms: Option<u64>,
    /// 这个版本对应的 MC 版本号，安装时记下。
    ///
    /// 版本目录可以改成任意名字（「我的整合」），靠目录名猜会失效；
    /// 「下载 Mod」页要知道它属于哪个原版，所以这里落一份权威值。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) game_version: Option<String>,
    /// 这个版本对全局启动配置的覆盖（`None` 字段 = 跟随全局）。
    ///
    /// 必须是最后一个字段：它序列化成 TOML 的 `[launch]` 表，
    /// toml crate 不允许在表之后再出现单个键（ValueAfterTable）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) launch: Option<crate::version_settings::LaunchOverride>,
}

impl InstanceSettings {
    /// 所有字段都没设置：这时文件本身就可以删掉，下次读回来还是「全跟随」。
    pub(crate) fn is_empty(&self) -> bool {
        self.isolate.is_none()
            && self.source.is_none()
            && self.description.is_none()
            && self.favorite.is_none()
            && self.launch.as_ref().map_or(true, |o| o.is_empty())
            && self.last_played_ms.is_none()
            && self.game_version.is_none()
    }
}

fn settings_file(layout: &PathLayout, version_id: &str) -> PathBuf {
    layout.version_settings_file(version_id)
}

/// 直接按路径读设置文件（`local::scan` 用，它手里是版本目录而不是 layout）。
pub(crate) fn settings_at(path: &Path) -> InstanceSettings {
    let Ok(text) = fs::read_to_string(path) else {
        return InstanceSettings::default();
    };
    match toml::from_str::<InstanceSettings>(&text) {
        Ok(settings) => settings,
        Err(err) => {
            warn!(path = %path.display(), error = %err, "版本设置损坏，按未设置处理");
            InstanceSettings::default()
        }
    }
}

fn read_settings(layout: &PathLayout, version_id: &str) -> Option<InstanceSettings> {
    let path = settings_file(layout, version_id);
    let text = fs::read_to_string(&path).ok()?;
    match toml::from_str::<InstanceSettings>(&text) {
        Ok(settings) => Some(settings),
        Err(err) => {
            warn!(path = %path.display(), error = %err, "版本设置损坏，按未设置处理");
            None
        }
    }
}

/// 安装 / 修复时记下「这个版本属于哪个原版」。
///
/// 只改这一个字段，描述、收藏、启动覆盖都保留。已经记对就什么都不写。
pub(crate) fn ensure_game_version(
    layout: &PathLayout,
    version_id: &str,
    game: &str,
) -> Result<(), AppError> {
    let mut settings = read_settings(layout, version_id).unwrap_or_default();
    if settings.game_version.as_deref() == Some(game) {
        return Ok(());
    }
    settings.game_version = Some(game.to_string());
    write_settings(layout, version_id, Some(settings))
}

fn write_override(
    layout: &PathLayout,
    version_id: &str,
    isolate: Option<bool>,
) -> Result<(), AppError> {
    // 改隔离不能顺手把描述 / 收藏清掉 —— 先读旧值再覆盖这一个字段
    let mut settings = read_settings(layout, version_id).unwrap_or_default();
    match isolate {
        Some(value) => {
            settings.isolate = Some(value);
            settings.source = Some(IsolationSource::User);
        }
        // 恢复「跟随全局」：只清判定相关字段，其它元数据保留
        None => {
            settings.isolate = None;
            settings.source = None;
        }
    }
    let empty = settings.is_empty();
    write_settings(layout, version_id, if empty { None } else { Some(settings) })
}

pub(crate) fn write_settings(
    layout: &PathLayout,
    version_id: &str,
    settings: Option<InstanceSettings>,
) -> Result<(), AppError> {
    let path = settings_file(layout, version_id);
    match settings {
        Some(settings) => {
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).map_err(|e| AppError::io(&e, "创建版本设置目录"))?;
            }
            let text = toml::to_string_pretty(&settings)
                .map_err(|e| AppError::parse("版本设置序列化失败", e.to_string()))?;
            let tmp = path.with_extension("toml.tmp");
            fs::write(&tmp, text).map_err(|e| AppError::io(&e, "写入版本设置"))?;
            fs::rename(&tmp, &path).map_err(|e| AppError::io(&e, "替换版本设置"))?;
            Ok(())
        }
        // 恢复默认等于删掉文件，下次判定会重新回到「未决定」。
        None => match fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(err) => Err(AppError::io(&err, "删除版本设置")),
        },
    }
}

// ---------- 旧布局检测 ----------

/// 判据：`versions/<id>/mods` 下有文件，或 `saves` 下有子目录。
fn version_dir_has_game_data(layout: &PathLayout, version_id: &str) -> bool {
    let version_dir = layout.version_dir(version_id);
    has_files(&version_dir.join("mods")) || has_subdirs(&version_dir.join("saves"))
}

fn has_files(dir: &Path) -> bool {
    fs::read_dir(dir)
        .map(|entries| {
            entries.flatten().any(|entry| {
                entry
                    .file_type()
                    .map(|kind| kind.is_file())
                    .unwrap_or(false)
            })
        })
        .unwrap_or(false)
}

fn has_subdirs(dir: &Path) -> bool {
    fs::read_dir(dir)
        .map(|entries| {
            entries.flatten().any(|entry| {
                entry
                    .file_type()
                    .map(|kind| kind.is_dir())
                    .unwrap_or(false)
            })
        })
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_layout(tag: &str) -> PathLayout {
        let dir = std::env::temp_dir().join(format!("xcl-iso-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        PathLayout::new(dir.clone(), dir.join("cache"))
    }

    fn config_with(policy: IsolationPolicy) -> Config {
        let mut config = Config::default();
        config.launch.isolation = policy;
        config
    }

    #[test]
    fn explicit_setting_beats_everything() {
        let layout = temp_layout("explicit");
        let id = "1.20.1";
        fs::create_dir_all(layout.version_dir(id).join("mods")).unwrap();
        fs::write(layout.version_dir(id).join("mods/a.jar"), "x").unwrap();
        write_override(&layout, id, Some(false)).unwrap();

        let res = resolve(
            &layout,
            &config_with(IsolationPolicy::All),
            id,
            VersionKind::Vanilla,
            Some("release"),
        );
        assert!(!res.enabled);
        assert_eq!(res.source, IsolationSource::User);
        assert_eq!(res.game_dir, layout.root());

        write_override(&layout, id, Some(true)).unwrap();
        let res = resolve(
            &layout,
            &config_with(IsolationPolicy::Off),
            id,
            VersionKind::Vanilla,
            Some("release"),
        );
        assert!(res.enabled);
        assert_eq!(res.game_dir, layout.version_dir(id));

        let _ = fs::remove_dir_all(layout.root());
    }

    /// 判定隔离时只改判定那两个字段。
    ///
    /// 这里曾经用 `InstanceSettings { ..Default::default() }` 整份重建，把安装时
    /// 写的 `gameVersion` 一起抹掉了。后果不是"少一个字段"那么轻：扫描时认不出
    /// 这个实例属于哪个原版，就退回「打开客户端 jar 读 version.json」，
    /// 每次刷新版本列表都要开一次几十 MB 的 zip。
    #[test]
    fn deciding_isolation_keeps_the_installed_game_version() {
        let layout = temp_layout("decide-keeps");
        let id = "我的整合包";
        // 安装时写下的权威值
        ensure_game_version(&layout, id, "1.20.1").unwrap();
        // 目录里有 mods → 走自动判定 → 会落盘
        fs::create_dir_all(layout.version_dir(id).join("mods")).unwrap();
        fs::write(layout.version_dir(id).join("mods/a.jar"), "x").unwrap();

        let (enabled, source) = decide(&layout, IsolationPolicy::All, id, VersionKind::Fabric, None);
        assert!(enabled);
        assert_eq!(source, IsolationSource::Auto);

        let settings = read_settings(&layout, id).expect("判定后设置文件应当存在");
        assert_eq!(
            settings.game_version.as_deref(),
            Some("1.20.1"),
            "隔离判定不能把安装时写的 gameVersion 抹掉"
        );
        assert_eq!(settings.isolate, Some(true));

        let _ = fs::remove_dir_all(layout.root());
    }

    #[test]
    fn old_game_data_in_version_dir_auto_enables() {        let layout = temp_layout("auto");
        let id = "1.7.10";
        let policy_off = config_with(IsolationPolicy::Off);

        // mods 目录为空不算
        fs::create_dir_all(layout.version_dir(id).join("mods")).unwrap();
        let res = resolve(&layout, &policy_off, id, VersionKind::Vanilla, Some("release"));
        assert!(!res.enabled);
        assert_eq!(res.source, IsolationSource::Policy);

        // 有文件就算了。首次判定已落盘，先删掉模拟「还没判定过」
        fs::remove_file(settings_file(&layout, id)).unwrap();
        fs::write(layout.version_dir(id).join("mods/a.jar"), "x").unwrap();
        let res = resolve(&layout, &policy_off, id, VersionKind::Vanilla, Some("release"));
        assert!(res.enabled);
        assert_eq!(res.source, IsolationSource::Auto);

        // saves 只看子目录
        let other = "1.12.2";
        fs::create_dir_all(layout.version_dir(other).join("saves")).unwrap();
        let res = resolve(&layout, &policy_off, other, VersionKind::Vanilla, Some("release"));
        assert!(!res.enabled);
        fs::remove_file(settings_file(&layout, other)).unwrap();
        fs::create_dir_all(layout.version_dir(other).join("saves/World")).unwrap();
        let res = resolve(&layout, &policy_off, other, VersionKind::Vanilla, Some("release"));
        assert!(res.enabled);

        let _ = fs::remove_dir_all(layout.root());
    }

    #[test]
    fn legacy_auto_detect_runs_only_once() {
        let layout = temp_layout("marker");
        let id = "1.20.1";

        // 首次：mods 为空，按全局策略判为不隔离，并留下「旧布局已检查」标记
        fs::create_dir_all(layout.version_dir(id).join("mods")).unwrap();
        let res = resolve(
            &layout,
            &config_with(IsolationPolicy::Off),
            id,
            VersionKind::Vanilla,
            Some("release"),
        );
        assert!(!res.enabled);
        assert_eq!(res.source, IsolationSource::Policy);
        assert!(settings_file(&layout, id).is_file());

        // 隔离后 mods 就写在版本目录里，不能再被当成旧布局的证据
        fs::write(layout.version_dir(id).join("mods/a.jar"), "x").unwrap();
        let res = resolve(
            &layout,
            &config_with(IsolationPolicy::Off),
            id,
            VersionKind::Vanilla,
            Some("release"),
        );
        assert!(!res.enabled);
        assert_eq!(res.source, IsolationSource::Policy);

        // 跟随策略的版本仍然随全局策略变化
        let res = resolve(
            &layout,
            &config_with(IsolationPolicy::All),
            id,
            VersionKind::Vanilla,
            Some("release"),
        );
        assert!(res.enabled);
        assert_eq!(res.source, IsolationSource::Policy);

        let _ = fs::remove_dir_all(layout.root());
    }

    #[test]
    fn policy_applies_when_nothing_is_saved() {
        let layout = temp_layout("policy");
        let id = "1.20.1";

        let res = resolve(
            &layout,
            &config_with(IsolationPolicy::All),
            id,
            VersionKind::Vanilla,
            Some("release"),
        );
        assert!(res.enabled);
        assert_eq!(res.source, IsolationSource::Policy);

        let res = resolve(
            &layout,
            &config_with(IsolationPolicy::Modable),
            id,
            VersionKind::Forge,
            Some("release"),
        );
        assert!(res.enabled);

        let res = resolve(
            &layout,
            &config_with(IsolationPolicy::Modable),
            id,
            VersionKind::Vanilla,
            Some("release"),
        );
        assert!(!res.enabled);

        let _ = fs::remove_dir_all(layout.root());
    }

    #[test]
    fn settings_roundtrip_and_reset() {
        let layout = temp_layout("roundtrip");
        let id = "1.20.1";

        write_override(&layout, id, Some(true)).unwrap();
        let settings = read_settings(&layout, id).unwrap();
        assert_eq!(settings.isolate, Some(true));
        assert_eq!(settings.source, Some(IsolationSource::User));
        assert!(settings_file(&layout, id).is_file());

        write_override(&layout, id, None).unwrap();
        assert!(read_settings(&layout, id).is_none());
        assert!(!settings_file(&layout, id).exists());

        // 临时文件不残留
        assert!(!settings_file(&layout, id).with_extension("toml.tmp").exists());
        let _ = fs::remove_dir_all(layout.root());
    }

    /// 安装时记下的「属于哪个原版」要能存住，且不会被改隔离顺手清掉。
    #[test]
    fn game_version_marker_survives_other_writes() {
        let layout = temp_layout("game-version");
        let id = "我的整合";

        ensure_game_version(&layout, id, "1.21.11").unwrap();
        assert_eq!(
            read_settings(&layout, id).unwrap().game_version.as_deref(),
            Some("1.21.11")
        );

        // 改隔离只动隔离字段，不能把原版版本号清掉
        write_override(&layout, id, Some(false)).unwrap();
        let settings = read_settings(&layout, id).unwrap();
        assert_eq!(settings.game_version.as_deref(), Some("1.21.11"));

        // 恢复「跟随全局」也不能顺手删文件
        write_override(&layout, id, None).unwrap();
        assert_eq!(
            read_settings(&layout, id).unwrap().game_version.as_deref(),
            Some("1.21.11")
        );

        let _ = fs::remove_dir_all(layout.root());
    }

    #[test]
    fn broken_settings_fall_back_to_unset() {
        let layout = temp_layout("broken");
        let id = "1.20.1";
        fs::create_dir_all(layout.version_settings_dir(id)).unwrap();
        fs::write(settings_file(&layout, id), "这不是 toml {{{").unwrap();

        assert!(read_settings(&layout, id).is_none());
        let _ = fs::remove_dir_all(layout.root());
    }

    #[test]
    fn view_explains_source_and_paths() {
        let layout = temp_layout("view");
        let id = "1.20.1";
        let config = config_with(IsolationPolicy::All);
        let res = resolve(&layout, &config, id, VersionKind::Vanilla, Some("release"));
        let view = to_view(&config, id, &res);

        assert!(view.enabled);
        assert_eq!(view.source, IsolationSource::Policy);
        assert_eq!(view.source_label, "跟随全局策略");
        assert_eq!(view.game_dir_label, "versions/1.20.1");
        assert_eq!(view.policy_label, "隔离所有版本");

        let _ = fs::remove_dir_all(layout.root());
    }
}
