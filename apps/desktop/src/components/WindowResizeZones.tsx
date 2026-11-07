import type { PointerEvent as ReactPointerEvent } from "react";
import { Box } from "@mui/material";
import { getCurrentWindow } from "@tauri-apps/api/window";

/**
 * 窗口四边 / 四角的缩放热区。
 *
 * `decorations: false` 的自绘标题栏窗口，Windows 原生的 resize 边框只有
 * 几像素，很难瞄。这里在窗口内缘铺一圈透明热区，按下即进入 OS 级缩放
 * （`startResizeDragging`），手感与原生边框一致，命中区还大得多。
 *
 * 两个刻意的设计：
 * - z-index 压在内容之上、Dialog（1300）之下 —— 弹窗打开时遮罩自动
 *   盖住热区，不用手动禁用；
 * - 容器 `pointer-events: none`、只有热区本身 `auto` —— 中间的页面内容
 *   完全不受影响。
 */

/** 四边热区厚度（px）。原生约 4px，这里放到 6，手感明显好瞄。 */
const EDGE = 6;
/** 四角热区边长（px），对角线方向。 */
const CORNER = 14;

type ZoneSpec = {
  /** 传给 startResizeDragging 的方向 */
  dir:
    | "North"
    | "South"
    | "West"
    | "East"
    | "NorthWest"
    | "NorthEast"
    | "SouthWest"
    | "SouthEast";
  /** 光标形状 */
  cursor: string;
  sx: Record<string, unknown>;
};

const ZONES: ZoneSpec[] = [
  // 四边（避开角，让角自己管对角线）
  {
    dir: "North",
    cursor: "ns-resize",
    sx: { top: 0, left: CORNER, right: CORNER, height: EDGE },
  },
  {
    dir: "South",
    cursor: "ns-resize",
    sx: { bottom: 0, left: CORNER, right: CORNER, height: EDGE },
  },
  {
    dir: "West",
    cursor: "ew-resize",
    sx: { left: 0, top: CORNER, bottom: CORNER, width: EDGE },
  },
  {
    dir: "East",
    cursor: "ew-resize",
    sx: { right: 0, top: CORNER, bottom: CORNER, width: EDGE },
  },
  // 四角
  {
    dir: "NorthWest",
    cursor: "nwse-resize",
    sx: { top: 0, left: 0, width: CORNER, height: CORNER },
  },
  {
    dir: "NorthEast",
    cursor: "nesw-resize",
    sx: { top: 0, right: 0, width: CORNER, height: CORNER },
  },
  {
    dir: "SouthWest",
    cursor: "nesw-resize",
    sx: { bottom: 0, left: 0, width: CORNER, height: CORNER },
  },
  {
    dir: "SouthEast",
    cursor: "nwse-resize",
    sx: { bottom: 0, right: 0, width: CORNER, height: CORNER },
  },
];

export default function WindowResizeZones() {
  const onPointerDown = (dir: ZoneSpec["dir"]) => (event: ReactPointerEvent<HTMLDivElement>) => {
    // 只接管左键；进入 OS 缩放循环后 JS 不会再收到 pointerup，无需清理
    if (event.button !== 0) return;
    event.preventDefault();
    void getCurrentWindow().startResizeDragging(dir).catch(() => undefined);
  };

  return (
    <Box
      aria-hidden="true"
      sx={{ position: "fixed", inset: 0, zIndex: 1200, pointerEvents: "none" }}
    >
      {ZONES.map((zone) => (
        <Box
          key={zone.dir}
          onPointerDown={onPointerDown(zone.dir)}
          sx={{
            position: "absolute",
            pointerEvents: "auto",
            cursor: zone.cursor,
            ...zone.sx,
          }}
        />
      ))}
    </Box>
  );
}
