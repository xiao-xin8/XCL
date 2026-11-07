import { memo, useCallback, useEffect, useLayoutEffect, useRef, useState } from "react";
import {
  Box,
  Button,
  ButtonBase,
  Dialog,
  DialogActions,
  DialogContent,
  DialogTitle,
  Divider,
  Paper,
  Stack,
  Typography,
  type SxProps,
  type Theme,
} from "@mui/material";
import Inventory2Outlined from "@mui/icons-material/Inventory2Outlined";
import { invoke } from "@tauri-apps/api/core";
import { apiGet } from "../lib/api";
import { formatBytes, formatDate } from "../lib/format";
import type {
  AppError,
  ModItem,
  ResourceFile,
  ResourceFilesView,
  ResourceProject,
  TaskView,
} from "../lib/types";
import { toAppError } from "../lib/types";
import { useTask } from "../lib/downloadStore";
import { navigateTo } from "../lib/appNav";
import { playEnter } from "../motion/pageMotion";
import { TaskProgress } from "./DownloadTask";
import ErrorState from "./ErrorState";
import PageLoaderGate from "./PageLoaderGate";
import VersionTypeChip from "./VersionTypeChip";
import ResourceProjectPanel from "./ResourceProjectPanel";
import { cardSx } from "./resource-detail/shared";

/** 行高对齐 `LocalTargetRow`（本地版本行），两种列表看起来要像同一个东西。 */
const ROW_H = 68;

/**
 * 闸门外层的确定高度。
 *
 * 闸门内部是 `flex: 1` 的居中容器 —— 父级高度是 auto 时它会被算成 0，
 * 环就贴到左上角去了。给个 minHeight 才谈得上居中。
 */
const GATE_BOX_SX: SxProps<Theme> = {
  display: "flex",
  flexDirection: "column",
  minHeight: 120,
};

/** 行的卡片样式。提成常量：每次渲染新对象等于每行都让 emotion 重算一遍。 */
const ROW_CARD_SX = cardSx(false);

const ROW_BUTTON_SX: SxProps<Theme> = {
  width: "100%",
  height: ROW_H,
  display: "flex",
  alignItems: "center",
  justifyContent: "flex-start",
  textAlign: "left",
  gap: 1.5,
  px: 1.75,
  cursor: "pointer",
};

const ROW_BUTTON_DISABLED_SX: SxProps<Theme> = {
  ...ROW_BUTTON_SX,
  cursor: "default",
  opacity: 0.6,
};

/**
 * 整合包详情页。
 *
 * 和 Mod / 资源包详情页不是同一套流程：
 * - 那两种是「把文件塞进某个已有的本地版本」，所以左边要列本地版本、右边升入侧栏；
 * - 整合包装出来是一个新实例，没有本地版本可选。
 *
 * 所以这里没有侧栏：项目详情卡下面直接一排一排列版本，点一行弹确认框，
 * 确定了就开装。行的样式、图标、间距全部复用 `LocalTargetRow` 那一套。
 */
export default function ModpackDetailPage(props: { item: ModItem }) {
  const { item } = props;

  const [project, setProject] = useState<ResourceProject | null>(null);
  const [projectError, setProjectError] = useState<AppError | null>(null);
  const [files, setFiles] = useState<ResourceFile[] | null>(null);
  const [filesError, setFilesError] = useState<AppError | null>(null);
  const [reload, setReload] = useState(0);

  /** 待确认的版本（点一行先弹框，不直接下） */
  const [pending, setPending] = useState<ResourceFile | null>(null);
  const [taskId, setTaskId] = useState<string | null>(null);
  const [starting, setStarting] = useState(false);
  const [installError, setInstallError] = useState<AppError | null>(null);

  const task = useTask(taskId);

  /**
   * 版本列表的逐项入场。
   *
   * 不能只在数据回来时播一次：`PageLoaderGate` 补它那 400ms 最短停留时，
   * 列表节点还没挂载 —— 这时候既拿不到 root，又容易把 key 记成「播过了」，
   * 结果就是「转圈消失后列表硬生生出现，一点动画都没有」。
   *
   * 所以 key 先记到 `pendingAnim`，真正播过才写进 `lastAnim`；
   * 节点挂上那一刻（ref 回调）再补一次。
   */
  const listRef = useRef<HTMLDivElement | null>(null);
  const pendingAnim = useRef("");
  const lastAnim = useRef("");

  const playPending = useCallback(() => {
    const root = listRef.current;
    const key = pendingAnim.current;
    if (!root || !key || key === lastAnim.current) return;
    lastAnim.current = key;
    playEnter(root);
  }, []);

  useLayoutEffect(() => {
    pendingAnim.current =
      files && files.length > 0 && !filesError ? `${item.id}:${files.length}` : "";
    playPending();
  }, [files, filesError, item.id, playPending]);

  /** 稳定的 ref 回调：节点真正挂上的那一刻补播欠着的动画。 */
  const attachList = useCallback(
    (node: HTMLDivElement | null) => {
      listRef.current = node;
      if (node) playPending();
    },
    [playPending],
  );

  useEffect(() => {
    let alive = true;
    setProject(null);
    setProjectError(null);
    apiGet<ResourceProject>("resource_project", {
      source: item.source,
      id: item.id,
      kind: "modpack",
    })
      .then((next) => {
        if (alive) setProject(next);
      })
      .catch((error: unknown) => {
        if (alive) setProjectError(toAppError(error, "读取整合包详情失败"));
      });
    return () => {
      alive = false;
    };
  }, [item.id, item.source]);

  useEffect(() => {
    let alive = true;
    setFiles(null);
    setFilesError(null);
    apiGet<ResourceFilesView>("resource_files", {
      source: item.source,
      id: item.id,
      // `Option<String>` 的 IPC 参数必须显式传 null，不能省略 ——
      // 省略会直接被 Tauri 判成参数缺失，整条命令失败。
      versionId: null,
      manual: true,
      kind: "modpack",
    })
      .then((next) => {
        if (alive) setFiles(next.items);
      })
      .catch((error: unknown) => {
        if (alive) setFilesError(toAppError(error, "读取整合包版本失败"));
      });
    return () => {
      alive = false;
    };
  }, [item.id, item.source, reload]);

  const running = task?.phase === "running";
  const disabled = running || starting;

  const selectVersion = useCallback((file: ResourceFile) => {
    setInstallError(null);
    setPending(file);
  }, []);

  const start = useCallback(
    async (file: ResourceFile) => {
      setStarting(true);
      setInstallError(null);
      try {
        const view = await invoke<TaskView>("modpack_install", {
          source: item.source,
          id: item.id,
          versionId: file.fileId,
        });
        setTaskId(view.taskId);
        setPending(null);
      } catch (error: unknown) {
        setInstallError(toAppError(error, "开始安装失败"));
      } finally {
        setStarting(false);
      }
    },
    [item.id, item.source],
  );

  // 装完直接回启动页 —— 新实例就在那儿，不用在这儿再看一遍结果。
  // 切页动画由 `PageTransition` 负责（它认 nav 的变化），这里只递个意图。
  const done = task?.phase === "done";
  useEffect(() => {
    if (done) navigateTo("launch");
  }, [done]);

  return (
    <Box
      sx={{
        flex: 1,
        minWidth: 0,
        minHeight: 0,
        overflow: "auto",
        scrollbarGutter: "stable",
        display: "flex",
        flexDirection: "column",
        gap: 1.5,
      }}
    >
      {/* 项目详情卡：和资源详情页同一个组件 */}
      <ResourceProjectPanel
        item={item}
        project={project}
        loading={project === null && projectError === null}
        error={projectError}
        onRetry={() => {
          setProjectError(null);
          setProject(null);
          void apiGet<ResourceProject>("resource_project", {
            source: item.source,
            id: item.id,
            kind: "modpack",
          })
            .then(setProject)
            .catch((error: unknown) =>
              setProjectError(toAppError(error, "读取整合包详情失败")),
            );
        }}
      />

      <Divider />

      {/* 版本：一排一排，样式照本地版本行 */}
      <Box sx={{ pb: 1 }}>
        <Box
          data-page-block
          sx={{
            display: "flex",
            alignItems: "center",
            gap: 1,
            mb: 1,
            minHeight: 24,
          }}
        >
          <Typography sx={{ fontSize: 13, fontWeight: 600 }}>选择版本</Typography>
          <Typography sx={{ fontSize: 11.5, color: "text.secondary" }}>
            点一下确认后开始安装，会新建一个版本
          </Typography>
        </Box>

        <Box sx={GATE_BOX_SX}>
          {/* 闸门必须包住列表：400ms 内回来不出环，超过才淡入且至少停 400ms。
              外面这层给个确定高度，闸门里的 `flex: 1` 才撑得开 —— 否则环会
              贴在左上角（`flex: 1` 在 auto 高度的父级里会塌成 0）。 */}
          <PageLoaderGate loading={files === null && filesError === null}>
            {filesError ? (
              <ErrorState error={filesError} onRetry={() => setReload((n) => n + 1)} />
            ) : files === null ? null /* 闸门在等 400ms，别先把空态漏出来 */ : files.length === 0 ? (
              <Typography sx={{ fontSize: 11.5, color: "text.secondary", p: 1.5 }}>
                这个整合包还没有可下载的版本。
              </Typography>
            ) : (
              <Stack ref={attachList} spacing={1} data-page-stagger>
                {files.map((file) => (
                  <ModpackVersionRow
                    key={file.fileId}
                    file={file}
                    iconUrl={item.iconUrl}
                    disabled={disabled}
                    onSelect={selectVersion}
                  />
                ))}
              </Stack>
            )}
          </PageLoaderGate>
        </Box>
      </Box>

      {/* 安装进度：卡片直接跟在列表下面，不做侧栏 */}
      {task && (
        <Paper variant="outlined" sx={{ ...cardSx(false), p: 1.5, mb: 1 }}>
          <Typography noWrap sx={{ fontSize: 12.5, fontWeight: 600, mb: 0.75 }}>
            {task.title ?? "整合包"}
          </Typography>
          <TaskProgress task={task} />
        </Paper>
      )}

      {installError && (
        <Typography sx={{ fontSize: 11.5, color: "error.main", pb: 1 }}>
          {installError.message}
        </Typography>
      )}

      {/* 确认框：点行先问一句，确定了才下 */}
      <Dialog
        open={pending !== null}
        onClose={() => {
          if (!starting) setPending(null);
        }}
        maxWidth="xs"
        fullWidth
        slotProps={{
          paper: {
            // 和更新弹窗一致：不投影，只留一圈描边
            elevation: 0,
            sx: { borderRadius: 1, border: "1px solid", borderColor: "divider" },
          },
        }}
      >
        <DialogTitle sx={{ fontSize: 15, fontWeight: 700, py: 1.75 }}>
          安装这个整合包？
        </DialogTitle>
        <DialogContent sx={{ pt: 0.5 }}>
          <Stack spacing={0.75}>
            <Typography sx={{ fontSize: 13 }}>
              {item.title}
              {pending ? ` · ${pending.versionNumber}` : ""}
            </Typography>
            {pending && (
              <Typography sx={{ fontSize: 11.5, color: "text.secondary" }}>
                {pending.gameVersions.slice(0, 4).join(" / ")}
                {pending.published ? ` · ${formatDate(pending.published)}` : ""}
                {` · ${formatBytes(pending.size)}`}
              </Typography>
            )}
            <Typography sx={{ fontSize: 11.5, color: "text.secondary" }}>
              会在启动页新建一个版本，名字用整合包名（重名会自动加序号）。
            </Typography>
          </Stack>
        </DialogContent>
        <DialogActions sx={{ px: 3, pb: 2, pt: 1 }}>
          <Button
            onClick={() => setPending(null)}
            disabled={starting}
            sx={{ height: 32 }}
          >
            取消
          </Button>
          <Button
            variant="contained"
            onClick={() => pending && void start(pending)}
            disabled={starting}
            sx={{ height: 32 }}
          >
            {starting ? "正在开始" : "安装"}
          </Button>
        </DialogActions>
      </Dialog>
    </Box>
  );
}

/**
 * 一行整合包版本。
 *
 * 结构、高度、圆角、悬停都和 `LocalTargetRow`（本地版本行）一致，
 * 只把左边的「加载器图标」换成整合包自己的图标。
 *
 * `memo` + 稳定的 `onSelect`：安装时 `useTask` 每来一帧进度都会让父组件
 * 重渲染，不拦的话 50 行全跟着重算一遍 `sx`，界面就卡住了。
 */
const ModpackVersionRow = memo(function ModpackVersionRow(props: {
  file: ResourceFile;
  iconUrl: string | null;
  disabled: boolean;
  onSelect: (file: ResourceFile) => void;
}) {
  const { file, iconUrl, disabled, onSelect } = props;
  const handleClick = useCallback(() => {
    if (!disabled) onSelect(file);
  }, [disabled, onSelect, file]);

  return (
    <Paper variant="outlined" sx={ROW_CARD_SX}>
      <ButtonBase
        // 不用真 `disabled`：那会掐掉涟漪（`UI.md` §4）。安装中点了不响应，
        // 但鼠标反馈还在，不会有「点一下什么都没发生」的错觉。
        onClick={handleClick}
        sx={disabled ? ROW_BUTTON_DISABLED_SX : ROW_BUTTON_SX}
      >
        <PackIcon url={iconUrl} />
        <Box sx={{ minWidth: 0, flex: 1 }}>
          <Box sx={{ display: "flex", alignItems: "center", gap: 0.75, minWidth: 0 }}>
            <Typography noWrap sx={{ fontSize: 13.5, fontWeight: 600 }}>
              {file.versionNumber}
            </Typography>
            {/* 类型徽标：全项目统一的那一个（`VersionTypeChip`） */}
            <VersionTypeChip type={file.releaseType} />
          </Box>
          <Typography noWrap sx={{ fontSize: 11.5, color: "text.secondary", mt: 0.25 }}>
            {versionLine(file)}
          </Typography>
        </Box>
      </ButtonBase>
    </Paper>
  );
});

/** 副行：MC 版本 · 体积 · 日期。缺哪项就不占位，不留「· ·」这种空档。 */
function versionLine(file: ResourceFile): string {
  const parts: string[] = [];
  if (file.gameVersions.length > 0) {
    parts.push(
      file.gameVersions.slice(0, 3).join(" / ") +
        (file.gameVersions.length > 3 ? " 等" : ""),
    );
  }
  if (file.size > 0) parts.push(formatBytes(file.size));
  if (file.published) parts.push(formatDate(file.published));
  return parts.join(" · ");
}

/** 整合包图标：没有就用占位图标，不让行塌掉。 */
function PackIcon(props: { url: string | null }) {
  const { url } = props;
  return (
    <Box
      sx={{
        width: 32,
        height: 32,
        flexShrink: 0,
        borderRadius: 0.75,
        overflow: "hidden",
        bgcolor: "action.hover",
        display: "grid",
        placeItems: "center",
      }}
    >
      {url ? (
        <Box
          component="img"
          src={url}
          alt=""
          sx={{ width: "100%", height: "100%", objectFit: "cover" }}
        />
      ) : (
        <Inventory2Outlined sx={{ fontSize: 18, color: "text.secondary" }} />
      )}
    </Box>
  );
}
