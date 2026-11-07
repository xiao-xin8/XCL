//! 联机（EasyTier）纯逻辑层。
//!
//! 硬约束：零 IO、零 async、零进程 —— 全部可单测。
//! 会碰文件与进程的部分在 `xcl-service::connect`，这里只做「值 → 值」的变换：
//!
//! - [`invite`]：邀请码打包 / 解包（房号、密码、MC 端口、共享节点）
//! - [`args`]：Host / Guest 两套命令行拼装
//! - [`peer`]：`easytier-cli -o json` 输出的解析
//! - [`state`]：连接阶段状态机（只进不退）
//!
//! 所有参数与结论都有实测出处，见 `docs/CONNECT-TEST.md` 的「第二轮实测（2026-11-01）」。
//! 其中两条经验直接写进了代码，改动前请先看那篇文档：
//!
//! 1. `--use-smoltcp` 下 easytier 连不了 `127.0.0.1`（`code 10049`）——
//!    客户端本地代理必须绑网卡 IP，[`guest_args`] 会拒绝回环地址。
//! 2. 中继只开 TCP，UDP 不开 —— 否则等于留了一条绕过准入的通道。

pub mod args;
pub mod invite;
pub mod manifest;
pub mod peer;
pub mod state;

pub use args::{GuestPlan, HostPlan, PeerEnd, guest_args, host_args};
pub use invite::Invite;
pub use manifest::{
    Archive, CLI_INSTALLED, ConnectManifest, CORE_INSTALLED, ModuleFile, installed_name,
};
pub use peer::{LinkKind, NodeInfo, Peer, nat_type_label, parse_node, parse_peers};
pub use state::{
    Device, DeviceRole, LinkSummary, Phase, RELAY_DISPLAY_NAME, advance, devices, summarize,
};
