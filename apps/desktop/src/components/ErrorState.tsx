import type { ReactNode } from "react";
import { Box, Button, Stack, Typography, alpha } from "@mui/material";
import type { SxProps, Theme } from "@mui/material/styles";
import ErrorOutlinedIcon from "@mui/icons-material/ErrorOutlined";
import WarningAmberOutlinedIcon from "@mui/icons-material/WarningAmberOutlined";
import InfoOutlinedIcon from "@mui/icons-material/InfoOutlined";
import type { AppError } from "../lib/types";

/**
 * 统一的错误展示。
 *
 * 一套骨架（图标 + 标题 + 说明 + 建议 + 操作），三档密度：
 * - `page`：整块区域都没内容时用，居中、不套白卡（延续加载/错误的规矩）；
 * - `banner`：面板里已经有内容时用，一条贴边、不把内容挤走，用状态色底纹；
 * - `inline`：列表里的一行，36px、方角，不打乱列表节奏。
 *
 * 传 `error` 就自动取 title/message/hint，并按 `retryable` 决定要不要「重试」；
 * 没有 AppError 的场景（例如启动页的纯文本原因）直接传 `title/message`。
 */

export type ErrorVariant = "page" | "banner" | "inline";
export type ErrorSeverity = "error" | "warning" | "info";

const ICONS = {
  error: ErrorOutlinedIcon,
  warning: WarningAmberOutlinedIcon,
  info: InfoOutlinedIcon,
};

const TINT = { error: 0.07, warning: 0.08, info: 0.07 } as const;

/** 建议前缀，颜色跟主色文字走，和正文区分开 */
function Hint(props: { text: string }) {
  return (
    <Typography sx={{ fontSize: 11.5, color: "text.secondary", mt: 0.5 }}>
      <Box component="span" sx={{ color: "brand.ink", fontWeight: 600 }}>
        建议：
      </Box>
      {props.text}
    </Typography>
  );
}

export default function ErrorState(props: {
  variant?: ErrorVariant;
  severity?: ErrorSeverity;
  /** 有值就自动取字段；`retryable` 决定是否显示重试 */
  error?: AppError | null;
  title?: string;
  message?: string | null;
  hint?: string | null;
  onRetry?: () => void;
  /** 额外动作（查看日志、下载 Java…），排在重试左边 */
  action?: ReactNode;
  /** banner 里跟在文字下面的附加内容（例如下载进度） */
  children?: ReactNode;
  sx?: SxProps<Theme>;
}) {
  const {
    variant = "banner",
    severity = "error",
    error,
    title,
    message,
    hint,
    onRetry,
    action,
    children,
    sx,
  } = props;

  const Icon = ICONS[severity];
  const resolvedTitle = error?.title ?? title ?? "出错了";
  const resolvedMessage = error?.message ?? message ?? null;
  const resolvedHint = error?.hint ?? hint ?? null;
  const canRetry = Boolean(onRetry) && (error ? error.retryable : true);
  const color = (theme: Theme) => theme.palette[severity].main;

  if (variant === "inline") {
    return (
      <Box
        sx={[
          {
            display: "flex",
            alignItems: "center",
            gap: 1,
            px: 1.75,
            py: 0.75,
            minWidth: 0,
            bgcolor: (theme) => alpha(color(theme), TINT[severity]),
          },
          ...(Array.isArray(sx) ? sx : [sx]),
        ]}
      >
        <Icon sx={{ fontSize: 16, color, flexShrink: 0 }} />
        <Typography sx={{ fontSize: 12, fontWeight: 500, flexShrink: 0 }}>
          {resolvedTitle}
        </Typography>
        {resolvedMessage && (
          <Typography
            title={resolvedMessage}
            sx={{
              fontSize: 11.5,
              color: "text.secondary",
              minWidth: 0,
              whiteSpace: "nowrap",
              overflow: "hidden",
              textOverflow: "ellipsis",
            }}
          >
            {resolvedMessage}
          </Typography>
        )}
        <Box sx={{ flex: 1 }} />
        {action}
        {canRetry && (
          <Button
            size="small"
            onClick={onRetry}
            sx={{ minWidth: 0, px: 1, flexShrink: 0 }}
          >
            重试
          </Button>
        )}
      </Box>
    );
  }

  if (variant === "page") {
    return (
      <Box
        sx={[
          {
            flex: 1,
            minHeight: 0,
            display: "flex",
            flexDirection: "column",
            alignItems: "center",
            justifyContent: "center",
            textAlign: "center",
            px: 3,
            py: 4,
          },
          ...(Array.isArray(sx) ? sx : [sx]),
        ]}
      >
        <Box
          sx={{
            width: 44,
            height: 44,
            borderRadius: "50%",
            display: "grid",
            placeItems: "center",
            bgcolor: (theme) => alpha(color(theme), TINT[severity]),
            mb: 1.5,
          }}
        >
          <Icon sx={{ fontSize: 24, color }} />
        </Box>
        <Typography sx={{ fontSize: 15, fontWeight: 600 }}>
          {resolvedTitle}
        </Typography>
        {resolvedMessage && (
          <Typography
            sx={{
              fontSize: 12.5,
              color: "text.secondary",
              mt: 0.75,
              maxWidth: 520,
              overflowWrap: "anywhere",
              lineHeight: 1.7,
            }}
          >
            {resolvedMessage}
          </Typography>
        )}
        {resolvedHint && (
          <Box sx={{ maxWidth: 520 }}>
            <Hint text={resolvedHint} />
          </Box>
        )}
        {(action || canRetry) && (
          <Stack direction="row" spacing={1} sx={{ mt: 2, alignItems: "center" }}>
            {action}
            {canRetry && (
              <Button variant="contained" size="small" onClick={onRetry}>
                重试
              </Button>
            )}
          </Stack>
        )}
        {children}
      </Box>
    );
  }

  return (
    <Box
      sx={[
        {
          display: "flex",
          flexWrap: "wrap",
          gap: 1.25,
          alignItems: "flex-start",
          px: 1.5,
          py: 1.25,
          borderRadius: 1,
          bgcolor: (theme) => alpha(color(theme), TINT[severity]),
        },
        ...(Array.isArray(sx) ? sx : [sx]),
      ]}
    >
      <Icon sx={{ fontSize: 18, color, mt: 0.15, flexShrink: 0 }} />
      <Box sx={{ flex: 1, minWidth: 160 }}>
        <Typography sx={{ fontSize: 12.5, fontWeight: 600, lineHeight: 1.6 }}>
          {resolvedTitle}
        </Typography>
        {resolvedMessage && (
          <Typography
            sx={{
              fontSize: 12,
              color: "text.secondary",
              mt: 0.25,
              lineHeight: 1.7,
              overflowWrap: "anywhere",
            }}
          >
            {resolvedMessage}
          </Typography>
        )}
        {resolvedHint && <Hint text={resolvedHint} />}
        {children}
      </Box>
      {(action || canRetry) && (
        <Stack
          direction="row"
          spacing={0.75}
          sx={{ alignItems: "center", ml: "auto", alignSelf: "center", flexShrink: 0 }}
        >
          {action}
          {canRetry && (
            <Button size="small" variant="contained" onClick={onRetry}>
              重试
            </Button>
          )}
        </Stack>
      )}
    </Box>
  );
}
