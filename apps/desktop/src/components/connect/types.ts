/**
 * 联机页的数据形状。
 *
 * 全部是后端算好的视图对象 —— 前端只渲染，不在这里推导状态、不比字符串。
 * 单独成文件是因为主组件、会话卡、组件卡都要引用同一份定义，
 * 留在主文件里会让子组件反向 import 主文件。
 */

/** 组件状态。后端算好，前端只管渲染。 */
export type ConnectModuleStatus = {
  ready: boolean;
  /** 下载完了、正在校验和解压。这一步没有进度可报。 */
  installing: boolean;
  version: string | null;
  corePath: string | null;
  cliPath: string | null;
  bytes: number;
  message: string | null;
};

export type InstallTask = {
  taskId: string;
  versionId: string;
  phase: "running" | "done" | "failed" | "cancelled";
  fraction: number;
  doneBytes: number;
  totalBytes: number;
  speedBps: number;
  message: string | null;
};

/** 设备列表里的一行。后端已经把身份和顺序算好了。 */
export type ConnectDevice = {
  name: string;
  label: string;
  kind: "self" | "host" | "player" | "node";
  address: string | null;
  latencyMs: number | null;
  viaRelay: boolean;
};

/** 联机会话状态。后端算好文案，前端只渲染。 */
export type ConnectState = {
  phase: "idle" | "preparing" | "downloading" | "ready" | "connecting" | "connected" | "failed";
  label: string;
  role: string | null;
  /// 房间名，用来认出「还是上次那个房」
  room: string | null;
  /// 房主的游戏端口（已写进邀请码）。加入方为 null。
  mcPort: number | null;
  /// 房间里的真人数量（不含自己、不含中继服务器）
  players: number;
  /// 中继 / 发现服务器数量，只用于诊断
  nodes: number;
  online: boolean;
  viaRelay: boolean;
  bestName: string | null;
  bestLatencyMs: number | null;
  localAddress: string | null;
  invite: string | null;
  devices: ConnectDevice[];
  error: string | null;
};

/** 组件安装任务在任务表里的键前缀，和后端 `TASK_KEY` 保持一致。 */
export const MODULE_TASK_PREFIX = "connect-module";

/**
 * 会话是否正占着房间。
 *
 * 只有「空闲」和「失败」才允许再点创建 / 加入 —— 其余状态都代表已经有一个
 * 联机进程在跑，再来一次会把它顶掉（后端 `ensure_stopped` 会先杀进程）。
 */
export function sessionActive(session: ConnectState | null): boolean {
  if (!session) return false;
  return session.phase !== "idle" && session.phase !== "failed";
}

/** 页面内的三个"全屏"视图：入口选择 / 创建 / 加入。房间在看的时候另算一档。 */
export type ConnectView = "entry" | "create" | "join";
