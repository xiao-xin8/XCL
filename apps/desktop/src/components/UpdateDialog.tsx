import { useState } from "react";
import {
  Box,
  Button,
  Checkbox,
  Dialog,
  DialogActions,
  DialogContent,
  DialogTitle,
  FormControlLabel,
  IconButton,
  LinearProgress,
  Stack,
  Typography,
  alpha,
} from "@mui/material";
import CloseIcon from "@mui/icons-material/Close";
import SystemUpdateAltOutlined from "@mui/icons-material/SystemUpdateAltOutlined";
import { BUTTON_SX } from "../lib/buttons";
import { formatBytes, formatDuration, formatSpeed } from "../lib/format";
import type { UpdateView } from "../lib/types";
import { useUpdateActions, useUpdateState } from "../lib/updateContext";
import ErrorState from "./ErrorState";
import Loading from "./Loading";

/**
 * 更新对话框。
 *
 * 两种形态：
 * - 普通：点遮罩 / 关闭键 / 「稍后」都能走，还能勾「忽略此版本」；
 * - 强制：没有任何关闭途径 —— 不给关闭按钮、不响应 Esc、点遮罩没反应。
 *   这种更新是云端明确要求的，必须本人升上去才能继续用。
 *
 * 放在 `UpdateProvider` 里渲染，所以它自己从 context 取状态。
 */
export default function UpdateDialog() {
  const { view, open } = useUpdateState();
  const { check, download, cancel, apply, skip, closeDialog } = useUpdateActions();
  const [skipNext, setSkipNext] = useState(false);

  if (!view) return null;
  const forced = view.forced;

  const handleClose = () => {
    if (forced) return;
    if (skipNext && view.latestVersion) {
      void skip();
      setSkipNext(false);
      return;
    }
    closeDialog();
  };

  const title = dialogTitle(view, forced);
  const showProgress = view.phase === "downloading" || view.phase === "verifying";

  return (
    <Dialog
      open={forced || open}
      // 强制模式不传 onClose：遮罩点击、Esc 都由 Modal 交给 onClose 处理，
      // 没有它就没有任何关闭途径
      onClose={forced ? undefined : handleClose}
      maxWidth="xs"
      fullWidth
      slotProps={{
        paper: {
          // 不投影：投影会在浅色背景上糊出一圈灰边
          elevation: 0,
          sx: { borderRadius: 1, border: "1px solid", borderColor: "divider" },
        },
      }}
    >
      <DialogTitle
        sx={{
          display: "flex",
          alignItems: "center",
          gap: 1,
          fontSize: 15,
          fontWeight: 700,
          py: 1.75,
          pr: 1.5,
        }}
      >
        <SystemUpdateAltOutlined sx={{ fontSize: 20, color: "text.secondary" }} />
        {title}
        {!forced && (
          <IconButton
            onClick={handleClose}
            aria-label="关闭"
            sx={{ ml: "auto", mr: -0.5, color: "text.secondary" }}
          >
            <CloseIcon fontSize="small" />
          </IconButton>
        )}
      </DialogTitle>

      <DialogContent sx={{ pt: 0.5, pb: 2 }}>
        {forced && <ForceBanner view={view} />}
        {!forced && view.phase === "available" && (
          <Typography sx={{ fontSize: 12, color: "text.secondary", mb: 1.25 }}>
            {view.currentVersion} → {view.latestVersion}
            {view.belowMinSupported && " · 当前版本太旧，建议尽快更新"}
            {view.critical && " · 这是一次重要更新"}
          </Typography>
        )}

        <Changelog view={view} />

        {showProgress && <Progress view={view} />}

        {view.phase === "ready" && !forced && (
          <Typography
            sx={{ mt: 1.5, fontSize: 11.5, color: "text.secondary", lineHeight: 1.7 }}
          >
            不用现在点。关掉启动器之后，下次打开会自动完成更新。
          </Typography>
        )}

        {view.phase === "upToDate" && (
          <Typography sx={{ fontSize: 12.5, color: "text.secondary" }}>
            已是最新版本（{view.currentVersion}）
          </Typography>
        )}

        {view.phase === "checking" && (
          <Box sx={{ display: "grid", placeItems: "center", py: 2 }}>
            <Loading />
          </Box>
        )}

        {view.phase === "failed" && view.error && (
          <Box sx={{ mt: 1.5 }}>
            <ErrorState variant="banner" error={view.error} />
          </Box>
        )}

        {!forced && view.phase === "available" && (
          <FormControlLabel
            sx={{ mt: 1.5, ml: 0 }}
            control={
              <Checkbox
                size="small"
                checked={skipNext}
                onChange={(event) => setSkipNext(event.target.checked)}
              />
            }
            label={
              <Typography sx={{ fontSize: 12, color: "text.secondary" }}>
                忽略此版本
              </Typography>
            }
          />
        )}
      </DialogContent>

      <DialogActions sx={{ px: 2, pb: 2, pt: 0, gap: 1 }}>
        {actions(view, forced, {
          check,
          download,
          cancel,
          apply,
          closeDialog,
        })}
      </DialogActions>
    </Dialog>
  );
}

function dialogTitle(view: UpdateView, forced: boolean): string {
  if (forced) return view.forceTitle ?? "必须更新";
  switch (view.phase) {
    case "available":
      return `发现新版本 ${view.latestVersion ?? ""}`.trim();
    case "downloading":
      return "正在下载更新";
    case "verifying":
      return "正在校验文件";
    case "ready":
      return "准备就绪";
    case "failed":
      return "更新失败";
    case "upToDate":
      return "已是最新";
    default:
      return "检查更新";
  }
}

/** 强制更新的提示条：状态色只做底纹，不描边。 */
function ForceBanner({ view }: { view: UpdateView }) {
  return (
    <Box
      sx={{
        mb: 1.5,
        px: 1.5,
        py: 1.25,
        borderRadius: 1,
        bgcolor: (t) => alpha(t.palette.warning.main, 0.08),
      }}
    >
      <Typography sx={{ fontSize: 12.5, fontWeight: 600 }}>
        这个版本必须更新后才能继续使用
      </Typography>
      {view.forceNotes && (
        <Typography sx={{ fontSize: 11.5, color: "text.secondary", mt: 0.5, lineHeight: 1.7 }}>
          {view.forceNotes}
        </Typography>
      )}
    </Box>
  );
}

/** 更新内容。有逐条清单就用清单，没有退化成一整段说明。 */
function Changelog({ view }: { view: UpdateView }) {
  const lines = view.forced
    ? view.forceChangelog
    : view.phase === "available" || view.phase === "downloading" || view.phase === "ready"
      ? view.changelog
      : [];

  if (lines.length === 0) {
    const fallback = view.forced ? view.forceNotes : view.notes;
    if (!fallback) return null;
    return (
      <Typography sx={{ fontSize: 12, color: "text.secondary", lineHeight: 1.8 }}>
        {fallback}
      </Typography>
    );
  }

  return (
    <Box
      sx={{
        maxHeight: 168,
        overflow: "auto",
        scrollbarGutter: "stable",
        pr: 0.5,
      }}
    >
      <Stack component="ul" sx={{ m: 0, pl: 0, listStyle: "none", gap: 0.75 }}>
        {lines.map((line, index) => (
          <Stack
            component="li"
            key={index}
            direction="row"
            sx={{ gap: 1, alignItems: "flex-start" }}
          >
            <Box
              aria-hidden
              sx={{
                width: 5,
                height: 5,
                mt: "8px",
                flexShrink: 0,
                borderRadius: "50%",
                bgcolor: "primary.main",
              }}
            />
            <Typography sx={{ fontSize: 12, lineHeight: 1.75, color: "text.secondary" }}>
              {line}
            </Typography>
          </Stack>
        ))}
      </Stack>
    </Box>
  );
}

function Progress({ view }: { view: UpdateView }) {
  const percent = Math.round(view.fraction * 100);
  return (
    <Box sx={{ mt: 2 }}>
      <LinearProgress variant="determinate" value={percent} sx={{ height: 6 }} />
      <Stack direction="row" sx={{ mt: 0.75, gap: 1 }}>
        <Typography sx={{ fontSize: 11.5, color: "text.secondary", fontVariantNumeric: "tabular-nums" }}>
          {view.phase === "verifying"
            ? "校验中"
            : `${formatBytes(view.doneBytes)} / ${formatBytes(view.totalBytes)}`}
        </Typography>
        <Box sx={{ flex: 1 }} />
        {view.phase === "downloading" && (
          <Typography
            sx={{ fontSize: 11.5, color: "text.secondary", fontVariantNumeric: "tabular-nums" }}
          >
            {formatSpeed(view.speedBps)}
            {view.etaSeconds !== null ? ` · 剩余 ${formatDuration(view.etaSeconds)}` : ""}
          </Typography>
        )}
      </Stack>
    </Box>
  );
}

function actions(
  view: UpdateView,
  forced: boolean,
  handlers: {
    check: (auto?: boolean) => Promise<void>;
    download: () => Promise<void>;
    cancel: () => Promise<void>;
    apply: () => Promise<void>;
    closeDialog: () => void;
  },
) {
  const { check, download, cancel, apply, closeDialog } = handlers;

  switch (view.phase) {
    case "checking":
      return null;

    case "upToDate":
      return (
        <Button variant="outlined" onClick={closeDialog} sx={BUTTON_SX}>
          关闭
        </Button>
      );

    case "available":
      return (
        <>
          {!forced && (
            <Button variant="text" onClick={closeDialog} sx={BUTTON_SX}>
              稍后
            </Button>
          )}
          <Button variant="contained" onClick={() => void download()} sx={BUTTON_SX} autoFocus>
            下载更新
          </Button>
        </>
      );

    case "downloading":
      return (
        <>
          <Typography sx={{ fontSize: 11.5, color: "text.secondary", mr: "auto" }}>
            {Math.round(view.fraction * 100)}%
          </Typography>
          <Button variant="outlined" onClick={() => void cancel()} sx={BUTTON_SX}>
            取消
          </Button>
        </>
      );

    case "verifying":
      return null;

    case "ready":
      return (
        <>
          {!forced && (
            <Button variant="text" onClick={closeDialog} sx={BUTTON_SX}>
              稍后
            </Button>
          )}
          <Button variant="contained" onClick={() => void apply()} sx={BUTTON_SX}>
            立即更新
          </Button>
        </>
      );

    case "failed":
      return (
        <>
          {!forced && (
            <Button variant="text" onClick={closeDialog} sx={BUTTON_SX}>
              关闭
            </Button>
          )}
          <Button variant="contained" onClick={() => void check(false)} sx={BUTTON_SX}>
            重试
          </Button>
        </>
      );

    case "applying":
      return (
        <Typography sx={{ fontSize: 12, color: "text.secondary" }}>
          正在重启，请稍候…
        </Typography>
      );

    default:
      return (
        <Button variant="contained" onClick={() => void check(false)} sx={BUTTON_SX}>
          检查更新
        </Button>
      );
  }
}
