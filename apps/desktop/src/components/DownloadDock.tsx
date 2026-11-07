import { useCallback, useEffect, useLayoutEffect, useRef, useState, type ReactNode } from "react";
import {
  Box,
  Button,
  CircularProgress,
  Drawer,
  Fab,
  LinearProgress,
  Paper,
  Stack,
  Typography,
  alpha,
  useTheme,
} from "@mui/material";
import DownloadOutlinedIcon from "@mui/icons-material/DownloadOutlined";
import { useDownloads } from "./DownloadProvider";
import { useAllTasks } from "../lib/downloadStore";
import { playEnter } from "../motion/pageMotion";
import { EASE, CARD_MOTION, FAB_MOTION } from "../motion/tokens";
import { animationsEnabled } from "../lib/appearance";
import { formatBytes, formatDuration, formatSpeed } from "../lib/format";
import type { TaskView } from "../lib/types";

const SINGLE_LINE = {
  whiteSpace: "nowrap",
  overflow: "hidden",
  textOverflow: "ellipsis",
} as const;

const NUM = { fontVariantNumeric: "tabular-nums" } as const;

/** 抽屉打开后多久让卡片开始落下（抽屉滑入 225ms，这里先走一半） */
const CARD_REVEAL_DELAY_MS = 120;

/** 一行「标签 + 值」的详细信息 */
function InfoLine(props: { label: string; value: string }) {
  return (
    <Box sx={{ display: "grid", gridTemplateColumns: "30px 1fr", columnGap: 1.25 }}>
      <Typography sx={{ fontSize: 11.5, color: "text.secondary" }}>
        {props.label}
      </Typography>
      <Typography sx={{ ...SINGLE_LINE, ...NUM, fontSize: 11.5 }}>
        {props.value}
      </Typography>
    </Box>
  );
}

/** 运行中才有的区块：结束时要平滑收起，不能整块高度啪一下没了 */
function Collapse(props: { open: boolean; children: ReactNode }) {
  return (
    <Box
      sx={{
        display: "grid",
        gridTemplateRows: props.open ? "1fr" : "0fr",
        transition: `grid-template-rows ${CARD_MOTION.collapse}ms ${EASE.outFluent}`,
      }}
    >
      {/* minHeight: 0 是 grid 折叠动画能生效的前提 */}
      <Box sx={{ minHeight: 0, overflow: "hidden" }}>{props.children}</Box>
    </Box>
  );
}

function TaskCard(props: {
  task: TaskView;
  /** 已经从队列里摘掉，正在播退场动画 */
  leaving: boolean;
  onCancel: () => void;
  onDismiss: () => void;
  onExited: () => void;
}) {
  const { task, leaving, onCancel, onDismiss, onExited } = props;
  const theme = useTheme();
  const running = task.phase === "running";
  const percent = Math.round(task.fraction * 100);
  const elapsed = Math.max(0, Math.round((Date.now() - task.startedAtMs) / 1000));
  const ref = useRef<HTMLDivElement>(null);

  /** 卸载回调用 ref 拿：否则外层每次重渲染都会换标识，把动画重启一遍 */
  const exited = useRef(onExited);
  exited.current = onExited;

  // 退场：向右滑出 + 淡出 + 轻微缩小，播完通知外层卸载
  useLayoutEffect(() => {
    if (!leaving) return;
    const el = ref.current;
    if (!el || !animationsEnabled()) {
      exited.current();
      return;
    }
    const anim = el.animate(
      [
        { opacity: 1, transform: "translateX(0) scale(1)" },
        {
          opacity: 0,
          transform: `translateX(${CARD_MOTION.exitDistance}px) scale(${CARD_MOTION.exitScale})`,
        },
      ],
      {
        duration: CARD_MOTION.exitDuration,
        easing: EASE.outFluent,
        fill: "both",
      },
    );
    anim.finished.then(() => exited.current()).catch(() => undefined);
    return () => anim.cancel();
  }, [leaving]);

  const statusColor = !running
    ? task.phase === "failed"
      ? theme.palette.error.main
      : task.phase === "done"
        ? theme.palette.success.main
        : "text.secondary"
    : undefined;

  return (
    <Paper
      ref={ref}
      variant="outlined"
      sx={{
        p: 1.5,
        pointerEvents: leaving ? "none" : "auto",
        ...(task.phase === "failed"
          ? { bgcolor: alpha(theme.palette.error.main, 0.06) }
          : {}),
      }}
    >
      {/* 标题行：版本号 / 文件名 + 结论 / 取消 / 删除 */}
      <Stack direction="row" spacing={0.75} sx={{ alignItems: "center", mb: 1 }}>
        <Typography
          sx={{ ...SINGLE_LINE, fontSize: 13.5, fontWeight: 700, flex: 1, minWidth: 0 }}
        >
          {task.title ?? task.versionId}
        </Typography>
        {running ? (
          <Button size="small" onClick={onCancel} sx={{ minWidth: 0, px: 1 }}>
            取消
          </Button>
        ) : (
          <>
            <Typography
              sx={{
                ...SINGLE_LINE,
                ...NUM,
                fontSize: 11.5,
                color: statusColor,
                maxWidth: 150,
              }}
            >
              {task.message ??
                (task.phase === "cancelled"
                  ? "已取消"
                  : task.phase === "done"
                    ? "已完成"
                    : "失败")}
            </Typography>
            <Button size="small" onClick={onDismiss} sx={{ minWidth: 0, px: 1 }}>
              删除
            </Button>
          </>
        )}
      </Stack>

      {/* 进度条：结束时收起，卡片是「慢慢变矮」而不是突然只剩两行 */}
      <Collapse open={running}>
        <Stack direction="row" spacing={1} sx={{ alignItems: "center", mb: 1 }}>
          <Typography sx={{ ...NUM, fontSize: 12.5, fontWeight: 700, minWidth: 34 }}>
            {percent}%
          </Typography>
          <LinearProgress
            variant="determinate"
            value={percent}
            sx={{ flex: 1, height: 5, borderRadius: 999 }}
          />
          <Typography
            sx={{ ...SINGLE_LINE, ...NUM, fontSize: 11.5, color: "text.secondary", minWidth: 70, textAlign: "right" }}
          >
            {formatSpeed(task.speedBps)}
          </Typography>
        </Stack>
      </Collapse>

      <Stack spacing={0.4}>
        <InfoLine
          label="体积"
          value={`${formatBytes(task.doneBytes)} / ${formatBytes(task.totalBytes)}`}
        />
        <InfoLine label="文件" value={`${task.finishedFiles} / ${task.totalFiles}`} />
        <Collapse open={running}>
          <Stack spacing={0.4}>
            {task.etaSeconds != null && (
              <InfoLine label="剩余" value={`约 ${formatDuration(task.etaSeconds)}`} />
            )}
            <InfoLine label="已用" value={formatDuration(elapsed)} />
          </Stack>
        </Collapse>
        {task.failed > 0 && (
          <InfoLine label="失败" value={`${task.failed} 个`} />
        )}
      </Stack>

      <Collapse open={task.failures.length > 0}>
        <Typography
          sx={{ mt: 0.5, fontSize: 11, color: "text.secondary", lineHeight: 1.6 }}
        >
          {task.failures.join("、")}
          {task.failed > task.failures.length ? ` 等 ${task.failed} 个` : ""}
        </Typography>
      </Collapse>
    </Paper>
  );
}

/**
 * 下载队列的悬浮球。
 *
 * 进出场用 WAAPI 自己管：进场 `scale(0.4) → 1` + 淡入（outBack 弹一下），
 * 退场 `scale(1) → 0.6` + 淡出，退场动画播完才卸载。
 * 参数在 `motion/tokens.ts` 的 `FAB_MOTION`，只动 transform / opacity。
 *
 * 刚弹出来的一小会儿不接点击：它常常正好盖在版本详情的「安装 / 取消」上，
 * 不挡一下会出现「点下载把抽屉点开了」的错觉。
 */
function DownloadBall(props: {
  show: boolean;
  percent: number;
  onClick: () => void;
}) {
  const { show, percent, onClick } = props;
  const ref = useRef<HTMLDivElement>(null);
  const [mounted, setMounted] = useState(show);
  /** 刚出现的一小会儿不接点击，避免盖住「安装」时被误点开 */
  const [interactive, setInteractive] = useState(false);

  // 先挂上再播进场；退场要等动画结束才卸载，所以不能反过来
  useEffect(() => {
    if (show) setMounted(true);
  }, [show]);

  useEffect(() => {
    if (!show) {
      setInteractive(false);
      return;
    }
    const timer = window.setTimeout(
      () => setInteractive(true),
      FAB_MOTION.enterDuration,
    );
    return () => window.clearTimeout(timer);
  }, [show]);

  useLayoutEffect(() => {
    const el = ref.current;
    if (!mounted || !el) return;

    // 关掉动画时不做过渡：直接挂/卸
    if (!animationsEnabled()) {
      if (!show) setMounted(false);
      return;
    }

    if (show) {
      const fade = el.animate([{ opacity: 0 }, { opacity: 1 }], {
        duration: FAB_MOTION.enterFade,
        easing: EASE.outFluent,
        fill: "both",
      });
      const grow = el.animate(
        [
          { transform: `scale(${FAB_MOTION.enterScaleFrom})` },
          { transform: "scale(1)" },
        ],
        {
          duration: FAB_MOTION.enterDuration,
          easing: EASE.outBack,
          fill: "both",
        },
      );
      return () => {
        fade.cancel();
        grow.cancel();
      };
    }

    const leave = el.animate(
      [
        { opacity: 1, transform: "scale(1)" },
        { opacity: 0, transform: `scale(${FAB_MOTION.exitScaleTo})` },
      ],
      {
        duration: FAB_MOTION.exitDuration,
        easing: EASE.outFluent,
        fill: "both",
      },
    );
    leave.finished
      .then(() => setMounted(false))
      .catch(() => undefined);
    return () => leave.cancel();
  }, [show, mounted]);

  if (!mounted) return null;

  return (
    <Box
      ref={ref}
      sx={{
        position: "fixed",
        right: 20,
        bottom: 20,
        zIndex: 1250,
        width: 52,
        height: 52,
        pointerEvents: interactive ? "auto" : "none",
      }}
    >
      <Fab
        size="medium"
        color="primary"
        onClick={onClick}
        aria-label="下载任务"
        sx={{ width: 52, height: 52 }}
      >
        <CircularProgress
          variant="determinate"
          value={percent}
          size={52}
          thickness={2.5}
          sx={{ position: "absolute", color: "primary.contrastText", opacity: 0.55 }}
        />
        <DownloadOutlinedIcon sx={{ fontSize: 22 }} />
      </Fab>
    </Box>
  );
}

/**
 * 下载管理：右下角悬浮球 + 从右侧推出来的抽屉。
 *
 * 这是所有下载任务的唯一入口 —— 版本列表、版本详情里都不再画自己的进度条，
 * 那里只留一个按钮。
 *
 * 规则（见 `docs/UI.md`）：
 * - 抽屉里不放标头。卡片本身就是内容，一张卡一个任务。
 * - 关闭靠点遮罩、按 Esc 或再点一次球，不额外做关闭按钮。
 * - 打开的瞬间球就退场；关掉抽屉时如果还有任务，球再弹回来。
 * - 队列空了不强制收回抽屉：留在原地显示「无下载任务」，点旁边自然关掉。
 */
export default function DownloadDock() {
  const tasks = useAllTasks();
  const { cancel, dismiss } = useDownloads();
  const [open, setOpen] = useState(false);
  const listRef = useRef<HTMLDivElement>(null);
  /** 卡片是否已经该出现了：抽屉先滑一段，卡片再落，两段节奏才看得出效果 */
  const [entered, setEntered] = useState(false);

  /**
   * 渲染用的卡片槽位。
   *
   * 任务从 `tasks` 消失后不能立刻卸载 —— 那样就是「啪一下没了」，
   * 得先标成 `leaving` 播退场动画，动画播完再真正摘掉。
   */
  const [slots, setSlots] = useState<{ task: TaskView; leaving: boolean }[]>([]);

  useEffect(() => {
    setSlots((prev) => {
      let changed = false;
      const next = prev.map((slot) => {
        const fresh = tasks.find((task) => task.taskId === slot.task.taskId);
        if (fresh) {
          if (fresh === slot.task) return slot;
          changed = true;
          return { task: fresh, leaving: false };
        }
        if (slot.leaving) return slot;
        changed = true;
        return { ...slot, leaving: true };
      });

      for (const task of tasks) {
        if (!prev.some((slot) => slot.task.taskId === task.taskId)) {
          next.push({ task, leaving: false });
          changed = true;
        }
      }
      return changed ? next : prev;
    });
  }, [tasks]);

  const dropSlot = useCallback((taskId: string) => {
    setSlots((prev) => prev.filter((slot) => slot.task.taskId !== taskId));
  }, []);

  // 抽屉刚打开时卡片是不存在的（opacity 0）。等抽屉滑进来一小段，
  // 再交给 playEnter 逐块落下 —— 不能先露出来再重播一遍。
  useEffect(() => {
    if (!open) {
      setEntered(false);
      return;
    }
    const timer = window.setTimeout(() => setEntered(true), CARD_REVEAL_DELAY_MS);
    return () => window.clearTimeout(timer);
  }, [open]);

  // 在 layoutEffect 里跑：翻出 opacity 的那一帧动画已经贴好首帧，不会闪
  useLayoutEffect(() => {
    if (!open || !entered) return;
    const list = listRef.current;
    if (!list) return;
    return playEnter(list);
  }, [open, entered]);

  const running = tasks.filter((task) => task.phase === "running");
  const doneBytes = running.reduce((sum, task) => sum + task.doneBytes, 0);
  const totalBytes = running.reduce((sum, task) => sum + task.totalBytes, 0);
  // 没有在跑的任务时环是满的：球还在，但一眼看出「没事干了」
  const percent =
    running.length === 0
      ? 100
      : totalBytes > 0
        ? Math.min(100, Math.round((doneBytes / totalBytes) * 100))
        : 0;

  return (
    <>
      <DownloadBall
        show={tasks.length > 0 && !open}
        percent={percent}
        onClick={() => setOpen(true)}
      />

      <Drawer
        anchor="right"
        open={open}
        onClose={() => setOpen(false)}
        // 抽屉要压在 Toast 之上（snackbar 默认 1400）
        sx={{ zIndex: (t) => t.zIndex.snackbar + 100 }}
        slotProps={{
          backdrop: {
            // 不要压暗整屏：面板自己是半透明的，后面该看见的东西就让它看见
            sx: { backgroundColor: "transparent" },
          },
          paper: {
            sx: {
              width: 400,
              maxWidth: "92vw",
              // 半透明毛玻璃：底色带 alpha，靠 blur 把后面糊掉
              bgcolor: (t) =>
                alpha(t.palette.background.paper, t.palette.mode === "dark" ? 0.72 : 0.78),
              backdropFilter: "blur(18px) saturate(1.15)",
              borderLeft: "1px solid",
              borderColor: "divider",
              backgroundImage: "none",
            },
          },
        }}
      >
        {slots.length === 0 && tasks.length === 0 ? (
          <Box sx={{ flex: 1, minHeight: 0, display: "grid", placeItems: "center" }}>
            <Typography sx={{ fontSize: 12.5, color: "text.secondary" }}>
              无下载任务
            </Typography>
          </Box>
        ) : (
          /* 没有标头：一张卡片一个任务 */
          <Stack
            ref={listRef}
            spacing={1}
            sx={{
              flex: 1,
              minHeight: 0,
              overflow: "auto",
              scrollbarGutter: "stable",
              p: 1.5,
              // 播放前先藏住，避免「先露一遍再重播」
              opacity: entered ? 1 : 0,
            }}
          >
            {slots.map((slot) => (
              <TaskCard
                key={slot.task.taskId}
                task={slot.task}
                leaving={slot.leaving}
                onCancel={() => void cancel(slot.task.taskId)}
                onDismiss={() => void dismiss(slot.task.taskId)}
                onExited={() => dropSlot(slot.task.taskId)}
              />
            ))}
          </Stack>
        )}
      </Drawer>
    </>
  );
}
