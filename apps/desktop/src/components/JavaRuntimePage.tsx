import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import {
  Box,
  Button,
  Divider,
  LinearProgress,
  Paper,
  Stack,
  Typography,
  alpha,
} from "@mui/material";
import { invoke } from "@tauri-apps/api/core";
import { formatBytes } from "../lib/format";
import {
  JAVA_TASK_PREFIX,
  isJavaTask,
  toAppError,
  type AppError,
  type JavaComponent,
  type SystemJava,
} from "../lib/types";
import ErrorState from "./ErrorState";
import { TaskProgress } from "./DownloadTask";
import { useDownloads } from "./DownloadProvider";
import { useAllTasks } from "../lib/downloadStore";
import { useToast } from "./Toast";
import Loading from "./Loading";
import { playEnter } from "../motion/pageMotion";

/**
 * Java 运行环境管理。
 *
 * 数据来自 Mojang 官方的 java-runtime 清单，下载复用现有的并发引擎
 * （任务表跟游戏文件共用一张，所以进度条、取消都是同一套）。
 *
 * 下载到 `data/java/<组件>/`，跟 `versions/`、`libraries/` 平级 ——
 * 整个 `data/` 拷到别的机器就能直接玩。
 *
 * 布局：两张卡片 —— 「可下载的」和「本机已装的」。各自是一个动画块，
 * 加载完成后依次落下（`playEnter`；转圈换成内容时补一次入场，规则同 VersionList）。
 */
export default function JavaRuntimePage() {
  const toast = useToast();
  const tasks = useAllTasks();
  const { cancel } = useDownloads();

  const [components, setComponents] = useState<JavaComponent[]>([]);
  /** 本机扫出来的所有 Java（含我们自己下的） */
  const [detected, setDetected] = useState<SystemJava[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<AppError | null>(null);
  const [pendingRemove, setPendingRemove] = useState<string | null>(null);

  const rootRef = useRef<HTMLDivElement>(null);
  const entered = useRef(false);

  const load = useCallback(
    (silent = false) => {
      if (!silent) setLoading(true);
      setError(null);
      // 两件事一起拉：能下什么、本机已经有什么。
      // `java_detected` 走的是跟启动选 Java 同一套探测，所以「页面说已有」= 启动真能用到。
      Promise.all([
        invoke<JavaComponent[]>("java_components"),
        invoke<SystemJava[]>("java_detected"),
      ])
        .then(([list, found]) => {
          setComponents(list);
          setDetected(found);
        })
        .catch((e: unknown) => setError(toAppError(e, "读取 Java 列表失败")))
        .finally(() => setLoading(false));
    },
    [],
  );

  useEffect(() => {
    load();
  }, [load]);

  // 任何一个 Java 任务刚跑完，就刷新一次列表（装完/删完状态要同步）
  const finishedKey = useMemo(
    () =>
      tasks
        .filter((task) => isJavaTask(task.versionId) && task.phase !== "running")
        .map((task) => `${task.versionId}:${task.phase}`)
        .join("|"),
    [tasks],
  );
  const lastFinished = useRef("");
  useEffect(() => {
    if (!finishedKey || finishedKey === lastFinished.current) return;
    lastFinished.current = finishedKey;
    load(true);
  }, [finishedKey, load]);

  // 第二次点击才真删，避免误触把 143MB 点了
  useEffect(() => {
    if (!pendingRemove) return;
    const timer = window.setTimeout(() => setPendingRemove(null), 4000);
    return () => window.clearTimeout(timer);
  }, [pendingRemove]);

  // 转圈换成内容：两张卡片补一次逐个入场（缓存/已有数据时由外层页面动画负责）
  useLayoutEffect(() => {
    if (entered.current || loading) return;
    entered.current = true;
    if (rootRef.current) playEnter(rootRef.current);
  }, [loading]);

  const handleInstall = useCallback(
    async (component: JavaComponent) => {
      try {
        await invoke("java_install", { component: component.name });
      } catch (e: unknown) {
        toast(`无法下载 ${component.label}：${String(e)}`, "error");
      }
    },
    [toast],
  );

  const handleRemove = useCallback(
    async (component: JavaComponent) => {
      if (pendingRemove !== component.name) {
        setPendingRemove(component.name);
        return;
      }
      setPendingRemove(null);
      try {
        await invoke("java_remove", { component: component.name });
        toast(`已删除 ${component.label}`, "success");
        load(true);
      } catch (e: unknown) {
        toast(`删除失败：${String(e)}`, "error");
      }
    },
    [pendingRemove, toast, load],
  );

  return (
    <Box
      ref={rootRef}
      sx={{
        flex: 1,
        minHeight: 0,
        overflow: "auto",
        scrollbarGutter: "stable",
        display: "flex",
        flexDirection: "column",
        gap: 1.5,
        pb: 0.5,
        "& > *": { flexShrink: 0 },
      }}
    >
      {/* 卡片一：可下载的运行时 */}
      <Paper variant="outlined" sx={{ overflow: "hidden" }}>
        <Stack
          direction="row"
          spacing={1.5}
          sx={{ alignItems: "center", px: 2, py: 1, flexShrink: 0 }}
        >
          <Typography sx={{ fontSize: 13.5, fontWeight: 600 }}>
            可下载的 Java
          </Typography>
          <Typography sx={{ fontSize: 11.5, color: "text.secondary" }}>
            来自 Mojang 官方清单，装在 data/java/
          </Typography>
          <Box sx={{ flex: 1 }} />
          <Button size="small" onClick={() => load()} disabled={loading}>
            刷新
          </Button>
        </Stack>

        <Box sx={{ height: 2, flexShrink: 0 }}>
          {loading && <LinearProgress sx={{ height: 2 }} />}
        </Box>

        <Box sx={{ px: 2, py: 1 }}>
          {error && (
            <ErrorState variant="inline" error={error} onRetry={() => load()} />
          )}

          {loading && components.length === 0 && (
            <Box sx={{ display: "grid", placeItems: "center", py: 6 }}>
              <Loading />
            </Box>
          )}

          {components.map((component, index) => {
            const task = tasks.find(
              (item) => item.versionId === `${JAVA_TASK_PREFIX}${component.name}`,
            );
            const running = task?.phase === "running";
            const confirming = pendingRemove === component.name;

            return (
              <Box key={component.name}>
                {index > 0 && <Divider />}
                <Stack
                  direction="row"
                  spacing={1.5}
                  sx={{ alignItems: "center", py: 1.5 }}
                >
                  <Box sx={{ flex: 1, minWidth: 0 }}>
                    <Stack
                      direction="row"
                      spacing={1}
                      sx={{ alignItems: "center", mb: 0.25 }}
                    >
                      <Typography sx={{ fontSize: 13.5, fontWeight: 600 }}>
                        {component.label}
                      </Typography>
                      {component.installed && (
                        <Box
                          sx={{
                            fontSize: 10.5,
                            fontWeight: 600,
                            lineHeight: 1.5,
                            px: 0.6,
                            borderRadius: "4px",
                            bgcolor: (t) => alpha(t.palette.primary.dark, 0.16),
                            color: "text.secondary",
                          }}
                        >
                          已安装
                        </Box>
                      )}
                      {!component.installed && component.systemJava && (
                        <Box
                          sx={{
                            fontSize: 10.5,
                            fontWeight: 600,
                            lineHeight: 1.5,
                            px: 0.6,
                            borderRadius: "4px",
                            bgcolor: (t) => alpha(t.palette.primary.dark, 0.16),
                            color: "text.secondary",
                          }}
                        >
                          本机已有
                        </Box>
                      )}
                    </Stack>

                    <Typography
                      sx={{
                        fontSize: 11.5,
                        color: "text.secondary",
                        wordBreak: "break-all",
                      }}
                    >
                      {component.version || "版本未知"}
                      {component.installed && component.sizeOnDisk != null
                        ? ` · ${formatBytes(component.sizeOnDisk)}`
                        : ""}
                      {component.installed && component.path
                        ? ` · ${component.path}`
                        : ""}
                    </Typography>

                    {/* 本机已经有同主版本的 Java：直说，别白下 143MB */}
                    {!component.installed && component.systemJava && (
                      <Typography
                        sx={{
                          fontSize: 11.5,
                          color: "text.secondary",
                          wordBreak: "break-all",
                          mt: 0.25,
                        }}
                      >
                        本机已有 Java {component.systemJava.major}（
                        {component.systemJava.version}）· {component.systemJava.path}
                      </Typography>
                    )}

                    {task && (
                      <Box sx={{ mt: 1, maxWidth: 460 }}>
                        <TaskProgress task={task} />
                      </Box>
                    )}
                  </Box>

                  {running ? (
                    <Button
                      size="small"
                      variant="outlined"
                      onClick={() => cancel(task.taskId)}
                    >
                      取消
                    </Button>
                  ) : (
                    <Stack direction="row" spacing={1}>
                      {component.installed && (
                        <Button
                          size="small"
                          color={confirming ? "error" : "inherit"}
                          onClick={() => void handleRemove(component)}
                        >
                          {confirming ? "确认删除" : "删除"}
                        </Button>
                      )}
                      <Button
                        size="small"
                        variant={
                          component.installed
                            ? "outlined"
                            : component.systemJava
                              ? "text"
                              : "contained"
                        }
                        onClick={() => void handleInstall(component)}
                      >
                        {component.installed
                          ? "重新下载"
                          : component.systemJava
                            ? "仍要下载"
                            : "下载"}
                      </Button>
                    </Stack>
                  )}
                </Stack>
              </Box>
            );
          })}

          {!loading && !error && components.length === 0 && (
            <Typography sx={{ fontSize: 12.5, color: "text.secondary", py: 2 }}>
              当前平台没有可用的官方 Java 运行时。
            </Typography>
          )}
        </Box>

        <Divider />
        <Box sx={{ px: 2, py: 1 }}>
          <Typography sx={{ fontSize: 11.5, color: "text.secondary" }}>
            需要哪个版本：1.12 及更早用 Java 8，1.17 用 16，1.18~1.20.4 用 17，1.20.5 起用 21。
          </Typography>
        </Box>
      </Paper>

      {/* 卡片二：本机已装（注册表 / JAVA_HOME / PATH / Program Files / data/java 全扫一遍） */}
      <Paper variant="outlined" sx={{ overflow: "hidden" }}>
        <Stack
          direction="row"
          spacing={1.5}
          sx={{ alignItems: "center", px: 2, py: 1 }}
        >
          <Typography sx={{ fontSize: 13.5, fontWeight: 600 }}>
            本机已装 Java（{detected.length}）
          </Typography>
          <Typography sx={{ fontSize: 11.5, color: "text.secondary" }}>
            启动时自动挑版本合适的那个；标「XCL 管理」的是装在 data/java/ 的
          </Typography>
        </Stack>

        <Divider />

        <Box sx={{ px: 2, py: 1 }}>
          {loading && detected.length === 0 ? (
            <Box sx={{ display: "grid", placeItems: "center", py: 4 }}>
              <Loading />
            </Box>
          ) : detected.length === 0 ? (
            <Typography sx={{ fontSize: 12.5, color: "text.secondary", py: 1.5 }}>
              没检测到任何 Java。上面卡片里下载一个就能用。
            </Typography>
          ) : (
            detected.map((java) => (
              <Stack
                key={java.path}
                direction="row"
                spacing={1}
                sx={{ alignItems: "baseline", py: 0.5 }}
              >
                <Typography
                  sx={{ fontSize: 12.5, fontWeight: 600, minWidth: 58, flexShrink: 0 }}
                >
                  Java {java.major}
                </Typography>
                <Typography
                  sx={{
                    flex: 1,
                    minWidth: 0,
                    fontSize: 11.5,
                    color: "text.secondary",
                    wordBreak: "break-all",
                  }}
                >
                  {java.version} · {java.path}
                </Typography>
                {java.managed && (
                  <Box
                    sx={{
                      flexShrink: 0,
                      fontSize: 10.5,
                      fontWeight: 600,
                      lineHeight: 1.5,
                      px: 0.6,
                      borderRadius: "4px",
                      bgcolor: (t) => alpha(t.palette.primary.dark, 0.16),
                      color: "text.secondary",
                    }}
                  >
                    XCL 管理
                  </Box>
                )}
              </Stack>
            ))
          )}
        </Box>
      </Paper>
    </Box>
  );
}
