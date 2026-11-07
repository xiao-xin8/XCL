//! 邀请码编解码演示。
//!
//! ```bash
//! cargo run -p xcl-connect --example invite
//! ```

use std::net::Ipv4Addr;
use xcl_connect::Invite;

fn main() {
    let invite = Invite {
        room: "xcl-房-7c31".to_string(),
        secret: "k9Qm2XvT".to_string(),
        host_ip: Ipv4Addr::new(10, 144, 144, 1),
        mc_port: 25565,
        node: "tcp://relay.example.com:11010".to_string(),
    };

    let code = invite.encode().expect("编码失败");
    println!("邀请码（{} 字符）：\n{code}\n", code.chars().count());

    let back = Invite::decode(&code).expect("解码失败");
    println!("解回来：");
    println!("  房间   {}", back.room);
    println!("  密码   {}", back.secret);
    println!("  房主   {}", back.host_ip);
    println!("  MC端口 {}", back.mc_port);
    println!("  节点   {}", back.node);
    println!("\n一致：{}", back == invite);
}
