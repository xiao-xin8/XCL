//! 联机会话：起进程、轮询状态、停止。
//!
//! 会话只有一份（一个启动器同时只联一个房间），所以存在模块内的 `static` 里，
//! 不去动 `AppState` 的字段。所有状态变化都通过 [`StateSink`] 推给上层。

use std::path::PathBuf;
use std::process::Stdio;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use serde::Serialize;
use tokio::process::{Child, Command};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

use xcl_connect::{GuestPlan, HostPlan, Invite, LinkSummary, PeerEnd, Phase, advance, devices, guest_args, host_args, parse_peers, summarize};
use xcl_core::AppError;

use crate::AppState;
use crate::connect_node::{detect_local_ip, mapped_cidr, mapped_ip, pick_free_port};

/// 状态回调。Tauri 层用它推事件。
pub type StateSink = Arc<dyn Fn(ConnectStateView) + Send + Sync>;

/// 轮询间隔。
///
/// 每次轮询都要 spawn 一个 9.5MB 的 CLI 进程，1 秒一次纯属浪费 ——
/// 还容易被杀软拖成超时。2 秒对「谁进谁出」这件事完全够用。
const POLL_INTERVAL: Duration = Duration::from_secs(2);

/// 连续几次查询失败才判「联机模块没响应」。
///
/// 2 秒一次 × 3 = 6 秒，足够盖住刚启动那几秒的窗口期，
/// 又不至于对着一个已经死掉的进程干等。
const FAIL_LIMIT: u32 = 3;

/// 只能靠中继撑多少轮之后劝换网络（2 秒一轮，约 90 秒）。
///
/// 原来是 8 轮（16 秒）—— 太急了。对称 NAT 下打洞经常要换几次端口才穿，
/// 实测「看着像秒连，其实是连了半天才打穿」，中途就被这句劝退提示吓到过。
/// 90 秒以内一句话都不说：中继本来就能玩，只是延迟高，不该催人换网络。
const RELAY_HINT_TICKS: u32 = 45;

/// 给前端看的会话状态。所有派生量都已算好。
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ConnectStateView {
    /// `idle` / `preparing` / `connecting` / `connected` / `failed`
    pub phase: String,
    /// 中文状态，直接显示。
    pub label: String,
    pub role: Option<String>,
    /// 房间名。给界面显示「你在哪个房间」，房主改过名字也认得出。
    pub room: Option<String>,
    /// 房主的游戏端口（已写进邀请码）。加入方是 `None`。
    pub mc_port: Option<u16>,
    /// 房间里的真人数量（不含自己、不含中继服务器）。
    pub players: usize,
    /// 中继 / 发现服务器数量。只用于诊断。
    pub nodes: usize,
    pub online: bool,
    pub via_relay: bool,
    pub best_name: Option<String>,
    pub best_latency_ms: Option<f64>,
    /// 加入方连游戏用的地址，形如 `127.0.0.1:25565`。
    ///
    /// 写死 IPv4 字面量，不用 `localhost`：Java 解析 `localhost` 时可能先拿到 `::1`，
    /// 而端口转发只监听 IPv4 回环，撞上就是一个毫无线索的「连接被拒绝」。
    pub local_address: Option<String>,
    /// 邀请码。房主整局都带着，加入方为 `None`。
    pub invite: Option<String>,
    /// 房间里的设备列表：本机 / 房主 / 成员 / 服务器。顺序已排好。
    pub devices: Vec<DeviceView>,
    pub error: Option<String>,
}

/// 设备列表里的一行。前端只负责摆样式，不含任何判断。
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DeviceView {
    pub name: String,
    /// 中文身份，直接显示。
    pub label: String,
    /// `self` / `host` / `player` / `node`，给前端配色用。
    pub kind: String,
    pub address: Option<String>,
    pub latency_ms: Option<f64>,
    pub via_relay: bool,
}

impl ConnectStateView {
    fn idle() -> Self {
        Self {
            phase: "idle".into(),
            label: "未连接".into(),
            role: None,
            room: None,
            mc_port: None,
            players: 0,
            nodes: 0,
            online: false,
            via_relay: false,
            best_name: None,
            best_latency_ms: None,
            local_address: None,
            invite: None,
            devices: Vec::new(),
            error: None,
        }
    }

    fn from_phase(phase: Phase, role: &str) -> Self {
        Self {
            phase: phase_key(phase).into(),
            label: phase.label().into(),
            role: Some(role.to_string()),
            ..Self::idle()
        }
    }
}

fn phase_key(phase: Phase) -> &'static str {
    match phase {
        Phase::Idle => "idle",
        Phase::Preparing => "preparing",
        Phase::Downloading => "downloading",
        Phase::Ready => "ready",
        Phase::Connecting => "connecting",
        Phase::Connected => "connected",
        Phase::Failed => "failed",
    }
}

struct Session {
    child: Child,
    cancel: CancellationToken,
    role: &'static str,
    /// 当前房间名与密码。用来拦住「把自己房间的邀请码填进加入框」这种操作 ——
    /// 那会把房主自己顶掉，然后卡在「连接中」等一个永远不会来的对端。
    room: String,
    secret: String,
    /// 端口与邀请码都是「随时会变」的，和轮询任务共享同一份，别各存一个副本。
    port: Arc<std::sync::Mutex<u16>>,
    invite: Arc<std::sync::Mutex<Option<String>>>,
    /// 房主的转发目标（映射进隧道后的地址）。端口一变就要重编邀请码，
    /// 重编时得用同一个目标，不能再现算一遍 —— 网卡换了就会算成别的地址。
    forward_ip: std::net::Ipv4Addr,
    /// 共享节点地址，写进邀请码的那一份。
    ///
    /// 存下来而不是每次现读配置：`set_host_port` 拿不到 `AppState`，而且
    /// 邀请码里的节点必须和房主进程连的是同一个。
    node: String,
    /// 推送状态用。`set_host_port` 需要主动通知前端。
    sink: StateSink,
    /// 本地中继代理。靠 Drop 关：会话被顶掉 / 退出房间 / 启动器退出，
    /// 这里一放手代理就停，不用每个出口都记得手动收。
    _relay: crate::connect_relay::Relay,
}

static SESSION: OnceLock<Mutex<Option<Session>>> = OnceLock::new();
static LAST: OnceLock<Mutex<ConnectStateView>> = OnceLock::new();

fn session() -> &'static Mutex<Option<Session>> {
    SESSION.get_or_init(|| Mutex::new(None))
}

fn last() -> &'static Mutex<ConnectStateView> {
    LAST.get_or_init(|| Mutex::new(ConnectStateView::idle()))
}

/// 当前状态。切页回来时用它补一次。
pub async fn current_state() -> ConnectStateView {
    last().lock().await.clone()
}

/// 最近一次会话的联机日志路径。
///
/// 组件按版本分目录（`tools/easytier/<version>/`），升级后旧目录还留着，
/// 所以取「最近改过的那个」—— 要看的一定是刚才这一轮。
pub fn log_path(state: &AppState) -> Option<PathBuf> {
    let mut best: Option<(u128, PathBuf)> = None;
    for entry in std::fs::read_dir(state.layout.connect_tools_dir())
        .ok()?
        .flatten()
    {
        let log = entry.path().join("xcl-connect.log");
        if !log.is_file() {
            continue;
        }
        let stamp = log
            .metadata()
            .and_then(|meta| meta.modified())
            .ok()
            .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|dur| dur.as_millis())
            .unwrap_or(0);
        if best.as_ref().map(|(best, _)| stamp > *best).unwrap_or(true) {
            best = Some((stamp, log));
        }
    }
    best.map(|(_, path)| path)
}

/// 读日志尾部。
///
/// `max_bytes` 是硬上限：easytier 的日志能涨到几 MB，全塞给前端既卡又没用，
/// 排障真正需要的只有最后这一截。
pub fn read_log_tail(state: &AppState, max_bytes: u64) -> Result<String, AppError> {
    use std::io::{Read, Seek, SeekFrom};

    let path = log_path(state).ok_or_else(|| {
        AppError::not_found("还没有联机日志", "开一次房间之后才会生成")
            .with_hint("日志位置：数据目录/tools/easytier/<版本>/xcl-connect.log")
    })?;

    let len = std::fs::metadata(&path)
        .map_err(|err| AppError::io(&err, "读取联机日志"))?
        .len();
    let start = len.saturating_sub(max_bytes);
    let mut file = std::fs::File::open(&path).map_err(|err| AppError::io(&err, "打开联机日志"))?;
    if start > 0 {
        file.seek(SeekFrom::Start(start))
            .map_err(|err| AppError::io(&err, "定位联机日志"))?;
    }
    let mut buf = Vec::new();
    file.read_to_end(&mut buf)
        .map_err(|err| AppError::io(&err, "读取联机日志"))?;
    // 从中间截断可能切坏 UTF-8，这里用 lossy：日志宁可有一两个乱码也不能读失败
    Ok(String::from_utf8_lossy(&buf).into_owned())
}

async fn publish(sink: &StateSink, next: ConnectStateView) {
    *last().lock().await = next.clone();
    sink(next);
}

/// 起房主。
///
/// `mc_port` 是房主那边游戏的端口，会编进邀请码 —— 好友粘一条就能进，
/// 不用再问「端口多少」。0 表示还没扫到游戏：码照样能出，加入方那边会说明原因。
pub async fn start_host(
    state: &Arc<AppState>,
    room: String,
    secret: String,
    mc_port: u16,
    sink: StateSink,
) -> Result<ConnectStateView, AppError> {
    ensure_stopped().await;

    // 共享节点是内置的（打包时注入）。空着就不给开房：邀请码里的节点是
    // 好友要连的地址，编一条连不上的码比直接说「没有」更糟 —— 对方会一直卡住。
    let node = xcl_core::config::RELAY_ENDPOINT.to_string();
    if node.is_empty() {
        return Err(AppError::unsupported(
            "这个版本没有联机节点",
            "共享节点地址没有打进这个构建",
        ));
    }

    // 房间名与密码必填：以前空着就随手生成一个，结果不知道自己的房间叫什么，
    // 下次开房也对不上号。房间是自己起的，名字也该自己定。
    let (room, secret) = validate_identity(&room, &secret)?;
    remember_room(state, &room, &secret);

    let local = detect_local_ip()?;
    // 把真实局域网映射进隧道。免 TUN 模式下房主没有虚拟网卡，`--ipv4 10.144.144.1`
    // 只活在 easytier 的用户态协议栈里，游戏不可能在那个地址上接客；对端必须打到
    // 「房主局域网 → 隧道映射网段」之后的地址。这一步不做，组网再通也进不了服。
    let (lan_cidr, forward_ip) = forward_target(local)?;
    // 出口是本地代理（不是节点），代理负责向桥接出示令牌
    let relay = start_relay(state, local).await?;

    let plan = HostPlan {
        room: room.clone(),
        secret: secret.clone(),
        virtual_ip: xcl_connect::args::HOST_VIRTUAL_IP,
        lan_cidr,
        listen_port: pick_free_port()?,
        rpc_port: pick_free_port()?,
        peer: peer_of(&relay)?,
        instance: "xcl-host".into(),
    };
    let args = host_args(&plan)?;
    // 没有端口就不给邀请码：粘一条没有端口的码进去也连不上游戏，
    // 与其给一条半成品，不如等到扫到 / 填了端口再生成。
    let code = if mc_port == 0 {
        None
    } else {
        Some(
            Invite {
                room: plan.room.clone(),
                secret: plan.secret.clone(),
                host_ip: forward_ip,
                mc_port,
                node: node.clone(),
            }
            .encode()?,
        )
    };

    let view = ConnectStateView {
        invite: code.clone(),
        mc_port: (mc_port > 0).then_some(mc_port),
        ..ConnectStateView::from_phase(Phase::Connecting, "host")
    };
    spawn_node(state, &args, plan.rpc_port, "host", sink, view, relay, RoomIdentity {
        room: plan.room.clone(),
        secret: plan.secret.clone(),
        mc_port,
        forward_ip,
        invite: code,
        node,
    })
    .await
}

/// 由本机局域网地址推出「导出的网段」与「对端要用的转发目标」。
///
/// 两件事必须一起算：网段是房主进程的参数，转发目标是写进邀请码的地址，
/// 一旦两者对不上，对端会拿到一个隧道里根本不存在的目标，表现为「连上了但进不去服」。
fn forward_target(local: std::net::Ipv4Addr) -> Result<(String, std::net::Ipv4Addr), AppError> {
    match (mapped_cidr(local), mapped_ip(local)) {
        (Some(cidr), Some(ip)) => Ok((cidr, ip)),
        _ => Err(AppError::unsupported(
            "这台机器的网络暂时不支持开房",
            format!("本机地址 {local} 不在常见的家用私有网段里，没法把游戏分享给好友"),
        )
        .with_hint("换一个家用 Wi-Fi 或手机热点再试；公司 / 校园网常常是这种结构")),
    }
}

/// 一个会话的房间身份。房主的端口会随游戏变化，扫到后要重编邀请码，
/// 所以这几个字段都得留着，不能算完就扔。
struct RoomIdentity {
    room: String,
    secret: String,
    mc_port: u16,
    /// 转发目标。加入方没有，用不着，但字段留着省得分两种结构。
    forward_ip: std::net::Ipv4Addr,
    /// 房主有邀请码，加入方没有。
    invite: Option<String>,
    /// 邀请码里要写的共享节点。加入方用不到（节点在别人的码里）。
    node: String,
}

/// 房间名与密码的必填校验（顺便去掉首尾空白）。
fn validate_identity(room: &str, secret: &str) -> Result<(String, String), AppError> {
    let room = room.trim();
    let secret = secret.trim();
    if room.is_empty() {
        return Err(AppError::validation("房间名不能为空", "房间名是给好友认的标识")
            .with_hint("随便起一个，比如「小鑫的房间」"));
    }
    if secret.is_empty() {
        return Err(AppError::validation("房间密码不能为空", "没有密码，任何人都能进")
            .with_hint("随手打几个字就行，好友靠它进同一个房间"));
    }
    Ok((room.to_string(), secret.to_string()))
}

/// 把这次用的房间名与密码记进配置，下次开房默认就用它。
///
/// 落盘失败不当成开房失败 —— 房间已经在组了，记不住只是下次得重新填。
fn remember_room(state: &Arc<AppState>, room: &str, secret: &str) {
    let current = state.config();
    if current.connect.room == room && current.connect.secret == secret {
        return;
    }
    let mut next = current;
    next.connect.room = room.to_string();
    next.connect.secret = secret.to_string();
    if let Err(err) = state.update_config(next) {
        tracing::warn!(%err, "记住联机房间失败");
    }
}

/// 起加入方。
///
/// 端口从邀请码里读 —— 房主把码发过来就够了，加入方不需要再填任何东西。
pub async fn start_guest(
    state: &Arc<AppState>,
    code: &str,
    sink: StateSink,
) -> Result<ConnectStateView, AppError> {
    let invite = Invite::decode(code)?;
    if invite.mc_port == 0 {
        return Err(AppError::validation(
            "房主还没开游戏",
            "这条邀请码里没有游戏端口，房主那边还没扫到游戏",
        )
        .with_hint("让房主先点「对局域网开放」，再重新复制一条邀请码给你"));
    }
    ensure_not_own_room(&invite).await?;
    ensure_stopped().await;

    // 本机映射端口单独取：用 MC 端口的话，本机 25565 被占就绑不上。
    let local_port = pick_free_port()?;
    // 出口同样是本地代理：加入方也要出示令牌才能连上节点
    let relay = start_relay(state, detect_local_ip()?).await?;
    let plan = GuestPlan {
        room: invite.room.clone(),
        secret: invite.secret.clone(),
        // 加入方的虚拟 IP 先按端口派生一个，避免多人撞同一个地址
        virtual_ip: guest_ip(local_port),
        host_ip: invite.host_ip,
        mc_port: invite.mc_port,
        local_port,
        forward_bind: xcl_connect::args::DEFAULT_FORWARD_BIND,
        listen_port: pick_free_port()?,
        rpc_port: pick_free_port()?,
        peer: peer_of(&relay)?,
        instance: "xcl-guest".into(),
    };
    let args = guest_args(&plan)?;

    let view = ConnectStateView {
        local_address: Some(format!("127.0.0.1:{local_port}")),
        ..ConnectStateView::from_phase(Phase::Connecting, "guest")
    };
    spawn_node(state, &args, plan.rpc_port, "guest", sink, view, relay, RoomIdentity {
        room: plan.room.clone(),
        secret: plan.secret.clone(),
        mc_port: plan.mc_port,
        forward_ip: plan.host_ip,
        invite: None,
        // 加入方不需要：节点地址来自邀请码，不是本地配置
        node: String::new(),
    })
    .await
}

/// 拦住「把自己房间的邀请码填进加入框」。
///
/// 这个操作一定会失败：`start_guest` 会先 `ensure_stopped()` 把房主进程杀掉，
/// 然后用同一套房间名/密码去连一个已经不存在的房主，界面就永远卡在「连接中」。
/// 与其让它卡住，不如直接说清楚。
async fn ensure_not_own_room(invite: &Invite) -> Result<(), AppError> {
    let slot = session().lock().await;
    let Some(running) = slot.as_ref() else {
        return Ok(());
    };
    if running.role == "host" && running.room == invite.room && running.secret == invite.secret {
        return Err(AppError::validation(
            "这是你自己创建的房间",
            "本机已经在以房主身份待在这个房间里了",
        )
        .with_hint("把邀请码发给朋友让他加入，本机不用再填一次"));
    }
    Ok(())
}

/// 停掉当前会话。没在跑也当成功。
pub async fn stop() -> ConnectStateView {
    let mut slot = session().lock().await;
    if let Some(mut running) = slot.take() {
        running.cancel.cancel();
        let _ = running.child.kill().await;
        // 子进程可能还有孙子（easytier 会自己起线程，不一定有子进程），Windows 上补一刀
        #[cfg(windows)]
        {
            if let Some(pid) = running.child.id() {
                // taskkill 同样是控制台程序，不隐藏就会闪黑框
                use std::os::windows::process::CommandExt as _;
                let mut kill = std::process::Command::new("taskkill");
                let _ = kill
                    .args(["/T", "/F", "/PID", &pid.to_string()])
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .creation_flags(0x0800_0000)
                    .status();
            }
        }
    }
    drop(slot);
    // 必须把 idle 写回 `last()`：切页回来时前端调的是 `connect_state`，
    // 它读的就是这里。不写的话卡片会在切 Tab 之后原地复活。
    let idle = ConnectStateView::idle();
    *last().lock().await = idle.clone();
    idle
}

/// 房主扫到游戏端口（或游戏换了端口）：重编邀请码，立刻推给界面。
///
/// 端口在邀请码里，所以它一变码就得跟着变 —— 界面只管重新复制，
/// 不用解释「为什么刚才那条码不能用了」。
pub async fn set_host_port(port: u16) -> Result<ConnectStateView, AppError> {
    let mut slot = session().lock().await;
    let Some(running) = slot.as_mut() else {
        return Err(AppError::unsupported("还没有房间", "先创建房间再设置端口"));
    };
    if running.role != "host" {
        return Err(AppError::unsupported(
            "加入方不需要设端口",
            "端口已经写在邀请码里了",
        ));
    }
    if port == 0 {
        return Err(AppError::validation("端口不合法", "0 不是一个可用的游戏端口"));
    }
    if *running.port.lock().unwrap_or_else(|e| e.into_inner()) == port {
        return Ok(last().lock().await.clone());
    }

    let code = Invite {
        room: running.room.clone(),
        secret: running.secret.clone(),
        // 必须用会话里存的那一份：现算的话网卡一变（插网线、开热点）就会算成别的地址，
        // 而房主进程导出的还是启动时那个网段，两条信息就对不上了。
        host_ip: running.forward_ip,
        mc_port: port,
        node: running.node.clone(),
    }
    .encode()?;
    *running.port.lock().unwrap_or_else(|e| e.into_inner()) = port;
    *running.invite.lock().unwrap_or_else(|e| e.into_inner()) = Some(code.clone());

    let sink = Arc::clone(&running.sink);
    let mut view = last().lock().await.clone();
    view.invite = Some(code);
    view.mc_port = Some(port);
    drop(slot);

    *last().lock().await = view.clone();
    sink(view.clone());
    Ok(view)
}

/// 清掉上次异常退出留下的联机进程。
///
/// 启动器崩了或被强杀时，子进程会活下来，占着端口还留着网卡设置。每次起新会话前扫一遍：
/// 进程名是我们自己固定的，按镜像名杀就行。单实例插件保证同时只有一个启动器，不会误伤。
pub fn cleanup_orphans() {
    #[cfg(windows)]
    {
        // taskkill 是控制台程序：不隐藏的话，每次开房前清理残留进程都会闪一下黑框
        use std::os::windows::process::CommandExt as _;
        let mut kill = std::process::Command::new("taskkill");
        let _ = kill
            .args(["/IM", "XCL联机模块.exe", "/F"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .creation_flags(0x0800_0000)
            .status();
    }
}

async fn ensure_stopped() {
    cleanup_orphans();
    let mut slot = session().lock().await;
    if let Some(mut running) = slot.take() {
        running.cancel.cancel();
        let _ = running.child.kill().await;
    }
}

/// 起本地代理，把 easytier 的出口接过来。
///
/// 节点现在只放行出示了登录令牌的连接，而 easytier 不会替我们出示 ——
/// 所以中间必须有这个代理。没登录就直接报错：让它带着「先登录」的提示
/// 停在开房按钮上，比让 easytier 连上去被 RST、再显示「网络异常」要清楚得多。
async fn start_relay(
    state: &Arc<AppState>,
    local: std::net::Ipv4Addr,
) -> Result<crate::connect_relay::Relay, AppError> {
    let remote = crate::connect_account::bridge_addr(state).ok_or_else(|| {
        AppError::parse("联机服务地址不合法", "读不出节点地址")
            .with_hint("检查账号服务器地址配置")
    })?;

    let holder = Arc::clone(state);
    let tokens: crate::connect_relay::TokenSource =
        Arc::new(move || crate::connect_account::token(&holder));

    if tokens().is_none() {
        return Err(AppError::validation("请先登录 XCL 账号", "没有登录就拿不到连节点的凭证")
            .with_hint("在联机页登录之后，再创建或加入房间"));
    }

    crate::connect_relay::start(local, remote, tokens).await
}

/// 本地代理的监听地址 → easytier 的 `-p` 出口。
fn peer_of(relay: &crate::connect_relay::Relay) -> Result<PeerEnd, AppError> {
    let addr = relay.addr();
    let std::net::IpAddr::V4(ip) = addr.ip() else {
        return Err(AppError::parse(
            "本地代理地址不合法",
            "代理监听在 IPv6 上，easytier 这边只支持 IPv4",
        ));
    };
    PeerEnd::new(ip, addr.port())
}

fn guest_ip(port: u16) -> std::net::Ipv4Addr {
    let last = (port % 200 + 20) as u8;
    std::net::Ipv4Addr::new(10, 144, 144, last)
}

async fn spawn_node(
    state: &Arc<AppState>,
    args: &[String],
    rpc_port: u16,
    role: &'static str,
    sink: StateSink,
    initial: ConnectStateView,
    relay: crate::connect_relay::Relay,
    identity: RoomIdentity,
) -> Result<ConnectStateView, AppError> {
    let status = state.connect_module_status();
    let core = status.core_path.ok_or_else(|| {
        AppError::not_found("联机组件还没准备好", "找不到 XCL联机模块.exe")
            .with_hint("先在联机页下载组件")
    })?;
    let cli = status.cli_path.ok_or_else(|| {
        AppError::not_found("联机组件还没准备好", "找不到 XCL联机模块 CLI.exe")
    })?;
    let dir = PathBuf::from(&core)
        .parent()
        .map(|path| path.to_path_buf())
        .unwrap_or_default();

    publish(&sink, initial.clone()).await;

    let mut command = Command::new(&core);
    // stderr 落盘：启动参数错、端口被占、缺少 dll 都只会写在这里。
    // 不留这个文件的话，进程秒退时我们只能看到「没了」，完全没法诊断。
    let log_file = std::fs::File::create(dir.join("xcl-connect.log")).ok();
    command
        .args(args)
        // cwd 指组件目录：et.db 会落在那里，不污染别处
        .current_dir(&dir)
        .stdout(match log_file.as_ref() {
            Some(file) => Stdio::from(file.try_clone().expect("克隆日志句柄")),
            None => Stdio::null(),
        })
        .stderr(match log_file {
            Some(file) => Stdio::from(file),
            None => Stdio::null(),
        })
        .kill_on_drop(true);
    #[cfg(windows)]
    {
        // CREATE_NO_WINDOW：不弹黑框
        command.creation_flags(0x0800_0000);
    }

    let child = command.spawn().map_err(|err| {
        AppError::io(&err, "启动联机模块").with_hint("可能是杀毒软件拦了，把它加进白名单")
    })?;

    // 把它的命拴在启动器上：启动器一死（退出 / 崩溃 / 被强杀），Windows 内核直接终结它。
    // 这是「幽灵进程」的根治手段 —— 退出清理代码只能覆盖正常退出的那条路。
    if !crate::connect_job::bind(&child) {
        // 绑不上（极少数环境不支持嵌套 job）也不影响功能，退出时的 taskkill 仍然兜着
        tracing::warn!("联机模块没能绑定到 Job Object，退出时依赖 taskkill 清理");
    }

    let cancel = CancellationToken::new();
    // 轮询每一轮都要带上房间名和当前邀请码（扫到端口后会变），
    // 所以在这里共享一份，而不是让两个地方各存一个副本。
    let room_label = identity.room.clone();
    let invite_cell = Arc::new(std::sync::Mutex::new(identity.invite.clone()));
    let port_cell = Arc::new(std::sync::Mutex::new(identity.mc_port));
    *session().lock().await = Some(Session {
        child,
        cancel: cancel.clone(),
        role,
        room: identity.room,
        secret: identity.secret,
        port: Arc::clone(&port_cell),
        invite: Arc::clone(&invite_cell),
        forward_ip: identity.forward_ip,
        node: identity.node,
        sink: Arc::clone(&sink),
        _relay: relay,
    });

    // 轮询任务
    let poll_sink = Arc::clone(&sink);
    let poll_ports = Arc::clone(&port_cell);
    // 进服地址是会话里定死的，但轮询每轮都从 from_phase() 重建 view（local_address = None），
    // 不带一份进去，加入方的地址 2 秒后就从界面上消失了。
    let poll_address = initial.local_address.clone();
    let cli_path = PathBuf::from(cli);
    tokio::spawn(async move {
        let mut current = Phase::Connecting;
        let mut streak = 0u32;
        let mut ever_online = false;
        // 绕中继持续了多久（按轮数算）。一直打不通就直接说别等了。
        let mut relay_ticks = 0u32;
        loop {
            if cancel.is_cancelled() {
                break;
            }
            tokio::time::sleep(POLL_INTERVAL).await;
            if cancel.is_cancelled() {
                break;
            }

            match query_peers(&cli_path, rpc_port).await {
                Ok(peers) => {
                    streak = 0;
                    let summary: LinkSummary = summarize(&peers);
                    let next = if summary.online { Phase::Connected } else { Phase::Connecting };
                    current = advance(current, next);
                    if summary.online {
                        ever_online = true;
                    }
                    let code = invite_cell
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .clone();
                    let mut view = ConnectStateView::from_phase(current, role);
                    view.local_address = poll_address.clone();
                    apply_summary(&mut view, &summary, &peers, role, code.as_deref());
                    // 连上过、现在列表里没人了 —— 这不是「正在等待」，
                    // 是「对方走了」。分开说，否则两边界面会各说各话。
                    if ever_online && !summary.online {
                        view.label = if role == "host" {
                            "好友已离开房间".to_string()
                        } else {
                            "房主已离开房间".to_string()
                        };
                    }
                    // 一直只能靠中继转发（绕服务器），说明打洞还没成功。
                    // 打洞本身要时间（对称 NAT 下经常几十秒才穿），所以这里给得比较宽松，
                    // 只有真的拖到 90 秒还没直连，才提示可以换个网络试试。
                    if summary.online {
                        relay_ticks = 0;
                    } else if summary.relay_players > 0 {
                        relay_ticks += 1;
                        if relay_ticks >= RELAY_HINT_TICKS {
                            view.label = "网络无法直连，建议换个网络再试".to_string();
                        }
                    } else {
                        relay_ticks = 0;
                    }
                    view.room = Some(room_label.clone());
                    if role == "host" {
                        // 端口 0 表示还没扫到游戏，界面上就不显示
                        let port = *poll_ports.lock().unwrap_or_else(|e| e.into_inner());
                        view.mc_port = (port > 0).then_some(port);
                    }
                    publish(&poll_sink, view).await;
                }
                Err(_) => {
                    streak += 1;
                    // 连不上管理接口就先忍着：刚起来的头一两秒本来就查不到
                    if streak >= FAIL_LIMIT {
                        current = advance(current, Phase::Failed);
                        publish(
                            &poll_sink,
                            ConnectStateView {
                                // 曾经连上过就说明是中途失联，文案得说清是「断了」，
                                // 而不是含糊的「没响应」——据此决定要不要重开房间。
                                error: Some(if ever_online {
                                    "与联机模块的连接中断了，请退出房间后重试".into()
                                } else {
                                    "联机模块没有响应，请退出房间后重试".into()
                                }),
                                local_address: poll_address.clone(),
                                ..ConnectStateView::from_phase(current, role)
                            },
                        )
                        .await;
                        break;
                    }
                }
            }
        }
    });

    Ok(initial)
}

/// 跑一次 `XCL联机模块 CLI.exe -o json -p <rpc> peer list`。
async fn query_peers(cli: &std::path::Path, rpc_port: u16) -> Result<Vec<xcl_connect::Peer>, AppError> {
    let portal = format!("127.0.0.1:{rpc_port}");
    let mut command = Command::new(cli);
    command
        .args(["-o", "json", "-p", &portal, "peer", "list"])
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        // 必须有这一行。CLI 是控制台程序，而启动器是 GUI 子系统（没有控制台），
        // 从资源管理器双击启动时，每 spawn 一次就会新分配一个控制台窗口 ——
        // 轮询每两秒一次，看到的就是「黑框反复闪」。从终端启动时子进程会
        // 继承父控制台，所以开发机上反而看不到这个问题。
        command.creation_flags(0x0800_0000);
    }

    let output = tokio::time::timeout(Duration::from_secs(3), command.output())
        .await
        .map_err(|_| AppError::network("查询联机状态超时", "联机模块 3 秒没有返回"))?
        .map_err(|err| AppError::io(&err, "调用联机模块"))?;

    if !output.status.success() {
        return Err(AppError::network(
            "查询联机状态失败",
            format!("联机模块返回 {}", output.status),
        ));
    }
    let text = String::from_utf8_lossy(&output.stdout);
    parse_peers(&text)
}

/// 把轮询结果写进视图。
///
/// `label` 也在这里定：状态文案是「后端算好」的一部分，
/// 房主在等人时不该显示「连接中」，那会让人以为在连别人。
fn apply_summary(
    view: &mut ConnectStateView,
    summary: &LinkSummary,
    peers: &[xcl_connect::Peer],
    role: &str,
    invite: Option<&str>,
) {
    view.players = summary.players;
    view.nodes = summary.nodes;
    view.online = summary.online;
    view.via_relay = summary.via_relay;
    view.best_name = summary.best.as_ref().map(|peer| peer.display_name().to_string());
    view.best_latency_ms = summary.best_latency_ms;
    // 邀请码是房主的全部凭据，轮询每一轮都要带着它，
    // 否则第一秒之后界面上的邀请码就消失了（曾经真的这样）。
    view.invite = invite.map(|code| code.to_string());

    // 设备列表：谁在这个房间里，一眼看全（本机 / 房主 / 成员 / 服务器）。
    view.devices = devices(peers, role == "host")
        .into_iter()
        .map(|device| DeviceView {
            name: device.name,
            label: device.role.label().to_string(),
            kind: device.role.key().to_string(),
            address: device.address,
            latency_ms: device.latency_ms,
            via_relay: device.via_relay,
        })
        .collect();

    view.label = if summary.online {
        "已连接".to_string()
    } else if summary.relay_players > 0 {
        // 绕中继 = 链路通但没打穿，看起来还是「在连」
        "正在建立直连…".to_string()
    } else if role == "host" {
        "等待好友加入".to_string()
    } else {
        "正在连接房主".to_string()
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 房间名与密码是必填项：空着不该偷偷生成一个，否则根本不知道自己的房间叫什么。
    #[test]
    fn empty_room_or_secret_is_rejected() {
        let err = validate_identity("   ", "secret").expect_err("房间名为空应当被拒");
        assert!(err.title.contains("房间名"), "{}", err.title);
        assert!(err.hint.is_some(), "要写清楚怎么填");

        let err = validate_identity("room", "").expect_err("密码为空应当被拒");
        assert!(err.title.contains("密码"), "{}", err.title);
    }

    #[test]
    fn identity_is_trimmed() {
        let (room, secret) = validate_identity("  我的房间  ", " pw ").unwrap();
        assert_eq!(room, "我的房间");
        assert_eq!(secret, "pw");
    }
}
