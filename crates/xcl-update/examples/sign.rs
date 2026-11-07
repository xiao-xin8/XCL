//! 给更新清单签名。
//!
//! ```text
//! cargo run -p xcl-update --example sign -- ../../cloud/update.json
//! ```
//!
//! 生成同名 `.sig`（base64 的 64 字节），里面是清单原始字节的签名。
//! 签原始字节而不是解析后的结构：JSON 序列化没有唯一表示，换个字段顺序
//! 签名就废了。
//!
//! 私钥从 `%USERPROFILE%\.xcl\update-signing-key.txt` 读，不进仓库。

use std::path::{Path, PathBuf};

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine as _;
use ed25519_dalek::{Signer, SigningKey};

fn main() {
    let Some(target) = std::env::args().nth(1) else {
        eprintln!("用法：cargo run -p xcl-update --example sign -- <update.json>");
        std::process::exit(2);
    };
    let target = PathBuf::from(target);
    let payload = match std::fs::read(&target) {
        Ok(bytes) => bytes,
        Err(err) => {
            eprintln!("读不到 {}：{err}", target.display());
            std::process::exit(1);
        }
    };

    let seed = match read_seed() {
        Ok(seed) => seed,
        Err(message) => {
            eprintln!("{message}");
            std::process::exit(1);
        }
    };
    let key = SigningKey::from_bytes(&seed);
    let signature = key.sign(&payload);

    // 签完立刻自校验一次：公私钥配错时应该在这里报警，而不是等更新失败
    if let Err(err) = key.verifying_key().verify_strict(&payload, &signature) {
        eprintln!("签名自校验失败：{err}");
        std::process::exit(1);
    }

    let sig_path = sig_path(&target);
    if let Err(err) = std::fs::write(&sig_path, B64.encode(signature.to_bytes())) {
        eprintln!("写不进 {}：{err}", sig_path.display());
        std::process::exit(1);
    }
    println!("signed = {}", sig_path.display());
    println!("public_key = {}", B64.encode(key.verifying_key().to_bytes()));
}

fn sig_path(target: &Path) -> PathBuf {
    let mut name = target.as_os_str().to_os_string();
    name.push(".sig");
    PathBuf::from(name)
}

/// 私钥文件里除了注释只有一行 base64。
fn read_seed() -> Result<[u8; 32], String> {
    let path = home_dir().join(".xcl").join("update-signing-key.txt");
    let text = std::fs::read_to_string(&path)
        .map_err(|err| format!("读不到私钥 {}：{err}", path.display()))?;
    let line = text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .next_back()
        .ok_or_else(|| format!("{} 里没有密钥", path.display()))?;

    let bytes = B64
        .decode(line)
        .map_err(|err| format!("私钥不是合法的 base64：{err}"))?;
    bytes
        .try_into()
        .map_err(|_| "私钥长度不对（应为 32 字节种子）".to_string())
}

fn home_dir() -> PathBuf {
    if let Ok(profile) = std::env::var("USERPROFILE") {
        if !profile.trim().is_empty() {
            return PathBuf::from(profile);
        }
    }
    if let Ok(home) = std::env::var("HOME") {
        if !home.trim().is_empty() {
            return PathBuf::from(home);
        }
    }
    std::env::temp_dir()
}
