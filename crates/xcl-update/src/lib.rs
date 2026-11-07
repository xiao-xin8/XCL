//! 便携版自更新。
//!
//! 本 crate 只负责「不需要界面」的那一半，所有逻辑都能单测：
//!
//! - [`manifest`]：`update.json` 解析、semver 比较、频道与最低支持版本
//! - [`verify`]：清单的 Ed25519 验签 + exe 的 SHA-256 校验
//! - [`args`]：`--xcl-apply-update` 的命令行参数解析
//! - [`replace`]：原地替换 exe（`ReplaceFileW` → 退化重命名 → 回滚）
//! - [`wait`] / [`mutex`] / [`ready`]：等旧进程退出、更新期互斥、父子握手
//! - [`spawn`]：父进程侧准备 helper
//! - [`apply`]：helper 侧的主流程（`main()` 第一件事）
//! - [`log`]：helper 的失败日志（apply 模式下没有界面，只能落文件）
//!
//! 边界：不依赖 Tauri、不依赖 `xcl-net`。网络请求由 `xcl-service` 完成，
//! 这里只接收已经拿到手的文本和文件；helper 的原生小窗口也不在这里。
//!
//! 更新单位是一个 `XCL.exe`。数据目录 `data/` 永远不碰。

pub mod apply;
pub mod args;
pub mod cleanup;
pub mod log;
pub mod manifest;
pub mod mutex;
pub mod pending;
pub mod ready;
pub mod replace;
pub mod spawn;
pub mod splash;
pub mod unpack;
pub mod verify;
pub mod wait;

#[cfg(test)]
mod testutil;

pub use apply::{try_apply_pending, try_run, NEW_ALIVE_WATCH, OLD_EXIT_TIMEOUT};
pub use args::{has_updated_flag, parse_apply_args, ApplyArgs, APPLY_FLAG, UPDATED_FLAG};
pub use cleanup::{run as cleanup_leftovers, CleanupReport};
pub use manifest::{
    parse_version, ArchiveArtifact, Channel, CheckResult, CheckState, ExeArtifact, Manifest, Severity,
};
pub use mutex::{wait_until_free, UpdateMutex};
pub use pending::PendingUpdate;
pub use replace::{apply as swap_exe, backup_path, dir_is_writable, rollback};
pub use spawn::launch as launch_helper;
pub use verify::{
    sha256_bytes, sha256_file, verify_artifact, verify_manifest, verify_signature, SignatureCheck,
};
pub use wait::{wait_for_exit, WaitOutcome};

/// 更新清单签名公钥（Ed25519，base64，32 字节）。
///
/// 私钥只留在本机（`%USERPROFILE%\.xcl\update-signing-key.txt`，仓库外），
/// 公钥硬编码在这里。生成方式见 `examples/genkey.rs`，签名见 `examples/sign.rs`。
///
/// 留空表示还没配密钥，此时 [`verify::verify_manifest`] 会返回
/// [`SignatureCheck::Skipped`] —— 发布构建里不该出现这种状态。
pub const MANIFEST_PUBKEY_B64: &str = "fp7Rv7tGO2CxFgWaaFqYXyYjpo+RT7vslmCOSqrsZAU=";

/// 是否已经配了公钥。
///
/// 配了就必须验签（服务器少一份 `.sig` 也算失败）；没配就是开发模式，
/// 调用方可以放行，但发布构建不该让这种状态存在。
pub fn requires_signature() -> bool {
    !MANIFEST_PUBKEY_B64.trim().is_empty()
}

/// 更新工作目录：`<程序目录>/data/update`。
///
/// 临时文件（更新包、helper、`.old` 备份）全放这里，程序目录只留一个 exe ——
/// 在安装目录里看见两三个 exe 会点错，其中一个还是跑不起来的 helper。
///
/// 为什么挂在程序目录下而不是数据根：`ReplaceFileW` 要求源和目标同卷，
/// 而数据根可能被 `XCL_HOME` 指到别的盘。`data` 一定和 exe 同卷。
pub fn work_dir(exe_dir: &std::path::Path) -> std::path::PathBuf {
    exe_dir.join("data").join("update")
}
