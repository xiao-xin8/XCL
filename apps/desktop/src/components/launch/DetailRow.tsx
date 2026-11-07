/**
 * 详情卡里的一行「标签 + 值」。
 *
 * 标签固定宽、值靠右并允许换行 —— 版本名和路径都可能很长，
 * 挤成一行省略号反而看不到关键部分。
 */
import { Box, Typography } from "@mui/material";

export function DetailRow(props: { label: string; value: string }) {
  const { label, value } = props;
  return (
    <Box
      sx={{
        display: "flex",
        alignItems: "baseline",
        gap: 2,
        py: 0.6,
        borderBottom: "1px solid",
        borderColor: "divider",
        "&:last-of-type": { borderBottom: "none" },
      }}
    >
      <Typography sx={{ fontSize: 12.5, color: "text.secondary", flexShrink: 0 }}>
        {label}
      </Typography>
      <Typography
        title={value}
        sx={{
          flex: 1,
          minWidth: 0,
          fontSize: 12.5,
          textAlign: "right",
          wordBreak: "break-all",
          lineHeight: 1.6,
        }}
      >
        {value}
      </Typography>
    </Box>
  );
}
