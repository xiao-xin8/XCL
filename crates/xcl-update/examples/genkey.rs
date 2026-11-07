//! 生成更新清单的签名密钥对。
//!
//! ```text
//! cargo run -p xcl-update --example genkey -- <口令>
//! ```
//!
//! - 公钥打印出来，填进 `crates/xcl-update/src/lib.rs` 的 `MANIFEST_PUBKEY_B64`
//! - 私钥写到家目录下的 `.xcl/update-signing-key.txt`（仓库外，别提交）
//!
//! 种子的派生方式：`sha256(口令)` 取 32 字节。同一个口令永远得到同一对密钥，
//! 这一点对「换了电脑也能重新算出来」有用；代价是口令强度就是密钥强度，
//! 弱口令等于没有签名。

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine as _;
use ed25519_dalek::SigningKey;
use sha2::{Digest, Sha256};

fn main() {
    let mut args = std::env::args().skip(1);
    let Some(secret) = args.next() else {
        eprintln!("用法：cargo run -p xcl-update --example genkey -- <口令>");
        std::process::exit(2);
    };
    if secret.len() < 8 {
        eprintln!("口令太短了，至少 8 个字符");
        std::process::exit(2);
    }

    let seed: [u8; 32] = Sha256::digest(secret.as_bytes()).into();
    let key = SigningKey::from_bytes(&seed);
    let public = B64.encode(key.verifying_key().to_bytes());

    let dir = home_dir().join(".xcl");
    let path = dir.join("update-signing-key.txt");
    if let Err(err) = std::fs::create_dir_all(&dir) {
        eprintln!("无法创建 {}：{err}", dir.display());
        std::process::exit(1);
    }
    let body = format!(
        "# XCL 更新清单签名私钥（Ed25519，base64 的 32 字节种子）\n\
         # 生成方式：sha256(口令)\n\
         # 丢了就换密钥、重发一版启动器；泄露了同理。\n\
         {}\n",
        B64.encode(seed)
    );
    if let Err(err) = std::fs::write(&path, body) {
        eprintln!("无法写入 {}：{err}", path.display());
        std::process::exit(1);
    }

    println!("public_key = {public}");
    println!("private_key_file = {}", path.display());
}

fn home_dir() -> std::path::PathBuf {
    if let Ok(profile) = std::env::var("USERPROFILE") {
        if !profile.trim().is_empty() {
            return std::path::PathBuf::from(profile);
        }
    }
    if let Ok(home) = std::env::var("HOME") {
        if !home.trim().is_empty() {
            return std::path::PathBuf::from(home);
        }
    }
    std::env::temp_dir()
}
