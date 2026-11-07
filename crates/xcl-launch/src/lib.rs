//! 启动逻辑：把「版本 JSON + 路径 + 玩家」变成真正跑起来的游戏进程。
//!
//! 分工（刻意这么切）：
//! - `xcl-core::launch` 负责参数怎么拼 —— 纯逻辑，能 100% 单测
//! - 本 crate 负责碰系统的部分：找 Java、解 natives、起进程、读日志
//!
//! 启动顺序（不能换）：
//! 找 Java → 补全文件 → 拼参数 → 解 natives → 起进程 → 读日志。
//! 其中「补全文件」在 `xcl-service` 里做（复用已有的安装计划）。

pub mod java;
pub mod natives;
pub mod os;
pub mod process;

pub use java::{detect_all, detect_all_with, missing_java_error, pick, probe, JavaInstall};
pub use natives::{extract, ExtractReport, NativeArchive};
pub use process::{describe_exit, kill, spawn, GameProcess, LogLine, LogStream, SpawnOptions};
