//! 校验：清单签名（Ed25519）与更新文件（SHA-256）。
//!
//! 两道锁，顺序不能反：
//!
//! 1. 清单签名 —— 证明这份 `update.json` 确实是我们发的；
//! 2. 清单里的 `sha256` —— 证明这个 exe 就是清单里说的那个。
//!
//! 少任何一道，攻击者只要能改托管上的文件（换 CDN、劫持、镜像被投毒），
//! 就能让启动器装上一个任意 exe。HTTPS 只挡得住链路，挡不住源。

use std::io::Read;
use std::path::Path;

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine as _;
use ed25519_dalek::{Signature, VerifyingKey};
use sha2::{Digest, Sha256};
use xcl_core::{AppError, ErrorCode};

use crate::MANIFEST_PUBKEY_B64;

/// 一个 SHA-256 摘要的十六进制长度。
const SHA256_HEX_LEN: usize = 64;
/// Ed25519 签名固定 64 字节。
const SIG_LEN: usize = 64;
/// Ed25519 公钥固定 32 字节。
const KEY_LEN: usize = 32;

/// 签名校验结果。
///
/// `Skipped` 不是「通过」：它表示还没配公钥（见 [`MANIFEST_PUBKEY_B64`]）。
/// 由调用方决定要不要继续 —— 开发构建可以继续，发布构建应该停下来。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignatureCheck {
    Verified,
    Skipped,
}

/// 计算字节的 SHA-256，返回小写十六进制。
pub fn sha256_bytes(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex_lower(&hasher.finalize())
}

/// 流式计算文件的 SHA-256。
///
/// exe 有几 MB 到几十 MB，一次读进内存没必要；8 KiB 缓冲足够。
pub fn sha256_file(path: &Path) -> Result<String, AppError> {
    let mut file = std::fs::File::open(path).map_err(|err| {
        AppError::io(&err, "读取更新文件")
            .with_hint("文件可能已被移动或删除，请重新下载")
    })?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 8 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|err| AppError::io(&err, "读取更新文件"))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hex_lower(&hasher.finalize()))
}

/// 校验一个更新文件：存在、是普通文件、大小与哈希都对得上。
///
/// 三条全部通过才算过 —— 大小不符通常意味着下载被截断，
/// 哈希不符意味着内容被换过，两者都不该进入替换流程。
pub fn verify_artifact(path: &Path, expected_size: u64, expected_sha256: &str) -> Result<(), AppError> {
    let meta = std::fs::metadata(path).map_err(|err| {
        AppError::io(&err, "检查更新文件").with_hint("文件可能已被安全软件隔离，请重新下载")
    })?;
    if !meta.is_file() {
        return Err(AppError::new(
            ErrorCode::Validation,
            "更新文件无效",
            "下载到的不是一个文件",
        ));
    }
    if expected_size > 0 && meta.len() != expected_size {
        return Err(AppError::new(
            ErrorCode::Validation,
            "更新文件校验失败",
            format!("大小不符：应为 {expected_size} 字节，实际 {} 字节", meta.len()),
        )
        .with_hint("下载可能被中断，请重新下载")
        .with_retryable(true));
    }
    let actual = sha256_file(path)?;
    if !actual.eq_ignore_ascii_case(expected_sha256.trim()) {
        return Err(AppError::new(
            ErrorCode::Validation,
            "更新文件校验失败",
            format!("哈希不符：应为 {expected_sha256}，实际 {actual}"),
        )
        .with_hint("文件可能被篡改或损坏，请重新下载")
        .with_retryable(true));
    }
    Ok(())
}

/// 校验清单签名。
///
/// `signature_b64` 是签名（base64，64 字节），`public_key_b64` 是公钥
/// （base64，32 字节）。签名必须覆盖清单原文的字节，不要签名解析后的
/// JSON 结构 —— 那需要对序列化做规范化，容易出岔子。
///
/// 用 `verify_strict`：拒绝低阶公钥等弱密钥，比默认 `verify` 严。
pub fn verify_signature(
    payload: &[u8],
    signature_b64: &str,
    public_key_b64: &str,
) -> Result<(), AppError> {
    let signature = decode_fixed::<SIG_LEN>(signature_b64, "签名").ok_or_else(|| {
        AppError::parse("更新清单签名无效", "签名格式不是 base64 的 64 字节")
            .with_hint("清单文件可能被改动，请稍后重试")
    })?;
    let key = decode_fixed::<KEY_LEN>(public_key_b64, "公钥").ok_or_else(|| {
        AppError::new(
            ErrorCode::Unsupported,
            "无法校验更新清单",
            "内置公钥格式不正确（应为 base64 的 32 字节）",
        )
    })?;

    let verifying = VerifyingKey::from_bytes(&key).map_err(|_| {
        AppError::new(
            ErrorCode::Unsupported,
            "无法校验更新清单",
            "内置公钥不是合法的 Ed25519 公钥",
        )
    })?;
    let signature = Signature::from_bytes(&signature);
    verifying.verify_strict(payload, &signature).map_err(|_| {
        AppError::new(
            ErrorCode::Validation,
            "更新清单签名不匹配",
            "签名与清单内容不一致",
        )
        .with_hint("清单可能已被篡改，本次更新已中止")
    })
}

/// 用内置公钥校验清单签名。
///
/// 公钥还没配（[`MANIFEST_PUBKEY_B64`] 为空）时返回 `Skipped`，
/// 而不是当作通过 —— 让调用方自己决定能不能放行。
pub fn verify_manifest(payload: &[u8], signature_b64: &str) -> Result<SignatureCheck, AppError> {
    if MANIFEST_PUBKEY_B64.trim().is_empty() {
        return Ok(SignatureCheck::Skipped);
    }
    verify_signature(payload, signature_b64, MANIFEST_PUBKEY_B64)?;
    Ok(SignatureCheck::Verified)
}

/// 是不是 64 个十六进制字符。清单里的 `sha256` 字段用它把关。
pub fn is_sha256_hex(text: &str) -> bool {
    let text = text.trim();
    text.len() == SHA256_HEX_LEN && text.bytes().all(|b| b.is_ascii_hexdigit())
}

fn decode_fixed<const N: usize>(text: &str, _what: &str) -> Option<[u8; N]> {
    let bytes = B64.decode(text.trim()).ok()?;
    if bytes.len() != N {
        return None;
    }
    bytes.try_into().ok()
}

fn hex_lower(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_matches_known_value() {
        // 与 `sha256sum` 对照过的固定值，防止实现被改坏
        assert_eq!(
            sha256_bytes(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn sha256_of_file_matches_bytes() {
        let dir = crate::testutil::temp_dir("hash");
        let path = dir.join("payload.bin");
        std::fs::write(&path, b"hello xcl").unwrap();
        assert_eq!(sha256_file(&path).unwrap(), sha256_bytes(b"hello xcl"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn artifact_size_and_hash_are_both_checked() {
        let dir = crate::testutil::temp_dir("artifact");
        let path = dir.join("XCL.exe");
        std::fs::write(&path, b"new-build").unwrap();
        let hash = sha256_bytes(b"new-build");

        assert!(verify_artifact(&path, 9, &hash).is_ok());
        // 大小不符：下载被截断
        let err = verify_artifact(&path, 99, &hash).unwrap_err();
        assert_eq!(err.code, ErrorCode::Validation);
        assert!(err.retryable);
        // 哈希不符：内容被换过
        let wrong = "0".repeat(64);
        assert!(verify_artifact(&path, 9, &wrong).is_err());
        // 文件不存在
        assert!(verify_artifact(&dir.join("missing.exe"), 9, &hash).is_err());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn size_zero_skips_size_check() {
        // 清单没写 size 时不该因为 0 而失败
        let dir = crate::testutil::temp_dir("size-zero");
        let path = dir.join("a.bin");
        std::fs::write(&path, b"x").unwrap();
        assert!(verify_artifact(&path, 0, &sha256_bytes(b"x")).is_ok());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn sha256_hex_shape() {
        assert!(is_sha256_hex(&"a".repeat(64)));
        assert!(is_sha256_hex(&"A".repeat(64)));
        assert!(!is_sha256_hex(&"a".repeat(63)));
        assert!(!is_sha256_hex(&"g".repeat(64)));
    }

    #[test]
    fn signature_rejects_malformed_input() {
        let err = verify_signature(b"payload", "not-base64", &"A".repeat(44)).unwrap_err();
        assert!(err.message.contains("签名格式"));

        // 公钥长度不对 → 是我们自己的配置问题，不是对方的问题
        let err = verify_signature(b"payload", &B64.encode([0u8; 64]), "short").unwrap_err();
        assert_eq!(err.code, ErrorCode::Unsupported);
    }

    #[test]
    fn signature_verifies_real_keypair() {
        use ed25519_dalek::{Signer, SigningKey};
        // 固定种子，测试可重复；只是测试用，跟发布密钥无关
        let signing = SigningKey::from_bytes(&[7u8; 32]);
        let public = B64.encode(signing.verifying_key().to_bytes());
        let payload = br#"{"schema":1,"version":"0.2.0"}"#;
        let sig = B64.encode(signing.sign(payload).to_bytes());

        assert!(verify_signature(payload, &sig, &public).is_ok());
        // 改一个字节就验不过
        assert!(verify_signature(br#"{"schema":1,"version":"0.2.1"}"#, &sig, &public).is_err());
    }

    #[test]
    fn builtin_public_key_is_configured() {
        // 公钥留空时 `verify_manifest` 会返回 Skipped，等于自更新没有签名保护。
        // 发布构建里不该出现这种状态，所以这条测试守着它。
        assert!(crate::requires_signature(), "必须先配置 MANIFEST_PUBKEY_B64");
        // base64 的 32 字节正好 44 个字符（含一个 padding）
        assert_eq!(MANIFEST_PUBKEY_B64.len(), 44);
    }
}
