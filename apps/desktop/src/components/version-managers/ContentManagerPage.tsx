import {
  memo,
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type ReactNode,
} from "react";
import {
  Box,
  Button,
  ButtonBase,
  Chip,
  Dialog,
  DialogActions,
  DialogContent,
  DialogContentText,
  DialogTitle,
  IconButton,
  Stack,
  Switch,
  TextField,
  Tooltip,
  Typography,
  alpha,
} from "@mui/material";
import AddIcon from "@mui/icons-material/Add";
import DeleteOutlineOutlinedIcon from "@mui/icons-material/DeleteOutlined";
import FolderOpenOutlinedIcon from "@mui/icons-material/FolderOpenOutlined";
import RefreshOutlinedIcon from "@mui/icons-material/Refresh";
import { invoke } from "@tauri-apps/api/core";
import { contentConfig, type ContentKindConfig } from "../../lib/contentKinds";
import { useRegisterNestedPage } from "../../lib/nestedNav";
import { useScrollRestore } from "../../lib/useScrollRestore";
import {
  toAppError,
  type AppError,
  type ContentKind,
  type ContentListView,
  type ContentToggleView,
  type LocalContentView,
} from "../../lib/types";
import { playEnter } from "../../motion/pageMotion";
import PageTransition from "../../motion/PageTransition";
import ErrorState from "../ErrorState";
import Loading from "../Loading";
import PillTabs from "../PillTabs";
import { useToast } from "../Toast";
import ContentDetailPage from "./ContentDetailPage";

type ContentFilter = "all" | "enabled" | "disabled";

type ContentOverride = { enabled: boolean; fileName: string };
type ContentCounts = { enabled: number; disabled: number; duplicate: number };

const FILTERS: { value: ContentFilter; label: string }[] = [
  { value: "all", label: "全部" },
  { value: "enabled", label: "已启用" },
  { value: "disabled", label: "已禁用" },
];

/**
 * 版本内容管理器（整页子页）：Mod / 资源包 / 光影 / 材质包共用。
 *
 * 列表和下载页的 Mod 列表同一套视觉：72px 卡片、左侧图标、标题/作者/标签、
 * 单行简介；筛选切换时列表重新挂载并重播入场动画。
 *
 * 启停不按状态排序，也不做乐观更新：点开关后保持原状态，等后端返回再更新，
 * 行不会「飞」到列表末尾。文件一直在内容目录里，禁用只是改名为 `.disabled`。
 */
export default function ContentManagerPage(props: {
  versionId: string;
  kind: ContentKind;
}) {
  const { versionId, kind } = props;
  const config = contentConfig(kind);
  const toast = useToast();

  const [data, setData] = useState<ContentListView | null>(null);
  const [error, setError] = useState<AppError | null>(null);
  const [loaded, setLoaded] = useState(false);
  const [filter, setFilter] = useState<ContentFilter>("all");
  const [query, setQuery] = useState("");
  /** 启停后的本地覆盖：只影响这一条，不动 `data.items`、不重排、不重新加载。 */
  const [overrides, setOverrides] = useState<Record<string, ContentOverride>>({});
  const [counts, setCounts] = useState<ContentCounts>({
    enabled: 0,
    disabled: 0,
    duplicate: 0,
  });
  const [busyKey, setBusyKey] = useState<string | null>(null);
  const [installing, setInstalling] = useState(false);
  const [deleteTarget, setDeleteTarget] = useState<LocalContentView | null>(null);
  const [deleting, setDeleting] = useState(false);
  const [selected, setSelected] = useState<LocalContentView | null>(null);
  /** 详情页是否打开。返回时只关这个，`selected` 留到退场结束，旧页面才有东西可渲染 */
  const [detailOpen, setDetailOpen] = useState(false);

  const rootRef = useRef<HTMLDivElement>(null);
  const played = useRef(false);
  const listRef = useRef<HTMLDivElement>(null);
  const prevFilter = useRef(filter);
  const busyRef = useRef<string | null>(null);
  const dataRef = useRef<ContentListView | null>(null);
  dataRef.current = data;
  const overridesRef = useRef(overrides);
  overridesRef.current = overrides;

  const { ref: scrollRef, save: saveScroll, restore: restoreScroll } =
    useScrollRestore(`content:${kind}:${versionId}`);
  const attachScroll = useCallback(
    (node: HTMLDivElement | null) => {
      scrollRef.current = node;
      if (node) restoreScroll();
    },
    [restoreScroll, scrollRef],
  );

  const load = useCallback(
    async (silent = false) => {
      try {
        const next = await invoke<ContentListView>("content_list", {
          versionId,
          kind,
        });
        setData(next);
        setOverrides({});
        setCounts({
          enabled: next.enabledCount,
          disabled: next.disabledCount,
          duplicate: next.duplicateCount,
        });
        setError(null);
      } catch (err: unknown) {
        const appError = toAppError(err, `读取${config.label}列表失败`);
        if (silent) toast(appError.message, "error");
        else setError(appError);
      } finally {
        setLoaded(true);
      }
    },
    [config.label, kind, toast, versionId],
  );

  useEffect(() => {
    setData(null);
    setError(null);
    setLoaded(false);
    setSelected(null);
    setDetailOpen(false);
    setOverrides({});
    setCounts({ enabled: 0, disabled: 0, duplicate: 0 });
    setBusyKey(null);
    busyRef.current = null;
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

  // 只换了筛选档位时重新挂载卡片并重播一次入场；搜索输入不重播。
  useLayoutEffect(() => {
    if (prevFilter.current === filter) return;
    prevFilter.current = filter;
    const root = listRef.current;
    if (root) playEnter(root);
  }, [filter]);

  useRegisterNestedPage(
    detailOpen && selected ? selected.name : null,
    () => setDetailOpen(false),
  );

  const visible = useMemo(() => {
    if (!data) return [];
    const needle = query.trim().toLowerCase();
    return data.items.filter((item) => {
      const enabled = overrides[fileKey(item.fileName)]?.enabled ?? item.enabled;
      if (filter === "enabled" && !enabled) return false;
      if (filter === "disabled" && enabled) return false;
      if (!needle) return true;
      return (
        item.name.toLowerCase().includes(needle) ||
        (item.contentId ?? "").toLowerCase().includes(needle) ||
        item.fileName.toLowerCase().includes(needle)
      );
    });
  }, [data, filter, query, overrides]);

  const locked = Boolean(data?.running) || installing;

  const toggle = useCallback(
    async (fileName: string) => {
      if (busyRef.current || dataRef.current?.running) return;
      const key = fileKey(fileName);
      busyRef.current = key;
      setBusyKey(key);
      try {
        const result = await invoke<ContentToggleView>("content_toggle", {
          versionId,
          kind,
          fileName,
        });
        // 只记录这一条的本地状态，列表数据、顺序和其它卡片都不动。
        setOverrides((prev) => ({
          ...prev,
          [key]: { enabled: result.item.enabled, fileName: result.item.fileName },
        }));
        setCounts({
          enabled: result.enabledCount,
          disabled: result.disabledCount,
          duplicate: result.duplicateCount,
        });
      } catch (err: unknown) {
        toast(toAppError(err, `切换${config.label}状态失败`).message, "error");
      } finally {
        busyRef.current = null;
        setBusyKey(null);
      }
    },
    [config.label, kind, toast, versionId],
  );

  const remove = useCallback(async () => {
    if (!deleteTarget) return;
    setDeleting(true);
    try {
      const next = await invoke<ContentListView>("content_delete", {
        versionId,
        kind,
        fileName: deleteTarget.fileName,
      });
      setData(next);
      setOverrides({});
      setCounts({
        enabled: next.enabledCount,
        disabled: next.disabledCount,
        duplicate: next.duplicateCount,
      });
      setDeleteTarget(null);
      toast("已移到版本的回收站", "success");
    } catch (err: unknown) {
      toast(toAppError(err, `删除${config.label}失败`).message, "error");
    } finally {
      setDeleting(false);
    }
  }, [config.label, deleteTarget, kind, toast, versionId]);

  const install = useCallback(async () => {
    if (!data || locked) return;
    setInstalling(true);
    try {
      const next = await invoke<ContentListView | null>("content_install_local", {
        versionId,
        kind,
      });
      if (next) {
        setData(next);
        setOverrides({});
        setCounts({
          enabled: next.enabledCount,
          disabled: next.disabledCount,
          duplicate: next.duplicateCount,
        });
        toast(`已安装到 ${config.folder} 目录`, "success");
      }
    } catch (err: unknown) {
      toast(toAppError(err, `安装${config.label}失败`).message, "error");
    } finally {
      setInstalling(false);
    }
  }, [config.folder, config.label, data, kind, locked, toast, versionId]);

  const openFolder = useCallback(() => {
    void invoke("version_open_folder", { versionId, folder: config.folder }).catch(
      (err: unknown) => {
        toast(toAppError(err, "打开文件夹失败").message, "error");
      },
    );
  }, [config.folder, toast, versionId]);

  const openDetail = useCallback(
    (item: LocalContentView) => {
      const key = fileKey(item.fileName);
      const override = overridesRef.current[key];
      saveScroll();
      setSelected({
        ...item,
        enabled: override?.enabled ?? item.enabled,
        fileName: override?.fileName ?? item.fileName,
      });
      setDetailOpen(true);
    },
    [saveScroll],
  );

  const requestDelete = useCallback((item: LocalContentView, fileName: string) => {
    const override = overridesRef.current[fileKey(fileName)];
    setDeleteTarget({
      ...item,
      enabled: override?.enabled ?? item.enabled,
      fileName,
    });
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
          label={config.searchLabel}
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
          startIcon={<AddIcon sx={{ fontSize: 16 }} />}
          disabled={locked}
          onClick={() => void install()}
        >
          从本地安装
        </Button>
        <Button
          size="small"
          variant="outlined"
          startIcon={<FolderOpenOutlinedIcon sx={{ fontSize: 16 }} />}
          onClick={openFolder}
        >
          打开文件夹
        </Button>
        <Tooltip title="刷新">
          <span>
            <IconButton
              size="small"
              aria-label={`刷新${config.label}列表`}
              disabled={installing || busyKey !== null}
              onClick={() => void load(true)}
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
          共 {data.items.length} 个 · 启用 {counts.enabled} · 禁用 {counts.disabled}
        </Typography>
        {config.duplicateLabel && counts.duplicate > 0 && (
          <Chip
            size="small"
            label={`${counts.duplicate} 个 ${config.label} 有${config.duplicateLabel}`}
            sx={{ height: 20, fontSize: 10.5 }}
          />
        )}
        {data.folderCount > 0 && (
          <Typography sx={{ fontSize: 11.5, color: "text.secondary" }}>
            另有 {data.folderCount} 个文件夹条目未纳入管理，可用「打开文件夹」查看。
          </Typography>
        )}
      </Stack>

      {data.running && (
        <Box data-page-block>
          <ErrorState
            variant="inline"
            severity="warning"
            title="游戏正在运行或准备中"
            message={`先等启动结束并关闭游戏，再启用 / 禁用 / 删除或安装${config.label}。`}
          />
        </Box>
      )}

      {!data.isolated && (
        <Box data-page-block>
          <ErrorState
            variant="inline"
            severity="info"
            title="共享内容目录"
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
            {data.items.length === 0 ? config.empty : config.noMatch}
          </Typography>
        ) : (
          <Stack key={filter} ref={listRef} data-page-stagger spacing={1}>
            {visible.map((item) => {
              const key = fileKey(item.fileName);
              const override = overrides[key];
              return (
                <LocalContentCard
                  key={key}
                  item={item}
                  config={config}
                  fileName={override?.fileName ?? item.fileName}
                  enabled={override?.enabled ?? item.enabled}
                  locked={locked}
                  busy={busyKey === key}
                  onOpen={openDetail}
                  onToggle={toggle}
                  onDelete={requestDelete}
                />
              );
            })}
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
            文件会移到 versions/{versionId}/XCL/trash/{config.folder}，不会直接永久删除。
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
      pageKey={detailOpen ? "detail" : "list"}
      initial={false}
      render={(key) =>
        key === "detail" && selected ? (
          <ContentDetailPage versionId={versionId} kind={kind} item={selected} />
        ) : (
          list
        )
      }
    />
  );
}

function fileKey(fileName: string): string {
  return fileName.replace(/\.disabled$/, "");
}

const LocalContentCard = memo(function LocalContentCard(props: {
  item: LocalContentView;
  config: ContentKindConfig;
  /** 当前实际文件名（启停后带 / 不带 .disabled） */
  fileName: string;
  /** 当前实际启用状态；可能来自本地覆盖，不一定等于 item.enabled */
  enabled: boolean;
  locked: boolean;
  busy: boolean;
  onOpen: (item: LocalContentView) => void;
  onToggle: (fileName: string) => void;
  onDelete: (item: LocalContentView, fileName: string) => void;
}) {
  const { item, config, fileName, enabled, locked, busy, onOpen, onToggle, onDelete } =
    props;
  const Icon = config.icon;
  const sourceLabel =
    item.origin?.source === "curseforge"
      ? "CurseForge"
      : item.origin?.source === "modrinth"
        ? "Modrinth"
        : null;

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
            <Icon sx={{ fontSize: 21, color: "text.secondary" }} />
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
            {item.authors.length > 0 && (
              <Typography
                noWrap
                sx={{ fontSize: 11.5, color: "text.secondary", flexShrink: 0 }}
              >
                {item.authors.join(" / ")}
              </Typography>
            )}
            {item.version && <MiniLabel>v{item.version}</MiniLabel>}
            {item.packFormat != null && <MiniLabel>格式 {item.packFormat}</MiniLabel>}
            {sourceLabel && <MiniLabel>{sourceLabel}</MiniLabel>}
            {!enabled && <MiniLabel>已禁用</MiniLabel>}
            {item.duplicate && config.duplicateLabel && (
              <MiniLabel tone="warning">{config.duplicateLabel}</MiniLabel>
            )}
          </Stack>
          <Typography
            noWrap
            sx={{ fontSize: 11.5, color: "text.secondary", mt: 0.25 }}
          >
            {item.description || "没有简介"}
          </Typography>
        </Box>
      </ButtonBase>

      <Stack
        direction="row"
        sx={{ alignItems: "center", gap: 0.25, pr: 1, flexShrink: 0 }}
      >
        <Tooltip
          title={
            enabled
              ? `禁用（改名为 .disabled，文件保留在 ${config.folder}）`
              : "启用（去掉 .disabled 后缀）"
          }
        >
          <span>
            <Switch
              size="small"
              checked={enabled}
              disabled={locked || busy}
              onChange={() => onToggle(fileName)}
              slotProps={{ input: { "aria-label": `${item.name} 启用状态` } }}
            />
          </span>
        </Tooltip>
        <Tooltip title="删除（移到回收站）">
          <span>
            <IconButton
              size="small"
              disabled={locked || busy}
              aria-label={`删除 ${item.name}`}
              onClick={() => onDelete(item, fileName)}
            >
              <DeleteOutlineOutlinedIcon sx={{ fontSize: 17 }} />
            </IconButton>
          </span>
        </Tooltip>
      </Stack>
    </Box>
  );
});

function MiniLabel(props: { children: ReactNode; tone?: "warning" }) {
  const warning = props.tone === "warning";
  return (
    <Typography
      sx={{
        fontSize: 9.5,
        lineHeight: 1.6,
        px: 0.6,
        flexShrink: 0,
        borderRadius: 999,
        color: warning ? "warning.main" : "text.secondary",
        bgcolor: (t) =>
          alpha(warning ? t.palette.warning.main : t.palette.text.primary, warning ? 0.18 : 0.08),
      }}
    >
      {props.children}
    </Typography>
  );
}
