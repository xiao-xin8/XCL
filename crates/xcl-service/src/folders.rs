//! 用系统文件管理器打开版本相关的文件夹。
//!
//! 前端只传「哪个版本 + 哪个文件夹」，路径一律在这里算 —— 游戏目录可能
//! 因版本隔离落在数据根或 `versions/<id>`，前端不该自己拼。
//!
//! `version_id` 会被拼进路径，必须先挡住分隔符与 `..`，否则这条 IPC
//! 就等于「打开任意目录」。

use std::path::{Path, PathBuf};
use std::process::Command;

use serde::Deserialize;
use xcl_core::{AppError, Config, PathLayout};

use crate::AppState;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum VersionFolder {
    /// `versions/<id>/`：版本 JSON、客户端 jar、natives
    Version,
    /// 游戏目录根，隔离与否由 `isolation` 决定
    Game,
    Saves,
    Mods,
    Logs,
    Resourcepacks,
    /// 1.5 及更早的材质包目录
    Texturepacks,
}

impl VersionFolder {
    fn game_subdir(self) -> Option<&'static str> {
        match self {
            Self::Version | Self::Game => None,
            Self::Saves => Some("saves"),
            Self::Mods => Some("mods"),
            Self::Logs => Some("logs"),
            Self::Resourcepacks => Some("resourcepacks"),
            Self::Texturepacks => Some("texturepacks"),
        }
    }
}

/// 启动器自己的目录（设置页「关于与高级」用）。
///
/// 只给枚举、不给任意路径：这条 IPC 一旦收字符串就等于「打开任意目录」。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AppDir {
    /// 数据根：versions / libraries / assets / instances / config.toml
    Data,
    /// 缓存根：http / temp / skin（不进便携包）
    Cache,
}

impl AppState {
    pub fn open_app_dir(&self, dir: AppDir) -> Result<(), AppError> {
        let path = match dir {
            AppDir::Data => self.layout.root().to_path_buf(),
            AppDir::Cache => self.layout.cache_root().to_path_buf(),
        };
        // 缓存目录可能还没建过（全新安装、还没下过东西）
        std::fs::create_dir_all(&path).map_err(|e| AppError::io(&e, "创建目录"))?;
        open_in_file_manager(&path)
    }
}

impl AppState {
    pub fn open_version_folder(
        &self,
        version_id: &str,
        folder: VersionFolder,
    ) -> Result<(), AppError> {
        let path = folder_path(&self.layout, &self.config(), version_id, folder)?;
        open_in_file_manager(&path)
    }
}

fn folder_path(
    layout: &PathLayout,
    config: &Config,
    version_id: &str,
    folder: VersionFolder,
) -> Result<PathBuf, AppError> {
    validate_version_id(version_id)?;

    let path = match folder {
        VersionFolder::Version => {
            let dir = layout.version_dir(version_id);
            if !dir.is_dir() {
                return Err(AppError::not_found(
                    "版本未安装",
                    format!("找不到 {}", dir.display()),
                ));
            }
            dir
        }
        VersionFolder::Game => game_dir(layout, config, version_id)?,
        other => game_dir(layout, config, version_id)?
            .join(other.game_subdir().expect("非 Game 的分支都有子目录")),
    };

    // 游戏还没启动过时 saved/mods/logs 都不存在；按钮点了就该有反应，
    // 所以直接建出来（都是空目录，不会覆盖任何东西）。
    if !path.exists() {
        std::fs::create_dir_all(&path).map_err(|e| AppError::io(&e, "创建文件夹"))?;
    }
    Ok(path)
}

fn game_dir(layout: &PathLayout, config: &Config, version_id: &str) -> Result<PathBuf, AppError> {
    Ok(crate::isolation::resolution_for(layout, config, version_id)?.game_dir)
}

fn validate_version_id(version_id: &str) -> Result<(), AppError> {
    let invalid = version_id.is_empty()
        || version_id == "."
        || version_id == ".."
        || version_id.contains('/')
        || version_id.contains('\\')
        || version_id.contains(':')
        || version_id.contains('\0');
    if invalid {
        return Err(AppError::validation(
            "版本 id 非法",
            format!("{version_id:?} 不能作为目录名"),
        ));
    }
    Ok(())
}

fn open_in_file_manager(path: &Path) -> Result<(), AppError> {
    let program = if cfg!(windows) {
        "explorer"
    } else if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    // 不等待退出：explorer 打开后会立刻返回，等它反而会卡住命令线程
    Command::new(program)
        .arg(path)
        .spawn()
        .map(|_| ())
        .map_err(|e| AppError::io(&e, "打开文件夹"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use xcl_core::config::IsolationPolicy;

    fn temp_layout(tag: &str) -> PathLayout {
        let dir = std::env::temp_dir().join(format!("xcl-folders-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("versions/1.20.1")).unwrap();
        std::fs::write(dir.join("versions/1.20.1/1.20.1.json"), r#"{"id":"1.20.1"}"#).unwrap();
        PathLayout::new(dir.clone(), dir.join("cache"))
    }

    fn config_with(policy: IsolationPolicy) -> Config {
        let mut config = Config::default();
        config.launch.isolation = policy;
        config
    }

    #[test]
    fn folders_follow_the_isolated_game_dir() {
        let layout = temp_layout("isolated");
        let config = config_with(IsolationPolicy::All);
        let id = "1.20.1";

        let saves = folder_path(&layout, &config, id, VersionFolder::Saves).unwrap();
        assert_eq!(saves, layout.version_dir(id).join("saves"));
        assert!(saves.is_dir(), "不存在的文件夹应当被创建");

        let game = folder_path(&layout, &config, id, VersionFolder::Game).unwrap();
        assert_eq!(game, layout.version_dir(id));

        let version = folder_path(&layout, &config, id, VersionFolder::Version).unwrap();
        assert_eq!(version, layout.version_dir(id));

        let _ = std::fs::remove_dir_all(layout.root());
    }

    #[test]
    fn unisolated_version_uses_the_data_root() {
        let layout = temp_layout("shared");
        let config = config_with(IsolationPolicy::Off);
        let id = "1.20.1";

        let game = folder_path(&layout, &config, id, VersionFolder::Game).unwrap();
        assert_eq!(game, layout.root());

        let mods = folder_path(&layout, &config, id, VersionFolder::Mods).unwrap();
        assert_eq!(mods, layout.root().join("mods"));

        let _ = std::fs::remove_dir_all(layout.root());
    }

    #[test]
    fn version_folder_of_an_uninstalled_version_is_reported() {
        let layout = temp_layout("missing");
        let config = config_with(IsolationPolicy::All);
        let err = folder_path(&layout, &config, "9.9.9", VersionFolder::Version).unwrap_err();
        assert_eq!(err.title, "版本未安装");

        let _ = std::fs::remove_dir_all(layout.root());
    }

    #[test]
    fn frontend_keys_deserialize() {
        for (key, expected) in [
            ("version", VersionFolder::Version),
            ("game", VersionFolder::Game),
            ("saves", VersionFolder::Saves),
            ("mods", VersionFolder::Mods),
            ("logs", VersionFolder::Logs),
            ("resourcepacks", VersionFolder::Resourcepacks),
            ("texturepacks", VersionFolder::Texturepacks),
        ] {
            let parsed: VersionFolder = serde_json::from_str(&format!("\"{key}\"")).unwrap();
            assert_eq!(parsed, expected);
        }
    }

    #[test]
    fn version_id_cannot_escape_the_data_root() {
        let layout = temp_layout("traversal");
        let config = config_with(IsolationPolicy::All);
        for bad in ["..", "../x", "a/b", "a\\b", "C:evil", ""] {
            assert!(
                folder_path(&layout, &config, bad, VersionFolder::Game).is_err(),
                "{bad:?} 应当被拒绝"
            );
        }

        let _ = std::fs::remove_dir_all(layout.root());
    }
}
