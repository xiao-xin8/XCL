//! 联机组件清单。
//!
//! 清单由我们自己托管（`cloud/connect/manifest.json`），格式很小，但有几个点必须校验，
//! 否则会出现「下载成功但装不起来」这种最难查的故障：
//!
//! - 至少一个下载地址，且主地址可用；
//! - sha256 必须是 64 位十六进制 —— 校验值是唯一的完整性依据，不能空着；
//! - size 必须大于 0，否则进度条会算出 0/0；
//! - 包里必须同时含 `easytier-core.exe` 与 `easytier-cli.exe`，缺一个就跑不起来。
//!
//! 下载下来的文件名会被重命名成 `XCL联机模块.exe` / `XCL联机模块 CLI.exe`，
//! 改的是文件名不是内容，因此 sha256 不受影响，也符合 LGPL
//! 对「未修改二进制」的要求。

use xcl_core::AppError;

/// 主程序在包里的原名。
pub const CORE_FILE: &str = "easytier-core.exe";
/// 命令行工具在包里的原名。
pub const CLI_FILE: &str = "easytier-cli.exe";
/// 主程序装好后的名字。
pub const CORE_INSTALLED: &str = "XCL联机模块.exe";
/// 命令行工具装好后的名字（注意中间有空格）。
pub const CLI_INSTALLED: &str = "XCL联机模块 CLI.exe";

/// 组件清单。
#[derive(Debug, Clone, PartialEq)]
pub struct ConnectManifest {
    pub product: String,
    pub version: String,
    pub build: Option<String>,
    pub platform: String,
    pub archive: Archive,
    pub files: Vec<ModuleFile>,
}

/// 压缩包本身。
#[derive(Debug, Clone, PartialEq)]
pub struct Archive {
    /// 首选下载地址（国内 CDN）。
    pub primary: String,
    /// 备用地址，按顺序回退。
    pub fallbacks: Vec<String>,
    pub size: u64,
    pub sha256: String,
    /// 是否允许分片。组件包体积小、线路总带宽有限，默认关闭。
    pub allow_split: bool,
}

/// 包里每个文件的期望值，解压后逐个核对。
#[derive(Debug, Clone, PartialEq)]
pub struct ModuleFile {
    pub name: String,
    pub size: u64,
    pub sha256: String,
    /// 缺了就不能用（`wintun.dll` 是可选）。
    pub required: bool,
}

impl ConnectManifest {
    /// 解析清单。
    pub fn parse(json: &str) -> Result<Self, AppError> {
        let root: serde_json::Value = serde_json::from_str(json.trim()).map_err(|err| {
            AppError::parse("联机组件清单看不懂", format!("不是合法 JSON：{err}"))
        })?;
        let obj = root.as_object().ok_or_else(|| {
            AppError::parse("联机组件清单格式不对", "顶层应当是对象")
        })?;

        let product = str_field(obj, "product").unwrap_or_else(|| "easytier".to_string());
        let version = str_field(obj, "version").ok_or_else(|| missing("version"))?;
        let build = str_field(obj, "build");
        let platform = str_field(obj, "platform").unwrap_or_else(|| "win-x64".to_string());

        if platform != "win-x64" {
            return Err(AppError::unsupported(
                "这个组件包不适用于当前系统",
                format!("清单里写的是 {platform}，XCL 只支持 win-x64"),
            ));
        }

        let archive_obj = obj
            .get("archive")
            .and_then(|v| v.as_object())
            .ok_or_else(|| missing("archive"))?;

        let primary = str_field(archive_obj, "primary")
            .or_else(|| str_field(archive_obj, "url"))
            .ok_or_else(|| missing("archive.primary"))?;
        let fallbacks = archive_obj
            .get("fallbacks")
            .and_then(|v| v.as_array())
            .map(|list| {
                list.iter()
                    .filter_map(|item| item.as_str())
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect()
            })
            .unwrap_or_default();
        let size = u64_field(archive_obj, "size").ok_or_else(|| missing("archive.size"))?;
        if size == 0 {
            return Err(AppError::parse(
                "联机组件清单里的体积是 0",
                "进度会因此算不出来",
            ));
        }
        let sha256 = str_field(archive_obj, "sha256").ok_or_else(|| missing("archive.sha256"))?;
        expect_sha256(&sha256, "archive.sha256")?;
        let allow_split = archive_obj
            .get("allowSplit")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let files = obj
            .get("files")
            .and_then(|v| v.as_array())
            .map(|list| {
                list.iter()
                    .filter_map(|item| item.as_object())
                    .filter_map(|file| {
                        let name = str_field(file, "name")?;
                        let size = u64_field(file, "size").unwrap_or(0);
                        let sha256 = str_field(file, "sha256").unwrap_or_default();
                        let required = file
                            .get("required")
                            .and_then(|v| v.as_bool())
                            .unwrap_or(true);
                        Some(ModuleFile {
                            name,
                            size,
                            sha256,
                            required,
                        })
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();

        let manifest = Self {
            product,
            version,
            build,
            platform,
            archive: Archive {
                primary,
                fallbacks,
                size,
                sha256,
                allow_split,
            },
            files,
        };
        manifest.check_files()?;
        Ok(manifest)
    }

    /// 按优先级排好的下载地址。
    pub fn urls(&self) -> Vec<String> {
        let mut out = Vec::with_capacity(1 + self.archive.fallbacks.len());
        out.push(self.archive.primary.clone());
        out.extend(self.archive.fallbacks.iter().cloned());
        out
    }

    /// 包名，直接用地址最后一段，省得清单里再写一次。
    pub fn archive_name(&self) -> String {
        self.archive
            .primary
            .rsplit('/')
            .next()
            .filter(|s| !s.is_empty())
            .unwrap_or("easytier.zip")
            .to_string()
    }

    /// 包里必带的两个可执行文件是否齐全。
    fn check_files(&self) -> Result<(), AppError> {
        if self.files.is_empty() {
            return Ok(()); // 老清单可能没有 files 段，交给解压后再校验
        }
        for name in [CORE_FILE, CLI_FILE] {
            let found = self.files.iter().any(|f| f.name == name);
            if !found {
                return Err(AppError::parse(
                    "联机组件包不完整",
                    format!("清单里没有 {name}"),
                )
                .with_hint("可能是发布时打错了包，请联系维护者"));
            }
        }
        Ok(())
    }
}

/// 解压时该把包里的文件落到什么名字。
///
/// 只有两个可执行文件改名，其余（`Packet.dll`、`NOTICE.txt` 等）原样保留 ——
/// 动态库是按原名被加载的，改了反而起不来。
pub fn installed_name(archive_name: &str) -> &str {
    match archive_name {
        CORE_FILE => CORE_INSTALLED,
        CLI_FILE => CLI_INSTALLED,
        other => other,
    }
}

fn str_field(obj: &serde_json::Map<String, serde_json::Value>, key: &str) -> Option<String> {
    obj.get(key)
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn u64_field(obj: &serde_json::Map<String, serde_json::Value>, key: &str) -> Option<u64> {
    match obj.get(key) {
        Some(serde_json::Value::Number(num)) => num.as_u64(),
        Some(serde_json::Value::String(raw)) => raw.trim().parse().ok(),
        _ => None,
    }
}

fn expect_sha256(value: &str, label: &str) -> Result<(), AppError> {
    if value.len() != 64 || !value.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(AppError::parse(
            "联机组件清单里的校验值不合法",
            format!("{label} 应当是 64 位十六进制"),
        )
        .with_hint("联系维护者重新发布清单"));
    }
    Ok(())
}

fn missing(field: &str) -> AppError {
    AppError::parse("联机组件清单缺少必要字段", format!("少了 {field}"))
        .with_hint("清单可能没上传完整，稍后重试")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 线上清单的原文（2026-11-01 部署，字段顺序一致）。
    const REAL: &str = r#"{
      "schema": 1,
      "product": "easytier",
      "version": "2.6.4",
      "build": "8428a89d",
      "platform": "win-x64",
      "archive": {
        "primary": "https://cdn.example.com/app/dl/abc",
        "fallbacks": ["https://downloads.example.com/connect/easytier-2.6.4-win-x64.zip"],
        "size": 14124725,
        "sha256": "58a51a101d3ef5c8cfcfd77a8322e41b936461c7351af33203b47df8fbbbbcfb",
        "allowSplit": false,
        "redirects": true
      },
      "files": [
        {"name":"easytier-core.exe","size":24395776,"sha256":"da7eb2d2","required":true},
        {"name":"easytier-cli.exe","size":9973248,"sha256":"d8783e85","required":true},
        {"name":"Packet.dll","size":220032,"sha256":"c7c03a87","required":true},
        {"name":"wintun.dll","size":427552,"sha256":"e5da8447","required":false}
      ]
    }"#;

    #[test]
    fn parses_deployed_manifest() {
        let m = ConnectManifest::parse(REAL).expect("线上清单必须能解析");
        assert_eq!(m.version, "2.6.4");
        assert_eq!(m.platform, "win-x64");
        assert_eq!(m.archive.size, 14_124_725);
        assert!(!m.archive.allow_split, "组件包默认不分片");
        assert_eq!(m.urls().len(), 2, "主源 + 一个备源");
        assert_eq!(m.archive_name(), "abc"); // 分享地址最后一段不是文件名
        assert_eq!(m.files.len(), 4);
        assert!(!m.files[3].required, "wintun 是可选");
    }

    #[test]
    fn urls_keep_priority_order() {
        let m = ConnectManifest::parse(REAL).unwrap();
        let urls = m.urls();
        assert!(urls[0].contains("cdn.example.com"), "主源排第一");
        assert!(urls[1].contains("downloads.example.com"));
    }

    #[test]
    fn rejects_invalid_checksum() {
        let broken = REAL.replace(
            "58a51a101d3ef5c8cfcfd77a8322e41b936461c7351af33203b47df8fbbbbcfb",
            "abc",
        );
        let err = ConnectManifest::parse(&broken).unwrap_err();
        assert!(err.title.contains("校验值"));
    }

    #[test]
    fn rejects_missing_core_entry() {
        let broken = REAL.replace("easytier-core.exe", "something-else.exe");
        let err = ConnectManifest::parse(&broken).unwrap_err();
        assert!(err.title.contains("不完整"));
        assert!(err.hint.is_some());
    }

    #[test]
    fn rejects_zero_size_and_wrong_platform() {
        let zero = REAL.replace("\"size\": 14124725", "\"size\": 0");
        assert!(ConnectManifest::parse(&zero).is_err());

        let platform = REAL.replace("win-x64", "linux-x64");
        let err = ConnectManifest::parse(&platform).unwrap_err();
        assert_eq!(err.code, xcl_core::ErrorCode::Unsupported);
    }

    #[test]
    fn rejects_garbage() {
        assert!(ConnectManifest::parse("").is_err());
        assert!(ConnectManifest::parse("[]").is_err());
        assert!(ConnectManifest::parse("{\"version\":\"1\"}").is_err());
    }

    #[test]
    fn renames_only_executables() {
        assert_eq!(installed_name("easytier-core.exe"), "XCL联机模块.exe");
        assert_eq!(installed_name("easytier-cli.exe"), "XCL联机模块 CLI.exe");
        // 动态库必须保持原名，否则加载失败
        assert_eq!(installed_name("Packet.dll"), "Packet.dll");
        assert_eq!(installed_name("wintun.dll"), "wintun.dll");
        assert_eq!(installed_name("NOTICE.txt"), "NOTICE.txt");
    }
}
