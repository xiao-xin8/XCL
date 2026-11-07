//! 联机会话的机器侧准备：出口 IP、局域网网段、空闲端口。
//!
//! 这几件事看着琐碎，却是起进程前最容易翻车的地方：
//!
//! - 出口 IP 不能是回环：`--use-smoltcp` 连不了 `127.0.0.1`（实测 `code 10049`）；
//! - 网段不能瞎猜：房主要把它导出给对端，猜错了对端就打不进来；
//! - 端口不能撞：同一个网络里已经有别的 easytier 时，抢占会直接启动失败。
//!
//! 网段这件事有第二层：免 TUN 模式下房主没有虚拟网卡，虚拟 IP 只活在 easytier 的
//! 用户态协议栈里，游戏不可能在 `10.144.144.1` 上接客。所以房主要把真实局域网
//! 映射进隧道（[`mapped_cidr`]），加入方转发到映射后的地址（[`mapped_ip`]）。
//!
//! 依赖网卡的部分天生没法单测，所以把「IP + 网段 → CIDR / 映射地址」抽成了纯函数，
//! 其余两个只做一层薄封装。

use std::net::{Ipv4Addr, SocketAddr, UdpSocket};

use xcl_core::AppError;

/// 从本机地址推出要导出给对端的网段。
///
/// 按 /24 算：家庭和小办公网络几乎都是 /24，而掩码在 Windows 上要另取（`GetAdaptersAddresses`），
/// 为这点收益引平台代码不划算。只接受私有地址——公网地址不该被导出，回环更不行。
pub fn cidr_for(ip: Ipv4Addr) -> Option<String> {
    if !is_private(ip) {
        return None;
    }
    let octets = ip.octets();
    Some(format!("{}.{}.{}.0/24", octets[0], octets[1], octets[2]))
}

fn is_private(ip: Ipv4Addr) -> bool {
    let o = ip.octets();
    matches!(o[0], 10)
        || (o[0] == 172 && (16..=31).contains(&o[1]))
        || (o[0] == 192 && o[1] == 168)
}

/// 房主局域网在隧道内被映射到的网段。
///
/// 为什么不干脆让加入方直接访问房主真实的 `192.168.1.x`：
///
/// 1. 撞车。两边都是 `192.168.1.0/24` 时，加入方的操作系统会在本地就把包吃掉，
///    根本进不了隧道 —— 更糟的是看起来"能通"，排查时会误判成功（`CONNECT-TEST.md`
///    第 2 节记的就是这个假阳性）。
/// 2. 可读性。映射后地址只由房主的网段最后一位决定，日志里一眼能认出是隧道地址。
///
/// 选 `10.99.0.0/24` 是因为它既不与虚拟网 `10.144.144.0/24` 重叠，也几乎不会出现在家庭网络里。
pub const MAPPED_NET: &str = "10.99.0.0/24";

/// 房主局域网 → 隧道映射网段的 `--proxy-networks` 表达式。
///
/// 形如 `192.168.1.0/24->10.99.0.0/24`。本机不是私有网段（直连公网、或只有 IPv6）时返回
/// `None` —— 那种情况导出局域网的语义本身就不成立，上层应当明确拒绝开房而不是硬着头皮。
pub fn mapped_cidr(lan: Ipv4Addr) -> Option<String> {
    cidr_for(lan).map(|cidr| format!("{cidr}->{MAPPED_NET}"))
}

/// 房主在映射网段里的地址，也就是加入方端口转发的目标。
///
/// 只换网段、保留主机位：`192.168.1.7` → `10.99.0.7`。同一台机器永远得到同一个地址，
/// 重开房间也不会变，方便对着日志核对。
pub fn mapped_ip(lan: Ipv4Addr) -> Option<Ipv4Addr> {
    if !is_private(lan) {
        return None;
    }
    Some(Ipv4Addr::new(10, 99, 0, lan.octets()[3]))
}

/// 查本机用来出网的那个 IPv4。
///
/// 开一个 UDP socket 往公网地址「连」一下，然后读它自己的 `local_addr` ——
/// 这个动作不发任何包，只是让系统选一条路由并把源地址填进内核结构。
/// 比枚举网卡靠谱：你这台机器上还有 VMware、Tailscale 好几块虚拟网卡，枚举必然选错。
pub fn detect_local_ip() -> Result<Ipv4Addr, AppError> {
    let socket = UdpSocket::bind("0.0.0.0:0").map_err(|err| AppError::io(&err, "创建探测 socket"))?;
    // 223.5.5.5 是阿里 DNS，随便哪个公网地址都行；连不上也无所谓，系统照样会填源地址
    let target: SocketAddr = "223.5.5.5:80".parse().expect("静态地址必然能解析");
    let _ = socket.connect(target);
    let local = socket
        .local_addr()
        .map_err(|err| AppError::io(&err, "读取本机地址"))?;
    match local.ip() {
        std::net::IpAddr::V4(ip) if !ip.is_loopback() && !ip.is_unspecified() => Ok(ip),
        _ => Err(AppError::network(
            "找不到可用的本机地址",
            "这台机器没有能对外的 IPv4 地址",
        )
        .with_hint("检查一下网络连接是不是断的")),
    }
}

/// 挑一个没被占用的本地端口。
///
/// 做法是绑一次 `0` 号端口让系统分配，拿到号再立刻释放 —— 释放到真正用起来之间
/// 有几毫秒空窗，实际撞车概率可以忽略；真撞了 easytier 会启动失败并报出来。
pub fn pick_free_port() -> Result<u16, AppError> {
    let listener = std::net::TcpListener::bind(("0.0.0.0", 0))
        .map_err(|err| AppError::io(&err, "分配本地端口"))?;
    let port = listener
        .local_addr()
        .map_err(|err| AppError::io(&err, "读取分配的端口"))?
        .port();
    drop(listener);
    Ok(port)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cidr_from_typical_home_network() {
        assert_eq!(cidr_for(Ipv4Addr::new(192, 168, 0, 104)).as_deref(), Some("192.168.0.0/24"));
        assert_eq!(cidr_for(Ipv4Addr::new(192, 168, 1, 7)).as_deref(), Some("192.168.1.0/24"));
    }

    #[test]
    fn cidr_from_10_and_172_ranges() {
        assert_eq!(cidr_for(Ipv4Addr::new(10, 0, 0, 5)).as_deref(), Some("10.0.0.0/24"));
        assert_eq!(cidr_for(Ipv4Addr::new(172, 16, 3, 9)).as_deref(), Some("172.16.3.0/24"));
        // 172.32 已经出了私有段
        assert_eq!(cidr_for(Ipv4Addr::new(172, 32, 0, 1)), None);
    }

    #[test]
    fn cidr_rejects_loopback_link_local_and_public() {
        assert_eq!(cidr_for(Ipv4Addr::LOCALHOST), None);
        assert_eq!(cidr_for(Ipv4Addr::new(169, 254, 1, 2)), None);
        assert_eq!(cidr_for(Ipv4Addr::new(8, 8, 8, 8)), None);
    }

    /// 映射表达式必须能被 easytier 直接吃掉：`本机网段->隧道网段`，两侧都带前缀长度。
    #[test]
    fn mapped_cidr_uses_arrow_form() {
        assert_eq!(
            mapped_cidr(Ipv4Addr::new(192, 168, 1, 7)).as_deref(),
            Some("192.168.1.0/24->10.99.0.0/24")
        );
        assert_eq!(
            mapped_cidr(Ipv4Addr::new(10, 0, 3, 9)).as_deref(),
            Some("10.0.3.0/24->10.99.0.0/24")
        );
        // 非私有网段没有「局域网」可言，不给表达式
        assert_eq!(mapped_cidr(Ipv4Addr::new(8, 8, 8, 8)), None);
    }

    /// 只换网段、保留主机位：房主的转发目标就是它在映射网段里的地址。
    #[test]
    fn mapped_ip_keeps_host_octet() {
        assert_eq!(
            mapped_ip(Ipv4Addr::new(192, 168, 1, 7)),
            Some(Ipv4Addr::new(10, 99, 0, 7))
        );
        assert_eq!(
            mapped_ip(Ipv4Addr::new(172, 16, 3, 104)),
            Some(Ipv4Addr::new(10, 99, 0, 104))
        );
        assert_eq!(mapped_ip(Ipv4Addr::LOCALHOST), None);
    }

    /// 映射网段不能和虚拟网 `10.144.144.0/24` 重叠，否则端口转发会打到错误的主机上。
    #[test]
    fn mapped_net_does_not_overlap_virtual_net() {
        assert!(!MAPPED_NET.starts_with("10.144.144."), "{MAPPED_NET}");
    }

    #[test]
    fn detects_a_non_loopback_local_ip() {
        // 开发机一定连着网；真断网时这里会返回 Err，测试就跳过而不是误报失败
        match detect_local_ip() {
            Ok(ip) => {
                assert!(!ip.is_loopback());
                assert!(!ip.is_unspecified());
            }
            Err(err) => {
                assert!(err.hint.is_some(), "失败也要给建议");
            }
        }
    }

    #[test]
    fn picked_ports_are_usable() {
        let port = pick_free_port().expect("应当能拿到端口");
        assert!(port >= 1024, "不应该拿到系统保留端口：{port}");
        // 拿到的端口确实能被绑定（说明它当时是空的）
        let listener = std::net::TcpListener::bind(("0.0.0.0", port));
        assert!(listener.is_ok(), "端口 {port} 分配后应当可绑定");
    }
}
