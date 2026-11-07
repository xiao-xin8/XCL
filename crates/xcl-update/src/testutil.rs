//! 测试专用：每个用例一个独立的临时目录。
//!
//! 替换逻辑必须真的动文件，所以只能在磁盘上跑。目录名带 pid + 递增序号，
//! 并行测试之间不会互相踩。

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

static COUNTER: AtomicUsize = AtomicUsize::new(0);

pub(crate) fn temp_dir(prefix: &str) -> PathBuf {
    let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "xcl-update-{prefix}-{}-{seq}",
        std::process::id()
    ));
    // 同名目录理论上不会出现（seq 唯一），保险起见先清一下
    std::fs::remove_dir_all(&dir).ok();
    std::fs::create_dir_all(&dir).expect("创建测试临时目录失败");
    dir
}
