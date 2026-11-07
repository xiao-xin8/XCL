//! 单个版本的启动设置覆盖。
//!
//! 全局那份在 `config.toml`（设置页管），这里管的是「这个版本例外」：
//! 每个字段都是 `Option`，`None` = 跟随全局，`Some` = 只对这个版本生效。
//!
//! 语义只有一条：覆盖就是整份替换。JVM 参数、游戏参数填了就是这一份，
//! 不会和全局那份拼在一起 —— 解释起来简单，也不会出现「全局的某个参数
//! 悄悄混进来把游戏搞崩、而界面上根本看不出来」这种事故。

use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use xcl_core::config::MemoryMode;
use xcl_core::{AppError, Config, VersionKind};

use crate::isolation::{settings_at, write_settings};
use crate::local;
use crate::AppState;

/// 版本级启动覆盖。字段全空 = 这个版本完全跟随全局。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct LaunchOverride {
    /// 这个版本的内存方式：`None` = 跟随全局，`Auto` / `Custom` = 单独设置。
    pub memory_mode: Option<MemoryMode>,
    /// 手动模式下的最大内存（MB）
    pub memory_mb: Option<u32>,
    /// 指定的 Java 可执行文件路径
    pub java_path: Option<String>,
    /// 额外 JVM 参数（整份替换全局那份）
    pub jvm_args: Option<String>,
    /// 额外游戏参数（整份替换全局那份）
    pub game_args: Option<String>,
    pub window_width: Option<u32>,
    pub window_height: Option<u32>,
    pub fullscreen: Option<bool>,
    /// 游戏语言（写进 options.txt 的默认值）
    pub language: Option<String>,
    pub pre_launch_command: Option<String>,
    pub wait_command: Option<bool>,
}

impl LaunchOverride {
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    /// 越界值夹回合法区间。前端已经钳过一次，这里是兜底。
    fn normalized(mut self) -> Self {
        // 旧版本只写了 memoryMb：它本来就是「只对这个版本手动设置」，
        // 这里补成显式的 Custom，免得被全局的自动模式带走。
        if self.memory_mode.is_none() && self.memory_mb.is_some() {
            self.memory_mode = Some(MemoryMode::Custom);
        }
        if self.memory_mode == Some(MemoryMode::Auto) {
            self.memory_mb = None;
        }
        self.memory_mb = self.memory_mb.map(|v| v.clamp(256, 65536));
        self.window_width = self.window_width.map(|v| v.clamp(160, 16384));
        self.window_height = self.window_height.map(|v| v.clamp(160, 16384));
        self
    }
}

/// 「最终会用到的那份值」。全局和合并结果都用这个形状，前端直接并排显示。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LaunchValues {
    pub memory_mode: MemoryMode,
    pub memory_mb: u32,
    pub java_path: Option<String>,
    pub jvm_args: String,
    pub game_args: String,
    pub window_width: u32,
    pub window_height: u32,
    pub fullscreen: bool,
    pub language: String,
    pub pre_launch_command: String,
    pub wait_command: bool,
}

impl LaunchValues {
    fn from_global(config: &Config) -> Self {
        let global = &config.launch;
        Self {
            memory_mode: global.memory_mode,
            memory_mb: global.memory_mb,
            java_path: config
                .java
                .manual_path
                .as_ref()
                .map(|path| path.display().to_string()),
            jvm_args: global.extra_jvm_args.clone(),
            game_args: global.extra_game_args.clone(),
            window_width: global.window_width,
            window_height: global.window_height,
            fullscreen: global.fullscreen,
            language: global.language.clone(),
            pre_launch_command: global.pre_launch_command.clone(),
            wait_command: global.wait_command,
        }
    }

    /// 全局 + 版本覆盖。启动链路和界面都读这一份，两边不会算出不同结果。
    pub fn merged(config: &Config, over: &LaunchOverride) -> Self {
        let global = &config.launch;
        // 旧覆盖只有 memory_mb 时按 Custom 处理；显式 memoryMode 优先。
        let memory_mode = over
            .memory_mode
            .or_else(|| over.memory_mb.map(|_| MemoryMode::Custom))
            .unwrap_or(global.memory_mode);
        Self {
            memory_mode,
            memory_mb: over.memory_mb.unwrap_or(global.memory_mb),
            java_path: over.java_path.clone().or_else(|| {
                config
                    .java
                    .manual_path
                    .as_ref()
                    .map(|path| path.display().to_string())
            }),
            jvm_args: over
                .jvm_args
                .clone()
                .unwrap_or_else(|| global.extra_jvm_args.clone()),
            game_args: over
                .game_args
                .clone()
                .unwrap_or_else(|| global.extra_game_args.clone()),
            window_width: over.window_width.unwrap_or(global.window_width),
            window_height: over.window_height.unwrap_or(global.window_height),
            fullscreen: over.fullscreen.unwrap_or(global.fullscreen),
            language: over
                .language
                .clone()
                .unwrap_or_else(|| global.language.clone()),
            pre_launch_command: over
                .pre_launch_command
                .clone()
                .unwrap_or_else(|| global.pre_launch_command.clone()),
            wait_command: over.wait_command.unwrap_or(global.wait_command),
        }
    }
}

/// 弹窗要的全部数据：覆盖项、合并结果、全局值。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VersionSettingsView {
    pub version_id: String,
    pub overrides: LaunchOverride,
    pub effective: LaunchValues,
    pub global: LaunchValues,
}

/// 版本目录的大小与内容统计。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VersionStats {
    pub size_bytes: u64,
    pub mods: u32,
    pub saves: u32,
    pub screenshots: u32,
    pub resourcepacks: u32,
    pub last_played_ms: Option<u64>,
    /// 统计时用的游戏目录（隔离与否决定它指向哪）
    pub game_dir: String,
}

impl AppState {
    /// 读这个版本的设置视图。
    pub fn version_settings(&self, version_id: &str) -> Result<VersionSettingsView, AppError> {
        self.require_version_dir(version_id)?;
        let settings = settings_at(&self.layout.version_settings_file(version_id));
        let over = settings
            .launch
            .clone()
            .unwrap_or_default()
            .normalized();
        let config = self.config();
        Ok(VersionSettingsView {
            version_id: version_id.to_string(),
            effective: LaunchValues::merged(&config, &over),
            global: LaunchValues::from_global(&config),
            overrides: over,
        })
    }

    /// 局部合并：只改传进来的字段，`null` 表示「恢复跟随全局」。
    pub fn patch_version_settings(
        &self,
        version_id: &str,
        patch: &Value,
    ) -> Result<VersionSettingsView, AppError> {
        self.require_version_dir(version_id)?;
        let object = patch
            .as_object()
            .ok_or_else(|| AppError::validation("设置格式不对", "需要传一个 JSON 对象"))?;

        let path = self.layout.version_settings_file(version_id);
        let mut settings = settings_at(&path);
        let mut over = settings.launch.take().unwrap_or_default();
        for (key, value) in object {
            apply_field(&mut over, key, value)?;
        }
        settings.launch = (!over.clone().normalized().is_empty()).then(|| over.normalized());
        write_settings(
            &self.layout,
            version_id,
            if settings.is_empty() {
                None
            } else {
                Some(settings)
            },
        )?;
        self.version_settings(version_id)
    }

    /// 启动成功后记一笔「上次启动时间」，详情页用它显示「2 小时前」。
    pub fn mark_version_played(&self, version_id: &str) {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        let path = self.layout.version_settings_file(version_id);
        let mut settings = settings_at(&path);
        settings.last_played_ms = Some(now);
        if let Err(err) = write_settings(&self.layout, version_id, Some(settings)) {
            // 记时间失败不影响游戏，最多详情里少一行
            tracing::warn!(error = %err, "写入上次启动时间失败");
        }
    }

    /// 目录大小 + Mod / 存档 / 截图 / 资源包数量。
    ///
    /// 走的是真正的游戏目录（隔离与否由 `isolation::resolve` 决定），
    /// 所以隔离版本看到的是版本目录里的数据，不隔离的就是数据根那套。
    pub fn version_stats(&self, version_id: &str) -> Result<VersionStats, AppError> {
        self.require_version_dir(version_id)?;
        let config = self.config();
        let settings = settings_at(&self.layout.version_settings_file(version_id));

        let local_version = local::scan(&self.layout)
            .into_iter()
            .find(|item| item.id == version_id);
        let kind = local_version
            .as_ref()
            .map(|item| item.kind)
            .unwrap_or(VersionKind::Unknown);
        let version_type = local_version.as_ref().map(|item| item.version_type.clone());

        let isolation = crate::isolation::resolve(
            &self.layout,
            &config,
            version_id,
            kind,
            version_type.as_deref(),
        );
        let game_dir = isolation.game_dir.clone();

        Ok(VersionStats {
            size_bytes: dir_size(&game_dir),
            mods: count_files(&game_dir.join("mods")),
            saves: count_dirs(&game_dir.join("saves")),
            screenshots: count_files(&game_dir.join("screenshots")),
            resourcepacks: count_files(&game_dir.join("resourcepacks"))
                + count_files(&game_dir.join("texturepacks")),
            last_played_ms: settings.last_played_ms,
            game_dir: game_dir.display().to_string(),
        })
    }
}

/// 单个字段的写入。`null` = 清掉覆盖（回到跟随全局）。
fn apply_field(over: &mut LaunchOverride, key: &str, value: &Value) -> Result<(), AppError> {
    match key {
        "memoryMode" => {
            over.memory_mode = parse_memory_mode(value)?;
            // Auto / 跟随全局都不保留手动值，避免旧值在下次切回手动时突然冒出来
            if over.memory_mode != Some(MemoryMode::Custom) {
                over.memory_mb = None;
            }
        }
        "memoryMb" => {
            over.memory_mb = parse_opt(value)?;
            if over.memory_mb.is_some() {
                over.memory_mode = Some(MemoryMode::Custom);
            } else if over.memory_mode == Some(MemoryMode::Custom) {
                over.memory_mode = None;
            }
        }
        "javaPath" => over.java_path = parse_text(value),
        // 这三项空串是有意义的：「这个版本不要额外参数」，要能压住全局那份。
        // 所以只有 `null` 才算「恢复跟随全局」。
        "jvmArgs" => over.jvm_args = parse_blank_text(value),
        "gameArgs" => over.game_args = parse_blank_text(value),
        "preLaunchCommand" => over.pre_launch_command = parse_blank_text(value),
        "windowWidth" => over.window_width = parse_opt(value)?,
        "windowHeight" => over.window_height = parse_opt(value)?,
        "fullscreen" => over.fullscreen = parse_opt(value)?,
        "language" => over.language = parse_text(value),
        "waitCommand" => over.wait_command = parse_opt(value)?,
        other => {
            return Err(AppError::validation(
                "不认识的设置项",
                format!("没有名为 {other} 的版本设置"),
            ))
        }
    }
    Ok(())
}

fn parse_opt<T: DeserializeOwned>(value: &Value) -> Result<Option<T>, AppError> {
    if value.is_null() {
        return Ok(None);
    }
    serde_json::from_value(value.clone())
        .map(Some)
        .map_err(|err| AppError::validation("设置值不合法", err.to_string()))
}

/// 内存方式：`null` = 跟随全局，`"auto"` / `"custom"` = 单独设置。
fn parse_memory_mode(value: &Value) -> Result<Option<MemoryMode>, AppError> {
    if value.is_null() {
        return Ok(None);
    }
    serde_json::from_value::<MemoryMode>(value.clone())
        .map(Some)
        .map_err(|_| {
            AppError::validation(
                "内存设置格式不对",
                "memoryMode 只能是 \"auto\"、\"custom\" 或 null",
            )
        })
}

/// 文本类：`null` 和空串都当「没有」，免得存一堆空字符串进 toml。
fn parse_text(value: &Value) -> Option<String> {
    let text = value.as_str()?.trim();
    (!text.is_empty()).then(|| text.to_string())
}

/// 参数类：只有 `null` 表示「跟随全局」，空串表示「这个版本就是不要参数」。
fn parse_blank_text(value: &Value) -> Option<String> {
    value.as_str().map(|text| text.trim().to_string())
}

/// 递归算目录大小。带文件数上限：几万个文件之后就没必要再细算了，
/// 界面要的是「大概多大」，不是精确到字节。
fn dir_size(dir: &Path) -> u64 {
    let mut budget = 20_000usize;
    walk_size(dir, &mut budget)
}

fn walk_size(dir: &Path, budget: &mut usize) -> u64 {
    if *budget == 0 {
        return 0;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    let mut total = 0u64;
    for entry in entries.flatten() {
        if *budget == 0 {
            break;
        }
        *budget -= 1;
        let Ok(kind) = entry.file_type() else { continue };
        if kind.is_dir() {
            total += walk_size(&entry.path(), budget);
        } else if kind.is_file() {
            total += entry.metadata().map(|meta| meta.len()).unwrap_or(0);
        }
    }
    total
}

fn count_files(dir: &Path) -> u32 {
    std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .flatten()
                .filter(|entry| {
                    entry
                        .file_type()
                        .map(|kind| kind.is_file())
                        .unwrap_or(false)
                })
                .count() as u32
        })
        .unwrap_or(0)
}

fn count_dirs(dir: &Path) -> u32 {
    std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .flatten()
                .filter(|entry| {
                    entry
                        .file_type()
                        .map(|kind| kind.is_dir())
                        .unwrap_or(false)
                })
                .count() as u32
        })
        .unwrap_or(0)
}

/// 启动链路用：读覆盖 + 合并全局，一次算好。
pub(crate) fn load_override(
    layout: &xcl_core::PathLayout,
    version_id: &str,
) -> LaunchOverride {
    settings_at(&layout.version_settings_file(version_id))
        .launch
        .unwrap_or_default()
        .normalized()
}

/// 合并结果给 `launch.rs` 用。参数是全局快照，保证一次启动只用一个版本的值。
pub(crate) fn resolve_launch(
    config: &Config,
    layout: &xcl_core::PathLayout,
    version_id: &str,
) -> LaunchValues {
    LaunchValues::merged(config, &load_override(layout, version_id))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn override_replaces_whole_list() {
        let mut global = Config::default();
        global.launch.extra_jvm_args = "-Xmx1G -Dglobal=1".to_string();
        let over = LaunchOverride {
            jvm_args: Some("-Dmine=1".to_string()),
            ..Default::default()
        };
        let merged = LaunchValues::merged(&global, &over);
        // 覆盖是整份替换，不是拼接
        assert_eq!(merged.jvm_args, "-Dmine=1");
        assert_eq!(merged.game_args, global.launch.extra_game_args);
    }

    #[test]
    fn empty_override_follows_global() {
        let global = Config::default();
        let merged = LaunchValues::merged(&global, &LaunchOverride::default());
        assert_eq!(merged.memory_mode, global.launch.memory_mode);
        assert_eq!(merged.memory_mb, global.launch.memory_mb);
        assert_eq!(merged.language, global.launch.language);
    }

    #[test]
    fn memory_modes_merge_and_legacy_custom_still_works() {
        let mut global = Config::default();
        global.launch.memory_mode = MemoryMode::Custom;
        global.launch.memory_mb = 6144;

        // 跟随全局
        let merged = LaunchValues::merged(&global, &LaunchOverride::default());
        assert_eq!(merged.memory_mode, MemoryMode::Custom);
        assert_eq!(merged.memory_mb, 6144);

        // 这个版本单独自动
        let merged = LaunchValues::merged(
            &global,
            &LaunchOverride {
                memory_mode: Some(MemoryMode::Auto),
                ..Default::default()
            },
        );
        assert_eq!(merged.memory_mode, MemoryMode::Auto);

        // 这个版本单独手动
        let merged = LaunchValues::merged(
            &global,
            &LaunchOverride {
                memory_mode: Some(MemoryMode::Custom),
                memory_mb: Some(8192),
                ..Default::default()
            },
        );
        assert_eq!(merged.memory_mode, MemoryMode::Custom);
        assert_eq!(merged.memory_mb, 8192);

        // 旧数据只有 memoryMb：按自定义处理，不被全局自动带走
        let legacy = LaunchOverride {
            memory_mb: Some(9216),
            ..Default::default()
        }
        .normalized();
        let merged = LaunchValues::merged(
            &Config {
                launch: xcl_core::config::LaunchConfig {
                    memory_mode: MemoryMode::Auto,
                    ..global.launch.clone()
                },
                ..Config::default()
            },
            &legacy,
        );
        assert_eq!(merged.memory_mode, MemoryMode::Custom);
        assert_eq!(merged.memory_mb, 9216);
    }

    #[test]
    fn memory_mode_field_switches_and_clears_value() {
        let mut over = LaunchOverride::default();
        apply_field(&mut over, "memoryMode", &serde_json::json!("auto")).unwrap();
        assert_eq!(over.memory_mode, Some(MemoryMode::Auto));
        assert_eq!(over.memory_mb, None);

        apply_field(&mut over, "memoryMode", &serde_json::json!("custom")).unwrap();
        apply_field(&mut over, "memoryMb", &serde_json::json!(4096)).unwrap();
        assert_eq!(over.memory_mode, Some(MemoryMode::Custom));
        assert_eq!(over.memory_mb, Some(4096));

        apply_field(&mut over, "memoryMode", &Value::Null).unwrap();
        assert_eq!(over.memory_mode, None);
        assert_eq!(over.memory_mb, None);
    }

    #[test]
    fn null_and_blank_text_clear_fields() {
        let mut over = LaunchOverride {
            jvm_args: Some("-Dx=1".into()),
            language: Some("en_us".into()),
            ..Default::default()
        };
        apply_field(&mut over, "jvmArgs", &Value::Null).unwrap();
        apply_field(&mut over, "language", &Value::String("  ".into())).unwrap();
        assert!(over.is_empty());
    }

    #[test]
    fn unknown_key_is_rejected() {
        let mut over = LaunchOverride::default();
        assert!(apply_field(&mut over, "nope", &Value::Bool(true)).is_err());
    }

    /// `[launch]` 是 TOML 表，写在别的键后面会直接序列化失败（ValueAfterTable），
    /// 所以这条测试盯的是字段顺序，不只是内容。
    #[test]
    fn instance_toml_roundtrips_with_launch_table() {
        let dir = std::env::temp_dir().join(format!("xcl-vs-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let layout = xcl_core::PathLayout::new(dir.clone(), dir.join("cache"));
        std::fs::create_dir_all(layout.version_settings_dir("v")).unwrap();

        let mut settings = crate::isolation::InstanceSettings::default();
        settings.description = Some("生电用的".to_string());
        settings.last_played_ms = Some(1_700_000_000_000);
        settings.launch = Some(LaunchOverride {
            memory_mb: Some(8192),
            jvm_args: Some(String::new()),
            ..Default::default()
        });
        write_settings(&layout, "v", Some(settings)).unwrap();

        let back = settings_at(&layout.version_settings_file("v"));
        assert_eq!(back.description.as_deref(), Some("生电用的"));
        assert_eq!(back.last_played_ms, Some(1_700_000_000_000));
        let launch = back.launch.unwrap();
        assert_eq!(launch.memory_mb, Some(8192));
        // 空串是「这个版本不要额外参数」，不能读回来变成「跟随全局」
        assert_eq!(launch.jvm_args.as_deref(), Some(""));

        let _ = std::fs::remove_dir_all(&dir);
    }
}
