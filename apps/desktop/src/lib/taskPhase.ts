import type { TaskPhase, TaskView } from "./types";

/**
 * 任务阶段的状态规则。
 *
 * 抽成纯函数的原因：这里出过一个很难看的 bug —— `install_start` 的返回值和
 * 进度事件抢跑，迟到的初始快照（`running`）把已经终态的视图覆盖回去，
 * 界面于是永远停在「取消安装」。规则本身很简单，但必须被测试钉住，
 * 否则下次改 `DownloadProvider` 很容易再破一次。
 */

/**
 * 阶段只能单向推进：`running` 最低，三个终态并列且不可回退。
 *
 * 三个终态同权重是刻意的：`done → cancelled` 这种也算合法（取消后
 * 后端补发状态），但 `done → running` 一定是错误数据。
 */
const PHASE_ORDER: Record<TaskPhase, number> = {
  running: 0,
  done: 1,
  failed: 1,
  cancelled: 1,
};

/** 任务是否已经结束。 */
export function isTerminal(phase: TaskPhase): boolean {
  return phase !== "running";
}

/**
 * 新视图是否可以覆盖已有记录。
 *
 * - 同阶段：接受（`running` 的进度帧必须持续更新）
 * - 向前推进：接受
 * - 终态回退到 `running`：拒绝
 */
export function shouldAccept(incoming: TaskView, existing?: TaskView): boolean {
  if (!existing) return true;
  return PHASE_ORDER[incoming.phase] >= PHASE_ORDER[existing.phase];
}

/** 阶段是否算「往前走了」—— 只有前进才值得弹提示。 */
export function isPhaseAdvance(next: TaskPhase, previous: TaskPhase): boolean {
  return PHASE_ORDER[next] > PHASE_ORDER[previous];
}

/** 阶段是否算「倒退」—— 迟到的旧快照，状态和记录都不该动。 */
export function isPhaseRollback(next: TaskPhase, previous: TaskPhase): boolean {
  return PHASE_ORDER[next] < PHASE_ORDER[previous];
}
