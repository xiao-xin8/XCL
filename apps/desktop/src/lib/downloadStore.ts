import { useSyncExternalStore } from "react";
import type { TaskView } from "./types";
import { isPhaseRollback, shouldAccept } from "./taskPhase.ts";

/**
 * 下载任务的数据源（模块级）。
 *
 * 为什么不在 React state 里：一条进度事件每 250ms 就来一次，放 context 里会
 * 让所有消费者（整个启动页、3D 预览、版本列表…）一起重渲染，下载时动画
 * 就掉帧。这里改成「谁要哪条就订阅哪条」：
 *
 * - 写入时只替换发生变化的那一条任务对象，其余引用保持不变；
 * - 选择器返回的引用没变，React 就不会重渲染那个组件。
 *
 * 事件订阅、Toast、成功后清场这些副作用留在 `DownloadProvider` 里，
 * 这里只存数据，保持纯函数式。
 */

type TaskMap = Record<string, TaskView>;

/** 单文件下载（Mod）任务的 `versionId` 前缀，后端用 `mod:<fileId>`。 */
const FILE_TASK_PREFIX = "mod:";

function isFileTask(view: TaskView): boolean {
  return view.versionId.startsWith(FILE_TASK_PREFIX);
}

/**
 * 本轮已经下完的「文件@落点」。
 *
 * 成功的任务 1.2s 后就被清掉了，但可能还在连着下别的：
 * 行上的「已完成」不能跟着任务一起消失，否则那一行看起来又是「没下过」。
 */
const completedPairs = new Set<string>();

function pairKey(versionId: string, target: string): string {
  return `${versionId}@${target}`;
}

let tasks: TaskMap = {};
let starting: string | null = null;
let doneSignal = 0;
/** 单文件下载（Mod）完成的次数。资源详情页靠它刷新本地状态，不与版本安装混在一起。 */
let fileDoneSignal = 0;
/** 上次见到的阶段，用于「状态只能往前走」和「只在跃迁时弹提示」 */
const lastPhase: Record<string, TaskView["phase"]> = {};
/** 排序后的列表缓存：只有任务变化时才重建，保证快照引用稳定 */
let sortedCache: TaskView[] | null = null;

const listeners = new Set<() => void>();

function emit() {
  sortedCache = null;
  listeners.forEach((listener) => listener());
}

function subscribe(listener: () => void): () => void {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

/** 任务变化的结果，供 Provider 决定要不要弹提示 / 清场。 */
export type PublishResult = {
  accepted: boolean;
  /** 阶段真的向前跃迁了（用来触发 Toast，避免刷屏） */
  phaseChanged: boolean;
  previous: TaskView["phase"] | undefined;
};

const REJECTED: PublishResult = { accepted: false, phaseChanged: false, previous: undefined };

/**
 * 写入一条任务快照（事件推送、命令返回值都走这里）。
 *
 * 规则和之前完全一致：
 * - 迟到的旧快照（阶段回退）直接丢弃；
 * - 同阶段内只接受「有进展」的更新（`taskPhase.ts`）。
 */
export function publishTask(view: TaskView): PublishResult {
  const previous = lastPhase[view.taskId];
  if (previous !== undefined && isPhaseRollback(view.phase, previous)) {
    return { ...REJECTED, previous };
  }
  lastPhase[view.taskId] = view.phase;

  if (!shouldAccept(view, tasks[view.taskId])) {
    return { accepted: false, phaseChanged: false, previous };
  }

  tasks = { ...tasks, [view.taskId]: view };
  const phaseChanged = previous !== undefined && previous !== view.phase;
  if (isFileTask(view) && view.target) {
    const key = pairKey(view.versionId, view.target);
    // 重新开始下同一个「文件@落点」时，上一次的「已完成」要作废
    if (view.phase === "running") completedPairs.delete(key);
    if (phaseChanged && view.phase === "done") completedPairs.add(key);
  }
  if (phaseChanged && view.phase === "done") {
    doneSignal += 1;
    if (isFileTask(view)) fileDoneSignal += 1;
  }
  emit();
  return { accepted: true, phaseChanged, previous };
}

/** 用后端任务表整体对齐（挂载时 / 取消被拒时）。返回这次新收进来的任务。 */
export function syncTasks(list: TaskView[]): TaskView[] {
  const added: TaskView[] = [];
  for (const task of list) {
    const known = tasks[task.taskId];
    lastPhase[task.taskId] = task.phase;
    if (!known) {
      tasks = { ...tasks, [task.taskId]: task };
      added.push(task);
    } else if (shouldAccept(task, known)) {
      tasks = { ...tasks, [task.taskId]: task };
    }
  }
  if (added.length > 0) emit();
  return added;
}

/** 取消成功后把本地这条改成「已取消」（后端随后还会推一条，内容一致）。 */
export function markCancelled(taskId: string, message: string) {
  const current = tasks[taskId];
  if (!current || current.phase !== "running") return;
  lastPhase[taskId] = "cancelled";
  tasks = {
    ...tasks,
    [taskId]: { ...current, phase: "cancelled", speedBps: 0, etaSeconds: null, message },
  };
  emit();
}

/** 从队列里摘掉一条（本地 + 后端 forget）。 */
export function removeTask(taskId: string) {
  if (!tasks[taskId]) return;
  const next = { ...tasks };
  delete next[taskId];
  delete lastPhase[taskId];
  tasks = next;
  emit();
}

/** 读一条任务当前快照（不订阅）。给 Provider 做「还在跑的不让删」这类判断用。 */
export function peekTask(taskId: string): TaskView | undefined {
  return tasks[taskId];
}

export function setStarting(versionId: string | null) {
  if (starting === versionId) return;
  starting = versionId;
  emit();
}

// `useSyncExternalStore` 的快照必须引用稳定，否则会无限重渲染：
// 这里统一走缓存，只有任务集合真的变了才重建数组。
function sortedTasks(): TaskView[] {
  if (!sortedCache) {
    sortedCache = Object.values(tasks).sort((a, b) => b.startedAtMs - a.startedAtMs);
  }
  return sortedCache;
}

function readTask(taskId: string | null): TaskView | undefined {
  return taskId ? tasks[taskId] : undefined;
}

function readTaskForVersion(versionId: string | null): TaskView | undefined {
  if (!versionId) return undefined;
  return sortedTasks().find((task) => task.versionId === versionId);
}

/**
 * 某个「文件 + 落点」现在的任务。
 *
 * 同一个 Mod 文件装到两个实例时 `versionId` 完全一样，只有 `target`（实例 id
 * 或自定义目录）能区分 —— 只按文件找会把 A 实例的进度显示到 B 实例那一行。
 */
export function peekTaskForFile(
  fileId: string | null,
  target: string | null,
): TaskView | undefined {
  if (!fileId || !target) return undefined;
  const versionId = `${FILE_TASK_PREFIX}${fileId}`;
  return sortedTasks().find(
    (task) => task.versionId === versionId && (task.target ?? null) === target,
  );
}

/** 这一对「文件 + 落点」本轮是否已经下完（任务被清掉后行上仍显示「已完成」）。 */
export function isFilePairDone(
  fileId: string | null,
  target: string | null,
): boolean {
  if (!fileId || !target) return false;
  return completedPairs.has(pairKey(`${FILE_TASK_PREFIX}${fileId}`, target));
}

/** 全部任务（按开始时间倒序）。只有悬浮球 / 抽屉这类进度界面才该用它。 */
export function useAllTasks(): TaskView[] {
  return useSyncExternalStore(subscribe, sortedTasks, sortedTasks);
}

/** 精确订阅一条任务；不存在的 id 返回 `undefined`（快照稳定，不会引起重渲染）。 */
export function useTask(taskId: string | null): TaskView | undefined {
  return useSyncExternalStore(
    subscribe,
    () => readTask(taskId),
    () => readTask(taskId),
  );
}

/** 订阅「某个版本 id 对应的任务」（加载器实例用清单目标 id，Java 用 `java:<组件>`）。 */
export function useTaskForVersion(versionId: string | null): TaskView | undefined {
  return useSyncExternalStore(
    subscribe,
    () => readTaskForVersion(versionId),
    () => readTaskForVersion(versionId),
  );
}

/** 订阅「某个文件装到某个落点」的那条任务（Mod 下载用）。 */
export function useTaskForFile(
  fileId: string | null,
  target: string | null,
): TaskView | undefined {
  return useSyncExternalStore(
    subscribe,
    () => peekTaskForFile(fileId, target),
    () => peekTaskForFile(fileId, target),
  );
}

/** 每完成一个任务 +1。列表页靠它刷新，不必订阅整份任务表。 */
export function useDoneSignal(): number {
  return useSyncExternalStore(subscribe, () => doneSignal, () => doneSignal);
}

/** 每完成一个单文件下载 +1。资源详情页靠它刷新「已安装」，不与版本安装混在一起。 */
export function useFileDoneSignal(): number {
  return useSyncExternalStore(subscribe, () => fileDoneSignal, () => fileDoneSignal);
}

/** 正在准备清单的版本 id（安装按钮的「正在准备清单…」状态）。 */
export function useStarting(): string | null {
  return useSyncExternalStore(subscribe, () => starting, () => starting);
}

/** 当前完成计数（给测试 / 调试用，界面请用 `useDoneSignal`）。 */
export function peekDoneSignal(): number {
  return doneSignal;
}

/** 单文件下载的完成计数（测试用）。 */
export function peekFileDoneSignal(): number {
  return fileDoneSignal;
}
