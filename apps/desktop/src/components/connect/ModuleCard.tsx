/**
 * 组件没就绪时，整页只有这一张卡。
 *
 * 这里画的进度条是例外：联机组件是应用自己的 33 MB，不是游戏资源，
 * 不进全局下载队列（否则悬浮球里会多出一条和游戏无关的条目）。
 * 代价是切走页面看不到进度 —— 只有几十秒，可以接受。
 */
import { Box, Button, LinearProgress, Stack, Typography } from "@mui/material";

import { BUTTON_SX } from "../../lib/buttons";
import { formatBytes } from "../../lib/format";
import type { AppError } from "../../lib/types";
import ErrorState from "../ErrorState";
import SettingGroup from "../settings/SettingGroup";
import type { ConnectModuleStatus, InstallTask } from "./types";

export function ModuleCard({
  status,
  task,
  busy,
  error,
  onInstall,
  onRetry,
}: {
  status: ConnectModuleStatus;
  task: InstallTask | null;
  busy: boolean;
  error: AppError | null;
  onInstall: () => void;
  onRetry: () => void;
}) {
  const running = task?.phase === "running";
  // 下载跑完之后、组件就绪之前，一律算「正在准备」——哪怕后端的状态事件还没到。
  // 这样按「下载组件」按钮永远不会在装好前又跳回来。
  const unpacking = !running && (status.installing || task?.phase === "done");

  return (
    <SettingGroup
      title="联机组件"
      desc="联机需要先准备一份组件（约 33 MB），装好后就能创建和加入房间。"
    >
      <Box sx={{ px: 2, py: 1.5 }}>
        {running ? (
          <Stack sx={{ mt: 1.5, gap: 0.75 }}>
            <LinearProgress
              variant="determinate"
              value={Math.round((task?.fraction ?? 0) * 100)}
              sx={{ height: 2, borderRadius: 0 }}
            />
            <Typography sx={{ fontSize: 12, color: "text.secondary" }}>
              {formatBytes(task?.doneBytes ?? 0)} / {formatBytes(task?.totalBytes ?? 0)}
              {task?.speedBps ? ` · ${formatBytes(task.speedBps)}/s` : ""}
            </Typography>
          </Stack>
        ) : unpacking ? (
          <Typography sx={{ fontSize: 12.5, color: "text.secondary", mt: 1.5 }}>
            正在准备组件（校验 + 解压）…
          </Typography>
        ) : (
          <Button
            variant="contained"
            sx={{ ...BUTTON_SX, mt: 1.5 }}
            disabled={busy}
            onClick={onInstall}
          >
            下载组件
          </Button>
        )}

        {task?.phase === "failed" && task.message && (
          <Typography sx={{ fontSize: 12, color: "error.main", mt: 1 }}>
            {task.message}
          </Typography>
        )}

        {error && (
          <Box sx={{ mt: 1.5 }}>
            <ErrorState variant="banner" error={error} onRetry={onRetry} />
          </Box>
        )}

        {status.message && !error && (
          <Typography sx={{ fontSize: 12, color: "text.secondary", mt: 1 }}>
            {status.message}
          </Typography>
        )}
      </Box>
    </SettingGroup>
  );
}
