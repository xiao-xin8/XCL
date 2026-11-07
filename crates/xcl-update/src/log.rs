//! helper 的失败日志。
//!
//! apply 模式跑起来时主界面已经关了，没有任何地方能看到错误。
//! 所以它必须自己留一份痕迹，否则只会看到「点了更新，然后什么都没发生」。
//!
//! 位置跟缓存目录的规则保持一致（`%LOCALAPPDATA%\XCL\cache\update\apply.log`，
//! 见 `xcl_service::paths`）。这里不依赖 `xcl-service`，就重复几行路径推导 ——
//! 反过来依赖会让这个 crate 拖进 reqwest/tokio 一大串。

use std::io::Write;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

const APP_DIR_NAME: &str = "XCL";

/// 日志文件路径。
pub fn log_path() -> PathBuf {
    log_dir().join("apply.log")
}

fn log_dir() -> PathBuf {
    let cache = match std::env::var("LOCALAPPDATA") {
        Ok(local) if !local.trim().is_empty() => PathBuf::from(local).join(APP_DIR_NAME).join("cache"),
        _ => std::env::temp_dir().join(APP_DIR_NAME).join("cache"),
    };
    cache.join("update")
}

/// 追加一行。任何失败都静默 —— 记日志本身不该把更新弄挂。
pub fn append(message: &str) {
    let path = log_path();
    if let Some(parent) = path.parent() {
        if std::fs::create_dir_all(parent).is_err() {
            return;
        }
    }
    let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    else {
        return;
    };
    // 一次 `write_all` 写完一整行。
    //
    // 父进程和 helper 会同时往同一个文件追加：`writeln!` 会分多次写，
    // 于是两边的半行会交错成 `[[1793...4793409428954] 等待旧进程退出]`
    // 这种鬼东西。单次 write 在追加模式下是原子的。
    let line = format!("[{}] {message}\n", timestamp());
    let _ = file.write_all(line.as_bytes());
}

/// 毫秒时间戳。不引时间库，够定位「哪一次、什么顺序」了。
fn timestamp() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log_path_sits_under_cache() {
        let path = log_path();
        assert!(path.ends_with("update/apply.log") || path.ends_with("update\\apply.log"));
    }

    #[test]
    fn append_never_panics() {
        // 真实写一次，然后清掉，避免污染用户目录
        let path = log_path();
        let before = std::fs::read_to_string(&path).ok();
        append("测试写入");
        let after = std::fs::read_to_string(&path).unwrap_or_default();
        assert!(after.contains("测试写入"));
        match before {
            Some(old) => std::fs::write(&path, old).ok(),
            None => std::fs::remove_file(&path).ok(),
        };
    }
}
