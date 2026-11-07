import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useRef,
  useState,
  type ReactNode,
} from "react";
import { listen } from "@tauri-apps/api/event";
import { invoke } from "@tauri-apps/api/core";
import {
  type LaunchEvent,
  type LaunchLog,
  type LaunchStatus,
} from "../lib/types";
import { useToast } from "./Toast";

/**
 * 全局唯一的游戏会话状态。
 *
 * 为什么放在 Provider 而不是页面里：游戏日志是持续推送的，随时可能切到
 * 别的 Tab 再切回来。状态挂在页面组件上会被卸载清空，回来就看不到日志了。
 *
 * 日志先攒在 ref 里、每 120ms 批量刷一次 —— MC 启动瞬间能出几百行，
 * 一行一次 setState 会把界面拖到卡顿。
 */

const MAX_LOGS = 3000;
const FLUSH_MS = 120;

type LaunchContextValue = {
  status: LaunchStatus | null;
  logs: LaunchLog[];
  /** 正在准备启动的版本 id */
  starting: string | null;
  /** 是否有游戏在跑 */
  running: boolean;
  start: (versionId: string) => Promise<void>;
  stop: () => Promise<void>;
  clearLogs: () => void;
};

const LaunchContext = createContext<LaunchContextValue | null>(null);

export function useLaunch(): LaunchContextValue {
  const value = useContext(LaunchContext);
  if (!value) throw new Error("useLaunch 必须在 LaunchProvider 内使用");
  return value;
}

export function LaunchProvider({ children }: { children: ReactNode }) {
  const toast = useToast();

  const [status, setStatus] = useState<LaunchStatus | null>(null);
  const [logs, setLogs] = useState<LaunchLog[]>([]);
  const [starting, setStarting] = useState<string | null>(null);

  // 待刷入界面的日志
  const pending = useRef<LaunchLog[]>([]);
  const lastPhase = useRef<string | null>(null);

  // 批量刷新日志
  useEffect(() => {
    const timer = window.setInterval(() => {
      if (pending.current.length === 0) return;
      const batch = pending.current;
      pending.current = [];
      setLogs((prev) => {
        const next = prev.concat(batch);
        return next.length > MAX_LOGS ? next.slice(next.length - MAX_LOGS) : next;
      });
    }, FLUSH_MS);
    return () => window.clearInterval(timer);
  }, []);

  // 订阅后端事件
  useEffect(() => {
    let disposed = false;
    const unlisten = listen<LaunchEvent>("launch://event", (event) => {
      if (disposed) return;
      const payload = event.payload;

      if (payload.kind === "log") {
        pending.current.push(payload);
        return;
      }

      // 状态事件
      const next: LaunchStatus = payload;
      setStatus((prev) => {
        // 终态不许被迟到的 running 覆盖（和下载任务同样的坑）
        if (prev && prev.sessionId === next.sessionId) {
          const isTerminal = (phase: string) =>
            phase === "exited" || phase === "failed" || phase === "cancelled";
          if (isTerminal(prev.phase) && !isTerminal(next.phase)) return prev;
        }
        return next;
      });

      const key = `${next.sessionId}:${next.phase}`;
      if (lastPhase.current === key) return;
      lastPhase.current = key;

      if (next.phase === "failed") {
        toast(`${next.versionId}：${next.message}`, "error");
      } else if (next.phase === "exited") {
        toast(next.message, "success");
      } else if (next.phase === "cancelled") {
        toast("游戏已关闭", "info");
      }
    });

    // 界面重挂时恢复已有的会话。
    // 这里不能走带缓存的 apiGet —— 会话状态是会变的，缓存一次就再也不更新了
    invoke<LaunchStatus[]>("launch_sessions")
      .then((list) => {
        if (disposed) return;
        if (list.length > 0) setStatus(list[0]);
      })
      .catch(() => {
        // 恢复失败不影响主流程，忽略
      });

    return () => {
      disposed = true;
      unlisten.then((fn) => fn()).catch(() => {});
    };
  }, [toast]);

  const start = useCallback(
    async (versionId: string) => {
      setStarting(versionId);
      setLogs([]);
      pending.current = [];
      try {
        // 直接 invoke：启动是有副作用的动作，走缓存会导致第二次点「启动」
        // 只拿到上次的返回值，游戏根本不会起来
        const initial = await invoke<LaunchStatus>("launch_start", { versionId });
        setStatus(initial);
      } catch (error: unknown) {
        // 后端在失败路径上一定会先推一条 `failed` 状态事件，提示在那里弹；
        // 这里再弹一次就重复了，所以只把错误继续抛给调用方
        throw error;
      } finally {
        setStarting(null);
      }
    },
    [toast],
  );

  const stop = useCallback(async () => {
    const target = status?.sessionId ?? status?.versionId;
    if (!target) return;
    await invoke<boolean>("launch_stop", { target }).catch(() => false);
  }, [status]);

  const clearLogs = useCallback(() => {
    pending.current = [];
    setLogs([]);
  }, []);

  const running =
    status?.phase === "running" || status?.phase === "preparing";

  return (
    <LaunchContext.Provider
      value={{ status, logs, starting, running, start, stop, clearLogs }}
    >
      {children}
    </LaunchContext.Provider>
  );
}
