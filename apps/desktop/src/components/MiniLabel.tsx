import { Typography, alpha } from "@mui/material";
import type { ReactNode } from "react";

/**
 * 列表行里的小圆角标签：`v1.2`、`格式 15`、`已禁用`、`1.20.1`。
 *
 * 和标题挤在同一行，所以字号压到 9.5px 并且 `flexShrink: 0` —— 空间不够时
 * 让标题先截断，标签保持完整。内容列表和存档列表共用这一个。
 */
export default function MiniLabel(props: { children: ReactNode; tone?: "warning" }) {
  const warning = props.tone === "warning";
  return (
    <Typography
      sx={{
        fontSize: 9.5,
        lineHeight: 1.6,
        px: 0.6,
        flexShrink: 0,
        borderRadius: 999,
        color: warning ? "warning.main" : "text.secondary",
        bgcolor: (t) =>
          alpha(
            warning ? t.palette.warning.main : t.palette.text.primary,
            warning ? 0.18 : 0.08,
          ),
      }}
    >
      {props.children}
    </Typography>
  );
}
