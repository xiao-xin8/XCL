//! 统一错误模型。
//!
//! 设计目标：前端不需要判断错误类型。Rust 侧把「怎么解释、能不能重试、
//! 该给什么建议」全部算好，前端只管把 `title` / `message` / `hint` 画出来。
//!
//! 因此这里的字段全是「可直接展示」的成品文案，而不是技术堆栈信息。

use serde::{Deserialize, Serialize};
use std::fmt;

/// 错误分类。前端可用于图标/配色区分，但不应该用它来做流程判断。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ErrorCode {
    /// 网络请求失败、超时、连接被重置
    Network,
    /// 文件或版本不存在
    NotFound,
    /// 文件系统读写失败
    Io,
    /// JSON / TOML 等解析失败
    Parse,
    /// 功能上不支持（版本过旧、平台不符等）
    Unsupported,
    /// 输入不合法（玩家名、服务器地址等表单校验失败）
    Validation,
    /// 手动取消，通常不该弹错误框
    Canceled,
    /// 登录态无效（令牌过期、会话被吊销、账号被封）。
    /// 单独分一类是为了让调用方能「静默地退回未登录」，而不是弹一个网络错误。
    Unauthorized,
    Unknown,
}

/// 面向看的错误。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppError {
    pub code: ErrorCode,
    /// 一句话标题，例如「无法连接下载源」
    pub title: String,
    /// 详细说明，可含路径、状态码等
    pub message: String,
    /// 建议动作，例如「试试在设置里切换下载源」
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
    /// 是否值得重试。前端据此决定要不要显示「重试」按钮。
    pub retryable: bool,
}

impl AppError {
    pub fn new(code: ErrorCode, title: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code,
            title: title.into(),
            message: message.into(),
            hint: None,
            retryable: false,
        }
    }

    pub fn network(title: impl Into<String>, message: impl Into<String>) -> Self {
        Self::new(ErrorCode::Network, title, message).with_retryable(true)
    }

    pub fn not_found(title: impl Into<String>, message: impl Into<String>) -> Self {
        Self::new(ErrorCode::NotFound, title, message)
    }

    pub fn parse(title: impl Into<String>, message: impl Into<String>) -> Self {
        Self::new(ErrorCode::Parse, title, message)
    }

    pub fn unsupported(title: impl Into<String>, message: impl Into<String>) -> Self {
        Self::new(ErrorCode::Unsupported, title, message)
    }

    /// 输入校验失败。不可重试 —— 重试同样的输入还是错。
    pub fn validation(title: impl Into<String>, message: impl Into<String>) -> Self {
        Self::new(ErrorCode::Validation, title, message)
    }

    pub fn canceled() -> Self {
        Self::new(ErrorCode::Canceled, "已取消", "操作被取消")
    }

    /// 把 `std::io::Error` 翻译成带建议的错误提示。
    pub fn io(err: &std::io::Error, action: &str) -> Self {
        let message = format!("{action}：{err}");
        match err.kind() {
            std::io::ErrorKind::NotFound => {
                Self::new(ErrorCode::NotFound, "找不到文件", message)
                    .with_hint("确认该路径确实存在，或重新下载该文件")
            }
            std::io::ErrorKind::PermissionDenied => {
                Self::new(ErrorCode::Io, "没有权限", message)
                    .with_hint("尝试把程序放到可写目录，或以管理员身份运行")
            }
            _ => Self::new(ErrorCode::Io, "文件操作失败", message)
                .with_hint("稍后重试；若持续失败请检查磁盘空间与杀毒软件")
                .with_retryable(true),
        }
    }

    pub fn with_hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }

    pub fn with_retryable(mut self, retryable: bool) -> Self {
        self.retryable = retryable;
        self
    }
}

impl fmt::Display for AppError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}：{}", self.title, self.message)
    }
}

impl std::error::Error for AppError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn io_error_maps_kind_to_hint() {
        let not_found = std::io::Error::new(std::io::ErrorKind::NotFound, "missing");
        let err = AppError::io(&not_found, "读取版本文件");
        assert_eq!(err.code, ErrorCode::NotFound);
        assert!(err.hint.is_some());
        assert!(!err.retryable);

        let denied = std::io::Error::new(std::io::ErrorKind::PermissionDenied, "denied");
        let err = AppError::io(&denied, "写入配置");
        assert_eq!(err.code, ErrorCode::Io);
        assert!(err.hint.unwrap().contains("可写目录"));
    }

    #[test]
    fn network_error_is_retryable_by_default() {
        let err = AppError::network("无法连接下载源", "连接超时");
        assert!(err.retryable);
        assert_eq!(err.code, ErrorCode::Network);
    }

    #[test]
    fn serializes_to_camel_case() {
        let err = AppError::network("标题", "说明").with_hint("建议");
        let json = serde_json::to_string(&err).unwrap();
        assert!(json.contains("\"code\":\"network\""));
        assert!(json.contains("\"retryable\":true"));
        assert!(json.contains("\"hint\":\"建议\""));
    }

    #[test]
    fn hint_omitted_when_none() {
        let err = AppError::canceled();
        let json = serde_json::to_string(&err).unwrap();
        assert!(!json.contains("hint"));
    }
}
