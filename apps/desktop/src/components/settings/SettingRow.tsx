import type { ReactNode } from "react";
import { Box, Typography } from "@mui/material";

/**
 * 设置页的一行：左边标题 + 副文案，右边控件。
 *
 * `block` 用于「控件必须占满整行」的项（多行文本、成组的按钮），
 * 其余情况控件靠右，行高由内容决定 —— 不要写死高度，副文案可能折行。
 */
export default function SettingRow(props: {
  title: string;
  desc?: ReactNode;
  children?: ReactNode;
  block?: boolean;
}) {
  const { title, desc, children, block = false } = props;

  return (
    <Box
      sx={{
        display: "flex",
        flexDirection: block ? "column" : "row",
        alignItems: block ? "stretch" : "center",
        gap: block ? 1 : 2,
        px: 2,
        py: 1.25,
      }}
    >
      <Box sx={{ flex: 1, minWidth: 0 }}>
        <Typography sx={{ fontSize: 13.5, lineHeight: 1.6 }}>{title}</Typography>
        {desc && (
          <Typography
            component="div"
            sx={{ fontSize: 11.5, color: "text.secondary", lineHeight: 1.65, mt: 0.25 }}
          >
            {desc}
          </Typography>
        )}
      </Box>
      {children && (
        <Box
          sx={{
            flexShrink: 0,
            display: "flex",
            alignItems: "center",
            gap: 1,
            ...(block ? { width: "100%" } : {}),
          }}
        >
          {children}
        </Box>
      )}
    </Box>
  );
}
