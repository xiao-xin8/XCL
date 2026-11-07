//! 命令行拼装。
//!
//! 参数顺序固定 —— 便于日志排查，也让单测能直接断言数组。
//! 每一组取值都对应 `docs/CONNECT-TEST.md`「第二轮实测」里跑通过的组合，不要在没实测的情况下改。
//!
//! # 两条硬约束
//!
//! 1. 免 TUN：`--no-tun` + `--use-smoltcp`，房主与加入方都不需要管理员权限。
//! 2. 不能连回环：`--use-smoltcp` 的出站连接走用户态协议栈，栈里没有 `127.0.0.0/8` 的路由，
//!    连 `127.0.0.1` 会直接 `code 10049 AddrNotAvailable`。所以指向本地代理的地址必须是网卡 IP，
//!    [`PeerEnd::new`] 会拦下回环地址。
//!
//! 反过来，`--port-forward` 的监听地址留在回环是安全的（那是本机监听，不走出站），
//! 而且不会把端口暴露到局域网。

use std::net::Ipv4Addr;
use xcl_core::AppError;

/// `--port-forward` 默认监听在回环，只有本机能连。
pub const DEFAULT_FORWARD_BIND: Ipv4Addr = Ipv4Addr::new(127, 0, 0, 1);

/// 房主在虚拟网里的固定地址。
///
/// 固定而不是随机：加入方只要拿这一个地址当端口转发的目标，
/// 不用先问「谁是房主」；设备列表也能据此把房主标出来。
pub const HOST_VIRTUAL_IP: Ipv4Addr = Ipv4Addr::new(10, 144, 144, 1);

/// 虚拟网段前缀长度。所有节点固定在同一个 /24 里。
const VIRTUAL_PREFIX: u8 = 24;

/// easytier 要连接的本地出口。
///
/// 之所以要专门造一个类型：这个地址不能是回环，是整个联机方案里最容易踩的坑，
/// 必须在构造时就拦住，而不是等到运行时报 `AddrNotAvailable`。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PeerEnd {
    bind: Ipv4Addr,
    port: u16,
}

impl PeerEnd {
    /// 构造一个指向本地代理的出口。
    pub fn new(bind: Ipv4Addr, port: u16) -> Result<Self, AppError> {
        if bind.is_loopback() {
            return Err(AppError::validation(
                "出口地址不能是本机回环地址",
                "easytier 在免 TUN 模式下连不了 127.0.0.1",
            )
            .with_hint("改用本机网卡的 IPv4 地址，例如 192.168.1.10"));
        }
        if bind.is_unspecified() {
            return Err(AppError::validation(
                "出口地址不能是 0.0.0.0",
                "0.0.0.0 只能用于监听，不能用于连接",
            )
            .with_hint("填入本机网卡的实际 IPv4 地址"));
        }
        if port == 0 {
            return Err(AppError::validation("端口不合法", "0 不是一个可用的端口"));
        }
        Ok(Self { bind, port })
    }

    pub fn bind(&self) -> Ipv4Addr {
        self.bind
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    /// `tcp://<bind>:<port>`，直接喂给 `-p`。
    pub fn url(&self) -> String {
        format!("tcp://{}:{}", self.bind, self.port)
    }
}

/// 房主：把自己所在局域网的 Minecraft 端口导出到虚拟网。
///
/// 不需要游戏端口 —— 房主只负责组网，游戏端口是「告诉好友」的信息，
/// 由界面单独展示。这里少一个参数，房间身份就不会被绑在某次会话上。
#[derive(Debug, Clone)]
pub struct HostPlan {
    pub room: String,
    pub secret: String,
    /// 房主在虚拟网里的地址，形如 `10.144.144.1`。
    pub virtual_ip: Ipv4Addr,
    /// 要导出给对端的网段，可带映射，形如 `192.168.1.0/24->10.99.0.0/24`。
    ///
    /// 免 TUN 模式下必须走映射：房主没有虚拟网卡，对端只能通过映射后的网段
    /// 打到房主的真实局域网地址上。空串表示「只组网、不导出网段」。
    pub lan_cidr: String,
    /// 本机对虚拟网的监听端口（TCP）。
    pub listen_port: u16,
    /// 管理接口端口，只绑回环。
    pub rpc_port: u16,
    /// 本地代理出口。
    pub peer: PeerEnd,
    /// 实例名，写进日志便于区分。
    pub instance: String,
}

/// 加入方：把房主局域网里的游戏端口映射到本机，游戏连 `127.0.0.1:<local_port>`。
#[derive(Debug, Clone)]
pub struct GuestPlan {
    pub room: String,
    pub secret: String,
    /// 自己在虚拟网里的地址。
    pub virtual_ip: Ipv4Addr,
    /// 端口转发的目标：房主局域网映射进隧道后的地址（形如 `10.99.0.7`）。
    ///
    /// 不是房主的虚拟 IP —— 免 TUN 模式没有虚拟网卡，虚拟 IP 上没有游戏在听。
    pub host_ip: Ipv4Addr,
    /// 房主那边 Minecraft 的端口。
    pub mc_port: u16,
    /// 映射到本机的端口。
    pub local_port: u16,
    /// 端口转发的监听地址，留回环即可。
    pub forward_bind: Ipv4Addr,
    pub listen_port: u16,
    pub rpc_port: u16,
    pub peer: PeerEnd,
    pub instance: String,
}

/// 拼房主的参数。
pub fn host_args(plan: &HostPlan) -> Result<Vec<String>, AppError> {
    check_common(&plan.room, &plan.secret)?;
    if !plan.lan_cidr.trim().is_empty() {
        check_proxy_networks(plan.lan_cidr.trim())?;
    }

    let mut out = Vec::with_capacity(24);
    push_common(
        &mut out,
        &plan.room,
        &plan.secret,
        plan.virtual_ip,
        plan.listen_port,
        plan.rpc_port,
        &plan.instance,
    );
    // 免 TUN 导出本地局域网：对端用 --port-forward 打到映射后的房主地址。
    // `lan_cidr` 为空表示「只组网、不导出网段」。
    if !plan.lan_cidr.trim().is_empty() {
        push(&mut out, "--proxy-networks", plan.lan_cidr.trim());
    }
    push(&mut out, "-p", &plan.peer.url());
    Ok(out)
}

/// 拼加入方的参数。
pub fn guest_args(plan: &GuestPlan) -> Result<Vec<String>, AppError> {
    check_common(&plan.room, &plan.secret)?;
    if plan.local_port == 0 {
        return Err(AppError::validation("端口不合法", "本机映射端口不能是 0"));
    }
    if plan.mc_port == 0 {
        return Err(AppError::validation(
            "房主的游戏端口不能是 0",
            "加入方要拿它当端口转发的目标",
        ));
    }
    if plan.host_ip.is_loopback() || plan.host_ip.is_unspecified() {
        return Err(AppError::validation(
            "房主地址不合法",
            "房主地址应当是隧道内的地址，例如 10.99.0.7",
        ));
    }

    let forward = format!(
        "tcp://{}:{}/{}:{}",
        plan.forward_bind, plan.local_port, plan.host_ip, plan.mc_port
    );

    let mut out = Vec::with_capacity(24);
    push_common(
        &mut out,
        &plan.room,
        &plan.secret,
        plan.virtual_ip,
        plan.listen_port,
        plan.rpc_port,
        &plan.instance,
    );
    push(&mut out, "--port-forward", &forward);
    push(&mut out, "-p", &plan.peer.url());
    Ok(out)
}

fn check_common(room: &str, secret: &str) -> Result<(), AppError> {
    if room.trim().is_empty() {
        return Err(AppError::validation("房间名不能为空", "没有房间名就组不了网"));
    }
    if secret.is_empty() {
        return Err(AppError::validation("房间密码不能为空", "没有密码，任何人都能进"));
    }
    Ok(())
}

fn push_common(
    out: &mut Vec<String>,
    room: &str,
    secret: &str,
    virtual_ip: Ipv4Addr,
    listen_port: u16,
    rpc_port: u16,
    instance: &str,
) {
    push(out, "--network-name", room.trim());
    push(out, "--network-secret", secret);
    // 免 TUN：不建网卡，因此不需要管理员。
    push(out, "--no-tun", "true");
    push(out, "--use-smoltcp", "true");
    push(out, "--ipv4", &format!("{virtual_ip}/{VIRTUAL_PREFIX}"));
    // 只开 TCP 监听：中继不给 UDP 留口子，避免绕过准入。
    push(out, "-l", &format!("tcp://0.0.0.0:{listen_port}"));
    push(out, "--rpc-portal", &format!("127.0.0.1:{rpc_port}"));
    // 只允许能验证同一密钥的节点接入。
    push(out, "--private-mode", "true");
    push(out, "-m", instance);
}

fn push(out: &mut Vec<String>, key: &str, value: &str) {
    out.push(key.to_string());
    out.push(value.to_string());
}

/// 校验 `--proxy-networks` 的取值。
///
/// easytier 接受两种写法（`easytier-core --help` 原文）：
///
/// - `192.168.1.0/24`：原样导出本机局域网；
/// - `192.168.1.0/24->10.99.0.0/24`：导出并映射到另一段地址。
///
/// 我们用第二种 —— 房主没有虚拟网卡，对端只能通过映射后的地址打到房主的真实局域网 IP；
/// 顺带避开「两边都是 192.168.1.x 时流量在自己系统里闭环」这个坑。
fn check_proxy_networks(value: &str) -> Result<(), AppError> {
    let terms: Vec<&str> = value.split("->").collect();
    if terms.len() > 2 {
        return Err(AppError::validation(
            "局域网网段格式不对",
            format!("「{value}」里有多个「->」"),
        )
        .with_hint("形如 192.168.1.0/24->10.99.0.0/24"));
    }
    terms.into_iter().try_for_each(check_cidr)
}

fn check_cidr(value: &str) -> Result<(), AppError> {
    let (ip, len) = value.split_once('/').ok_or_else(|| {
        AppError::validation("局域网网段格式不对", format!("「{value}」缺少前缀长度"))
            .with_hint("形如 192.168.1.0/24")
    })?;
    ip.parse::<Ipv4Addr>().map_err(|_| {
        AppError::validation("局域网网段格式不对", format!("「{ip}」不是合法的 IPv4 地址"))
            .with_hint("形如 192.168.1.0/24")
    })?;
    let len: u8 = len.parse().map_err(|_| {
        AppError::validation("局域网网段格式不对", format!("「{len}」不是前缀长度"))
            .with_hint("前缀长度是 1~32 之间的数字")
    })?;
    if !(1..=32).contains(&len) {
        return Err(AppError::validation(
            "局域网网段格式不对",
            format!("前缀长度 {len} 超出范围"),
        )
        .with_hint("前缀长度是 1~32 之间的数字"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn host() -> HostPlan {
        HostPlan {
            room: "xcl-room".into(),
            secret: "s3cret".into(),
            virtual_ip: Ipv4Addr::new(10, 144, 144, 1),
            lan_cidr: "192.168.1.0/24->10.99.0.0/24".into(),
            listen_port: 11021,
            rpc_port: 15888,
            peer: PeerEnd::new(Ipv4Addr::new(192, 168, 1, 10), 20001).unwrap(),
            instance: "host".into(),
        }
    }

    fn guest() -> GuestPlan {
        GuestPlan {
            room: "xcl-room".into(),
            secret: "s3cret".into(),
            virtual_ip: Ipv4Addr::new(10, 144, 144, 2),
            host_ip: Ipv4Addr::new(10, 99, 0, 7),
            mc_port: 25565,
            local_port: 25565,
            forward_bind: DEFAULT_FORWARD_BIND,
            listen_port: 11022,
            rpc_port: 15889,
            peer: PeerEnd::new(Ipv4Addr::new(192, 168, 1, 10), 20002).unwrap(),
            instance: "guest".into(),
        }
    }

    fn value_of<'a>(args: &'a [String], key: &str) -> Option<&'a str> {
        let idx = args.iter().position(|a| a == key)?;
        args.get(idx + 1).map(|s| s.as_str())
    }

    #[test]
    fn host_uses_verified_no_tun_flags() {
        let args = host_args(&host()).expect("应当能拼出来");
        assert_eq!(value_of(&args, "--no-tun"), Some("true"));
        assert_eq!(value_of(&args, "--use-smoltcp"), Some("true"));
        assert_eq!(value_of(&args, "--private-mode"), Some("true"));
        assert_eq!(value_of(&args, "--ipv4"), Some("10.144.144.1/24"));
        assert_eq!(
            value_of(&args, "--proxy-networks"),
            Some("192.168.1.0/24->10.99.0.0/24")
        );
        assert_eq!(value_of(&args, "-p"), Some("tcp://192.168.1.10:20001"));
        // 免 TUN 路线绝不能出现 TUN 专属参数
        assert!(!args.iter().any(|a| a == "--tcp-whitelist"));
    }

    #[test]
    fn host_never_listens_on_udp() {
        let args = host_args(&host()).unwrap();
        assert!(
            args.iter().all(|a| !a.starts_with("udp://")),
            "中继/节点不应监听 UDP：{args:?}"
        );
    }

    #[test]
    fn guest_builds_port_forward_url() {
        let args = guest_args(&guest()).expect("应当能拼出来");
        assert_eq!(
            value_of(&args, "--port-forward"),
            Some("tcp://127.0.0.1:25565/10.99.0.7:25565")
        );
        assert_eq!(value_of(&args, "-p"), Some("tcp://192.168.1.10:20002"));
        assert_eq!(value_of(&args, "--ipv4"), Some("10.144.144.2/24"));
    }

    #[test]
    fn loopback_peer_is_rejected() {
        let err = PeerEnd::new(Ipv4Addr::LOCALHOST, 20001).unwrap_err();
        assert_eq!(err.code, xcl_core::ErrorCode::Validation);
        assert!(err.hint.is_some(), "必须写清楚换成网卡地址");
    }

    #[test]
    fn wildcard_peer_is_rejected() {
        assert!(PeerEnd::new(Ipv4Addr::UNSPECIFIED, 20001).is_err());
    }

    #[test]
    fn peer_is_a_valid_url() {
        let peer = PeerEnd::new(Ipv4Addr::new(10, 0, 0, 5), 40000).unwrap();
        assert_eq!(peer.url(), "tcp://10.0.0.5:40000");
        assert_eq!(peer.port(), 40000);
    }

    #[test]
    fn bad_inputs_are_rejected() {
        let mut plan = host();
        plan.lan_cidr = "192.168.1.0".into();
        assert!(host_args(&plan).is_err(), "缺少前缀长度应当被拒");

        let mut plan = guest();
        plan.local_port = 0;
        assert!(guest_args(&plan).is_err());

        let mut plan = guest();
        plan.host_ip = Ipv4Addr::LOCALHOST;
        assert!(guest_args(&plan).is_err(), "房主地址不能是回环");

        let mut plan = host();
        plan.secret.clear();
        assert!(host_args(&plan).is_err());
    }

    /// `--proxy-networks` 两种写法都要认，坏格式要在起进程之前拦下 ——
    /// easytier 对参数错误只会写日志然后退出，看到的是「房间起不来」。
    #[test]
    fn proxy_networks_formats() {
        for good in [
            "192.168.1.0/24",
            "192.168.1.0/24->10.99.0.0/24",
            "10.0.0.0/8->10.99.0.0/24",
        ] {
            assert!(check_proxy_networks(good).is_ok(), "应当接受：{good}");
        }
        for bad in [
            "",
            "192.168.1.0",
            "192.168.1.0/",
            "192.168.1.0/33",
            "192.168.1.0/0",
            "192.168.1.0/24->",
            "192.168.1.0/24->10.99.0.0",
            "192.168.1.0/24->10.99.0.0/24->10.1.0.0/24",
            "not-a-cidr/24",
        ] {
            assert!(check_proxy_networks(bad).is_err(), "应当拒绝：{bad}");
        }
    }

    #[test]
    fn arguments_come_in_pairs() {
        for args in [host_args(&host()).unwrap(), guest_args(&guest()).unwrap()] {
            assert_eq!(args.len() % 2, 0, "参数应当成对出现：{args:?}");
            assert!(
                args.iter().all(|a| !a.trim().is_empty()),
                "不应出现空参数：{args:?}"
            );
            assert!(args.len() >= 16, "参数数量偏少，可能漏了：{args:?}");
        }
    }
}
