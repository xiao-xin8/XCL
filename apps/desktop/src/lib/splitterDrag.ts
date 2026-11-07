import { useSyncExternalStore } from "react";

/**
 * 分栏手柄的「正在拖拽」全局状态。
 *
 * 为什么需要它：拖 Splitter 手柄时，被它分隔的两个面板尺寸在连续变化 ——
 * 皮肤预览里的 WebGL canvas 每帧重画会闪。需求是「摁住手柄立刻把预览换成
 * 静态占位，松手再恢复」，而拖拽状态只存在于 Splitter 组件内部，预览在
 * 另一棵子树里，所以用这个模块级小广播把状态送出去。
 *
 * 只存布尔（有没有手柄正在被拖），不存方向 —— 订阅方不关心是哪根。
 */

let dragging = false;
const listeners = new Set<() => void>();

function emit() {
  listeners.forEach((listener) => listener());
}

/** Splitter 在拖拽开始 / 结束时调用。 */
export function setSplitterDragging(value: boolean): void {
  if (dragging === value) return;
  dragging = value;
  emit();
}

function subscribe(listener: () => void): () => void {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

function getSnapshot(): boolean {
  return dragging;
}

/** 当前是否有 Splitter 手柄正在被拖。 */
export function useSplitterDragging(): boolean {
  return useSyncExternalStore(subscribe, getSnapshot, getSnapshot);
}
