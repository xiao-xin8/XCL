/**
 * 主按钮的统一尺寸。
 *
 * 登录侧栏的「管理账号 / 启动游戏」、资源详情页的「手动安装」等默认尺寸
 * 的按钮都用这一套：py 1.1 + 13.5px + 600。之前各处自己写 `size="small"` /
 * `sx={{ px: 1 }}`，结果同一个 App 里的按钮高度、字号、hover 手感都不一样。
 *
 * 注意：
 * - 不要在这里写 hover。hover 由主题（`theme.ts` 的 MuiButton 覆盖）统一管，
 *   局部再改就会发现"这个按钮跟别的地方不一样"。
 * - 面板里的按钮（版本详情 / 资源安装栏）不套这个 —— 那边对齐的是 MUI 默认尺寸。
 */
export const BUTTON_SX = {
  py: 1.1,
  fontSize: 13.5,
  fontWeight: 600,
  flexShrink: 0,
} as const;
