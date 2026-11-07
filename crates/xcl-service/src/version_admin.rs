//! 版本的增删改：改名、描述、收藏、图标、删除。
//!
//! 一个版本的全部东西（jar、natives、存档、mods、`XCL/instance.toml`）都在
//! `versions/<id>/` 里，所以删除就是删这一个目录；共享的 `libraries` / `assets`
//! 一根手指都不碰。删错了就是真没了，界面那边必须二次确认。

use std::fs;
use std::path::{Path, PathBuf};

use base64::Engine as _;
use serde_json::json;
use tracing::warn;
use xcl_core::AppError;

use crate::isolation::{settings_at, write_settings};
use crate::launch::LaunchPhase;
use crate::local;
use crate::AppState;

impl AppState {
    /// 删掉一个本机版本目录。
    ///
    /// 拒绝的三种情况：版本不存在、这个版本正在下载、这个版本正在运行。
    /// 后两种不是因为做不到，而是删了之后会被下载重新写回来 / Windows 上文件还占着。
    pub fn delete_version(&self, version_id: &str) -> Result<(), AppError> {
        validate_version_id(version_id)?;

        let dir = self.layout.version_dir(version_id);
        if !dir.is_dir() {
            return Err(
                AppError::not_found("版本不存在", format!("找不到 {}", dir.display()))
                    .with_hint("刷新一下版本列表再看"),
            );
        }

        self.ensure_idle(version_id, "删掉的文件会被它重新写回来")?;

        fs::remove_dir_all(&dir).map_err(|err| AppError::io(&err, "删除版本目录"))?;
        Ok(())
    }

    /// 改版本名：目录名、JSON 里的 `id`、子版本的 `inheritsFrom` 一起改。
    ///
    /// 顺序是先在最外层写好新 JSON（内容里的 id 已经改掉），再换目录名，
    /// 最后修子版本。中途失败会把 JSON 名字还原，尽量不留半截状态。
    pub fn rename_version(&self, old_id: &str, new_id: &str) -> Result<String, AppError> {
        validate_version_id(old_id)?;
        validate_version_id(new_id)?;
        let new_id = new_id.trim();
        if old_id == new_id {
            return Ok(new_id.to_string());
        }
        self.ensure_idle(old_id, "改名时它正被占用")?;
        rename_on_disk(&self.layout, old_id, new_id)?;
        Ok(new_id.to_string())
    }

    /// 写版本描述（传空串等于清掉）。
    pub fn set_version_description(
        &self,
        version_id: &str,
        description: &str,
    ) -> Result<(), AppError> {
        self.patch_settings(version_id, |settings| {
            let trimmed = description.trim();
            settings.description = (!trimmed.is_empty()).then(|| trimmed.to_string());
        })
    }

    /// 收藏 / 取消收藏。
    pub fn set_version_favorite(
        &self,
        version_id: &str,
        favorite: bool,
    ) -> Result<(), AppError> {
        self.patch_settings(version_id, |settings| {
            settings.favorite = favorite.then_some(true);
        })
    }

    /// 存自定义图标。只收 PNG（前端已经用 canvas 缩到 128×128）。
    pub fn set_version_icon(&self, version_id: &str, data_url: &str) -> Result<(), AppError> {
        self.require_version_dir(version_id)?;

        let payload = data_url
            .split_once(',')
            .map(|(_, data)| data)
            .ok_or_else(|| AppError::validation("图标数据不合法", "缺少 data URL 前缀"))?;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(payload)
            .map_err(|err| AppError::validation("图标数据不合法", err.to_string()))?;
        if bytes.len() > MAX_ICON_BYTES {
            return Err(AppError::validation(
                "图标太大了",
                format!("上限 {} KB", MAX_ICON_BYTES / 1024),
            ));
        }
        if !bytes.starts_with(&[0x89, b'P', b'N', b'G']) {
            return Err(AppError::validation("图标格式不支持", "只收 PNG"));
        }

        let path = self.icon_path(version_id);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|err| AppError::io(&err, "创建版本设置目录"))?;
        }
        fs::write(&path, bytes).map_err(|err| AppError::io(&err, "写入版本图标"))?;
        Ok(())
    }

    /// 恢复默认图标。
    pub fn clear_version_icon(&self, version_id: &str) -> Result<(), AppError> {
        self.require_version_dir(version_id)?;
        match fs::remove_file(self.icon_path(version_id)) {
            Ok(()) => Ok(()),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(err) => Err(AppError::io(&err, "删除版本图标")),
        }
    }

    // ---- 内部 ----

    fn icon_path(&self, version_id: &str) -> PathBuf {
        self.layout
            .version_settings_dir(version_id)
            .join("icon.png")
    }

    pub(crate) fn require_version_dir(&self, version_id: &str) -> Result<(), AppError> {
        validate_version_id(version_id)?;
        let dir = self.layout.version_dir(version_id);
        if !dir.is_dir() {
            return Err(AppError::not_found(
                "版本不存在",
                format!("找不到 {}", dir.display()),
            ));
        }
        Ok(())
    }

    /// 版本正在跑的时候不要动它的文件。
    fn ensure_idle(&self, version_id: &str, reason: &str) -> Result<(), AppError> {
        if self.tasks().running_for(version_id).is_some() {
            return Err(AppError::validation(
                "这个版本正在下载",
                format!("下载任务还没结束，{reason}"),
            )
            .with_hint("先在右下角的下载抽屉里取消任务"));
        }

        let running = self.launch_sessions().into_iter().any(|session| {
            session.version_id == version_id
                && matches!(session.phase, LaunchPhase::Preparing | LaunchPhase::Running)
        });
        if running {
            return Err(AppError::validation(
                "这个版本正在运行",
                format!("游戏还开着，{reason}"),
            )
            .with_hint("先把游戏关掉"));
        }
        Ok(())
    }

    /// 读-改-写 `XCL/instance.toml`。改完没剩任何字段就把文件删掉。
    fn patch_settings(
        &self,
        version_id: &str,
        edit: impl FnOnce(&mut crate::isolation::InstanceSettings),
    ) -> Result<(), AppError> {
        self.require_version_dir(version_id)?;
        let path = self.layout.version_settings_file(version_id);
        let mut settings = settings_at(&path);
        edit(&mut settings);
        write_settings(
            &self.layout,
            version_id,
            if settings.is_empty() { None } else { Some(settings) },
        )
    }
}

fn write_json_atomic(path: &Path, text: &str) -> std::io::Result<()> {
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, text)?;
    fs::rename(&tmp, path)
}

/// 真正在磁盘上改名：目录、JSON 文件名、JSON 里的 `id`、子版本的 `inheritsFrom`。
///
/// 顺序是先在最外层写好新 JSON（内容里的 id 已经改掉），再换目录名，
/// 最后修子版本。中途换目录失败会把 JSON 名字还原，尽量不留半截状态。
fn rename_on_disk(
    layout: &xcl_core::PathLayout,
    old_id: &str,
    new_id: &str,
) -> Result<(), AppError> {
    let old_dir = layout.version_dir(old_id);
    let new_dir = layout.version_dir(new_id);
    if !old_dir.is_dir() {
        return Err(AppError::not_found(
            "版本不存在",
            format!("找不到 {}", old_dir.display()),
        ));
    }
    if new_dir.exists() {
        return Err(AppError::validation(
            "这个名字已经有人用了",
            format!("versions/{new_id} 已经存在"),
        )
        .with_hint("换一个名字"));
    }

    let old_json = find_version_json(&old_dir, old_id);

    // 改完名之后，目录里再也看不出 MC 版本号了 —— 提前把它算出来，
    // 重命名成功后写进新目录的 `XCL/instance.toml`（「下载 Mod」页要靠它配对）。
    let game_version = settings_at(&layout.version_settings_file(old_id))
        .game_version
        .or_else(|| {
            let inherited = old_json
                .as_deref()
                .and_then(|path| local::read_detail(path).ok())
                .and_then(|detail| detail.inherits_from);
            local::guess_game_version(old_id, inherited.as_deref())
        });
    let new_json = old_dir.join(format!("{new_id}.json"));
    let rewrote_json = match old_json.as_deref() {
        Some(old_path) => {
            let text =
                fs::read_to_string(old_path).map_err(|err| AppError::io(&err, "读取版本 JSON"))?;
            let mut value: serde_json::Value = serde_json::from_str(&text)
                .map_err(|err| AppError::parse("版本 JSON 解析失败", err.to_string()))?;
            value["id"] = json!(new_id);
            let pretty = serde_json::to_string_pretty(&value)
                .map_err(|err| AppError::parse("版本 JSON 序列化失败", err.to_string()))?;
            fs::write(&new_json, pretty).map_err(|err| AppError::io(&err, "写入版本 JSON"))?;
            if old_path != new_json {
                fs::remove_file(old_path).map_err(|err| AppError::io(&err, "删除旧版本 JSON"))?;
            }
            Some(old_path.to_path_buf())
        }
        None => None,
    };

    if let Err(err) = fs::rename(&old_dir, &new_dir) {
        // 目录没换成，JSON 名字得退回去，否则这个版本会变成「没有描述文件」
        if let Some(old_path) = rewrote_json {
            let _ = fs::rename(&new_json, &old_path);
        }
        return Err(AppError::io(&err, "改名版本目录"));
    }

    repoint_children(layout, old_id, new_id);

    // 改名成功了才写：失败时旧目录还在，不能顺手留下一个错位的标记
    if let Some(game) = game_version {
        if let Err(err) = crate::isolation::ensure_game_version(layout, new_id, &game) {
            warn!(error = %err, "改名后记录原版版本号失败");
        }
    }
    Ok(())
}

/// 子版本的 `inheritsFrom` 指向新名字。失败只记日志 —— 改名本身已经成功了。
fn repoint_children(layout: &xcl_core::PathLayout, old_id: &str, new_id: &str) {
    let Ok(entries) = fs::read_dir(layout.versions_dir()) else {
        return;
    };
    for entry in entries.flatten() {
        let dir = entry.path();
        if !dir.is_dir() {
            continue;
        }
        let Some(name) = dir.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let json_path = dir.join(format!("{name}.json"));
        if !json_path.is_file() {
            continue;
        }
        let Ok(text) = fs::read_to_string(&json_path) else {
            continue;
        };
        let Ok(mut value) = serde_json::from_str::<serde_json::Value>(&text) else {
            continue;
        };
        if value.get("inheritsFrom").and_then(|v| v.as_str()) != Some(old_id) {
            continue;
        }
        value["inheritsFrom"] = json!(new_id);
        let Ok(pretty) = serde_json::to_string_pretty(&value) else {
            continue;
        };
        if let Err(err) = write_json_atomic(&json_path, &pretty) {
            warn!(path = %json_path.display(), error = %err, "改子版本 inheritsFrom 失败");
        }
    }
}

/// 版本 JSON 一般叫 `<id>.json`，但整合包/手动装的版本可能叫别的名字，
/// 所以退了再找目录下任意一个 `.json`。
fn find_version_json(dir: &Path, id: &str) -> Option<PathBuf> {
    let direct = dir.join(format!("{id}.json"));
    if direct.is_file() {
        return Some(direct);
    }
    fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|entry| entry.path())
        .find(|path| {
            path.is_file() && path.extension().and_then(|ext| ext.to_str()) == Some("json")
        })
}

/// 图标上限：前端缩到 128×128 之后通常十几 KB，这里只兜底。
const MAX_ICON_BYTES: usize = 512 * 1024;

/// 版本 id 直接来自目录名，这里只做一件事：别让它跳出 `versions/`。
///
/// 中文、空格、点号都是合法版本名，所以白名单太窄反而会误伤；
/// 只拒掉路径分隔符和 `.` / `..`。
pub fn validate_version_id(id: &str) -> Result<(), AppError> {
    let bad = id.trim().is_empty()
        || id == "."
        || id == ".."
        || id.contains('/')
        || id.contains('\\')
        || id.contains(':');
    if bad {
        return Err(AppError::validation("版本名不合法", format!("「{id}」不是一个版本目录名")));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use xcl_core::PathLayout;

    fn temp_layout(tag: &str) -> PathLayout {
        let dir = std::env::temp_dir().join(format!("xcl-ver-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        PathLayout::new(dir.clone(), dir.join("cache"))
    }

    #[test]
    fn rejects_path_escapes() {
        for id in ["", " ", ".", "..", "../x", "a/b", "a\\b", "C:1.20"] {
            assert!(validate_version_id(id).is_err(), "{id} 应该被拒绝");
        }
    }

    #[test]
    fn accepts_normal_names() {
        for id in ["1.12.2", "1.20.1-forge-47.2.0", "我的整合包", "1.7.10 OptiFine"] {
            assert!(validate_version_id(id).is_ok(), "{id} 应该被接受");
        }
    }

    fn write_version(layout: &PathLayout, id: &str, inherits: Option<&str>) {
        let dir = layout.version_dir(id);
        fs::create_dir_all(&dir).unwrap();
        let json = serde_json::json!({ "id": id, "inheritsFrom": inherits });
        fs::write(dir.join(format!("{id}.json")), json.to_string()).unwrap();
    }

    #[test]
    fn rename_moves_dir_json_and_children() {
        let layout = temp_layout("rename");
        write_version(&layout, "1.20.1", None);
        write_version(&layout, "fabric-1.20.1", Some("1.20.1"));
        // 版本设置跟着目录一起搬
        fs::create_dir_all(layout.version_settings_dir("1.20.1")).unwrap();
        fs::write(layout.version_settings_file("1.20.1"), "favorite = true\n").unwrap();

        rename_on_disk(&layout, "1.20.1", "生存服").unwrap();

        assert!(!layout.version_dir("1.20.1").exists());
        assert!(layout.version_dir("生存服").join("生存服.json").is_file());
        assert!(layout.version_settings_file("生存服").is_file());

        let renamed: serde_json::Value = serde_json::from_str(
            &fs::read_to_string(layout.version_dir("生存服").join("生存服.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(renamed["id"], "生存服");

        let child: serde_json::Value = serde_json::from_str(
            &fs::read_to_string(layout.version_dir("fabric-1.20.1").join("fabric-1.20.1.json"))
                .unwrap(),
        )
        .unwrap();
        assert_eq!(child["inheritsFrom"], "生存服");

        // 改名后目录名里没有 MC 版本号了，设置文件里必须补上
        // （「下载 Mod」页靠它配对；否则这个版本会从候选列表里消失）
        let settings = settings_at(&layout.version_settings_file("生存服"));
        assert_eq!(
            settings.game_version.as_deref(),
            Some("1.20.1"),
            "改名后要记住它属于哪个原版"
        );

        let _ = fs::remove_dir_all(layout.root());
    }

    #[test]
    fn rename_refuses_existing_target() {
        let layout = temp_layout("conflict");
        write_version(&layout, "a", None);
        write_version(&layout, "b", None);

        assert!(rename_on_disk(&layout, "a", "b").is_err());
        assert!(layout.version_dir("a").is_dir());

        let _ = fs::remove_dir_all(layout.root());
    }
}
