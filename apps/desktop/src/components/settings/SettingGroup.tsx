import type { ReactNode } from "react";
import { Box, Paper, Typography, alpha } from "@mui/material";

/**
 * 一组设置 = 一张卡片 = 一个动画块。
 *
 * 为什么用 `Paper`：`pageMotion` 会把「没有被块包住的 Paper」自动当成块，
 * 于是每张卡从上方 -16px 落下、相邻 +25ms —— 就是那种「一块一块掉下来」。
 * 标 `data-page-block` 也行，但没必要，这里用默认规则即可。
 *
 * 反过来：不要给外层滚动容器加 `data-page-block`，一标扫描就停，卡全不动。
 */
export default function SettingGroup(props: {
  title: string;
  desc?: ReactNode;
  children: ReactNode;
  /** 危险区用淡红底纹，不描彩色边（全局约定：状态色只做底纹） */
  tone?: "default" | "danger";
}) {
  const { title, desc, children, tone = "default" } = props;
  const danger = tone === "danger";

  return (
    <Paper
      variant="outlined"
      sx={{
        overflow: "hidden",
        ...(danger
          ? {
              borderColor: (t) => alpha(t.palette.error.main, 0.35),
              bgcolor: (t) => alpha(t.palette.error.main, 0.07),
            }
          : {}),
      }}
    >
      <Box sx={{ px: 2, pt: 1.5, pb: 0.75 }}>
        <Typography sx={{ fontSize: 13.5, fontWeight: 600, lineHeight: 1.6 }}>
          {title}
        </Typography>
        {desc && (
          <Typography
            component="div"
            sx={{ fontSize: 11.5, color: "text.secondary", lineHeight: 1.65 }}
          >
            {desc}
          </Typography>
        )}
      </Box>
      <Box
        sx={{
          // 行与行之间的分隔线交给容器画：行自己不需要知道自己是第几行
          "& > * + *": {
            borderTop: "1px solid",
            borderColor: "divider",
          },
        }}
      >
        {children}
      </Box>
    </Paper>
  );
}
