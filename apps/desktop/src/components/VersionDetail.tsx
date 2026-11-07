import { useCallback, useEffect, useRef, useState } from "react";
import {
  Box,
  Button,
  Divider,
  Paper,
  Stack,
  Typography,
} from "@mui/material";
import { apiGet } from "../lib/api";
import { animationsEnabled } from "../lib/appearance";
import { formatBytes } from "../lib/format";
import { loaderLabel, useLoaderKinds } from "../lib/loaderKinds";
import {
  toAppError,
  type AppError,
  type InstallPreview,
  type LoaderRef,
} from "../lib/types";
import { CONTENT_FADE } from "../motion/tokens";
import { useDownloads } from "./DownloadProvider";
import { useStarting, useTaskForVersion } from "../lib/downloadStore";
import { TaskProgress } from "./DownloadTask";
import ErrorState from "./ErrorState";
import LoaderPicker from "./LoaderPicker";
import Loading from "./Loading";
import VersionTypeChip from "./VersionTypeChip";

/**
 * 右侧版本详情面板。
 *
 * 拆出来的原因：清单展示 + 安装按钮 + 数据加载原本和版本列表挤在同一个组件里。
 * 现在它自带数据加载，容器只管「传哪个版本、开没开」。
 *
 * 关闭时不卸载、不清数据：滑出动画要有内容可播，下次打开也能秒显。
 *
 * 布局：标题行 + 滚动区（信息 / 加载器 / 清单统计）+ 固定底区（进度与按钮）。
 * 底部区 `flexShrink: 0`，加载器列表展开几十行也推不走它。
 *
 * 启动游戏的入口在启动页，这里不再重复放按钮（两个入口只会让人犹豫点哪个）。
 */
export default function VersionDetail(props: {
  open: boolean;
  versionId: string | null;
  /** 本地是否已安装。已安装时主按钮变成「校验并修复」 */
  installed: boolean;
  onClose: () => void;
}) {
  const { open, versionId, installed, onClose } = props;

  const [preview, setPreview] = useState<InstallPreview | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<AppError | null>(null);
  const [installError, setInstallError] = useState<AppError | null>(null);
  /** 装完之后重算清单：分组数量会变，`installed` 也要跟着翻成 true */
  const [previewRefresh, setPreviewRefresh] = useState(0);
  /**
   * 加载器选择跟着版本走（`for` 记它属于哪个版本）。
   *
   * 不用「切版本时 setLoader(null)」那种复位：复位是异步的，中间会有一帧
   * 拿着上一个版本的 loader 去查新版本的清单。这里直接按版本取值，
   * 换版本的那一刻 `loader` 就是 null，没有中间态。
   */
  const [selection, setSelection] = useState<{
    for: string | null;
    loader: LoaderRef | null;
  }>({ for: null, loader: null });
  const loader = selection.for === versionId ? selection.loader : null;
  const setLoader = useCallback(
    (next: LoaderRef | null) => setSelection({ for: versionId, loader: next }),
    [versionId],
  );

  const { start, cancel } = useDownloads();
  /** 「正在准备清单…」单独订阅，不跟任务表绑在一起 */
  const starting = useStarting();
  /** 加载器展示名由后端给（`Forge` / `NeoForge` / `Fabric`），前端不自己编 */
  const kinds = useLoaderKinds();

  /** 版本号还没落定的 loader 视为「没选」：空版本号既不能预览也不能安装 */
  const activeLoader = loader && loader.version ? loader : null;
  const activeLoaderKey = activeLoader
    ? `${activeLoader.kind}:${activeLoader.version}`
    : "";
  // 放进 ref，effect 只依赖字符串 key，避免每次都拿到新对象把请求打成循环
  const loaderRef = useRef<LoaderRef | null>(null);
  loaderRef.current = activeLoader;

  /**
   * 「装过」的单调标记。
   *
   * 任务一结束、卡片被清场，`task` 会变成 undefined，而外层清单可能还没重新拉到
   * （`installed` 仍是旧值），按钮就会从「校验并修复」弹回「安装」。
   * 这里记住「这次装成功过」——按版本 + 加载器记：装了 Forge 不等于
   * NeoForge 也装了，切加载器时必须清掉，否则按钮会冒充「校验并修复」。
   */
  const installedOnce = useRef(false);
  useEffect(() => {
    installedOnce.current = false;
  }, [versionId, activeLoaderKey]);
  const isInstalled = installed || installedOnce.current;

  /**
   * 这个面板该显示哪个任务。
   *
   * - 原版：任务的目标版本就是 `versionId`；
   * - 加载器：只有清单目标实例（`preview.versionId`，如 `forge-61.2.1-1.21.11`）
   *   对得上才算。只按 `parentId` 认会把「刚装完的 Forge」显示到 NeoForge 面板上。
   */
  const targetVersionId = activeLoader ? (preview?.versionId ?? null) : versionId;
  const task = useTaskForVersion(targetVersionId);

  if (task?.phase === "done") installedOnce.current = true;

  /** 当前 `preview` 属于哪个版本，用来区分「换版本」与「换加载器」 */
  const previewFor = useRef<string | null>(null);

  useEffect(() => {
    if (!versionId) return;
    let cancelled = false;

    // 换版本：清空并转圈；换加载器：旧数字留着，新清单回来直接替换，
    // 否则每点一个 loader 版本整个面板都会闪成转圈（连刚点的列表都没了）
    const sameVersion = previewFor.current === versionId;
    if (!sameVersion) {
      setLoading(true);
      setError(null);
      setPreview(null);
    }

    // 换加载器会重算清单（要拉 profile），连续点列表时只让最后一次生效
    const timer = window.setTimeout(
      () => {
        apiGet<InstallPreview>("install_preview", {
          versionId,
          loader: loaderRef.current,
        })
          .then((data) => {
            if (cancelled) return;
            previewFor.current = versionId;
            setPreview(data);
            setError(null);
          })
          .catch((e: unknown) => {
            if (!cancelled) {
              // 清单算不出来就把旧数字清掉，留着会和上面的错误提示互相矛盾
              setPreview(null);
              setError(toAppError(e, "无法生成安装清单"));
            }
          })
          .finally(() => {
            if (!cancelled) setLoading(false);
          });
      },
      sameVersion ? 250 : 0,
    );

    return () => {
      cancelled = true;
      window.clearTimeout(timer);
    };
  }, [versionId, activeLoaderKey, previewRefresh]);

  const handleInstall = useCallback(async () => {
    if (!versionId) return;
    setInstallError(null);
    try {
      await start(versionId, activeLoader ?? undefined);
    } catch (e: unknown) {
      setInstallError(toAppError(e, "无法开始安装"));
    }
  }, [activeLoader, start, versionId]);

  /** 装完之后重算一次清单（任务完成只触发一次） */
  const doneTaskId = useRef<string | null>(null);
  useEffect(() => {
    if (task?.phase !== "done") return;
    if (doneTaskId.current === task.taskId) return;
    doneTaskId.current = task.taskId;
    setPreviewRefresh((n) => n + 1);
  }, [task]);

  /**
   * 清单内容的淡入。
   *
   * 挂在 ref 回调上而不是 effect 里：内容块是「preview 有了才渲染」，
   * 所以它挂载的那一刻就是"转圈换成内容"的那一刻，直接在那里起播最可靠
   * （effect 还得多跑一轮依赖比较，且容易赶上面板展开的 260ms 而被吞掉）。
   *
   * `key={versionId}` 让内容块每个版本重建一次，于是每换一个版本刚好淡一次；
   * 换加载器只是同一块内容变数字，不重播（重播会整块闪一下，很吵）。
   */
  const contentRef = useRef<HTMLDivElement>(null);
  const fadedFor = useRef<string | null>(null);
  const attachContent = useCallback(
    (el: HTMLDivElement | null) => {
      contentRef.current = el;
      if (!el) return;
      const key = versionId ?? "";
      if (fadedFor.current === key) return;
      fadedFor.current = key;
      if (!animationsEnabled()) return;

      const fade = el.animate([{ opacity: 0 }, { opacity: 1 }], {
        duration: CONTENT_FADE.duration,
        easing: CONTENT_FADE.easing,
        fill: "both",
      });
      fade.finished.then(() => fade.cancel()).catch(() => undefined);
    },
    [versionId],
  );

  const busy = starting === versionId;
  /** 文案里要带上加载器名与版本号，否则不知道这次装的是什么 */
  const loaderSuffix = activeLoader
    ? ` ${loaderLabel(kinds, activeLoader.kind)} ${activeLoader.version}`
    : "";
  /**
   * 目标版本是否已经装好。
   *
   * 原版看清单里的「已安装」标记，加载器看预览里那个实例 —— 两者的 id 都不是
   * 同一个（`fabric-loader-x-y` vs `x-y`），不能让前端自己拼。
   */
  const targetInstalled =
    installedOnce.current ||
    (activeLoader ? preview?.installed === true : isInstalled);

  return (
    <Box
      sx={{
        width: open ? 328 : 0,
        ml: open ? 2 : 0,
        flexShrink: 0,
        overflow: "hidden",
        transition:
          "width 260ms cubic-bezier(0.4, 0, 0.2, 1), margin-left 260ms cubic-bezier(0.4, 0, 0.2, 1)",
      }}
    >
      <Paper
        variant="outlined"
        sx={{
          width: 328,
          height: "100%",
          display: "flex",
          flexDirection: "column",
          overflow: "hidden",
          borderRadius: 1,
          // 只淡入、不位移：和模组详情页的侧栏（`SlidePanel`）保持一致。
          // 面板宽度本来就在展开，内容再从右边滑 16px 会两个方向一起动。
          opacity: open ? 1 : 0,
          transition: "opacity 200ms ease",
        }}
      >
        <Stack
          direction="row"
          spacing={1}
          sx={{ alignItems: "center", px: 2, py: 1, flexShrink: 0 }}
        >
          <Typography sx={{ fontSize: 14, fontWeight: 600 }}>
            {versionId ?? preview?.versionId ?? ""}
          </Typography>
          {preview?.versionType && (
            <VersionTypeChip type={preview.versionType} />
          )}
          <Box sx={{ flex: 1 }} />
          <Button size="small" onClick={onClose}>
            关闭
          </Button>
        </Stack>
        <Divider />

        {/* 滚动区：只有信息、加载器、清单统计；按钮与进度钉在下面 */}
        <Box
          sx={{
            flex: 1,
            minHeight: 0,
            overflow: "auto",
            scrollbarGutter: "stable",
            p: 2,
          }}
        >
          {loading && !preview && (
            <Box sx={{ display: "grid", placeItems: "center", py: 6 }}>
              <Loading />
            </Box>
          )}

          {error && <ErrorState variant="banner" error={error} sx={{ mb: 1.25 }} />}

          {/* 加载中只留转圈：加载器那一行要等清单出来再一起出现，
              否则会出现「先冒一行空的加载器，内容才慢慢补上」 */}
          {(preview || error) && (
            <Stack key={versionId ?? "none"} ref={attachContent} spacing={1.25}>
              {preview?.releaseTime && (
                <InfoRow
                  label="发布时间"
                  value={preview.releaseTime.slice(0, 10)}
                />
              )}
              {preview && preview.javaMajor != null && (
                <InfoRow label="需要 Java" value={`Java ${preview.javaMajor}`} />
              )}
              {preview?.mainClass && (
                <InfoRow label="主类" value={preview.mainClass} mono />
              )}

              {preview && <Divider />}

              {/* 加载器不在清单块里面：清单拉失败时还得能切回来 */}
              {versionId && (
                <LoaderPicker
                  game={versionId}
                  refreshKey={previewRefresh}
                  value={loader}
                  onChange={setLoader}
                />
              )}

              {preview && <Divider />}

              {preview?.groups.map((group) => (
                <Stack
                  key={group.kind}
                  direction="row"
                  sx={{ alignItems: "center" }}
                >
                  <Typography
                    sx={{ fontSize: 12.5, color: "text.secondary", width: 72 }}
                  >
                    {group.label}
                  </Typography>
                  <Typography
                    sx={{
                      fontSize: 12.5,
                      width: 64,
                      textAlign: "right",
                      fontVariantNumeric: "tabular-nums",
                    }}
                  >
                    {group.count}
                  </Typography>
                  <Box sx={{ flex: 1 }} />
                  <Typography
                    sx={{ fontSize: 12.5, fontVariantNumeric: "tabular-nums" }}
                  >
                    {formatBytes(group.size)}
                  </Typography>
                </Stack>
              ))}

              {preview && <Divider />}

              {preview && (
                <Stack direction="row" sx={{ alignItems: "center" }}>
                  <Typography sx={{ fontSize: 13, fontWeight: 600 }}>
                    合计
                  </Typography>
                  <Box sx={{ flex: 1 }} />
                  <Typography
                    sx={{
                      fontSize: 13,
                      fontWeight: 600,
                      fontVariantNumeric: "tabular-nums",
                    }}
                  >
                    {preview.totalCount} 个 · {formatBytes(preview.totalSize)}
                  </Typography>
                </Stack>
              )}
            </Stack>
          )}
        </Box>

        {/* 固定底区：列表再长也推不走进度和按钮 */}
        <Divider />
        <Box sx={{ flexShrink: 0, px: 2, pt: 1.5, pb: 2 }}>
          <Stack spacing={1}>
            {installError && <ErrorState variant="banner" error={installError} />}

            {/* 这里显示这个版本自己的进度：球是全局入口，但盯着详情看的时候
                不该还得去点开抽屉才知道装到哪了 */}
            {task ? (
              <>
                <TaskProgress task={task} />
                {task.phase === "running" ? (
                  <Button
                    variant="outlined"
                    fullWidth
                    onClick={() => void cancel(task.taskId)}
                  >
                    取消安装
                  </Button>
                ) : (
                  <Button
                    variant="contained"
                    fullWidth
                    disabled={busy}
                    onClick={handleInstall}
                  >
                    {task.phase === "done"
                      ? `校验并修复${loaderSuffix}`
                      : "重试（只补缺失文件）"}
                  </Button>
                )}
              </>
            ) : (
              <Button
                variant={targetInstalled ? "outlined" : "contained"}
                fullWidth
                disabled={busy}
                onClick={handleInstall}
              >
                {busy
                  ? "正在准备清单…"
                  : `${targetInstalled ? "校验并修复" : "安装"}${loaderSuffix}`}
              </Button>
            )}
          </Stack>
        </Box>
      </Paper>
    </Box>
  );
}

function InfoRow(props: { label: string; value: string; mono?: boolean }) {
  return (
    <Stack direction="row" sx={{ alignItems: "baseline", gap: 1 }}>
      <Typography
        sx={{
          fontSize: 12.5,
          color: "text.secondary",
          width: 72,
          flexShrink: 0,
        }}
      >
        {props.label}
      </Typography>
      <Typography
        sx={{
          fontSize: 12.5,
          wordBreak: "break-all",
          fontFamily: props.mono ? "ui-monospace, monospace" : undefined,
        }}
      >
        {props.value}
      </Typography>
    </Stack>
  );
}
