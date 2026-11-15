import {
  memo,
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import {
  Box,
  Button,
  ButtonBase,
  Dialog,
  DialogActions,
  DialogContent,
  DialogContentText,
  DialogTitle,
  IconButton,
  Stack,
  TextField,
  Tooltip,
  Typography,
} from "@mui/material";
import DeleteOutlineOutlinedIcon from "@mui/icons-material/DeleteOutlined";
import FolderOpenOutlinedIcon from "@mui/icons-material/FolderOpenOutlined";
import RefreshOutlinedIcon from "@mui/icons-material/Refresh";
import SaveOutlinedIcon from "@mui/icons-material/SaveOutlined";
import { invoke } from "@tauri-apps/api/core";
import { formatBytes, formatRelativeTime } from "../../lib/format";
import { useRegisterNestedPage } from "../../lib/nestedNav";
import { GAME_MODE_LABEL } from "../../lib/saves";
import { useScrollRestore } from "../../lib/useScrollRestore";
import { toAppError, type AppError, type SaveListView, type SaveView } from "../../lib/types";
import { playEnter } from "../../motion/pageMotion";
import PageTransition from "../../motion/PageTransition";
import ErrorState from "../ErrorState";
import Loading from "../Loading";
import MiniLabel from "../MiniLabel";
import PillTabs from "../PillTabs";
import { useToast } from "../Toast";
import SaveDetailPage from "./SaveDetailPage";
import SaveMapPage from "./SaveMapPage";

type SaveFilter = "all" | "recent" | "stale";

const FILTERS: { value: SaveFilter; label: string }[] = [
  { value: "all", label: "全部" },
  { value: "recent", label: "最近玩过" },
  { value: "stale", label: "很久没动" },
];

/** 「最近玩过」的窗口：一周内。 */
const RECENT_MS = 7 * 24 * 60 * 60 * 1000;
/** 「很久没动」的门槛：一个月。 */
const STALE_MS = 30 * 24 * 60 * 60 * 1000;

/**
 * 存档管理（整页子页）：和 Mod / 资源包 / 光影管理器同一套视觉。
 *
 * 两处必须有意的差别：
 * 1. 存档是目录，没有启用 / 禁用 —— 那一格换成「打开存档文件夹」，开关概念不存在。
 * 2. 排序按最后游玩时间倒序。Mod 按名字是因为它们没有时间概念；
 *    存档按时间才是人找世界的方式。
 */
export default function SaveManagerPage(props: { versionId: string }) {
  const { versionId } = props;
  const toast = useToast();

  const [data, setData] = useState<SaveListView | null>(null);
  const [error, setError] = useState<AppError | null>(null);
  const [loaded, setLoaded] = useState(false);
  const [filter, setFilter] = useState<SaveFilter>("all");
  const [query, setQuery] = useState("");
  /** 后台刷新中：按钮压暗，不占行、不加转圈 */
  const [refreshing, setRefreshing] = useState(false);
  const [deleteTarget, setDeleteTarget] = useState<SaveView | null>(null);
  const [deleting, setDeleting] = useState(false);
  const [selected, setSelected] = useState<SaveView | null>(null);
  /**
   * 当前在哪一层：列表 → 详情 → 整页地图。
   * 只用一个变量管三层，`selected` 留到退场结束，旧页面才有东西可渲染。
   */
  const [page, setPage] = useState<"list" | "detail" | "map">("list");

  const rootRef = useRef<HTMLDivElement>(null);
  const played = useRef(false);
  const listRef = useRef<HTMLDivElement>(null);
  const prevFilter = useRef(filter);

  const {
    ref: scrollRef,
    save: saveScroll,
    restore: restoreScroll,
  } = useScrollRestore(`saves:${versionId}`);
  const attachScroll = useCallback(
    (node: HTMLDivElement | null) => {
      scrollRef.current = node;
      if (node) restoreScroll();
    },
    [restoreScroll, scrollRef],
  );

  const load = useCallback(async () => {
    try {
      const next = await invoke<SaveListView>("save_list", { versionId });
      setData(next);
      setError(null);
    } catch (err: unknown) {
      setError(toAppError(err, "读取存档失败"));
    } finally {
      setLoaded(true);
      setRefreshing(false);
    }
  }, [versionId]);

  useEffect(() => {
    setData(null);
    setError(null);
    setLoaded(false);
    setRefreshing(false);
    setDeleteTarget(null);
    setSelected(null);
    setPage("list");
    played.current = false;
    void load();
  }, [load]);

  useEffect(() => {
    if (!loaded || !data || played.current) return;
    const root = rootRef.current;
    if (!root) return;
    played.current = true;
    playEnter(root);
  }, [data, loaded]);

  // 只换筛选档位时重新挂载卡片并重播一次入场；搜索输入不重播。
  useLayoutEffect(() => {
    if (prevFilter.current === filter) return;
    prevFilter.current = filter;
    const root = listRef.current;
    if (root) playEnter(root);
  }, [filter]);

  const visible = useMemo(() => {
    if (!data) return [];
    const needle = query.trim().toLowerCase();
    const now = Date.now();
    return data.items.filter((item) => {
      if (filter === "recent") {
        if (!item.lastPlayedMs || now - item.lastPlayedMs > RECENT_MS) return false;
      }
      if (filter === "stale") {
        if (item.lastPlayedMs !== null && now - item.lastPlayedMs < STALE_MS) return false;
      }
      if (!needle) return true;
      return (
        item.name.toLowerCase().includes(needle) ||
        item.folderName.toLowerCase().includes(needle)
      );
    });
  }, [data, filter, query]);

  // 标题栏一层就够：地图页的返回要退回详情，不是直接退回列表。
  useRegisterNestedPage(
    page === "detail" && selected
      ? selected.name
      : page === "map" && selected
        ? `${selected.name} · 地图`
        : null,
    () => setPage((current) => (current === "map" ? "detail" : "list")),
  );

  const locked = Boolean(data?.running);

  const refresh = useCallback(() => {
    if (refreshing) return;
    setRefreshing(true);
    void load();
  }, [load, refreshing]);

  const openRootFolder = useCallback(() => {
    void invoke("version_open_folder", { versionId, folder: "saves" }).catch(
      (err: unknown) => {
        toast(toAppError(err, "打开文件夹失败").message, "error");
      },
    );
  }, [toast, versionId]);

  const openSaveFolder = useCallback(
    (item: SaveView) => {
      void invoke("save_open_folder", {
        versionId,
        folderName: item.folderName,
      }).catch((err: unknown) => {
        toast(toAppError(err, "打开存档文件夹失败").message, "error");
      });
    },
    [toast, versionId],
  );

  const remove = useCallback(async () => {
    if (!deleteTarget) return;
    setDeleting(true);
    try {
      const next = await invoke<SaveListView>("save_delete", {
        versionId,
        folderName: deleteTarget.folderName,
      });
      setData(next);
      setDeleteTarget(null);
      toast("已移到版本的回收站", "success");
    } catch (err: unknown) {
      toast(toAppError(err, "删除存档失败").message, "error");
    } finally {
      setDeleting(false);
    }
  }, [deleteTarget, toast, versionId]);

  const openDetail = useCallback(
    (item: SaveView) => {
      saveScroll();
      setSelected(item);
      setPage("detail");
    },
    [saveScroll],
  );

  const handleDeleted = useCallback((next: SaveListView) => {
    setData(next);
    setPage("list");
  }, []);

  if (!loaded) {
    return (
      <Box sx={{ flex: 1, minHeight: 0, display: "grid", placeItems: "center" }}>
        <Loading />
      </Box>
    );
  }

  if (error && !data) {
    return (
      <Box sx={{ flex: 1, minHeight: 0, display: "flex", flexDirection: "column", p: 2 }}>
        <ErrorState
          variant="page"
          error={error}
          onRetry={() => {
            setLoaded(false);
            setError(null);
            void load();
          }}
        />
      </Box>
    );
  }

  if (!data) return null;

  const list = (
    <Box
      ref={rootRef}
      sx={{
        flex: 1,
        minHeight: 0,
        display: "flex",
        flexDirection: "column",
        gap: 1.5,
        p: 2,
      }}
    >
      <Stack
        data-page-block
        direction="row"
        sx={{ alignItems: "center", gap: 1, flexWrap: "wrap", flexShrink: 0 }}
      >
        <Box sx={{ width: 248, flexShrink: 0 }}>
          <PillTabs items={FILTERS} value={filter} onChange={setFilter} />
        </Box>
        <TextField
          size="small"
          label="搜索世界名 / 文件夹名"
          value={query}
          onChange={(event) => setQuery(event.target.value)}
          sx={{
            flex: 1,
            minWidth: 180,
            "& .MuiInputBase-input": { fontSize: 13 },
            "& .MuiInputLabel-root": { fontSize: 13 },
          }}
        />
        <Button
          size="small"
          variant="contained"
          startIcon={<FolderOpenOutlinedIcon sx={{ fontSize: 16 }} />}
          onClick={openRootFolder}
        >
          打开文件夹
        </Button>
        <Tooltip title="刷新">
          <span>
            <IconButton
              size="small"
              aria-label="刷新存档列表"
              disabled={refreshing || deleting}
              onClick={refresh}
            >
              <RefreshOutlinedIcon sx={{ fontSize: 18 }} />
            </IconButton>
          </span>
        </Tooltip>
      </Stack>

      <Stack
        data-page-block
        direction="row"
        sx={{ alignItems: "center", gap: 1, flexWrap: "wrap" }}
      >
        <Typography sx={{ fontSize: 11.5, color: "text.secondary" }}>
          共 {data.items.length} 个世界 · 占用 {formatBytes(data.totalBytes)}
        </Typography>
      </Stack>

      {data.running && (
        <Box data-page-block>
          <ErrorState
            variant="inline"
            severity="warning"
            title="游戏正在运行或准备中"
            message="先等启动结束并关闭游戏，再删除存档。"
          />
        </Box>
      )}

      {!data.isolated && (
        <Box data-page-block>
          <ErrorState
            variant="inline"
            severity="info"
            title="共享存档目录"
            message="这个版本没有隔离，改动会影响所有未隔离版本。"
          />
        </Box>
      )}

      <Box
        ref={attachScroll}
        sx={{
          flex: 1,
          minHeight: 0,
          overflow: "auto",
          scrollbarGutter: "stable",
          pb: 0.5,
        }}
      >
        {visible.length === 0 ? (
          <Typography sx={{ fontSize: 11.5, color: "text.secondary", p: 1.5 }}>
            {data.items.length === 0
              ? "这个版本还没有存档。进游戏新建一个世界，这里就会出现。"
              : "没有匹配的存档。"}
          </Typography>
        ) : (
          <Stack key={filter} ref={listRef} data-page-stagger spacing={1}>
            {visible.map((item) => (
              <SaveCard
                key={item.folderName}
                item={item}
                locked={locked}
                onOpen={openDetail}
                onOpenFolder={openSaveFolder}
                onDelete={setDeleteTarget}
              />
            ))}
          </Stack>
        )}
      </Box>

      <Dialog
        open={deleteTarget !== null}
        onClose={() => {
          if (!deleting) setDeleteTarget(null);
        }}
        maxWidth="xs"
        fullWidth
      >
        <DialogTitle sx={{ fontSize: 16, fontWeight: 700 }}>
          删除 {deleteTarget?.name ?? ""}？
        </DialogTitle>
        <DialogContent>
          <DialogContentText sx={{ fontSize: 13 }}>
            整个世界文件夹会移到 versions/{versionId}/XCL/trash/saves，不会直接永久删除。
          </DialogContentText>
        </DialogContent>
        <DialogActions sx={{ px: 3, pb: 2 }}>
          <Button onClick={() => setDeleteTarget(null)} disabled={deleting}>
            取消
          </Button>
          <Button
            variant="contained"
            color="error"
            disabled={deleting}
            onClick={() => void remove()}
          >
            删除
          </Button>
        </DialogActions>
      </Dialog>
    </Box>
  );

  return (
    <PageTransition
      pageKey={page}
      initial={false}
      render={(key) => {
        if (!selected) return list;
        if (key === "map") {
          return <SaveMapPage versionId={versionId} item={selected} />;
        }
        if (key === "detail") {
          return (
            <SaveDetailPage
              versionId={versionId}
              item={selected}
              onOpenMap={() => setPage("map")}
              onDeleted={handleDeleted}
            />
          );
        }
        return list;
      }}
    />
  );
}

const SaveCard = memo(function SaveCard(props: {
  item: SaveView;
  locked: boolean;
  /** 点卡片主体进详情页 */
  onOpen: (item: SaveView) => void;
  /** 右侧图标直接开目录 */
  onOpenFolder: (item: SaveView) => void;
  onDelete: (item: SaveView) => void;
}) {
  const { item, locked, onOpen, onOpenFolder, onDelete } = props;
  const modeLabel = item.gameMode ? GAME_MODE_LABEL[item.gameMode] : null;

  return (
    <Box
      sx={{
        width: "100%",
        height: 72,
        flexShrink: 0,
        minWidth: 0,
        overflow: "hidden",
        display: "flex",
        alignItems: "center",
        border: "1px solid",
        borderColor: "divider",
        borderRadius: 1,
        bgcolor: "surface.paper",
        backdropFilter: (t) => t.palette.surface.filter,
        transition: "background-color 160ms ease",
        "&:hover": { bgcolor: "action.hover" },
      }}
    >
      <ButtonBase
        onClick={() => onOpen(item)}
        sx={{
          flex: 1,
          minWidth: 0,
          height: "100%",
          display: "flex",
          alignItems: "center",
          justifyContent: "flex-start",
          gap: 1.5,
          pl: 1.5,
          pr: 1,
          textAlign: "left",
          overflow: "hidden",
        }}
      >
        <Box
          sx={{
            width: 40,
            height: 40,
            flexShrink: 0,
            borderRadius: 1,
            overflow: "hidden",
            bgcolor: "action.hover",
            display: "grid",
            placeItems: "center",
          }}
        >
          {item.icon ? (
            <Box
              component="img"
              src={item.icon}
              alt=""
              loading="lazy"
              sx={{ width: "100%", height: "100%", objectFit: "cover" }}
            />
          ) : (
            <SaveOutlinedIcon sx={{ fontSize: 21, color: "text.secondary" }} />
          )}
        </Box>

        <Box sx={{ flex: 1, minWidth: 0 }}>
          <Stack
            direction="row"
            spacing={0.75}
            sx={{ alignItems: "center", minWidth: 0 }}
          >
            <Typography noWrap sx={{ fontSize: 13.5, fontWeight: 600 }}>
              {item.name}
            </Typography>
            {item.gameVersion && <MiniLabel>{item.gameVersion}</MiniLabel>}
            {item.snapshot && <MiniLabel>快照</MiniLabel>}
            {modeLabel && modeLabel !== "生存" && <MiniLabel>{modeLabel}</MiniLabel>}
            {item.hardcore && <MiniLabel tone="warning">硬核</MiniLabel>}
            {item.broken && <MiniLabel tone="warning">读不出 level.dat</MiniLabel>}
          </Stack>
          <Typography noWrap sx={{ fontSize: 11.5, color: "text.secondary", mt: 0.25 }}>
            {item.broken
              ? item.folderName
              : `${item.lastPlayedMs ? `上次游玩 ${formatRelativeTime(item.lastPlayedMs)}` : "没有游玩记录"} · ${formatBytes(item.sizeBytes)}`}
          </Typography>
        </Box>
      </ButtonBase>

      <Stack
        direction="row"
        sx={{ alignItems: "center", gap: 0.25, pr: 1, flexShrink: 0 }}
      >
        <Tooltip title="打开存档文件夹">
          <span>
            <IconButton
              size="small"
              aria-label={`打开 ${item.name} 的文件夹`}
              onClick={() => onOpenFolder(item)}
            >
              <FolderOpenOutlinedIcon sx={{ fontSize: 17 }} />
            </IconButton>
          </span>
        </Tooltip>
        <Tooltip title="删除（移到回收站）">
          <span>
            <IconButton
              size="small"
              disabled={locked}
              aria-label={`删除 ${item.name}`}
              onClick={() => onDelete(item)}
            >
              <DeleteOutlineOutlinedIcon sx={{ fontSize: 17 }} />
            </IconButton>
          </span>
        </Tooltip>
      </Stack>
    </Box>
  );
});
