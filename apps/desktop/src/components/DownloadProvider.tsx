import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useRef,
  type ReactNode,
} from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { invalidate } from "../lib/api";
import {
  markCancelled,
  peekTask,
  publishTask,
  removeTask,
  setStarting,
  syncTasks,
} from "../lib/downloadStore";
import type { LoaderRef, TaskView } from "../lib/types";
import TaskbarProgress from "./TaskbarProgress";
import { useToast } from "./Toast";

/**
 * 下载任务的动作入口（以及事件订阅、提示、清场这些副作用）。
 *
 * 任务数据不在这个 context 里 —— 那会让所有消费者随每条进度一起重渲染，
 * 下载时整个启动页（含 3D 预览）一起掉帧就是这么来的。
 * 要看进度请用 `lib/downloadStore` 的 `useTask` / `useTaskForVersion` / `useAllTasks`
 * 精确订阅；这里只负责把数据写进去。
 */

type DownloadActions = {
  /** `loader` 有值时装的是「原版 + 加载器」，任务 id 会是加载器实例的 id */
  start: (versionId: string, loader?: LoaderRef) => Promise<TaskView>;
  cancel: (taskId: string) => Promise<void>;
  /** 把一条已结束的任务从队列里删掉（后端也忘掉，重开界面不会再冒出来） */
  dismiss: (taskId: string) => Promise<void>;
};

/** 成功后队列里再留这么久，让卡片闪一下「已完成」，然后自动消失 */
const DONE_LINGER_MS = 1200;

const DownloadContext = createContext<DownloadActions | null>(null);

export function useDownloads(): DownloadActions {
  const value = useContext(DownloadContext);
  if (!value) {
    throw new Error("useDownloads 必须在 <DownloadProvider> 内使用");
  }
  return value;
}

export function DownloadProvider({ children }: { children: ReactNode }) {
  const toast = useToast();

  /** 成功后延迟删除的定时器，卸载时要清掉 */
  const reapTimers = useRef<number[]>([]);
  /** 已经排过清场的任务：同一条 done 反复到达时不要堆出一串定时器 */
  const reaping = useRef<Set<string>>(new Set());

  /** 把任务从队列里摘掉：队列只留「还得管」的东西 */
  const dismiss = useCallback(
    async (taskId: string) => {
      const current = peekTask(taskId);
      // 已经不在队列里了：没什么可做的
      if (!current) return;
      // 还在跑的不删 —— 但必须说一声，否则就是「点了没反应」
      if (current.phase === "running") {
        toast("正在下载中，先取消再删除", "info");
        return;
      }
      removeTask(taskId);
      try {
        // 后端也忘掉，否则重新挂载 / 重开界面时 `install_tasks` 又把它带回来
        await invoke<boolean>("install_forget", { taskId });
      } catch {
        /* 纯浏览器环境没有这个命令，忽略 */
      }
    },
    [toast],
  );

  /** 安装成功就把任务从队列里摘掉 */
  const reap = useCallback(
    (taskId: string) => {
      if (reaping.current.has(taskId)) return;
      reaping.current.add(taskId);
      const timer = window.setTimeout(() => {
        reaping.current.delete(taskId);
        reapTimers.current = reapTimers.current.filter((id) => id !== timer);
        void dismiss(taskId);
      }, DONE_LINGER_MS);
      reapTimers.current.push(timer);
    },
    [dismiss],
  );

  /**
   * 收一条进度：数据写进 store，提示只在阶段跃迁时弹。
   *
   * 但清场（`reap`）不看跃迁 —— 判据见下面 done 分支的注释，这是
   * 「下载完了但卡片永远留在队列里」的根因之一。
   */
  const accept = useCallback(
    (view: TaskView) => {
      const { accepted, phaseChanged } = publishTask(view);
      if (!accepted) return;

      // 单文件下载（Mod）带文件名，版本安装没有 → 用 id。别把 `mod:<fileId>` 露出来
      const name = view.title ?? view.versionId;

      if (view.phase === "done") {
        // 版本清单里带着「已安装」标记，装完必须让它失效，否则要等 5 分钟才更新
        invalidate("version_list");
        // 加载器列表里的「已安装」标记同理（装完要立刻看到）
        invalidate("loader_versions");
        // 资源详情页的目标 / 文件列表同理：不在那一页时下完也要能看到「已安装」
        invalidate("resource_targets");
        invalidate("resource_files");
        // 提示只在真的跃迁时弹（同一条 done 可能重复到达）
        if (phaseChanged) toast(`${name} 安装完成`, "success");
        // 清场不看 `phaseChanged`：任务「第一次出现在 store 里就是 done」
        // 时它是 false（`previous` 是 undefined），那种情况下定时器永远排不上，
        // 卡片就永久留在队列里 —— 而它已经出队，点它连涟漪都不会有。
        reap(view.taskId);
      } else if (view.phase === "failed" && phaseChanged) {
        toast(`${name}：${view.failed} 个文件下载失败`, "error");
      }
    },
    [reap, toast],
  );

  /** 用后端任务表对齐一次本地状态（兜底：事件与返回值都不一定可信）。 */
  const refresh = useCallback(async () => {
    try {
      const list = await invoke<TaskView[]>("install_tasks");
      syncTasks(list);
      // 按同步后的本地状态判断，而不是 `syncTasks` 的返回值：
      // 那个只返回「新收进来的」，已经在队列里、这次才被更新成 done 的会被漏掉，
      // 于是清场定时器不排，任务就永久挂在那儿。
      for (const task of list) {
        if (peekTask(task.taskId)?.phase === "done") reap(task.taskId);
      }
    } catch {
      /* 拿不到就算了，不影响主流程 */
    }
  }, [reap]);

  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;

    listen<TaskView>("download://progress", (event) => {
      if (!disposed) accept(event.payload);
    })
      .then((fn) => {
        if (disposed) fn();
        else unlisten = fn;
      })
      .catch(() => {
        /* 纯浏览器环境没有事件系统，忽略 */
      });

    // 界面重新挂载时补一次，避免看不到已经在跑的任务
    void refresh();

    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [accept, refresh]);

  // 卸载时把待删定时器清掉，别让回调落在已卸载的组件上
  useEffect(() => {
    const timers = reapTimers.current;
    return () => {
      for (const id of timers) window.clearTimeout(id);
    };
  }, []);

  const start = useCallback(async (versionId: string, loader?: LoaderRef) => {
    setStarting(versionId);
    try {
      const task = await invoke<TaskView>("install_start", {
        versionId,
        // 没选加载器就显式传 null：Rust 侧是 Option，省略也能过，但显式更清楚
        loader: loader ?? null,
      });
      // 后端可能在 invoke 返回之前就把任务跑完了（文件全都已存在），
      // `publishTask` 会挡住「用 running 覆盖 done」的旧快照
      publishTask(task);
      return task;
    } finally {
      setStarting(null);
    }
  }, []);

  const cancel = useCallback(
    async (taskId: string) => {
      // 后端 cancel 是同步的（把取消令牌打上），这里等它回来再改本地状态，
      // 保证"界面收掉进度条"和"后端真的停了"是同一件事。
      const accepted = await invoke<boolean>("install_cancel", { taskId });

      if (!accepted) {
        // 后端说这个任务早就结束了（典型场景：文件全都在，任务瞬间完成）。
        // 这时候把界面改成"已取消"是在造谣 —— 拉一次真实状态，让它显示成已安装。
        await refresh();
        toast("这个任务已经结束了", "info");
        return;
      }

      markCancelled(taskId, "已取消，已完成的文件保留");
      toast("已取消安装", "info");
    },
    [refresh, toast],
  );

  const value = useMemo<DownloadActions>(
    () => ({ start, cancel, dismiss }),
    [start, cancel, dismiss],
  );

  return (
    <DownloadContext.Provider value={value}>
      {/* 系统任务栏进度：自己订阅任务表，不把 Provider 拖着一起重渲染 */}
      <TaskbarProgress />
      {children}
    </DownloadContext.Provider>
  );
}
