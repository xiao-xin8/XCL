import { CircularProgress } from "@mui/material";

/** 唯一的尺寸：64 外径 / 5 粗细。改这里等于改全软件。 */
const SIZE = 64;
const THICKNESS = 5;

/**
 * 全局唯一的加载指示器（MUI 转圈）。
 *
 * 没有尺寸变体。以前有个 16px 的 `inline`，结果同一个界面里大圈小圈混着出现，
 * 看着像两个应用拼起来的 —— 现在只有一种，谁想「放小一点」就去改布局，
 * 不要改这里。
 *
 * 配套规矩（`docs/UI.md` 第 3 条）：不带任何加载文案；块级使用时外面套
 * `display: grid; place-items: center`。
 */
export default function Loading() {
  return <CircularProgress size={SIZE} thickness={THICKNESS} aria-label="加载中" />;
}
