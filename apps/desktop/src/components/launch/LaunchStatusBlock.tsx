/**
 * 详情卡顶部的启动状态。
 *
 * 不许做成进度条：准备阶段（读 JSON / 挑 Java / 解 natives）在本机 200ms 内
 * 就跑完了，进程起来到窗口出现那一段又没有可信的进度可报 —— 硬做的条只会
 * 闪到 100% 然后晾在那儿等窗口。这里只给状态本身，进度交给下载抽屉。
 *
 * 不转圈：这里没有任何加载动画，就是一句话。转圈留给真的在加载列表的地方。
 *
 * 「游戏已关闭」这类终态只留 [`CLOSED_LINGER_MS`] 就收起，不能一直挂在详情里。
 * 块的高度会变（运行中多一行 Java 路径、失败时多一行原因），所以整个块走
 * `Collapse` 进场 / 离场，里面附加的行再走 `Reveal`：长度和出现都有动画。
 */
import { useEffect, useRef, useState } from "react";
import { Box, Collapse, Fade, Stack, Typography, alpha } from "@mui/material";

import { animationsEnabled } from "../../lib/appearance";
import { PULSE } from "../../motion/tokens";
import type { LaunchStatus } from "../../lib/types";
import Reveal from "../settings/Reveal";
import { formatDuration } from "./format";

/** 「游戏已关闭」这类终态在详情里留多久，然后整块收回（不是动画时长，是停留时间） */
const CLOSED_LINGER_MS = 5000;

/** 收起动画的保守上限（`Collapse timeout="auto"` 一般 200ms 出头）：过了就摘掉外壳 */
const COLLAPSE_FALLBACK_MS = 400;

/**
 * 已经收起过的会话。
 *
 * 必须放在模块作用域：状态块会随页面卸载重建，只用组件 state 的话，
 * 切到别的页再切回来，「游戏已关闭」会重新出现再等 5 秒（踩过）。
 */
const closedSessions = new Set<string>();

export function LaunchStatusBlock(props: {
  status: LaunchStatus | null;
  starting: boolean;
}) {
  const { status, starting } = props;
  const [elapsed, setElapsed] = useState(0);
  /** 正在播收起动画（还没从 DOM 里摘掉） */
  const [collapsing, setCollapsing] = useState(false);
  /** 已经彻底收掉，连 `Collapse` 的外壳都不留 —— 否则切页回来会留一截空白 */
  const [dismissed, setDismissed] = useState<string | null>(() =>
    status && closedSessions.has(status.sessionId) ? status.sessionId : null,
  );
  const pulseRef = useRef<HTMLDivElement | null>(null);

  const phase = status?.phase ?? (starting ? "preparing" : null);
  const sessionId = status?.sessionId ?? null;
  const running = phase === "running";
  const failed = phase === "failed";
  const ended = phase === "exited" || phase === "cancelled";

  useEffect(() => {
    if (!status) {
      setElapsed(0);
      return;
    }
    setElapsed(status.durationMs);
    if (status.phase !== "running") return;
    const timer = window.setInterval(() => setElapsed((value) => value + 1000), 1000);
    return () => window.clearInterval(timer);
  }, [status?.sessionId, status?.phase, status?.durationMs]);

  // 关闭了就别赖着：留 5 秒让人看到「已关闭 + 时长」，然后收起。
  // 收起过就记在模块级 Set 里 —— 组件会随页面卸载重建，只用 state 记不住。
  useEffect(() => {
    if (!sessionId || !ended) {
      setCollapsing(false);
      return;
    }
    if (closedSessions.has(sessionId)) {
      setCollapsing(true);
      return;
    }
    const timer = window.setTimeout(() => {
      if (closedSessions.size > 64) closedSessions.clear();
      closedSessions.add(sessionId);
      setCollapsing(true);
    }, CLOSED_LINGER_MS);
    return () => window.clearTimeout(timer);
  }, [sessionId, ended]);

  // 收起动画播完就把壳也摘掉。不靠 `onExited`：它跟 Collapse 版本的实现细节
  // 绑得太紧，漏一次就又是一截空白。
  useEffect(() => {
    if (!collapsing || !sessionId) return;
    const timer = window.setTimeout(() => setDismissed(sessionId), COLLAPSE_FALLBACK_MS);
    return () => window.clearTimeout(timer);
  }, [collapsing, sessionId]);

  // 「正在启动」要动，但不许转圈：整行做呼吸（只动 opacity）
  useEffect(() => {
    const el = pulseRef.current;
    if (!el || phase !== "preparing" || !animationsEnabled()) return;
    const anim = el.animate(
      [{ opacity: 1 }, { opacity: PULSE.minOpacity }, { opacity: 1 }],
      { duration: PULSE.duration, iterations: Infinity, easing: "ease-in-out" },
    );
    return () => anim.cancel();
  }, [phase]);

  // 彻底收掉：直接不渲染，连一个空 div 都不留
  if (sessionId && dismissed === sessionId) return null;

  const visible = phase !== null && !collapsing;

  const title = !phase
    ? ""
    : phase === "preparing"
      ? "正在启动"
      : running
        ? "游戏运行中"
        : failed
          ? "启动失败"
          : "游戏已关闭";

  const tone = failed ? "error" : running ? "success" : "primary";

  // 附加行：运行中给 Java 信息，失败给原因；两者不会同时存在
  const detail = running
    ? status?.javaPath
      ? `${status.javaMajor != null ? `Java ${status.javaMajor} · ` : ""}${status.javaPath}`
      : null
    : failed
      ? (status?.message ?? null)
      : null;

  return (
    <Collapse in={visible} timeout="auto" unmountOnExit>
      <Box
        sx={{
          px: 1.5,
          py: 1.25,
          borderRadius: 1,
          bgcolor: (t) => alpha(t.palette[tone].main, 0.07),
        }}
      >
        <Stack direction="row" spacing={1} sx={{ alignItems: "center", minHeight: 22 }}>
          {/* 准备阶段不放图标：这里没有加载动画 */}
          <Box sx={{ width: 7, height: 7, flexShrink: 0, display: "grid", placeItems: "center" }}>
            {(running || failed) && (
              <Box
                sx={{
                  width: 7,
                  height: 7,
                  borderRadius: "50%",
                  bgcolor: failed ? "error.main" : "success.main",
                }}
              />
            )}
          </Box>
          {/* 呼吸容器：只在「正在启动」时整行明暗起伏（WAAPI 循环，不转圈） */}
          <Box ref={pulseRef} sx={{ display: "flex", alignItems: "center" }}>
            {/* 状态名字换的时候淡入，不要直接换字 */}
            <Fade in key={phase ?? "none"} timeout={160}>
              <Typography sx={{ fontSize: 13, fontWeight: 600, lineHeight: 1.5 }}>
                {title}
              </Typography>
            </Fade>
          </Box>
          <Box sx={{ flex: 1 }} />
          {running && (
            <Typography
              sx={{
                fontSize: 11.5,
                color: "text.secondary",
                fontVariantNumeric: "tabular-nums",
              }}
            >
              已运行 {formatDuration(elapsed)}
            </Typography>
          )}
          {ended && elapsed > 0 && (
            <Typography
              sx={{
                fontSize: 11.5,
                color: "text.secondary",
                fontVariantNumeric: "tabular-nums",
              }}
            >
              {formatDuration(elapsed)}
            </Typography>
          )}
        </Stack>

        <Reveal open={Boolean(detail)}>
          <Typography
            title={detail ?? ""}
            sx={{
              fontSize: 11.5,
              lineHeight: 1.7,
              mt: 0.25,
              color: failed ? "error.main" : "text.secondary",
              whiteSpace: "pre-wrap",
              wordBreak: "break-all",
            }}
          >
            {detail}
          </Typography>
        </Reveal>
      </Box>
    </Collapse>
  );
}
