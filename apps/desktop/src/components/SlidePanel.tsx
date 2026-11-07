import { Box, Paper } from "@mui/material";
import type { ReactNode } from "react";
import { animationsEnabled } from "../lib/appearance";

/**
 * 右侧滑入面板（详情页的「装哪个 / 装到哪」那一栏）。
 *
 * 参数逐字对齐 `VersionDetail` 里那块已经跑通的面板：宽度 0↔328 +
 * 内容 `translateX(16px→0)`，260ms `cubic-bezier(0.4, 0, 0.2, 1)`。
 * 之所以不直接复用那个组件：它绑死了版本安装的业务（预览 / 加载器 / 进度），
 * 抽成通用壳反而要改它。等以后有空再把 `VersionDetail` 换过来，视觉一致。
 *
 * 宽度变化用 CSS transition（不是 WAAPI）：它要跟着内容一起"撑开"，
 * 而宽高过渡 WAAPI 做起来更别扭。
 */
export default function SlidePanel(props: {
  open: boolean;
  /** 面板宽度，默认 328（和 `VersionDetail` 一致） */
  width?: number;
  children: ReactNode;
}) {
  const { open, width = 328, children } = props;
  const animate = animationsEnabled();
  const easing = "cubic-bezier(0.4, 0, 0.2, 1)";

  return (
    <Box
      sx={{
        width: open ? width : 0,
        ml: open ? 2 : 0,
        flexShrink: 0,
        overflow: "hidden",
        transition: animate
          ? `width 260ms ${easing}, margin-left 260ms ${easing}`
          : "none",
      }}
      aria-hidden={!open}
    >
      <Paper
        variant="outlined"
        sx={{
          width,
          height: "100%",
          display: "flex",
          flexDirection: "column",
          overflow: "hidden",
          borderRadius: 1,
          // 只做淡入，不位移：面板宽度本身在展开，内容再从右边滑进来会
          // 两个方向一起动，看着"飘"。淡入 200ms 就够（和版本详情同一档）。
          opacity: open ? 1 : 0,
          transition: animate ? "opacity 200ms ease" : "none",
          // 关着的时候别让它抢焦点，也别让里面的按钮被点到
          pointerEvents: open ? undefined : "none",
        }}
      >
        {children}
      </Paper>
    </Box>
  );
}
