//! 自定义背景图。
//!
//! 选的图会被复制进 `data/background/`，配置里只记文件名 —— 原文件挪走、
//! 删掉都不影响，也不会把磁盘上的绝对路径写进配置。
//!
//! 目录里只保留一张：换图时先清空再写，避免历史图片越攒越多。

use std::fs;
use std::path::{Path, PathBuf};

use base64::Engine;
use xcl_core::{AppError, PathLayout};

use crate::AppState;

/// 支持的扩展名 → MIME。
const TYPES: [(&str, &str); 6] = [
    ("png", "image/png"),
    ("jpg", "image/jpeg"),
    ("jpeg", "image/jpeg"),
    ("webp", "image/webp"),
    ("bmp", "image/bmp"),
    ("gif", "image/gif"),
];

/// 单张上限。再大 base64 过 IPC 会把内存和首帧拖垮。
const MAX_BYTES: u64 = 20 * 1024 * 1024;

pub fn dir(layout: &PathLayout) -> PathBuf {
    layout.root().join("background")
}

fn mime_for(name: &str) -> Option<&'static str> {
    let ext = name.rsplit_once('.')?.1.to_ascii_lowercase();
    TYPES
        .iter()
        .find(|(known, _)| *known == ext)
        .map(|(_, mime)| *mime)
}

/// 和 `config::is_background_name` 同一套规则：纯文件名 + 白名单扩展名。
pub fn is_valid_name(name: &str) -> bool {
    !name.is_empty()
        && !name.contains(['/', '\\'])
        && name != ".."
        && mime_for(name).is_some()
}

/// 复制一张图进目录，并清掉目录里的其它文件。返回存下来的文件名。
pub fn set_from_file(layout: &PathLayout, source: &Path) -> Result<String, AppError> {
    let ext = source
        .extension()
        .and_then(|value| value.to_str())
        .map(|value| value.to_ascii_lowercase())
        .ok_or_else(|| {
            AppError::validation("这个文件没有扩展名", source.display().to_string())
        })?;
    let name = format!("background.{ext}");
    if mime_for(&name).is_none() {
        return Err(AppError::validation(
            "不支持的图片格式",
            "支持 png / jpg / webp / bmp / gif",
        )
        .with_hint("换一张图片试试"));
    }

    let size = fs::metadata(source)
        .map_err(|e| AppError::io(&e, "读取图片"))?
        .len();
    if size > MAX_BYTES {
        return Err(AppError::validation(
            "图片太大了",
            format!("上限 {} MB", MAX_BYTES / 1024 / 1024),
        ));
    }

    let target = dir(layout);
    fs::create_dir_all(&target).map_err(|e| AppError::io(&e, "创建背景图目录"))?;
    clear_files(&target)?;
    fs::copy(source, target.join(&name)).map_err(|e| AppError::io(&e, "复制背景图"))?;
    Ok(name)
}

/// 读成 data URL。文件不在（换机器、手动删了）就返回 `None`，不算错误。
pub fn data_url(layout: &PathLayout, name: &str) -> Result<Option<String>, AppError> {
    if !is_valid_name(name) {
        return Ok(None);
    }
    let path = dir(layout).join(name);
    if !path.is_file() {
        return Ok(None);
    }
    let bytes = fs::read(&path).map_err(|e| AppError::io(&e, "读取背景图"))?;
    let mime = mime_for(name).unwrap_or("application/octet-stream");
    Ok(Some(format!(
        "data:{mime};base64,{}",
        base64::engine::general_purpose::STANDARD.encode(bytes)
    )))
}

/// 删掉目录里的所有文件（目录本身留着）。
pub fn clear(layout: &PathLayout) -> Result<(), AppError> {
    let target = dir(layout);
    if target.is_dir() {
        clear_files(&target)?;
    }
    Ok(())
}

fn clear_files(dir: &Path) -> Result<(), AppError> {
    for entry in fs::read_dir(dir).map_err(|e| AppError::io(&e, "读取背景图目录"))? {
        let entry = entry.map_err(|e| AppError::io(&e, "读取背景图目录"))?;
        let path = entry.path();
        if path.is_file() {
            fs::remove_file(&path).map_err(|e| AppError::io(&e, "清理旧背景图"))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use xcl_core::path::PathLayout;

    fn temp_layout(tag: &str) -> PathLayout {
        let dir = std::env::temp_dir().join(format!("xcl-bg-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        PathLayout::new(dir.clone(), dir.join("cache"))
    }

    #[test]
    fn keeps_only_one_image() {
        let layout = temp_layout("one");
        fs::create_dir_all(layout.root()).unwrap();
        let png = layout.root().join("a.png");
        let jpg = layout.root().join("b.jpg");
        fs::write(&png, b"\x89PNG").unwrap();
        fs::write(&jpg, b"\xff\xd8").unwrap();

        assert_eq!(set_from_file(&layout, &png).unwrap(), "background.png");
        assert_eq!(fs::read_dir(dir(&layout)).unwrap().count(), 1);

        // 换一张：旧的要被清掉，目录里始终只留一张
        assert_eq!(set_from_file(&layout, &jpg).unwrap(), "background.jpg");
        let files = fs::read_dir(dir(&layout)).unwrap().count();
        assert_eq!(files, 1, "目录里只该留一张图");

        let url = data_url(&layout, "background.jpg").unwrap().unwrap();
        assert!(url.starts_with("data:image/jpeg;base64,"));

        clear(&layout).unwrap();
        assert_eq!(fs::read_dir(dir(&layout)).unwrap().count(), 0);
        let _ = fs::remove_dir_all(layout.root());
    }

    #[test]
    fn rejects_paths_and_unknown_extensions() {
        assert!(!is_valid_name("../x.png"));
        assert!(!is_valid_name("a/b.png"));
        assert!(!is_valid_name("background.txt"));
        assert!(is_valid_name("background.png"));
    }
}

impl AppState {
    /// 当前背景图的 data URL。没设置 / 文件丢失都返回 `None`。
    pub fn background_data_url(&self) -> Result<Option<String>, AppError> {
        match self.config().ui.background_image {
            Some(name) => data_url(&self.layout, &name),
            None => Ok(None),
        }
    }

    /// 换背景图：复制进自己的目录、只留一张，并把文件名写进配置。
    /// 返回新图对应的 data URL（前端拿它可以立刻换上）。
    pub fn set_background(&self, source: &Path) -> Result<String, AppError> {
        let name = set_from_file(&self.layout, source)?;
        self.patch_config(serde_json::json!({ "ui": { "backgroundImage": name } }))?;
        Ok(data_url(&self.layout, &name)?.unwrap_or_default())
    }

    /// 移除背景图：清空目录，配置回到「没有背景图」。
    pub fn clear_background(&self) -> Result<(), AppError> {
        clear(&self.layout)?;
        self.patch_config(serde_json::json!({ "ui": { "backgroundImage": null } }))?;
        Ok(())
    }
}
