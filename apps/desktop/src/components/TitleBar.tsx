import { useEffect, useLayoutEffect, useRef, useState } from "react";
import {
  Box,
  IconButton,
  Tab,
  Tabs,
  Tooltip,
  alpha,
  useTheme,
} from "@mui/material";
import RemoveIcon from "@mui/icons-material/Remove";
import CropSquareIcon from "@mui/icons-material/CropSquare";
import FilterNoneIcon from "@mui/icons-material/FilterNone";
import CloseIcon from "@mui/icons-material/Close";
import DarkModeIcon from "@mui/icons-material/DarkMode";
import LightModeIcon from "@mui/icons-material/LightMode";
import { getCurrentWindow } from "@tauri-apps/api/window";
import TitleBarTitle from "./TitleBarTitle";

export type NavKey = "launch" | "download" | "connect" | "setting";

export const NAV: { key: NavKey; label: string }[] = [
  { key: "launch", label: "启动" },
  { key: "download", label: "下载" },
  { key: "connect", label: "联机" },
  { key: "setting", label: "设置" },
];

/** 标题栏高度。改这里会同时影响下方内容的布局。 */
export const TITLEBAR_HEIGHT = 52;

const TAB_HEIGHT = 40;
/** 胶囊高度必须等于 Tab 根节点高度：hover/背景按根节点裁剪，两者不等就一定错位。 */
const PILL_HEIGHT = TAB_HEIGHT;

export default function TitleBar(props: {
  current: NavKey;
  onChange: (key: NavKey) => void;
  onToggleMode: () => void;
  /** 实际显示的页签。设置里可以关掉「联机」，所以不直接用静态的 `NAV`。 */
  items?: { key: NavKey; label: string }[];
}) {
  const { current, onChange, onToggleMode, items = NAV } = props;
  const theme = useTheme();
  const isDark = theme.palette.mode === "dark";
  const [maximized, setMaximized] = useState(false);
  const barRef = useRef<HTMLDivElement>(null);

  /**
   * Tauri 的拖动区不向子元素继承，只认 `event.target` 自己有没有
   * `data-tauri-drag-region`。所以「看着是一整条标题栏、实际只有几个空隙能拖」，
   * 图标和文字全拖不动。
   *
   * 这里每次渲染后把标题栏里所有不是按钮的元素（包括 SVG 图标、文字、
   * 容器、胶囊的间隙）都打上拖动标记；按钮（Tab / IconButton / 返回键）
   * 及带 `data-no-drag` 的子树保持可点击、不拖。
   */
  useLayoutEffect(() => {
    const root = barRef.current;
    if (!root) return;
    const mark = (el: HTMLElement) => {
      for (const child of Array.from(el.children)) {
        if (!(child instanceof HTMLElement)) continue;
        if (!child.closest("button, [data-no-drag]")) {
          child.setAttribute("data-tauri-drag-region", "");
        }
        mark(child);
      }
    };
    mark(root);
  });

  useEffect(() => {
    let unlisten: (() => void) | undefined;
    let disposed = false;
    const win = getCurrentWindow();

    win.isMaximized().then(setMaximized).catch(() => undefined);
    win
      .onResized(() => {
        win.isMaximized().then(setMaximized).catch(() => undefined);
      })
      .then((fn) => {
        if (disposed) fn();
        else unlisten = fn;
      })
      .catch(() => undefined);

    return () => {
      disposed = true;
      unlisten?.();
    };
  }, []);

  const win = getCurrentWindow();
  const value = Math.max(
    0,
    items.findIndex((item) => item.key === current),
  );

  const iconBtnSx = {
    width: 40,
    height: 40,
    borderRadius: 999,
    color: theme.palette.text.secondary,
    "&:hover": {
      bgcolor: alpha(theme.palette.text.primary, 0.08),
      color: theme.palette.text.primary,
    },
    "& .MuiTouchRipple-root": { borderRadius: "inherit" },
  } as const;

  return (
    <Box
      ref={barRef}
      data-tauri-drag-region
      onDoubleClick={(e) => {
        const target = e.target as HTMLElement;
        // 按钮 / 明确标了 no-drag 的子树不触发展开-还原
        if (target.closest("button, [data-no-drag], input, a")) return;
        win.toggleMaximize().catch(() => undefined);
      }}
      sx={{
        height: TITLEBAR_HEIGHT,
        flexShrink: 0,
        display: "grid",
        gridTemplateColumns: "1fr auto 1fr",
        alignItems: "center",
        // 标签栏也是「面」：底色按界面不透明度等比例减淡，开了模糊一起毛玻璃
        bgcolor: alpha(
          theme.palette.primary.main,
          (isDark ? 0.1 : 0.35) * theme.palette.surface.opacity,
        ),
        backdropFilter: theme.palette.surface.filter,
        borderBottom: "1px solid",
        borderColor: "divider",
        userSelect: "none",
      }}
    >
      {/* 左：品牌区 / 子页返回（整块可拖动，返回键自己带 data-no-drag） */}
      <Box
        data-tauri-drag-region
        sx={{
          display: "flex",
          alignItems: "center",
          pl: 2,
          minWidth: 0,
          height: "100%",
        }}
      >
        <TitleBarTitle />
      </Box>

      {/* 中：胶囊 Tab。只有 Tab 本体（按钮）不拖，其余整块可拖 */}
      <Box
        data-tauri-drag-region
        sx={{ display: "flex", justifyContent: "center", height: "100%" }}
      >
        <Box sx={{ display: "flex", alignItems: "center" }}>
        <Tabs
          value={value}
          onChange={(_event, next: number) => {
            const item = items[next];
            if (item) onChange(item.key);
          }}
          variant="standard"
          sx={{
            minHeight: TAB_HEIGHT,
            "& .MuiTabs-flexContainer": { gap: 0.5 },
            "& .MuiTabs-indicator": {
              height: PILL_HEIGHT,
              top: 0,
              borderRadius: 999,
              bgcolor: "primary.main",
              // 标题栏本身就是淡黄，胶囊靠一圈深金细边才分得出来
              boxShadow: `inset 0 0 0 1px ${alpha(theme.palette.brand.ink, 0.22)}`,
              zIndex: 0,
            },
            "& .MuiTab-root": {
              minHeight: TAB_HEIGHT,
              minWidth: 0,
              height: TAB_HEIGHT,
              px: 3.5,
              zIndex: 1,
              borderRadius: 999,
              fontSize: 15,
              fontWeight: 500,
              color: "text.secondary",
              transition: "color 200ms ease",
              "&.Mui-selected": {
                color: "primary.contrastText",
                fontWeight: 600,
              },
              "&.Mui-focusVisible": {
                outline: `2px solid ${alpha(theme.palette.text.primary, 0.45)}`,
                outlineOffset: 2,
              },
              "& .MuiTouchRipple-root": { display: "none" },
            },
            "& .MuiTab-root:not(.Mui-selected):hover": {
              color: "text.primary",
              bgcolor: alpha(theme.palette.text.primary, 0.06),
            },
          }}
        >
          {items.map((item) => (
            <Tab key={item.key} label={item.label} disableRipple />
          ))}
        </Tabs>
        </Box>
      </Box>

      {/* 右：深浅色切换 + 三大键（都是按钮，按钮之间的空隙仍可拖动） */}
      <Box
        sx={{
          display: "flex",
          justifySelf: "end",
          alignItems: "center",
          gap: 0.5,
          pr: 1,
        }}
      >
        <Tooltip title={isDark ? "切换到浅色" : "切换到深色"}>
          <IconButton
            aria-label="切换深浅色"
            onClick={onToggleMode}
            sx={iconBtnSx}
          >
            {isDark ? (
              <LightModeIcon sx={{ fontSize: 18 }} />
            ) : (
              <DarkModeIcon sx={{ fontSize: 18 }} />
            )}
          </IconButton>
        </Tooltip>

        <IconButton
          aria-label="最小化"
          onClick={() => {
            win.minimize().catch(() => undefined);
          }}
          sx={iconBtnSx}
        >
          {/* Minimize 图标横线天生偏下 8/24，用几何居中的 Remove */}
          <RemoveIcon sx={{ fontSize: 18 }} />
        </IconButton>

        <IconButton
          aria-label={maximized ? "还原" : "最大化"}
          onClick={() => {
            win.toggleMaximize().catch(() => undefined);
          }}
          sx={iconBtnSx}
        >
          {maximized ? (
            <FilterNoneIcon sx={{ fontSize: 15 }} />
          ) : (
            <CropSquareIcon sx={{ fontSize: 16 }} />
          )}
        </IconButton>

        <IconButton
          aria-label="关闭"
          onClick={() => {
            win.close().catch(() => undefined);
          }}
          sx={{
            ...iconBtnSx,
            "&:hover": { bgcolor: "#E5484D", color: "#FFFFFF" },
          }}
        >
          <CloseIcon sx={{ fontSize: 19 }} />
        </IconButton>
      </Box>
    </Box>
  );
}
