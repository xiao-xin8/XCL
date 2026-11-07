//! helper 的命令行契约。
//!
//! ```text
//! XCL.exe --xcl-apply-update
//!         --pid <旧进程 PID>
//!         --target "<安装目录>\XCL.exe"
//!         --new "<安装目录>\.XCL-new-<version>.exe"
//!         --sha256 <清单里的 SHA-256>
//!         [--restart]
//! ```
//!
//! 这里是唯一解析这些参数的地方。父进程用 `Command::arg` 逐个传，
//! 所以不要做「按空格切分命令行」这种事 —— 路径里有空格就会断。

use std::ffi::OsString;
use std::path::PathBuf;

use xcl_core::{AppError, ErrorCode};

use crate::verify::is_sha256_hex;

/// 进入「替换模式」的标记。
pub const APPLY_FLAG: &str = "--xcl-apply-update";
/// 新版正常启动时带上的标记，用于清理与更新说明。
pub const UPDATED_FLAG: &str = "--xcl-updated";

/// helper 的入参。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApplyArgs {
    /// 旧进程 PID，helper 要等它退出
    pub pid: u32,
    /// 要被替换的 exe，绝对路径
    pub target: PathBuf,
    /// 已校验的新 exe，绝对路径
    pub new_file: PathBuf,
    /// 清单里的哈希，换文件前再确认一次
    pub sha256: String,
    /// 替换后是否立刻启动新版
    pub restart: bool,
}

/// 解析参数。
///
/// 返回 `Ok(None)` 表示「不是 apply 模式」，调用方应继续正常启动流程。
/// 返回 `Err` 表示是 apply 模式但参数不合法 —— 这种时候不能退回正常启动，
/// 否则 helper 会变成一个没有主窗口的僵尸进程。
pub fn parse_apply_args<I, S>(args: I) -> Result<Option<ApplyArgs>, AppError>
where
    I: IntoIterator<Item = S>,
    S: Into<OsString>,
{
    let args: Vec<OsString> = args.into_iter().map(Into::into).collect();
    if !args.iter().any(|a| a == APPLY_FLAG) {
        return Ok(None);
    }

    let pid_text = value_of(&args, "--pid")
        .ok_or_else(|| missing("--pid"))?
        .to_string_lossy()
        .to_string();
    let pid: u32 = pid_text
        .trim()
        .parse()
        .map_err(|_| bad_value("--pid", &pid_text, "应该是一个进程号"))?;
    if pid == 0 {
        return Err(bad_value("--pid", "0", "进程号不能是 0"));
    }

    let target = absolute_path(&args, "--target")?;
    let new_file = absolute_path(&args, "--new")?;

    let sha256 = value_of(&args, "--sha256")
        .ok_or_else(|| missing("--sha256"))?
        .to_string_lossy()
        .trim()
        .to_ascii_lowercase();
    if !is_sha256_hex(&sha256) {
        return Err(bad_value("--sha256", &sha256, "应该是 64 位十六进制"));
    }

    Ok(Some(ApplyArgs {
        pid,
        target,
        new_file,
        sha256,
        restart: args.iter().any(|a| a == "--restart"),
    }))
}

/// 带不带 `--xcl-updated`。新版启动时用它判断要不要显示更新说明。
pub fn has_updated_flag<I, S>(args: I) -> bool
where
    I: IntoIterator<Item = S>,
    S: Into<OsString>,
{
    args.into_iter().any(|a| a.into() == UPDATED_FLAG)
}

fn value_of(args: &[OsString], key: &str) -> Option<OsString> {
    let index = args.iter().position(|a| a == key)?;
    args.get(index + 1).cloned()
}

fn absolute_path(args: &[OsString], key: &str) -> Result<PathBuf, AppError> {
    let raw = value_of(args, key).ok_or_else(|| missing(key))?;
    let path = PathBuf::from(&raw);
    if !path.is_absolute() {
        return Err(bad_value(
            key,
            &raw.to_string_lossy(),
            "必须是绝对路径",
        ));
    }
    Ok(path)
}

fn missing(key: &str) -> AppError {
    AppError::new(
        ErrorCode::Validation,
        "更新参数不完整",
        format!("缺少 {key}"),
    )
}

fn bad_value(key: &str, value: &str, expect: &str) -> AppError {
    AppError::new(
        ErrorCode::Validation,
        "更新参数不合法",
        format!("{key} 的值「{value}」{expect}"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<OsString> {
        list.iter().map(OsString::from).collect()
    }

    fn full() -> Vec<&'static str> {
        vec![
            "XCL.exe",
            APPLY_FLAG,
            "--pid",
            "4321",
            "--target",
            "C:\\game\\XCL.exe",
            "--new",
            "C:\\game\\.XCL-new-0.2.0.exe",
            "--sha256",
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "--restart",
        ]
    }

    #[test]
    fn parses_full_invocation() {
        let parsed = parse_apply_args(args(&full())).unwrap().unwrap();
        assert_eq!(parsed.pid, 4321);
        assert_eq!(parsed.target, PathBuf::from("C:\\game\\XCL.exe"));
        assert_eq!(
            parsed.new_file,
            PathBuf::from("C:\\game\\.XCL-new-0.2.0.exe")
        );
        assert!(parsed.restart);
        assert_eq!(parsed.sha256.len(), 64);
    }

    #[test]
    fn normal_launch_is_not_apply_mode() {
        assert!(parse_apply_args(args(&["XCL.exe"])).unwrap().is_none());
        assert!(parse_apply_args(args(&["XCL.exe", "--xcl-updated"]))
            .unwrap()
            .is_none());
    }

    #[test]
    fn restart_is_optional() {
        let mut list = full();
        list.pop();
        assert!(!parse_apply_args(args(&list)).unwrap().unwrap().restart);
    }

    #[test]
    fn missing_value_lists_the_flag() {
        let mut list = full();
        // 去掉 --new 和它的值
        list.drain(6..8);
        let err = parse_apply_args(args(&list)).unwrap_err();
        assert_eq!(err.code, ErrorCode::Validation);
        assert!(err.message.contains("--new"));
    }

    #[test]
    fn bad_pid_is_rejected() {
        let list: Vec<&str> = full()
            .into_iter()
            .map(|a| if a == "4321" { "abc" } else { a })
            .collect();
        assert!(parse_apply_args(args(&list)).is_err());
    }

    #[test]
    fn zero_pid_is_rejected() {
        let list: Vec<&str> = full()
            .into_iter()
            .map(|a| if a == "4321" { "0" } else { a })
            .collect();
        assert!(parse_apply_args(args(&list)).is_err());
    }

    #[test]
    fn relative_paths_are_rejected() {
        let list: Vec<&str> = full()
            .into_iter()
            .map(|a| if a == "C:\\game\\XCL.exe" { "XCL.exe" } else { a })
            .collect();
        let err = parse_apply_args(args(&list)).unwrap_err();
        assert!(err.message.contains("绝对路径"));
    }

    #[test]
    fn bad_hash_is_rejected() {
        let list: Vec<&str> = full()
            .into_iter()
            .map(|a| if a.starts_with("aaaa") { "short" } else { a })
            .collect();
        assert!(parse_apply_args(args(&list)).is_err());
    }

    #[test]
    fn updated_flag_is_detected() {
        assert!(has_updated_flag(args(&["XCL.exe", UPDATED_FLAG])));
        assert!(!has_updated_flag(args(&["XCL.exe"])));
    }
}
