import type { ReactNode } from "react";
import { Box } from "@mui/material";
import type { SxProps, Theme } from "@mui/material";
import { CARD_MOTION, EASE } from "../../motion/tokens";

/**
 * 展开 / 收起的容器：内容一直挂着，只是把轨道从 0 拉到 1。
 *
 * 为什么不是 `height: auto`：WAAPI / CSS 对 auto 没法插值，只能跳。
 * `grid-template-rows: 0fr ↔ 1fr` 对任意内容高度都成立，而且内容不会被卸载
 * —— 输入框里正在打的字不会因为收一下就丢掉。
 *
 * `axis="x"` 是给「一行里横着长出来」的控件用的（比如开关旁边的下拉）。
 */
export default function Reveal(props: {
  open: boolean;
  children: ReactNode;
  axis?: "x" | "y";
  /** 内层容器的样式（横排时常用 flex + 居中） */
  innerSx?: SxProps<Theme>;
  sx?: SxProps<Theme>;
}) {
  const { open, children, axis = "y", innerSx, sx } = props;
  const horizontal = axis === "x";

  return (
    <Box
      sx={{
        display: "grid",
        ...(horizontal
          ? { gridTemplateColumns: open ? "1fr" : "0fr" }
          : { gridTemplateRows: open ? "1fr" : "0fr" }),
        opacity: open ? 1 : 0,
        // 收起来时别让里面的控件还能被 tab 到
        pointerEvents: open ? "auto" : "none",
        transition: [
          `${horizontal ? "grid-template-columns" : "grid-template-rows"} ${CARD_MOTION.collapse}ms ${EASE.outFluent}`,
          `opacity ${open ? 180 : 140}ms ${EASE.outFluent}`,
        ].join(", "),
        ...sx,
      }}
    >
      <Box sx={{ minWidth: 0, minHeight: 0, overflow: "hidden", ...innerSx }}>
        {children}
      </Box>
    </Box>
  );
}
