//! 诊断工具：把本机探测到的 Java 全部列出来。
//!
//! 「Java 检测不全」时用它对照，比翻设置页快：
//!
//! ```text
//! cargo run -p xcl-launch --example java_probe
//! ```

use xcl_launch::java::detect_all;

#[tokio::main]
async fn main() {
    let installs = detect_all().await;
    println!("共探测到 {} 个 Java：", installs.len());
    for java in &installs {
        println!(
            "  Java {} ({}) — {}",
            java.major,
            java.version,
            java.path.display()
        );
    }
}
