//! 解析 `easytier-cli -o json` 的输出。
//!
//! # 真实结构（实测，不要凭想象改）
//!
//! `peer list` 的顶层是数组，而且所有数值都是字符串，无值统一写成 `"-"`：
//!
//! ```json
//! [
//!   { "cidr": "10.144.144.1/24", "ipv4": "10.144.144.1", "hostname": "DESKTOP",
//!     "cost": "Local", "lat_ms": "-", "loss_rate": "-", "rx_bytes": "-",
//!     "tx_bytes": "-", "tunnel_proto": "-", "nat_type": "SymmetricEasyInc",
//!     "id": "919356490", "version": "2.6.4-8428a89d" }
//! ]
//! ```
//!
//! 因此解析要宽容：键名可能有别名、值可能有 `"-"`、数值可能是字符串。
//! 解析失败一律给成品中文文案，前端不需要判断错误类型。

use serde_json::Value;
use xcl_core::AppError;

/// 与对端之间这条连接的性质。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkKind {
    /// 就是自己。
    Local,
    /// 点对点直连（打洞成功）。
    P2p,
    /// 经中继转发。延迟高，但对称 NAT 下可能是唯一能通的方式。
    Relay,
    /// 认不出来的取值。保留原文由上层展示，不做猜测。
    Unknown,
}

impl LinkKind {
    /// 从 `cost` 字段的原文判断。值形如 `Local` / `p2p` / 含 `relay`。
    fn from_cost(raw: &str) -> Self {
        let lower = raw.to_ascii_lowercase();
        // relay 先判：某些版本会写成 `p2p(relay)` 这类组合，宁可报中继也不要谎报直连。
        if lower.contains("relay") {
            Self::Relay
        } else if lower.contains("p2p") || lower.contains("direct") {
            Self::P2p
        } else if lower.contains("local") {
            Self::Local
        } else {
            Self::Unknown
        }
    }

    /// 给界面用的中文说法。前端不再自己 match 枚举。
    pub fn label(self) -> &'static str {
        match self {
            Self::Local => "本机",
            Self::P2p => "点对点",
            Self::Relay => "经中继",
            Self::Unknown => "未知",
        }
    }
}

/// 一个对端。
#[derive(Debug, Clone, PartialEq)]
pub struct Peer {
    pub hostname: String,
    pub ipv4: Option<String>,
    pub cidr: Option<String>,
    pub link: LinkKind,
    /// 毫秒。无值时（例如自己）为 `None`。
    pub latency_ms: Option<f64>,
    /// 丢包率百分比，`0.0` 表示没丢。
    pub loss_rate: Option<f64>,
    pub tunnel: Option<String>,
    pub nat_type: Option<String>,
    pub rx_bytes: Option<u64>,
    pub tx_bytes: Option<u64>,
    pub id: Option<String>,
}

impl Peer {
    /// 是不是自己这一条。
    pub fn is_self(&self) -> bool {
        self.link == LinkKind::Local
    }

    /// 是不是中继 / 发现服务器这类「基建节点」，不是真人。
    ///
    /// 判据来自实测（2026-11-01，`peer list` 真实输出）：共享节点的 `cidr` 与 `ipv4`
    /// 都是空的，而真人节点一定有 `10.144.144.x/24` 这样的虚拟网地址。
    /// 不靠 hostname 判：`-m` 只设 `instance_name`，peer list 里的 hostname 是操作系统名，
    /// 同一台机器上两个实例会完全同名。
    ///
    /// ⚠️ 两个字段任一缺失就算基建。只看 `cidr` 不够：某些版本会给共享节点填上
    /// 网段，它就会被当成真人，于是房主一开房、服务器刚接进来，界面就显示
    /// 「正在建立直连…」，拖久了还报「网络无法直连」—— 房间里明明一个人都没有。
    pub fn is_node(&self) -> bool {
        self.ipv4.is_none() || self.cidr.is_none()
    }

    /// 展示用的名字：优先主机名，其次 IP，最后给个占位。
    pub fn display_name(&self) -> &str {
        if !self.hostname.is_empty() {
            return &self.hostname;
        }
        self.ipv4.as_deref().unwrap_or("未知设备")
    }
}

/// 解析 `peer list`。
pub fn parse_peers(json: &str) -> Result<Vec<Peer>, AppError> {
    let root = parse_root(json)?;
    let items: Vec<&Value> = match &root {
        Value::Array(items) => items.iter().collect(),
        // 防御：万一某个版本把它包在对象里
        Value::Object(map) => map
            .get("peers")
            .and_then(|v| v.as_array())
            .map(|a| a.iter().collect())
            .ok_or_else(|| unexpected("peer list 既不是数组，也没有 peers 字段"))?,
        _ => return Err(unexpected("peer list 的顶层不是数组")),
    };

    Ok(items
        .into_iter()
        .filter_map(|item| item.as_object().map(|obj| {
            let value = Value::Object(obj.clone());
            Peer {
                hostname: text(&value, &["hostname", "host_name"]).unwrap_or_default(),
                ipv4: text(&value, &["ipv4", "ipv4_addr", "ip"]),
                cidr: text(&value, &["cidr"]),
                link: LinkKind::from_cost(&text(&value, &["cost"]).unwrap_or_default()),
                latency_ms: number(&value, &["lat_ms", "latency_ms", "lat"]),
                loss_rate: percent(&value, &["loss_rate", "loss"]),
                tunnel: text(&value, &["tunnel_proto", "tunnel"]),
                nat_type: text(&value, &["nat_type"]),
                rx_bytes: bytes(&value, &["rx_bytes"]),
                tx_bytes: bytes(&value, &["tx_bytes"]),
                id: text(&value, &["id", "peer_id"]),
            }
        }))
        .collect())
}

/// 本机节点信息（`easytier-cli -o json node`）。
#[derive(Debug, Clone, PartialEq)]
pub struct NodeInfo {
    pub peer_id: Option<String>,
    pub ipv4: Option<String>,
    pub hostname: Option<String>,
    /// 本机导出给对端的网段（房主才有）。
    pub proxy_cidrs: Vec<String>,
    /// NAT 类型编号，见 [`nat_type_label`]。
    pub udp_nat_type: Option<u8>,
    pub tcp_nat_type: Option<u8>,
    pub public_ip: Vec<String>,
    pub listeners: Vec<String>,
}

impl NodeInfo {
    /// NAT 是否属于「双方都这样基本打不通」的那类。
    pub fn is_symmetric_nat(&self) -> bool {
        matches!(self.udp_nat_type, Some(6) | Some(8))
    }
}

/// 解析 `node`。
pub fn parse_node(json: &str) -> Result<NodeInfo, AppError> {
    let root = parse_root(json)?;
    let obj = root
        .as_object()
        .ok_or_else(|| unexpected("node 的顶层不是对象"))?;
    let value = Value::Object(obj.clone());

    let stun = value.get("stun_info").cloned().unwrap_or(Value::Null);
    let proxy_cidrs = string_list(&value, "proxy_cidrs");
    let listeners = string_list(&value, "listeners");

    Ok(NodeInfo {
        peer_id: text(&value, &["peer_id", "id"]),
        ipv4: text(&value, &["ipv4_addr", "ipv4"]),
        hostname: text(&value, &["hostname"]),
        proxy_cidrs,
        udp_nat_type: small_uint(&stun, &["udp_nat_type"]),
        tcp_nat_type: small_uint(&stun, &["tcp_nat_type"]),
        public_ip: string_list(&stun, "public_ip"),
        listeners,
    })
}

/// NAT 类型编号 → 中文。已知的只有实测碰到过的两个，其余不猜。
pub fn nat_type_label(kind: u8) -> &'static str {
    match kind {
        6 => "对称 NAT",
        8 => "对称 NAT（易变）",
        _ => "未知类型",
    }
}

fn parse_root(json: &str) -> Result<Value, AppError> {
    if json.trim().is_empty() {
        return Err(unexpected("命令没有输出任何内容"));
    }
    serde_json::from_str(json.trim()).map_err(|err| {
        AppError::parse("看不懂联机模块的输出", format!("返回的不是合法 JSON：{err}"))
            .with_hint("可能是联机模块版本与启动器不匹配")
    })
}

fn unexpected(detail: &str) -> AppError {
    AppError::parse("联机模块返回了意外的内容", detail.to_string())
}

/// 键名可能有别名；空串与 `"-"` 都当「没有值」。
fn text(value: &Value, keys: &[&str]) -> Option<String> {
    for key in keys {
        if let Some(Value::String(raw)) = value.get(*key) {
            let trimmed = raw.trim();
            if trimmed.is_empty() || trimmed == "-" {
                continue;
            }
            return Some(trimmed.to_string());
        }
        if let Some(Value::Number(num)) = value.get(*key) {
            return Some(num.to_string());
        }
    }
    None
}

fn number(value: &Value, keys: &[&str]) -> Option<f64> {
    for key in keys {
        match value.get(*key) {
            Some(Value::Number(num)) => {
                if let Some(f) = num.as_f64() {
                    return Some(f);
                }
            }
            Some(Value::String(raw)) => {
                let trimmed = raw.trim();
                if trimmed.is_empty() || trimmed == "-" {
                    continue;
                }
                if let Ok(parsed) = trimmed.parse::<f64>() {
                    return Some(parsed);
                }
            }
            _ => {}
        }
    }
    None
}

fn percent(value: &Value, keys: &[&str]) -> Option<f64> {
    for key in keys {
        if let Some(Value::String(raw)) = value.get(*key) {
            let trimmed = raw.trim().trim_end_matches('%');
            if trimmed.is_empty() || trimmed == "-" {
                continue;
            }
            if let Ok(parsed) = trimmed.parse::<f64>() {
                return Some(parsed);
            }
        }
    }
    number(value, keys)
}

fn bytes(value: &Value, keys: &[&str]) -> Option<u64> {
    for key in keys {
        if let Some(Value::String(raw)) = value.get(*key) {
            if let Some(parsed) = parse_human_bytes(raw) {
                return Some(parsed);
            }
        }
    }
    None
}

/// `"2.35 kB"` / `"512 B"` / `"1.20 MB"` → 字节数。识别不了就返回 `None`，不瞎猜。
fn parse_human_bytes(raw: &str) -> Option<u64> {
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed == "-" {
        return None;
    }
    let mut parts = trimmed.split_whitespace();
    let num: f64 = parts.next()?.parse().ok()?;
    let unit = parts.next().unwrap_or("B").to_ascii_lowercase();
    let scale = match unit.as_str() {
        "b" => 1.0,
        "kb" => 1_000.0,
        "mb" => 1_000_000.0,
        "gb" => 1_000_000_000.0,
        "kib" => 1024.0,
        "mib" => 1024.0 * 1024.0,
        "gib" => 1024.0 * 1024.0 * 1024.0,
        _ => return None,
    };
    Some((num * scale).round() as u64)
}

fn small_uint(value: &Value, keys: &[&str]) -> Option<u8> {
    for key in keys {
        match value.get(*key) {
            Some(Value::Number(num)) => {
                if let Some(v) = num.as_u64() {
                    return u8::try_from(v).ok();
                }
            }
            Some(Value::String(raw)) => {
                if let Ok(v) = raw.trim().parse::<u8>() {
                    return Some(v);
                }
            }
            _ => {}
        }
    }
    None
}

fn string_list(value: &Value, key: &str) -> Vec<String> {
    match value.get(key) {
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(|item| item.as_str())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty() && s != "-")
            .collect(),
        Some(Value::String(raw)) => raw
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty() && s != "-")
            .collect(),
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 2026-11-01 实测抓下来的真实输出（只删了与本模块无关的字段顺序差异）。
    const PEERS_JSON: &str = r#"[
      {
        "cidr": "10.144.144.1/24",
        "ipv4": "10.144.144.1",
        "hostname": "DESKTOP-B7PN79S",
        "cost": "Local",
        "lat_ms": "-",
        "loss_rate": "-",
        "rx_bytes": "-",
        "tx_bytes": "-",
        "tunnel_proto": "-",
        "nat_type": "SymmetricEasyInc",
        "id": "919356490",
        "version": "2.6.4-8428a89d"
      },
      {
        "cidr": "10.144.144.2/24",
        "ipv4": "10.144.144.2",
        "hostname": "DESKTOP-B7PN79S",
        "cost": "p2p",
        "lat_ms": "0.12",
        "loss_rate": "0.0%",
        "rx_bytes": "2.04 kB",
        "tx_bytes": "2.34 kB",
        "tunnel_proto": "tcp",
        "nat_type": "SymmetricEasyInc",
        "id": "544807398",
        "version": "2.6.4-8428a89d"
      }
    ]"#;

    const NODE_JSON: &str = r#"{
      "peer_id": 1638167380,
      "ipv4_addr": "10.144.144.9/24",
      "proxy_cidrs": [],
      "hostname": "DESKTOP-B7PN79S",
      "stun_info": {
        "udp_nat_type": 8,
        "tcp_nat_type": 6,
        "public_ip": ["117.167.2.73"]
      },
      "listeners": ["ring://abc", "tcp://0.0.0.0:11021"]
    }"#;

    #[test]
    fn parses_real_peer_list() {
        let peers = parse_peers(PEERS_JSON).expect("真实输出必须能解析");
        assert_eq!(peers.len(), 2);

        let me = &peers[0];
        assert!(me.is_self());
        assert_eq!(me.link, LinkKind::Local);
        assert_eq!(me.latency_ms, None, "`-` 必须当成无值");
        assert_eq!(me.tunnel, None);
        assert_eq!(me.rx_bytes, None);
        assert_eq!(me.hostname, "DESKTOP-B7PN79S");

        let other = &peers[1];
        assert!(!other.is_self());
        assert_eq!(other.link, LinkKind::P2p);
        assert_eq!(other.latency_ms, Some(0.12));
        assert_eq!(other.loss_rate, Some(0.0));
        assert_eq!(other.rx_bytes, Some(2040));
        assert_eq!(other.tx_bytes, Some(2340));
        assert_eq!(other.tunnel.as_deref(), Some("tcp"));
        assert_eq!(other.ipv4.as_deref(), Some("10.144.144.2"));
    }

    #[test]
    fn tolerates_bom_wrapped_object_and_missing_keys() {
        let wrapped = r#"{"peers":[{"hostname":"x","cost":"relay"}]}"#;
        let peers = parse_peers(wrapped).unwrap();
        assert_eq!(peers.len(), 1);
        assert_eq!(peers[0].link, LinkKind::Relay);
        assert_eq!(peers[0].latency_ms, None);
        assert_eq!(peers[0].display_name(), "x");
    }

    #[test]
    fn display_name_falls_back_to_ip() {
        let peers = parse_peers(r#"[{"hostname":"","ipv4":"10.0.0.2","cost":"p2p"}]"#).unwrap();
        assert_eq!(peers[0].display_name(), "10.0.0.2");
    }

    #[test]
    fn empty_or_broken_input_gives_friendly_error() {
        let empty = parse_peers("").unwrap_err();
        assert_eq!(empty.code, xcl_core::ErrorCode::Parse);

        let broken = parse_peers("{not json}").unwrap_err();
        assert_eq!(broken.code, xcl_core::ErrorCode::Parse);
        assert!(broken.hint.is_some(), "必须给出可操作的下一步");

        let wrong_shape = parse_peers("\"just a string\"").unwrap_err();
        assert!(wrong_shape.title.contains("意外"));
    }

    #[test]
    fn parses_real_node_json() {
        let node = parse_node(NODE_JSON).expect("真实 node 输出必须能解析");
        assert_eq!(node.peer_id.as_deref(), Some("1638167380"));
        assert_eq!(node.ipv4.as_deref(), Some("10.144.144.9/24"));
        assert!(node.proxy_cidrs.is_empty());
        assert_eq!(node.udp_nat_type, Some(8));
        assert_eq!(node.tcp_nat_type, Some(6));
        assert_eq!(node.public_ip, vec!["117.167.2.73".to_string()]);
        assert_eq!(node.listeners.len(), 2);
        assert!(node.is_symmetric_nat());
    }

    #[test]
    fn node_with_proxy_cidr_is_detected() {
        let json = r#"{"ipv4_addr":"10.144.144.1/24","proxy_cidrs":["192.168.0.0/24"]}"#;
        let node = parse_node(json).unwrap();
        assert_eq!(node.proxy_cidrs, vec!["192.168.0.0/24".to_string()]);
    }

    #[test]
    fn node_is_recognised_even_when_it_has_a_cidr() {
        // 共享节点可能只带网段、没有自己的虚拟 IP —— 这种绝不能被当成人。
        // 线上症状：房主刚开房、服务器刚接进来，界面就显示「正在建立直连…」，
        // 拖十几秒还报「网络无法直连」，可房间里一个人都没有。
        let only_node = parse_peers(r#"[{"cidr":"10.144.144.0/24","cost":"relay","lat_ms":"501"}]"#)
            .unwrap();
        assert!(only_node[0].is_node(), "只有 cidr、没有 ipv4 的是基建节点");

        let summary = crate::state::summarize(&only_node);
        assert!(!summary.online, "房间里只有服务器不算连上");
        assert_eq!(summary.players, 0);
        assert_eq!(summary.relay_players, 0, "基建节点不能被算成绕中继的玩家");
        assert!(!summary.via_relay, "不该因此催「正在建立直连」");
        assert_eq!(summary.nodes, 1);

        // 真人：虚拟 IP 和网段都有
        let real = parse_peers(
            r#"[{"hostname":"PC","ipv4":"10.144.144.2","cidr":"10.144.144.2/24","cost":"p2p"}]"#,
        )
        .unwrap();
        assert!(!real[0].is_node());
        assert!(crate::state::summarize(&real).online);
    }

    #[test]
    fn link_kind_labels_are_stable() {
        assert_eq!(LinkKind::from_cost("Local").label(), "本机");
        assert_eq!(LinkKind::from_cost("p2p").label(), "点对点");
        assert_eq!(LinkKind::from_cost("Relay(tcp)").label(), "经中继");
        assert_eq!(LinkKind::from_cost("p2p(relay)").label(), "经中继");
        assert_eq!(LinkKind::from_cost("whatever").label(), "未知");
    }

    #[test]
    fn human_bytes_parsing() {
        assert_eq!(parse_human_bytes("512 B"), Some(512));
        assert_eq!(parse_human_bytes("2.35 kB"), Some(2350));
        assert_eq!(parse_human_bytes("1.20 MB"), Some(1_200_000));
        assert_eq!(parse_human_bytes("1.00 KiB"), Some(1024));
        assert_eq!(parse_human_bytes("-"), None);
        assert_eq!(parse_human_bytes("weird"), None);
    }
}
