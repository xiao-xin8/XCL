/**
 * 联机页里被多个卡片复用的小块。
 *
 * 只服务联机页，不要往上提到通用组件里 —— `Steps` 的淡黄底、`BigCopy` 的整行实心，
 * 都是这一页的视觉决定。
 */
import { useState } from "react";
import { Box, Button, Stack, Typography, alpha } from "@mui/material";

import { BUTTON_SX } from "../../lib/buttons";

/**
 * 主操作按钮：整行、实心。
 *
 * 复制邀请码是这张卡片唯一要做的事，不能缩成角落里的小字按钮。
 */
export function BigCopy({
  text,
  label,
  copiedLabel,
}: {
  text: string;
  label: string;
  copiedLabel: string;
}) {
  const [copied, setCopied] = useState(false);

  return (
    <Button
      fullWidth
      variant="contained"
      sx={{ ...BUTTON_SX, mt: 1.5 }}
      onClick={() => {
        void navigator.clipboard
          ?.writeText(text)
          .then(() => {
            setCopied(true);
            window.setTimeout(() => setCopied(false), 1800);
          })
          .catch(() => {});
      }}
    >
      {copied ? copiedLabel : label}
    </Button>
  );
}

/**
 * 操作步骤。小字号灰色提示等于没写 —— 用带序号的块摆出来，正常字号。
 */
export function Steps({ items }: { items: string[] }) {
  return (
    <Box
      sx={{
        display: "grid",
        gap: 0.75,
        p: 1.25,
        mb: 1.5,
        borderRadius: 1,
        bgcolor: (t) => alpha(t.palette.primary.main, 0.18),
      }}
    >
      {items.map((text, index) => (
        <Stack key={text} direction="row" sx={{ gap: 1, alignItems: "flex-start" }}>
          <Box
            sx={{
              width: 20,
              height: 20,
              flexShrink: 0,
              borderRadius: "50%",
              bgcolor: "primary.main",
              color: "primary.contrastText",
              fontSize: 12,
              fontWeight: 700,
              display: "grid",
              placeItems: "center",
              mt: "1px",
            }}
          >
            {index + 1}
          </Box>
          <Typography sx={{ fontSize: 13.5, lineHeight: 1.55 }}>{text}</Typography>
        </Stack>
      ))}
    </Box>
  );
}
