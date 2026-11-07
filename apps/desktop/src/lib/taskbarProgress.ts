import type { TaskView } from "./types";

/**
 * 任务栏进度条该显示成什么样。
 *
 * 抽成纯函数是为了能单测：这段逻辑会直接影响系统级 UI（任务栏图标），
 * 出错的表现很刺眼 —— 比如「明明下完了还挂着 100%」，或者「有东西没下成
 * 却显示绿色完成」。
 */
export type TaskbarState = {
  status: "normal" | "indeterminate" | "error";
  /** 0~100。`indeterminate` 时没有意义 */
  progress?: number;
};

/**
 * 汇总所有任务 → 任务栏该显示什么。`null` 表示清除进度条。
 *
 * 规则：
 * - 有在跑的任务：按字节加起来算总进度（不是按任务条数，否则小文件会让进度乱跳）；
 *   总字节还是 0（正在拉清单 / 准备阶段）时给不确定态，让图标转起来；
 * - 没有在跑、但队列里还有失败：红条 100%，提醒「有东西没下成」；
 * - 其余（全成功 / 队列空）：清掉。
 */
export function summarizeTaskProgress(tasks: TaskView[]): TaskbarState | null {
  const running = tasks.filter((task) => task.phase === "running");
  if (running.length > 0) {
    const total = running.reduce((sum, task) => sum + task.totalBytes, 0);
    // 单个任务的 done 不该超过它的 total（进度账本按实际写盘量记，理论上不会越界，
    // 但除法一旦 >1 就会让任务栏卡在 100% 上不动，这里先夹住）
    const done = running.reduce(
      (sum, task) => sum + Math.min(task.doneBytes, task.totalBytes),
      0,
    );
    if (total <= 0) {
      return { status: "indeterminate" };
    }
    return {
      status: "normal",
      progress: Math.min(100, Math.max(0, (done / total) * 100)),
    };
  }

  if (tasks.some((task) => task.phase === "failed")) {
    return { status: "error", progress: 100 };
  }

  return null;
}
