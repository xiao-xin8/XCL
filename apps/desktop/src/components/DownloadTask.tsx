import {
  Box,
  LinearProgress,
  Typography,
  useTheme,
} from "@mui/material";
import { formatBytes, formatDuration, formatSpeed } from "../lib/format";
import type { TaskView } from "../lib/types";

/** 类型定义搬到了 `lib/types`，这里 re-export 保持原有 import 不变。 */
export type { TaskView };

/** 所有小字统一：单行、超长省略，绝不换行挤到下一行 */
const SINGLE_LINE = {
  whiteSpace: "nowrap",
  overflow: "hidden",
  textOverflow: "ellipsis",
} as const;

const NUM = { fontVariantNumeric: "tabular-nums" } as const;

/**
 * 进度显示。
 *
 * 布局是固定三列两行，每一块数据都有自己固定的位置：
 *
 * ```text
 * 42%  ████████████░░░░░░░  3.1 MB/s
 * 312 MB / 706 MB · 1680/3688 个   约 2 分
 * ```
 *
 * 之前是一行里堆四段文字，窗口一窄就折到下一行，看着又乱又抖。
 * 现在每段都 `nowrap + ellipsis`，宽度不够就省略号，绝不换行。
 */

export function TaskProgress(props: { task: TaskView }) {
  const { task } = props;
  const theme = useTheme();
  const running = task.phase === "running";
  const percent = Math.round(task.fraction * 100);

  // 结束态：不再画进度条，只留一句结论
  if (!running) {
    const color =
      task.phase === "failed"
        ? theme.palette.error.main
        : task.phase === "done"
          ? theme.palette.success.main
          : "text.secondary";
    return (
      <Typography sx={{ ...SINGLE_LINE, fontSize: 12, color }}>
        {task.message ??
          (task.phase === "cancelled"
            ? "已取消"
            : task.phase === "done"
              ? "已完成"
              : "失败")}
      </Typography>
    );
  }

  return (
    <Box
      sx={{
        width: "100%",
        display: "grid",
        gridTemplateColumns: "auto 1fr auto",
        columnGap: 1,
        rowGap: 0.25,
        alignItems: "center",
      }}
    >
      {/* 第一行：百分比 | 进度条 | 速度 */}
      <Typography sx={{ ...NUM, fontSize: 12.5, fontWeight: 700, minWidth: 34 }}>
        {percent}%
      </Typography>
      <LinearProgress
        variant="determinate"
        value={percent}
        sx={{ height: 5, borderRadius: 999 }}
      />
      <Typography
        sx={{
          ...SINGLE_LINE,
          ...NUM,
          fontSize: 11.5,
          color: "text.secondary",
          minWidth: 74,
          textAlign: "right",
        }}
      >
        {formatSpeed(task.speedBps)}
      </Typography>

      {/* 第二行：体积 + 文件数 | 剩余时间 */}
      <Typography
        sx={{
          ...SINGLE_LINE,
          ...NUM,
          gridColumn: "1 / span 2",
          fontSize: 11.5,
          color: "text.secondary",
        }}
      >
        {formatBytes(task.doneBytes)} / {formatBytes(task.totalBytes)} ·{" "}
        {task.finishedFiles}/{task.totalFiles} 个
      </Typography>
      <Typography
        sx={{
          ...SINGLE_LINE,
          ...NUM,
          fontSize: 11.5,
          color: "text.secondary",
          textAlign: "right",
        }}
      >
        {task.etaSeconds != null ? `约 ${formatDuration(task.etaSeconds)}` : ""}
      </Typography>

      {task.failures.length > 0 && (
        <Typography
          sx={{
            ...SINGLE_LINE,
            gridColumn: "1 / -1",
            fontSize: 11,
            color: "text.secondary",
          }}
          title={task.failures.join("、")}
        >
          失败：{task.failures.join("、")}
          {task.failed > task.failures.length ? ` 等 ${task.failed} 个` : ""}
        </Typography>
      )}
    </Box>
  );
}
