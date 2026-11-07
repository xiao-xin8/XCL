import { Box } from "@mui/material";
import { useLayoutEffect, useRef, type ReactNode } from "react";
import { collectUnits, playEnter, playExit } from "./pageMotion";
import { usePageTransition } from "./usePageTransition";

export type PageTransitionProps = {
  /** 当前页面的唯一标识（nav key、分类 key 都可以） */
  pageKey: string;
  /**
   * 按 visibleKey 渲染页面。
   *
   * 这里必须用 render 而不是 children：切换时旧页面还要留着播退场动画，
   * 直接传 children 的话 React 会立刻把旧节点卸载掉。
   */
  render: (key: string) => ReactNode;
  /**
   * 首次挂载是否播入场动画。嵌套在自己带入场动画的页面里时传 false，
   * 避免外层刚播完、内层又播一次。
   */
  initial?: boolean;
};

/**
 * 页面切换（旧页退场 → 换页 → 新页进场）：
 * 旧页面的块逐个淡出上移，110ms 后换页，新页面的块再逐个从上方落下。
 * 动画加在页面里的块上，页面容器和背景不动。
 */
export default function PageTransition({
  pageKey,
  render,
  initial = true,
}: PageTransitionProps) {
  const { visibleKey, phase } = usePageTransition(pageKey);
  const rootRef = useRef<HTMLDivElement>(null);

  // 切过一次页之后，之后每次换页（包括切回第一个 key）都要播入场动画
  const firstKey = useRef(pageKey);
  const hasChanged = useRef(false);
  if (pageKey !== firstKey.current) hasChanged.current = true;

  useLayoutEffect(() => {
    const root = rootRef.current;
    if (!root) return;
    if (phase === "exit") return playExit(root);
    if (!(initial || hasChanged.current)) return;

    /**
     * 页面是 `lazy()` 的：切过来的瞬间只有 Suspense 的 fallback，
     * 真正的页面要等 chunk 下载完才挂上。这时候 `playEnter` 一个块都收不到，
     * 动画等于播在空气上 —— 等页面挂上来之后就没有动画了（侧边栏尤其明显）。
     *
     * 所以这里轮询到「块出现」再播，最多等约 1.5 秒。
     */
    let frames = 0;
    let raf = 0;
    const play = () => {
      if (collectUnits(root).length > 0) {
        playEnter(root);
        return;
      }
      if (frames++ >= 90) return;
      raf = requestAnimationFrame(play);
    };
    play();
    return () => cancelAnimationFrame(raf);
  }, [visibleKey, phase, initial]);

  return (
    <Box
      ref={rootRef}
      sx={{
        flex: 1,
        minWidth: 0,
        minHeight: 0,
        display: "flex",
        flexDirection: "column",
        // 块从 -16px 起跑，不管会画到上方标题栏上；但硬裁又会切掉浮标 / 焦点圈
        // （浮标收缩后要 9~10px 的头部空间）。折中：`clip` 允许画出去 12px，
        // 只把入场动画多出来的那 4px 裁掉 —— 页面里正常内容不会被截。
        overflow: "clip",
        overflowClipMargin: "12px",
        pointerEvents: phase === "exit" ? "none" : undefined,
      }}
    >
      {render(visibleKey)}
    </Box>
  );
}
