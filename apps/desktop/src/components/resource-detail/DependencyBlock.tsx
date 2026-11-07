/**
 * 右栏的「依赖」区块。
 *
 * 数据是异步来的：整块出现时补一段纯透明度淡入（`CONTENT_FADE`），
 * 不做位移、不做逐条错峰 —— 它和「文件列表已经在显示」是并列内容，
 * 逐条落下会和列表的生存周期抢节奏。
 */
import { useLayoutEffect, useRef } from "react";
import { Box, Button, Chip, Stack, Typography, alpha } from "@mui/material";

import { animationsEnabled } from "../../lib/appearance";
import { CONTENT_FADE } from "../../motion/tokens";
import type { AppError, ResourceDependency } from "../../lib/types";
import ErrorState from "../ErrorState";
import Loading from "../Loading";
import { RELATION_LABEL, panelListSx } from "./shared";

export function DependencyBlock(props: {
  deps: ResourceDependency[];
  /** 已知有依赖、但依赖解析还没回来：这里先出加载动画 */
  loading: boolean;
  error: AppError | null;
  onOpen: (dep: ResourceDependency) => void;
  onRetry: () => void;
}) {
  const { deps, loading, error, onOpen, onRetry } = props;
  const rootRef = useRef<HTMLDivElement>(null);

  useLayoutEffect(() => {
    const el = rootRef.current;
    if (!el || !animationsEnabled()) return;
    const fade = el.animate([{ opacity: 0 }, { opacity: 1 }], {
      duration: CONTENT_FADE.duration,
      easing: CONTENT_FADE.easing,
      fill: "both",
    });
    fade.finished.then(() => fade.cancel()).catch(() => undefined);
  }, []);

  return (
    <Box ref={rootRef}>
      {error && (
        <ErrorState variant="inline" error={error} onRetry={onRetry} sx={{ mb: 1 }} />
      )}
      {loading && (
        <>
          <Typography
            sx={{ fontSize: 12, color: "text.secondary", mb: 0.5, px: 0.5 }}
          >
            依赖
          </Typography>
          <Box sx={{ display: "grid", placeItems: "center", py: 2 }}>
            <Loading />
          </Box>
        </>
      )}
      {!loading && deps.length > 0 && (
        <>
          <Typography
            sx={{ fontSize: 12, color: "text.secondary", mb: 0.5, px: 0.5 }}
          >
            依赖（{deps.length}）
          </Typography>
          <Box sx={panelListSx}>
            {deps.map((dep) => (
              <DependencyRow
                key={`${dep.project.source}:${dep.project.id}`}
                dep={dep}
                onOpen={() => onOpen(dep)}
              />
            ))}
          </Box>
        </>
      )}
    </Box>
  );
}

/**
 * 依赖列表的一张卡：上面是图标 / 名称 / 关系，下面单独一行动作。
 *
 * 动作只看「装没装」和「兼不兼容」，不看能不能直接装：以前这里还要求
 * `dep.file !== null`，等于「没解析到可安装的文件就锁死入口」—— 明明想去看
 * 那个依赖支持哪些版本，却被一个红字挡住。跳到详情页本身没有代价，那一页会把
 * 「这个实例能装哪个文件」讲清楚。
 */
function DependencyRow(props: { dep: ResourceDependency; onOpen: () => void }) {
  const { dep, onOpen } = props;
  const canOpen = !dep.installed && dep.relation !== "incompatible";

  return (
    <Box
      sx={{
        px: 1.25,
        py: 0.75,
        borderBottom: "1px solid",
        borderColor: "divider",
        "&:last-of-type": { borderBottom: 0 },
      }}
    >
      <Stack direction="row" spacing={0.75} sx={{ alignItems: "center", minWidth: 0 }}>
        <Box
          sx={{
            width: 20,
            height: 20,
            flexShrink: 0,
            borderRadius: 0.5,
            overflow: "hidden",
            bgcolor: "action.hover",
            display: "grid",
            placeItems: "center",
          }}
        >
          {dep.project.iconUrl ? (
            <Box
              component="img"
              src={dep.project.iconUrl}
              alt=""
              sx={{ width: "100%", height: "100%", objectFit: "cover" }}
            />
          ) : (
            <Typography sx={{ fontSize: 10, color: "text.secondary" }}>
              {dep.project.title.slice(0, 1).toUpperCase()}
            </Typography>
          )}
        </Box>
        <Typography noWrap sx={{ fontSize: 12.5, fontWeight: 600, flex: 1, minWidth: 0 }}>
          {dep.project.title}
        </Typography>
        <Chip
          size="small"
          label={RELATION_LABEL[dep.relation] ?? dep.relation}
          sx={{
            height: 16,
            fontSize: 9.5,
            flexShrink: 0,
            ...(dep.relation === "incompatible"
              ? { color: "error.main", bgcolor: (t) => alpha(t.palette.error.main, 0.1) }
              : {}),
          }}
        />
      </Stack>

      <Stack direction="row" sx={{ alignItems: "center", mt: 0.75, minHeight: 26 }}>
        <Box sx={{ flex: 1 }} />
        {dep.installed ? (
          <Typography sx={{ fontSize: 11.5, color: "text.secondary" }}>
            已安装
          </Typography>
        ) : canOpen ? (
          <Stack direction="row" spacing={1} sx={{ alignItems: "center", minWidth: 0 }}>
            {/* 没解析到可安装文件时给个提示，但不拦入口 —— 点进去那一页会说清楚。
                文案直接用后端给的原因（"没有支持 1.21.1 · fabric 的文件""这个版本没有
                安装加载器"），比笼统的"无兼容文件"有用得多。 */}
            {dep.reason && (
              <Typography
                noWrap
                title={dep.reason}
                sx={{ fontSize: 11, color: "error.main", minWidth: 0 }}
              >
                {dep.reason}
              </Typography>
            )}
            <Button
              size="small"
              variant="outlined"
              onClick={onOpen}
              sx={{ minWidth: 0, flexShrink: 0 }}
            >
              前往安装
            </Button>
          </Stack>
        ) : (
          <Typography sx={{ fontSize: 11.5, color: "error.main" }}>
            不兼容
          </Typography>
        )}
      </Stack>
    </Box>
  );
}
