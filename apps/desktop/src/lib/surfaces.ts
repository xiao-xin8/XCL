import type { SxProps, Theme } from "@mui/material/styles";

/**
 * 侧栏 / 分栏面板的底。
 *
 * 跟着「界面不透明度」走（`palette.surface`），开了模糊时一起毛玻璃。
 * 不要再写 `bgcolor: "background.default"` —— 那是不透明底，会把背景图挡死，
 * 也会让「界面透明度」在这一块失效。
 */
export const PANEL_SURFACE_SX: SxProps<Theme> = {
  bgcolor: "surface.default",
  backdropFilter: (theme) => theme.palette.surface.filter,
};
