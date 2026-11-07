//! 连接阶段状态机。
//!
//! 只做两件事：阶段推进与把 peer 列表压成一句话的结论。
//! 前者对应项目里的铁律「状态必须单调」—— `install_start` 那次因为返回值与进度事件抢跑，
//! 把 `done` 覆盖回 `running`，结果界面永远卡在「取消」；联机这边同样的坑是
//! 「已连接」被迟到的「连接中」覆盖掉，所以这里不让它回退。
//!
//! 后者是「后端胖前端瘦」：前端拿到的是 `online / via_relay / 谁 / 多少毫秒`，
//! 而不是一坨 peer 数组让它自己挑。

use crate::peer::{LinkKind, Peer};

/// 一次联机会话的阶段。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// 没开始。
    Idle,
    /// 准备中（查组件、算参数）。
    Preparing,
    /// 下载联机组件。
    Downloading,
    /// 组件就绪，等人点「创建 / 加入」。
    Ready,
    /// 已启动，正在等对端。
    Connecting,
    /// 连上了。
    Connected,
    /// 出错了，可以重试。
    Failed,
}

impl Phase {
    /// 阶段序号。只有比较用，数值本身没有含义。
    fn rank(self) -> u8 {
        match self {
            Self::Idle => 0,
            Self::Preparing => 1,
            Self::Downloading => 2,
            Self::Ready => 3,
            Self::Connecting => 4,
            Self::Connected => 5,
            Self::Failed => 6,
        }
    }

    /// 给界面用的中文名。前端不再自己 match。
    pub fn label(self) -> &'static str {
        match self {
            Self::Idle => "未连接",
            Self::Preparing => "准备中",
            Self::Downloading => "下载组件",
            Self::Ready => "已就绪",
            Self::Connecting => "连接中",
            Self::Connected => "已连接",
            Self::Failed => "连接失败",
        }
    }

    /// 是不是已经结束（成功或失败）。用来决定要不要停轮询。
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Connected | Self::Failed)
    }

    /// 还在跑，界面要显示「进行中」。
    pub fn is_busy(self) -> bool {
        matches!(
            self,
            Self::Preparing | Self::Downloading | Self::Connecting
        )
    }
}

/// 推进阶段。
///
/// 规则：
/// - 随时可以失败（网络断了、对端跑了）；
/// - 失败后可以重来（从任意阶段重新开始）；
/// - 其余情况只能往前走，不能退回 —— 迟到的「连接中」不能覆盖「已连接」。
pub fn advance(current: Phase, next: Phase) -> Phase {
    match (current, next) {
        (_, Phase::Failed) => Phase::Failed,
        (Phase::Failed, next) => next,
        (current, next) if next.rank() > current.rank() => next,
        (current, _) => current,
    }
}

/// peer 列表的结论。
///
/// 必须把「节点」和「真人」分开：只要连上中继服务器，peer 列表里就多一行，
/// 但那是基建、不是人。不分的话房主一开房就显示「已连接 · 344ms」—— 假象。
#[derive(Debug, Clone, PartialEq)]
pub struct LinkSummary {
    /// 房间里直连的真人数量（不含自己、不含中继服务器、不含绕中继的人）。
    pub players: usize,
    /// 走中继的真人数量。他们算「还没连上」，只是链路已经通了但绕了服务器。
    pub relay_players: usize,
    /// 中继 / 发现服务器的数量。只用于诊断，界面上通常不显示。
    pub nodes: usize,
    /// 至少有一个直连的真人 —— 这才是「连上了」的定义。
    pub online: bool,
    /// 有真人经中继在跑（此时 `online` 一定是 false）。
    pub via_relay: bool,
    /// 直连真人里延迟最低的那个。
    pub best: Option<Peer>,
    /// 上面那个真人的延迟。
    pub best_latency_ms: Option<f64>,
}

impl LinkSummary {
    /// 一句话结论，直接可以放到状态块里。
    pub fn headline(&self) -> String {
        if !self.online {
            return "正在等待对方加入".to_string();
        }
        match (self.best.as_ref(), self.best_latency_ms) {
            (Some(peer), Some(ms)) => {
                let how = if peer.link == LinkKind::Relay {
                    "经中继"
                } else {
                    "点对点"
                };
                format!("已连接 {} · {} · {:.0} ms", peer.display_name(), how, ms)
            }
            (Some(peer), None) => {
                format!("已连接 {} · {}", peer.display_name(), peer.link.label())
            }
            _ => "已连接".to_string(),
        }
    }
}

/// 一台设备在这个房间里的身份。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceRole {
    /// 本机自己。
    SelfNode,
    /// 房主。
    Host,
    /// 房间里的其他人。
    Player,
    /// 中继 / 发现服务器。
    Node,
}

impl DeviceRole {
    /// 给界面用的中文名。
    pub fn label(self) -> &'static str {
        match self {
            Self::SelfNode => "本机",
            Self::Host => "房主",
            Self::Player => "房间成员",
            Self::Node => "联机服务器",
        }
    }

    /// 给前端的稳定标识，用来配色。
    pub fn key(self) -> &'static str {
        match self {
            Self::SelfNode => "self",
            Self::Host => "host",
            Self::Player => "player",
            Self::Node => "node",
        }
    }
}

/// 设备列表里的一行。
#[derive(Debug, Clone, PartialEq)]
pub struct Device {
    pub name: String,
    pub role: DeviceRole,
    /// 虚拟网内的地址。中继服务器没有。
    pub address: Option<String>,
    pub latency_ms: Option<f64>,
    pub via_relay: bool,
}

/// 房间里「联机服务器」一行的显示名。
///
/// 服务端报上来的 hostname 是它自己的机器名（形如 `PublicServer_ip-172-26-1-10`），
/// 对外行来说就是天书。这里统一换成本地叫法 —— 只改显示，不动服务端配置。
pub const RELAY_DISPLAY_NAME: &str = "鑫乐云信令节点";

/// 把 peer 列表排成「房间里现在有谁」。
///
/// 顺序固定：本机 → 房主 → 其他成员（延迟升序）→ 服务器。
/// 服务器排最后，因为它是基建、不是人 —— 关心的是前面几行。
pub fn devices(peers: &[Peer], i_am_host: bool) -> Vec<Device> {
    let host_ip = crate::args::HOST_VIRTUAL_IP.to_string();

    let mut list: Vec<Device> = peers
        .iter()
        .map(|peer| {
            let role = if peer.is_self() {
                DeviceRole::SelfNode
            } else if peer.is_node() {
                DeviceRole::Node
            } else if !i_am_host && peer.ipv4.as_deref() == Some(host_ip.as_str()) {
                // 房主固定 10.144.144.1，加入方据此认人
                DeviceRole::Host
            } else {
                DeviceRole::Player
            };
            // 绕中继的人照样列出来。
            //
            // 一进来就进列表，再显示「正在连接」——之前把走中继的人藏掉，
            // 结果是列表空空如也、文案却在说「正在建立直连…」，看着像在跟空气较劲，
            // 也分不清到底是有人进来了但没打穿，还是压根没人。
            // 链路状态由 `via_relay` 单独表达（前端据此标「经中继」），不靠「不显示」暗示。
            Device {
                // 服务器那一行用本地叫法，不把它的机器名甩出来
                name: if role == DeviceRole::Node {
                    RELAY_DISPLAY_NAME.to_string()
                } else {
                    peer.display_name().to_string()
                },
                role,
                address: peer.ipv4.clone(),
                latency_ms: peer.latency_ms,
                via_relay: peer.link == LinkKind::Relay,
            }
        })
        .collect();

    list.sort_by(|a, b| {
        rank(a).cmp(&rank(b)).then_with(|| {
            match (a.latency_ms, b.latency_ms) {
                (Some(x), Some(y)) => x.partial_cmp(&y).unwrap_or(std::cmp::Ordering::Equal),
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (None, None) => std::cmp::Ordering::Equal,
            }
        })
    });
    list
}

fn rank(device: &Device) -> u8 {
    match device.role {
        DeviceRole::SelfNode => 0,
        DeviceRole::Host => 1,
        DeviceRole::Player => 2,
        DeviceRole::Node => 3,
    }
}

/// 把 peer 列表压成结论。自己那一条会被跳过。
pub fn summarize(peers: &[Peer]) -> LinkSummary {
    let others: Vec<&Peer> = peers.iter().filter(|peer| !peer.is_self()).collect();
    // 中继服务器也是 peer，但不算人。这一步决定了「已连接」是真话还是假话。
    let players: Vec<&Peer> = others
        .iter()
        .copied()
        .filter(|peer| !peer.is_node())
        .collect();
    let node_count = others.len() - players.len();

    // 绕中继 = 还没打通，不当作「房间里的人」，否则界面会显示 789ms 的「已连接」，
    // 而那边以为自己在直连。
    let direct: Vec<&Peer> = players
        .iter()
        .copied()
        .filter(|peer| peer.link != LinkKind::Relay)
        .collect();
    let relay_players = players.len() - direct.len();

    // 挑延迟最低的；没有延迟数据的排在后面，但不排除。
    let best = direct
        .iter()
        .min_by(|a, b| match (a.latency_ms, b.latency_ms) {
            (Some(x), Some(y)) => x.partial_cmp(&y).unwrap_or(std::cmp::Ordering::Equal),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => std::cmp::Ordering::Equal,
        })
        .map(|peer| (*peer).clone());

    let best_latency_ms = best.as_ref().and_then(|peer| peer.latency_ms);

    LinkSummary {
        players: direct.len(),
        relay_players,
        nodes: node_count,
        online: !direct.is_empty(),
        via_relay: relay_players > 0,
        best,
        best_latency_ms,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn phases_only_move_forward() {
        assert_eq!(advance(Phase::Idle, Phase::Preparing), Phase::Preparing);
        assert_eq!(advance(Phase::Preparing, Phase::Ready), Phase::Ready);
        // 迟到的「连接中」不能覆盖「已连接」—— 这正是 taskPhase 踩过的坑
        assert_eq!(advance(Phase::Connected, Phase::Connecting), Phase::Connected);
        assert_eq!(advance(Phase::Ready, Phase::Preparing), Phase::Ready);
    }

    #[test]
    fn failure_is_always_accepted() {
        for phase in [
            Phase::Idle,
            Phase::Preparing,
            Phase::Downloading,
            Phase::Ready,
            Phase::Connecting,
            Phase::Connected,
        ] {
            assert_eq!(advance(phase, Phase::Failed), Phase::Failed);
        }
    }

    #[test]
    fn failure_can_be_retried() {
        assert_eq!(advance(Phase::Failed, Phase::Preparing), Phase::Preparing);
        assert_eq!(advance(Phase::Failed, Phase::Idle), Phase::Idle);
    }

    #[test]
    fn skipping_ahead_is_allowed() {
        // 组件已在本地缓存时，可以从 Idle 直接到 Ready
        assert_eq!(advance(Phase::Idle, Phase::Ready), Phase::Ready);
    }

    #[test]
    fn terminal_and_busy_flags() {
        assert!(Phase::Connected.is_terminal());
        assert!(Phase::Failed.is_terminal());
        assert!(!Phase::Connected.is_busy());
        assert!(Phase::Connecting.is_busy());
        assert!(Phase::Downloading.is_busy());
        assert!(!Phase::Ready.is_busy());
    }

    fn peer(name: &str, link: LinkKind, latency: Option<f64>) -> Peer {
        Peer {
            hostname: name.to_string(),
            ipv4: Some("10.144.144.2".to_string()),
            // 真人一定有虚拟网地址 —— 判据就是 `cidr` 有没有值
            cidr: Some("10.144.144.2/24".to_string()),
            link,
            latency_ms: latency,
            loss_rate: None,
            tunnel: None,
            nat_type: None,
            rx_bytes: None,
            tx_bytes: None,
            id: None,
        }
    }

    /// 中继 / 发现服务器：实测 `cidr` 与 `ipv4` 都是空串。
    fn node_peer(name: &str, latency: Option<f64>) -> Peer {
        Peer {
            hostname: name.to_string(),
            ipv4: None,
            cidr: None,
            link: LinkKind::P2p,
            latency_ms: latency,
            loss_rate: None,
            tunnel: None,
            nat_type: None,
            rx_bytes: None,
            tx_bytes: None,
            id: None,
        }
    }

    /// 指定虚拟网地址的真人。
    fn peer_at(name: &str, ip: &str, link: LinkKind, latency: Option<f64>) -> Peer {
        Peer {
            hostname: name.to_string(),
            ipv4: Some(ip.to_string()),
            cidr: Some(format!("{ip}/24")),
            link,
            latency_ms: latency,
            loss_rate: None,
            tunnel: None,
            nat_type: None,
            rx_bytes: None,
            tx_bytes: None,
            id: None,
        }
    }

    #[test]
    fn empty_list_means_offline() {
        let summary = summarize(&[]);
        assert!(!summary.online);
        assert_eq!(summary.players, 0);
        assert_eq!(summary.headline(), "正在等待对方加入");

        let me_only = summarize(&[peer("me", LinkKind::Local, None)]);
        assert!(!me_only.online, "只有自己不算连上");
    }

    /// 这条盯的是那个假象：房主一个人开房时，列表里只有中继服务器。
    /// 旧实现把它当对端，界面显示「已连接 · 344ms」。
    #[test]
    fn relay_server_alone_is_not_connected() {
        let summary = summarize(&[
            peer("me", LinkKind::Local, None),
            node_peer("PublicServer_ip-172-26-1-10", Some(344.0)),
        ]);
        assert!(!summary.online, "只有一个中继服务器，不能说连上了");
        assert_eq!(summary.players, 0);
        assert_eq!(summary.nodes, 1, "节点要被单独计数");
        assert_eq!(summary.best, None, "不该把服务器的延迟当成房间延迟");
        assert_eq!(summary.headline(), "正在等待对方加入");
    }

    /// 中继服务器延迟最低，也不能被选成「最佳对端」。
    #[test]
    fn node_latency_never_wins_over_a_player() {
        let summary = summarize(&[
            peer("me", LinkKind::Local, None),
            node_peer("PublicServer", Some(40.0)),
            peer("friend", LinkKind::P2p, Some(180.0)),
        ]);
        assert!(summary.online);
        assert_eq!(summary.players, 1);
        assert_eq!(summary.nodes, 1);
        assert_eq!(summary.best.as_ref().unwrap().hostname, "friend");
        assert_eq!(summary.best_latency_ms, Some(180.0));
    }

    #[test]
    fn picks_lowest_latency_peer() {
        let peers = vec![
            peer("me", LinkKind::Local, None),
            peer("slow", LinkKind::P2p, Some(80.0)),
            peer("fast", LinkKind::P2p, Some(3.0)),
            peer("unknown", LinkKind::P2p, None),
        ];
        let summary = summarize(&peers);
        assert!(summary.online);
        assert_eq!(summary.players, 3);
        assert_eq!(summary.best.as_ref().unwrap().hostname, "fast");
        assert_eq!(summary.best_latency_ms, Some(3.0));
        assert!(!summary.via_relay);
        assert!(summary.headline().contains("fast"));
        assert!(summary.headline().contains("点对点"));
    }

    /// 绕中继 = 还没打通。以前这里显示「已连接 · 经中继 · 789ms」，
    /// 看起来是直连，其实数据在悉尼绕了一圈。
    #[test]
    fn relay_only_counts_as_not_connected() {
        let peers = vec![
            peer("me", LinkKind::Local, None),
            peer("via-relay", LinkKind::Relay, Some(789.0)),
        ];
        let summary = summarize(&peers);
        assert!(summary.via_relay, "要标记出「有人在绕中继」");
        assert_eq!(summary.relay_players, 1);
        assert_eq!(summary.players, 0, "绕中继的人不算房间里的人");
        assert!(!summary.online, "中继不能算已连接");
        assert_eq!(summary.best, None, "别把中继延迟当成房间延迟");
    }

    #[test]
    fn peer_without_latency_still_counts_as_best() {
        let peers = vec![peer("me", LinkKind::Local, None), peer("x", LinkKind::P2p, None)];
        let summary = summarize(&peers);
        assert!(summary.online);
        assert_eq!(summary.best_latency_ms, None);
        assert!(summary.headline().starts_with("已连接 x"));
    }

    #[test]
    fn devices_list_orders_self_host_players_then_nodes() {
        // 加入方视角：自己 .2，房主固定 .1，另一个成员 .3，服务器没地址
        let peers = vec![
            node_peer("PublicServer", Some(300.0)),
            peer_at("me", "10.144.144.2", LinkKind::Local, None),
            peer_at("friend", "10.144.144.3", LinkKind::P2p, Some(30.0)),
            peer_at("host", "10.144.144.1", LinkKind::P2p, Some(5.0)),
        ];
        let list = devices(&peers, false);
        let roles: Vec<DeviceRole> = list.iter().map(|item| item.role).collect();
        assert_eq!(
            roles,
            vec![
                DeviceRole::SelfNode,
                DeviceRole::Host,
                DeviceRole::Player,
                DeviceRole::Node
            ],
            "顺序应当固定：本机 → 房主 → 成员 → 服务器"
        );
        assert_eq!(list[1].name, "host");
        assert_eq!(list[1].address.as_deref(), Some("10.144.144.1"));
        assert_eq!(list[3].role.label(), "联机服务器");
        assert_eq!(
            list[3].name, RELAY_DISPLAY_NAME,
            "服务器那一行要用本地叫法，不能显示它自己的机器名"
        );
    }

    /// 走中继的人也要进名单：人一进来就先看到人，链路状态交给 `via_relay`。
    ///
    /// 曾经的写法是把他藏起来，结果列表空着、文案却在说「正在建立直连…」，
    /// 既看不出到底有没有人进来，也分不清是没打穿还是压根没人。
    #[test]
    fn relayed_player_is_listed_with_flag() {
        let peers = vec![
            peer_at("me", "10.144.144.1", LinkKind::Local, None),
            peer_at("relayed", "10.144.144.9", LinkKind::Relay, Some(789.0)),
        ];
        let list = devices(&peers, true);
        assert_eq!(list.len(), 2, "走中继的人也要列出来：{list:?}");
        let player = list
            .iter()
            .find(|item| item.role == DeviceRole::Player)
            .expect("走中继的人也是房间里的成员");
        assert!(player.via_relay, "必须标出「经中继」，别让人以为已经打穿了");
        assert_eq!(player.address.as_deref(), Some("10.144.144.9"));
    }

    #[test]
    fn host_does_not_show_a_host_row() {        let peers = vec![
            peer_at("me", "10.144.144.1", LinkKind::Local, None),
            peer_at("friend", "10.144.144.2", LinkKind::P2p, Some(12.0)),
        ];
        let list = devices(&peers, true);
        assert!(
            list.iter().all(|item| item.role != DeviceRole::Host),
            "自己就是房主，不该再出现一行房主"
        );
        assert_eq!(list.len(), 2);
        assert_eq!(list[1].role, DeviceRole::Player);
    }
}
