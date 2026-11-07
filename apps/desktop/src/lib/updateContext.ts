import { createContext, useContext } from "react";
import type { UpdateView } from "./types";

/**
 * 自更新的 context 定义单独放这里。
 *
 * 不放在 `UpdateProvider.tsx` 里的原因很实际：`UpdateProvider` 要渲染
 * `UpdateDialog`，而 `UpdateDialog` 又要用这些 hook —— 放在一起就是循环引用。
 * 拆出来两边都只依赖这个文件，图就成了一条线。
 */

export type UpdateActions = {
  /** `auto = true` 是启动时的自动检查：不主动弹窗，静默失败 */
  check: (auto?: boolean) => Promise<void>;
  download: () => Promise<void>;
  cancel: () => Promise<void>;
  /** 重启并更新。成功之后主进程几秒内会自己退出。 */
  apply: () => Promise<void>;
  /** 忽略当前这个版本；强制更新时前端不给这条路 */
  skip: () => Promise<void>;
  openDialog: () => void;
  closeDialog: () => void;
};

export type UpdateUiState = {
  view: UpdateView | null;
  open: boolean;
};

export const UpdateActionsContext = createContext<UpdateActions | null>(null);
export const UpdateStateContext = createContext<UpdateUiState>({
  view: null,
  open: false,
});

export function useUpdateActions(): UpdateActions {
  const value = useContext(UpdateActionsContext);
  if (!value) throw new Error("useUpdateActions 必须在 <UpdateProvider> 内使用");
  return value;
}

export function useUpdateState(): UpdateUiState {
  return useContext(UpdateStateContext);
}
