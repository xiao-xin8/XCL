//! 配置读写。
//!
//! 缺文件 = 用默认值（不是错误）。解析失败才算错误，并给「删掉重建」的建议。

use std::fs;

use xcl_core::{AppError, Config, PathLayout};

#[cfg(test)]
use xcl_core::config::CONCURRENCY_AUTO;

pub fn load(layout: &PathLayout) -> Result<Config, AppError> {
    let path = layout.config_file();
    if !path.exists() {
        return Ok(Config::default());
    }

    let text = fs::read_to_string(&path).map_err(|e| AppError::io(&e, "读取配置文件"))?;
    let cfg: Config = toml::from_str(&text).map_err(|e| {
        AppError::parse(
            "配置文件格式错误",
            format!("{}：{e}", path.display()),
        )
        .with_hint("可以删除该文件，程序启动时会重建默认配置")
    })?;

    Ok(cfg.normalized())
}

/// 序列化成可读的 TOML（设置页「导出」用）。
pub fn to_toml(cfg: &Config) -> Result<String, AppError> {
    toml::to_string_pretty(cfg).map_err(|e| AppError::parse("配置序列化失败", e.to_string()))
}

/// 解析导入的 TOML。
///
/// 缺字段取默认值（和读配置同一个规则），多出来的字段忽略；只有类型不对才算错。
pub fn from_toml(text: &str) -> Result<Config, AppError> {
    // 先认顶层标记。所有字段都有默认值，不先看这一眼的话，随便一个合法的空 TOML
    // （甚至只有 `[foo]` 的无关文件）都会解析成「全默认配置」—— 导入等于把
    // 原有的设置全抹掉。`schemaVersion` 是我们导出时必写的字段，拿它当身份证。
    let raw: toml::Value = toml::from_str(text).map_err(|e| {
        AppError::parse("配置文件格式错误", e.to_string())
            .with_hint("请选择由 XCL 导出的设置文件")
    })?;
    if raw.get("schemaVersion").is_none() {
        return Err(AppError::validation(
            "这不是 XCL 的设置文件",
            "文件里没有 schemaVersion",
        )
        .with_hint("请选择由「导出设置」生成的 .toml 文件"));
    }

    let cfg: Config = toml::from_str(text).map_err(|e| {
        AppError::parse("配置文件格式错误", e.to_string())
            .with_hint("请选择由 XCL 导出的设置文件")
    })?;
    Ok(cfg.normalized())
}

pub fn save(layout: &PathLayout, cfg: &Config) -> Result<(), AppError> {
    let path = layout.config_file();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| AppError::io(&e, "创建配置目录"))?;
    }

    let text = toml::to_string_pretty(cfg)
        .map_err(|e| AppError::parse("配置序列化失败", e.to_string()))?;

    // 先写临时文件再改名，避免写一半断电留下半个损坏文件
    let tmp = path.with_extension("toml.tmp");
    fs::write(&tmp, text).map_err(|e| AppError::io(&e, "写入配置文件"))?;
    fs::rename(&tmp, &path).map_err(|e| AppError::io(&e, "替换配置文件"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use xcl_core::path::PathLayout;

    fn temp_layout(tag: &str) -> PathLayout {
        let dir = std::env::temp_dir().join(format!("xcl-cfg-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        PathLayout::new(dir.clone(), dir.join("cache"))
    }

    #[test]
    fn missing_file_yields_default() {
        let layout = temp_layout("missing");
        let cfg = load(&layout).unwrap();
        // 并发默认是「自动」，具体数值在运行期按最终源决定
        assert_eq!(cfg.download.concurrency, CONCURRENCY_AUTO);
        let _ = fs::remove_dir_all(layout.root());
    }

    #[test]
    fn save_then_load_roundtrip() {
        let layout = temp_layout("roundtrip");
        let mut cfg = Config::default();
        cfg.download.concurrency = 16;
        cfg.ui.last_tab = "download".to_string();

        save(&layout, &cfg).unwrap();
        let back = load(&layout).unwrap();

        assert_eq!(back.download.concurrency, 16);
        assert_eq!(back.ui.last_tab, "download");
        // 临时文件不应残留
        assert!(!layout.config_file().with_extension("toml.tmp").exists());
        let _ = fs::remove_dir_all(layout.root());
    }

    #[test]
    fn export_import_roundtrip() {
        let mut cfg = Config::default();
        cfg.download.retries = 5;
        cfg.launch.priority = xcl_core::config::ProcessPriority::High;
        cfg.log.level = xcl_core::config::LogLevel::Info;

        let text = to_toml(&cfg).unwrap();
        let back = from_toml(&text).unwrap();

        assert_eq!(back.download.retries, 5);
        assert_eq!(
            back.launch.priority,
            xcl_core::config::ProcessPriority::High
        );
        assert_eq!(back.log.level, xcl_core::config::LogLevel::Info);
    }

    #[test]
    fn import_rejects_garbage() {
        let err = from_toml("这不是 toml {{{").unwrap_err();
        assert!(err.hint.is_some(), "要提示选对文件");
    }

    #[test]
    fn import_rejects_toml_without_schema_version() {
        // 合法 TOML 但不像我们的设置：不能解析成「全默认」把原有设置抹掉
        let err = from_toml("[foo]\nbar = 1\n").unwrap_err();
        assert!(err.hint.is_some());
        assert!(err.title.contains("不是 XCL"), "实际：{}", err.title);
    }

    #[test]
    fn broken_file_reports_parse_error() {
        let layout = temp_layout("broken");
        fs::create_dir_all(layout.root()).unwrap();
        fs::write(layout.config_file(), "这不是 toml {{{").unwrap();

        let err = load(&layout).unwrap_err();
        assert!(err.hint.is_some());
        let _ = fs::remove_dir_all(layout.root());
    }
}
