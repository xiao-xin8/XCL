import { useEffect, useState } from "react";
import { DURATION } from "./tokens";

export type PagePhase = "enter" | "exit";

/**
 * 页面切换状态机（两段式：旧页退场 → 换页 → 新页进场）。
 *
 * React 在 key 变化时会立刻卸载旧页面，所以 visibleKey 要晚一拍更新：
 * 退场动画期间旧页面仍然挂着，播完再换成新页面。
 */
export function usePageTransition(pageKey: string) {
  const [visibleKey, setVisibleKey] = useState(pageKey);
  const [phase, setPhase] = useState<PagePhase>("enter");

  useEffect(() => {
    // 快速来回切时可能又切回当前页：取消退场，直接回到正常状态
    if (pageKey === visibleKey) {
      setPhase("enter");
      return;
    }

    setPhase("exit");
    const timer = setTimeout(() => {
      setVisibleKey(pageKey);
      setPhase("enter");
    }, DURATION.swap);

    return () => clearTimeout(timer);
  }, [pageKey, visibleKey]);

  return { visibleKey, phase };
}
