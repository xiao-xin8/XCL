import {
  useCallback,
  useEffect,
  useRef,
  useState,
  type KeyboardEvent as ReactKeyboardEvent,
  type PointerEvent as ReactPointerEvent,
} from "react";
import { Box } from "@mui/material";
import { setSplitterDragging } from "../lib/splitterDrag";

/**
 * 可拖拽分栏条（splitter）。
 *
 * 放在两个 flex 面板中间当第三个子元素，拖它就能改面板占比；比例由父组件
 * 持有并持久化（localStorage），这里只负责「把手势翻译成比例」。
 *
 * 标准 splitter 交互（对齐 VS Code / 各类 IDE）：
 * - 拖拽：pointer capture 接管，指针划出条外也继续收事件，松手结束
 * - 键盘：方向键 / Home / End 微调（ARIA separator 规范）
 * - 双击：复位到默认比例
 * - 拖拽中全局禁文字选中 + 光标变 resize，否则会连带选中一堆文本
 * - 分隔线常驻（divider 色），悬停 / 拖拽时原地换成主色高亮 ——
 *   两侧面板自己不画 border，整条分隔只此一条线，高亮永不偏移
 */
export type SplitterProps = {
  /** vertical = 左右分栏的竖条；horizontal = 上下分栏的横条 */
  orientation: "vertical" | "horizontal";
  /** 当前比例 0~1（第一个面板占容器的比例） */
  value: number;
  /** 比例下限 / 上限，父组件按内容最小尺寸给 */
  min: number;
  max: number;
  onChange: (value: number) => void;
  /** 双击复位的目标比例 */
  resetValue: number;
};

/** 键盘每按一次的步进。 */
const STEP = 0.02;
/** 命中区宽度（比可见的线宽不少，好拖）。 */
const THICKNESS = 10;
/** 可见的线宽。线常驻显示（divider 色），高亮只是原地变色 —— 几何不动，永不偏移。 */
const LINE = 2;

export default function Splitter(props: SplitterProps) {
  const { orientation, value, min, max, onChange, resetValue } = props;
  const [dragging, setDragging] = useState(false);
  const barRef = useRef<HTMLDivElement>(null);

  const clamp = useCallback((v: number) => Math.min(max, Math.max(min, v)), [min, max]);

  /** 把手势位置换算成容器比例。容器 = 分栏条的父节点（两个面板 + 条本身）。 */
  const ratioFrom = useCallback(
    (clientX: number, clientY: number) => {
      const host = barRef.current?.parentElement;
      if (!host) return value;
      const rect = host.getBoundingClientRect();
      if (rect.width === 0 || rect.height === 0) return value;
      const raw =
        orientation === "vertical"
          ? (clientX - rect.left) / rect.width
          : (clientY - rect.top) / rect.height;
      return clamp(raw);
    },
    [clamp, orientation, value],
  );

  const handlePointerDown = (event: ReactPointerEvent<HTMLDivElement>) => {
    // 只接管左键 / 单指
    if (event.button !== 0) return;
    event.preventDefault();
    barRef.current?.setPointerCapture(event.pointerId);
    setDragging(true);
  };

  const handlePointerMove = (event: ReactPointerEvent<HTMLDivElement>) => {
    if (!dragging) return;
    onChange(ratioFrom(event.clientX, event.clientY));
  };

  const endDrag = (event: ReactPointerEvent<HTMLDivElement>) => {
    if (!dragging) return;
    if (barRef.current?.hasPointerCapture(event.pointerId)) {
      barRef.current.releasePointerCapture(event.pointerId);
    }
    setDragging(false);
  };

  const handleKeyDown = (event: ReactKeyboardEvent<HTMLDivElement>) => {
    // 两个方向都认：竖条用左右、横条用上下，另一轴忽略
    const shrink = orientation === "vertical" ? "ArrowLeft" : "ArrowUp";
    const grow = orientation === "vertical" ? "ArrowRight" : "ArrowDown";
    if (event.key === shrink) {
      event.preventDefault();
      onChange(clamp(value - STEP));
    } else if (event.key === grow) {
      event.preventDefault();
      onChange(clamp(value + STEP));
    } else if (event.key === "Home") {
      event.preventDefault();
      onChange(min);
    } else if (event.key === "End") {
      event.preventDefault();
      onChange(max);
    }
  };

  // 拖拽期间：全局禁选中 + 光标常驻 resize。
  // 光标必须挂 body —— 指针划到旁边面板里时，条自身已经管不到了。
  useEffect(() => {
    if (!dragging) return;
    const cursor = orientation === "vertical" ? "col-resize" : "row-resize";
    const prevCursor = document.body.style.cursor;
    const prevSelect = document.body.style.userSelect;
    document.body.style.cursor = cursor;
    document.body.style.userSelect = "none";
    return () => {
      document.body.style.cursor = prevCursor;
      document.body.style.userSelect = prevSelect;
    };
  }, [dragging, orientation]);

  // 向全局广播「有手柄正在被拖」：皮肤预览收到后立刻换成静态占位 ——
  // 拖手柄时面板尺寸连续变化，WebGL canvas 每帧重画会闪，不渲染才不闪。
  // 松手时 dragging 归 false，预览重挂 3D 并按新尺寸重算（「松开刷新」）。
  useEffect(() => {
    if (dragging) setSplitterDragging(true);
    return () => {
      if (dragging) setSplitterDragging(false);
    };
  }, [dragging]);

  const vertical = orientation === "vertical";
  // 可见的线：一次算全，避免 sx 对象展开时把前面的 &::after 整键覆盖掉
  const lineSx = vertical
    ? {
        left: (THICKNESS - LINE) / 2,
        top: 0,
        bottom: 0,
        width: LINE,
        height: "auto",
      }
    : {
        top: (THICKNESS - LINE) / 2,
        left: 0,
        right: 0,
        height: LINE,
        width: "auto",
      };

  return (
    <Box
      ref={barRef}
      role="separator"
      aria-orientation={vertical ? "vertical" : "horizontal"}
      aria-valuenow={Math.round(value * 100)}
      aria-valuemin={Math.round(min * 100)}
      aria-valuemax={Math.round(max * 100)}
      tabIndex={0}
      onPointerDown={handlePointerDown}
      onPointerMove={handlePointerMove}
      onPointerUp={endDrag}
      onPointerCancel={endDrag}
      onKeyDown={handleKeyDown}
      onDoubleClick={() => onChange(resetValue)}
      sx={{
        flexShrink: 0,
        position: "relative",
        zIndex: 2,
        // 竖条：宽 6 占位、线竖在中间；横条：高 6、线横在中间
        width: vertical ? THICKNESS : "100%",
        height: vertical ? "100%" : THICKNESS,
        cursor: vertical ? "col-resize" : "row-resize",
        touchAction: "none",
        // 焦点环画在命中区内侧，别把整条都描边
        outlineOffset: -2,
        // 注意：所有 &::after 属性必须写在同一个对象里 —— sx 展开时后出现的
        // &::after 会整键覆盖前面的（v1 就这么丢过 content 和几何）。
        // 高亮只换色不挪位置，所以 hover / 拖拽都不可能产生偏移。
        "&::after": {
          content: '""',
          position: "absolute",
          ...lineSx,
          borderRadius: 999,
          bgcolor: dragging ? "brand.ink" : "divider",
          opacity: dragging ? 1 : undefined,
          transition: "background-color 120ms ease, opacity 120ms ease",
        },
        "&:hover::after": dragging
          ? undefined
          : { bgcolor: "brand.ink", opacity: 0.55 },
      }}
    />
  );
}
