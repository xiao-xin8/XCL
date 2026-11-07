import { useCallback, useEffect, useMemo, useState, type ReactNode } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { UpdateView } from "../lib/types";
import {
  UpdateActionsContext,
  UpdateStateContext,
  type UpdateActions,
  type UpdateUiState,
} from "../lib/updateContext";
import UpdateDialog from "./UpdateDialog";

/**
 * 自更新状态。
 *
 * 和 `DownloadProvider` 的取舍不一样：更新只有一条流、一个视图，
 * 没有「很多任务要精确订阅」的问题，所以直接把 `UpdateView` 放进 context。
 *
 * 唯一要注意的是别让高频进度把整页拖下水：
 * 消费 `useUpdateState` 的只有 `UpdateGate` 和 `UpdateDialog`，
 * `UpdateGate` 的 children 是 props 传进来的稳定引用，所以它重渲染时
 * 子树会被 React 跳过，启动页的 3D 预览不会跟着掉帧。
 */
export function UpdateProvider(props: { autoCheck: boolean; children: ReactNode }) {
  const { autoCheck, children } = props;
  const [view, setView] = useState<UpdateView | null>(null);
  const [open, setOpen] = useState(false);

  const accept = useCallback((next: UpdateView) => setView(next), []);

  // 后端每推一次状态就换一份视图（下载时 250ms 一次）
  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;
    listen<UpdateView>("update://state", (event) => {
      if (!disposed) accept(event.payload);
    })
      .then((fn) => {
        if (disposed) fn();
        else unlisten = fn;
      })
      .catch(() => {
        /* 纯浏览器环境没有事件系统 */
      });
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [accept]);

  // 挂载时取一次状态（强制更新就在这里面，不依赖联网），然后自动检查
  useEffect(() => {
    let alive = true;
    void invoke<UpdateView>("update_status")
      .then((next) => {
        if (alive) accept(next);
      })
      .catch(() => undefined);

    if (autoCheck) {
      void invoke<UpdateView>("update_check", { auto: true })
        .then((next) => {
          if (alive) accept(next);
        })
        .catch(() => {
          // 断网、云端没开都会走这里：自动检查失败不该弹任何东西
        });
    }
    return () => {
      alive = false;
    };
  }, [accept, autoCheck]);

  const phase = view?.phase ?? "idle";
  const forced = view?.forced ?? false;

  // 发现新版本就自己弹出来。强制更新不看 phase —— 它从持久化状态起就一直为真
  useEffect(() => {
    if (forced || phase === "available") setOpen(true);
  }, [forced, phase]);

  const check = useCallback(
    async (auto = false) => {
      // 手动检查要立刻把弹窗打开，否则网络慢的时候以为什么都没发生
      if (!auto) setOpen(true);
      try {
        accept(await invoke<UpdateView>("update_check", { auto }));
      } catch {
        // 失败状态后端已经推过事件了，这里不用额外提示
      }
    },
    [accept],
  );

  const download = useCallback(async () => {
    try {
      accept(await invoke<UpdateView>("update_download"));
    } catch {
      /* 同上：失败会有事件 */
    }
  }, [accept]);

  const cancel = useCallback(async () => {
    try {
      await invoke<boolean>("update_cancel");
    } catch {
      /* 没在下载就什么都不做 */
    }
  }, []);

  const apply = useCallback(async () => {
    try {
      await invoke("update_apply");
    } catch {
      /* helper 没起来时后端已经把状态改成 failed 了 */
    }
  }, []);

  const latest = view?.latestVersion ?? null;
  const skip = useCallback(async () => {
    if (!latest) return;
    try {
      accept(await invoke<UpdateView>("update_skip", { version: latest }));
      setOpen(false);
    } catch {
      /* 写配置失败：保持弹窗，还能重试 */
    }
  }, [accept, latest]);

  const openDialog = useCallback(() => setOpen(true), []);
  const closeDialog = useCallback(() => setOpen(false), []);

  const actions = useMemo<UpdateActions>(
    () => ({ check, download, cancel, apply, skip, openDialog, closeDialog }),
    [check, download, cancel, apply, skip, openDialog, closeDialog],
  );

  const state = useMemo<UpdateUiState>(() => ({ view, open }), [view, open]);

  return (
    <UpdateActionsContext.Provider value={actions}>
      <UpdateStateContext.Provider value={state}>
        {children}
        <UpdateDialog />
      </UpdateStateContext.Provider>
    </UpdateActionsContext.Provider>
  );
}
