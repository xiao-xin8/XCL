import { Box, alpha, useTheme } from "@mui/material";
import { useAppearance } from "../lib/appearance";

/**
 * 自定义背景图。
 *
 * 单独一层固定在最底下（`z-index: -1`），而不是塞进 `body` 的 background：
 * 只有独立一层才能单独加 `filter: blur()`，蒙版和模糊也才是两个独立的旋钮。
 *
 * 模糊时四周多留 `2 × 半径` 的余量，否则图片边缘会被糊成透明，露出底色。
 */
export default function Wallpaper() {
  const { backgroundImage, backgroundOverlay, backgroundBlur } = useAppearance();
  const theme = useTheme();

  if (!backgroundImage) return null;

  const blur = Math.min(20, Math.max(0, backgroundBlur));
  const overlay = Math.min(100, Math.max(0, backgroundOverlay)) / 100;

  return (
    <Box
      aria-hidden
      sx={{
        position: "fixed",
        inset: 0,
        zIndex: -1,
        overflow: "hidden",
        pointerEvents: "none",
      }}
    >
      <Box
        sx={{
          position: "absolute",
          inset: blur > 0 ? -blur * 2 : 0,
          backgroundImage: `url("${backgroundImage}")`,
          backgroundSize: "cover",
          backgroundPosition: "center",
          backgroundRepeat: "no-repeat",
          filter: blur > 0 ? `blur(${blur}px)` : "none",
        }}
      />
      {overlay > 0 && (
        <Box
          sx={{
            position: "absolute",
            inset: 0,
            bgcolor: alpha(theme.palette.background.default, overlay),
          }}
        />
      )}
    </Box>
  );
}
