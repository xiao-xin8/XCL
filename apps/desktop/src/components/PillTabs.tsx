import { Box, Tab, Tabs, alpha } from "@mui/material";

/** 胶囊 Tab 的统一高度：外层 36（含 3px 内衬），里面按钮 30。 */
export const PILL_H = 30;

export type PillTabItem<T extends string> = {
  value: T;
  label: string;
};

/**
 * 胶囊 Tab：外层一个大圆角容器，里面的高亮块跟着选中项左右滑。
 *
 * 从 `ConnectLoginDialog` 抽出来的共享控件 —— 登录/注册、添加账号的
 * 微软/离线/第三方，凡是「一排互斥档位」都用它，样式只此一份。
 * 样式口径对齐 Mod 列表页的源切换（淡黄和背景太近，靠一圈深金细边分出来）。
 */
export default function PillTabs<T extends string>(props: {
  items: readonly PillTabItem<T>[];
  value: T;
  onChange: (next: T) => void;
  /** 挂到 Tabs 上的样式（一般不传）。 */
  sx?: object;
}) {
  const { items, value, onChange, sx } = props;
  return (
    <Box
      sx={{
        width: "100%",
        height: PILL_H + 6,
        p: "3px",
        borderRadius: 999,
        // 胶囊条也是「面」：跟着界面不透明度一起淡，开了模糊一起毛玻璃
        bgcolor: (t) => alpha(t.palette.text.primary, 0.06 * t.palette.surface.opacity),
        backdropFilter: (t) => t.palette.surface.filter,
        display: "flex",
        alignItems: "center",
      }}
    >
      <Tabs
        value={value}
        onChange={(_event, next: T) => onChange(next)}
        variant="standard"
        sx={{
          width: "100%",
          minHeight: PILL_H,
          "& .MuiTabs-flexContainer": { gap: 0 },
          "& .MuiTabs-indicator": {
            height: PILL_H,
            top: 0,
            borderRadius: 999,
            bgcolor: "primary.main",
            boxShadow: (t) => `inset 0 0 0 1px ${alpha(t.palette.brand.ink, 0.22)}`,
          },
          "& .MuiTab-root": {
            flex: 1,
            minHeight: PILL_H,
            height: PILL_H,
            minWidth: 0,
            px: 2,
            zIndex: 1,
            borderRadius: 999,
            fontSize: 13,
            fontWeight: 500,
            textTransform: "none",
            whiteSpace: "nowrap",
            color: "text.secondary",
            transition: "color 200ms ease",
            "&.Mui-selected": { color: "primary.contrastText" },
          },
          "& .MuiTab-root:not(.Mui-selected):hover": {
            color: "text.primary",
            bgcolor: (t) => alpha(t.palette.text.primary, 0.06),
          },
          ...sx,
        }}
      >
        {items.map((item) => (
          <Tab key={item.value} value={item.value} label={item.label} disableRipple />
        ))}
      </Tabs>
    </Box>
  );
}
