//! 构建期把 `bake.toml` 里的部署地址生成成常量，编进程序。
//!
//! 这样打出来的 exe 天生带着地址，用户不需要手填；而源码仓库里不含真实部署
//! 信息（`bake.toml` 不进 Git）。别人 clone 下来自己编、没有这个文件时，
//! 生成的常量是空字符串，对应功能就按「这个构建没有该服务」处理。
//!
//! 改了 `bake.toml` 不会自动重编 —— 得碰一下本文件，或者 `cargo clean -p xcl-core`。

use std::path::PathBuf;

fn pick(doc: Option<&toml::Value>, section: &str, key: &str) -> String {
    doc.and_then(|d| d.get(section))
        .and_then(|s| s.get(key))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string()
}

fn main() {
    let manifest_dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    // workspace 根（xcl-launcher/）
    let bake = manifest_dir.join("../../bake.toml");
    println!("cargo:rerun-if-changed={}", bake.display());

    let doc = std::fs::read_to_string(&bake)
        .ok()
        .and_then(|text| text.parse::<toml::Value>().ok());

    let body = format!(
        "// 由 build.rs 从 bake.toml 生成，不要手改。\n\
         pub const RELAY: &str = {relay:?};\n\
         pub const CONNECT_MANIFEST: &str = {connect:?};\n\
         pub const ACCOUNT_SERVER: &str = {account:?};\n\
         pub const UPDATE_MANIFEST: &str = {update:?};\n",
        relay = pick(doc.as_ref(), "connect", "relay"),
        connect = pick(doc.as_ref(), "connect", "manifestUrl"),
        account = pick(doc.as_ref(), "connect", "accountServer"),
        update = pick(doc.as_ref(), "update", "manifestUrl"),
    );

    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("baked.rs");
    std::fs::write(out, body).expect("写出构建期常量失败");
}
