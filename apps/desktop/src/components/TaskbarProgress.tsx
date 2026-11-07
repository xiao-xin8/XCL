import { useEffect, useRef } from "react";
import { getCurrentWindow, ProgressBarStatus } from "@tauri-apps/api/window";
import { useAllTasks } from "../lib/downloadStore";
import { summarizeTaskProgress } from "../lib/taskbarProgress";

/**
 * 把下载进度同步到任务栏图标（Windows 的叫法，macOS 是 Dock）。
 *
 * 为什么单开一个组件而不是塞进 `DownloadProvider`：`useAllTasks` 会让调用方
 * 随每条进度事件重渲染，塞进 Provider 等于把所有子组件拖着一起重画 ——
 * 下载时整页掉帧的老毛病就是这么来的。这里返回 `null`，重渲染只发生在自己身上。
 *
 * 只在真的变化时打一次 IPC：进度按整数百分比比较，否则每 250ms 一条事件
 * 就会变成一次跨进程调用。
 */
export default function TaskbarProgress() {
  const tasks = useAllTasks();
  const lastKey = useRef<string>("none");

  useEffect(() => {
    const state = summarizeTaskProgress(tasks);
    const percent = state?.progress === undefined ? null : Math.round(state.progress);
    const key = state === null ? "none" : `${state.status}:${percent ?? ""}`;
    if (key === lastKey.current) return;
    lastKey.current = key;

    const win = getCurrentWindow();
    const payload =
      state === null
        ? { status: ProgressBarStatus.None }
        : state.status === "error"
          ? { status: ProgressBarStatus.Error, progress: percent ?? 100 }
          : state.status === "indeterminate"
            ? { status: ProgressBarStatus.Indeterminate }
            : { status: ProgressBarStatus.Normal, progress: percent ?? 0 };

    // 窗口被销毁（托盘模式）/ 浏览器里跑 / 缺权限：都不该影响下载本身
    void win.setProgressBar(payload).catch(() => undefined);
  }, [tasks]);

  return null;
}
