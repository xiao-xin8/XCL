import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type ChangeEvent,
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
  List,
  Menu,
  MenuItem,
  Paper,
  Stack,
  TextField,
  Tooltip,
  Typography,
  useMediaQuery,
} from "@mui/material";
import { invoke } from "@tauri-apps/api/core";
import ArticleOutlinedIcon from "@mui/icons-material/ArticleOutlined";
import DeleteOutlinedIcon from "@mui/icons-material/DeleteOutlined";
import EditOutlinedIcon from "@mui/icons-material/EditOutlined";
import StarRoundedIcon from "@mui/icons-material/StarRounded";
import TuneOutlinedIcon from "@mui/icons-material/TuneOutlined";
import {
  toAppError,
  type AccountView,
  type AppConfig,
  type IsolationView,
  type JavaComponent,
  type LocalVersion,
  type VersionStats,
} from "../lib/types";
import { invalidate } from "../lib/api";
import { formatBytes, formatRelativeTime } from "../lib/format";
import { foldersFor, type VersionFolderKey } from "../lib/versionFolders";
import { TaskProgress } from "./DownloadTask";
import VersionIcon from "./VersionIcon";
import AccountPage from "./AccountPage";
import LoginSidebar from "./LoginSidebar";
import LogDialog from "./LogDialog";
import Splitter from "./Splitter";
import { useToast } from "./Toast";
import { useDownloads } from "./DownloadProvider";
import { useTaskForVersion } from "../lib/downloadStore";
import { useLaunch } from "./LaunchProvider";
import ErrorState from "./ErrorState";
import Loading from "./Loading";
import { playEnter } from "../motion/pageMotion";
import PageTransition from "../motion/PageTransition";
import { useRegisterNestedPage } from "../lib/nestedNav";
import { useScrollRestore } from "../lib/useScrollRestore";
import VersionSettingsPage from "./version-settings/VersionSettingsPage";
import VersionTypeChip, { versionTypeLabel } from "./VersionTypeChip";
import { VersionRow } from "./launch/VersionRow";
import { LaunchStatusBlock } from "./launch/LaunchStatusBlock";
import { DetailRow } from "./launch/DetailRow";
import {
  DETAIL_MAX,
  DETAIL_MIN,
  LAYOUT_DEFAULT,
  LAYOUT_STORAGE_KEY,
  SIDEBAR_MAX,
  SIDEBAR_MIN,
  readLayout,
} from "./launch/layout";
import type { LaunchLayout } from "./launch/layout";

/**
 * 启动页。
 *
 * 两栏，中间的分栏比例可以拖着调（双击复位），选择记在 localStorage：
 * - 左：登录侧边栏（皮肤 3D 预览 + 账号一键切换 + 启动按钮）。
 * - 右：上半是「我的版本」列表，下半是版本信息，中间也能拖。
 *
 * 日志不再内嵌在页面上 —— 后端静默收集，版本信息栏只放一个图标入口，
 * 想看输出随时点开（85% 大对话框）。启动报几个错是常态，不自动弹、不标红。
 */

// ---------- 分栏比例（localStorage 记忆） ----------

// 比例、钳制范围、读写全在 `./launch/layout.ts`：它只跟窗口有关，
// 和启动状态机没有关系，不该占着主文件。

/** 详情面板底部的文件夹入口来自 `lib/versionFolders`，和版本设置子页共用一份。 */

export default function LaunchPage() {
  const { status, logs, starting, running, start, clearLogs } = useLaunch();
  const { cancel: cancelDownload } = useDownloads();
  const toast = useToast();
  const [versions, setVersions] = useState<LocalVersion[]>([]);
  const [selected, setSelected] = useState<string | null>(null);
  /**
   * 手动点过版本没有。
   *
   * 配置里「上次选中的版本」是异步读回来的，可能晚于版本列表 —— 如果已经
   * 自己点了一个，就不能再拿旧值把他覆盖回去。
   */
  const pickedByUser = useRef(false);
  const [loading, setLoading] = useState(true);
  /** 启动失败时后端推荐的 Java 运行时；不需要补就是 null */
  const [javaFix, setJavaFix] = useState<JavaComponent | null>(null);
  /** 下完 Java 之后要自动重试的版本 */
  const pendingRetry = useRef<string | null>(null);
  /** 全部账号（后端已按创建时间排好序）；当前账号 = active 的那个 */
  const [accounts, setAccounts] = useState<AccountView[]>([]);
  const account = useMemo(() => accounts.find((a) => a.active) ?? null, [accounts]);
  /**
   * 当前在哪个页面：`null` = 启动页主界面，其余两个 = 整页子页。
   * 不再用 85% 弹窗（`Dialog`），改成标题栏下铺满的页面，返回键交给标题栏。
   */
  const [subPage, setSubPage] = useState<"accounts" | "settings" | null>(null);
  const [logOpen, setLogOpen] = useState(false);
  /** 删除版本的二次确认 */
  const [deleteOpen, setDeleteOpen] = useState(false);
  const [deleting, setDeleting] = useState(false);
  /** 改名 / 改说明的草稿 */
  const [renameOpen, setRenameOpen] = useState(false);
  const [renameDraft, setRenameDraft] = useState("");
  /** 详情看板上的统计（占用 / mod 数 / 上次启动），版本设置子页里另拉一份 */
  const [stats, setStats] = useState<VersionStats | null>(null);
  const [savingMeta, setSavingMeta] = useState(false);
  /** 选图标用的隐藏 input */
  const iconInput = useRef<HTMLInputElement>(null);
  /** 已有自定义图标时，点图标弹的是「更换 / 恢复默认」 */
  const [iconMenu, setIconMenu] = useState<HTMLElement | null>(null);
  /** 分栏比例：改了就落盘，下次启动还原 */
  const [layout, setLayout] = useState<LaunchLayout>(readLayout);

  /**
   * 选版本：把这次选择记住，切回首页 / 重启都还停在这儿。
   *
   * 写配置失败不影响选择本身 —— 只是下次回来会跳回第一个，不值得弹错误。
   */
  const selectVersion = useCallback((id: string) => {
    pickedByUser.current = true;
    setSelected(id);
    void invoke("config_patch", { patch: { ui: { selectedVersion: id } } }).catch(
      () => undefined,
    );
  }, []);

  const setSidebarRatio = useCallback((sidebar: number) => {
    setLayout((prev) => {
      const next = { ...prev, sidebar };
      localStorage.setItem(LAYOUT_STORAGE_KEY, JSON.stringify(next));
      return next;
    });
  }, []);

  const setDetailRatio = useCallback((detail: number) => {
    setLayout((prev) => {
      const next = { ...prev, detail };
      localStorage.setItem(LAYOUT_STORAGE_KEY, JSON.stringify(next));
      return next;
    });
  }, []);

  const reloadAccounts = useCallback(() => {
    invoke<AccountView[]>("account_list")
      .then(setAccounts)
      .catch(() => {
        // 读不到就当没有，启动链路会报错说缺账号
      });
  }, []);

  /**
   * 版本列表的滚动位置：进整页子页时主界面会随退场卸载，回来得放回原处
   * （和 Mod 列表进详情页同一套，`useScrollRestore`）。
   */
  const {
    ref: versionsScrollRef,
    save: saveVersionsScroll,
    restore: restoreVersionsScroll,
  } = useScrollRestore("launch-versions");

  /** 稳定的 ref 回调：列表节点真正挂上（含返回后重新挂载）时恢复位置 */
  const attachVersionsScroll = useCallback(
    (node: HTMLDivElement | null) => {
      versionsScrollRef.current = node;
      if (node) restoreVersionsScroll();
    },
    [versionsScrollRef, restoreVersionsScroll],
  );

  /** 进整页子页前先记住列表位置（退场 110ms 后旧节点就卸了） */
  const openSubPage = useCallback(
    (page: "accounts" | "settings") => {
      saveVersionsScroll();
      setSubPage(page);
    },
    [saveVersionsScroll],
  );

  /**
   * 版本列表容器。
   *
   * 列表是异步扫回来的：页面入场动画播完时 `versions` 还是空的，
   * 数据到了以后行是硬生生插进来的。所以第一次拿到数据时对列表补一次
   * `playEnter`（和 Mod 搜索、下载页「转圈换内容」同一套处理）。
   * 之后装/删版本再刷新不重播，避免列表整体闪一下。
   */
  const versionListRef = useRef<HTMLUListElement>(null);
  const listEnterPlayed = useRef(false);
  useLayoutEffect(() => {
    if (listEnterPlayed.current) return;
    const root = versionListRef.current;
    if (!root || loading || versions.length === 0) return;
    listEnterPlayed.current = true;
    return playEnter(root);
  }, [loading, versions]);

  /** 重新扫一遍本机版本：装完 / 删完都要用 */
  const reloadVersions = useCallback(async () => {
    try {
      const list = await invoke<LocalVersion[]>("installed_versions");
      setVersions(list);
      return list;
    } catch {
      return [];
    }
  }, []);

  // 启动失败时问一下后端：这个版本是不是缺 Java，缺的话该下哪个
  useEffect(() => {
    if (status?.phase !== "failed") {
      setJavaFix(null);
      return;
    }
    let disposed = false;
    invoke<JavaComponent | null>("java_suggest", { versionId: status.versionId })
      .then((suggest) => {
        if (!disposed) setJavaFix(suggest);
      })
      .catch(() => {
        // 推荐失败就只是不显示补装入口，不影响其他
      });
    return () => {
      disposed = true;
    };
  }, [status?.phase, status?.versionId]);

  // 启动失败不再自动弹日志：报点错是常态，想看自己点「查看日志」

  const javaTask = useTaskForVersion(javaFix ? `java:${javaFix.name}` : null);

  const downloadJava = async () => {
    if (!javaFix || !status) return;
    // 记住要重试的版本 —— status 马上会被新的启动覆盖
    pendingRetry.current = status.versionId;
    try {
      await invoke("java_install", { component: javaFix.name });
    } catch (e: unknown) {
      pendingRetry.current = null;
      // 错误详情由任务表里的失败状态展示，这里只兜住异常
      console.error(e);
    }
  };

  // Java 装好之后自动接着启动 —— 这就是「自举」：只点了一次启动
  useEffect(() => {
    if (javaTask?.phase !== "done") return;
    const target = pendingRetry.current;
    if (!target) return;
    pendingRetry.current = null;
    void start(target).catch(() => {});
  }, [javaTask?.phase, start]);

  // 读本地版本列表
  useEffect(() => {
    let disposed = false;
    invoke<LocalVersion[]>("installed_versions")
      .then((list) => {
        if (disposed) return;
        setVersions(list);
        // 默认选中第一个能启动的
        const first = list.find((item) => item.launchable) ?? list[0];
        setSelected((prev) => prev ?? first?.id ?? null);

        // 上次选中的是哪个，切回首页还停在那儿 —— 别每次都跳回第一个。
        // 那个版本可能已经被删了，对不上就保持默认。
        void invoke<AppConfig>("config_get")
          .then((config) => {
            if (disposed || pickedByUser.current) return;
            const saved = config.ui.selectedVersion;
            if (saved && list.some((item) => item.id === saved)) {
              setSelected(saved);
            }
          })
          .catch(() => undefined);
      })
      .catch(() => {
        if (!disposed) setVersions([]);
      })
      .finally(() => {
        if (!disposed) setLoading(false);
      });
    return () => {
      disposed = true;
    };
  }, []);

  // 装完新版本回到这个页面时刷新一下
  useEffect(() => {
    if (status?.phase === "exited" || status?.phase === "cancelled") {
      void reloadVersions();
    }
  }, [status?.phase, reloadVersions]);

  // 账号：进来先读一次；侧边栏切换 / 管理面板增删后再读
  useEffect(() => {
    reloadAccounts();
  }, [reloadAccounts]);

  const current = useMemo(
    () => versions.find((item) => item.id === selected) ?? null,
    [versions, selected],
  );

  /**
   * 详情卡头部按两级降级，不是一刀切：
   *
   * - 稍窄 → 「删除」先收成图标（它优先级最低，且带红框最占地方）；
   * - 再窄 → 「版本设置」也收成图标。
   *
   * 断点看窗口宽度：这个头部在右栏里，宽度由「窗口 − 左栏」决定，没法用容器
   * 查询切换组件结构。默认窗口 1180（详情区约 960）两个文字按钮放得下。
   */
  const compactDelete = useMediaQuery("(max-width: 1120px)");
  const compactSettings = useMediaQuery("(max-width: 1000px)");

  // 标题栏左侧换「← 返回 + 项目名」。注册放在这一层：点下去的瞬间标题就开始换位，
  // 不用等页面切换的 110ms；返回 = 回到启动页主界面。
  useRegisterNestedPage(
    subPage === "accounts"
      ? "账号管理"
      : subPage === "settings" && current
        ? `版本设置 · ${current.id}`
        : null,
    () => setSubPage(null),
  );

  /** 当前版本的隔离状态；换版本时重新向后端要，前端不自己算 */
  const [isolation, setIsolation] = useState<IsolationView | null>(null);

  useEffect(() => {
    const id = current?.id;
    if (!id || !current.valid) {
      setIsolation(null);
      return;
    }
    let disposed = false;
    invoke<IsolationView>("version_isolation", { versionId: id })
      .then((view) => {
        if (!disposed) setIsolation(view);
      })
      .catch(() => {
        if (!disposed) setIsolation(null);
      });
    return () => {
      disposed = true;
    };
  }, [current?.id, current?.valid]);

  /** 详情看板的统计：换版本、启动结束、离开版本设置子页都重拉，数字才不会过期 */
  useEffect(() => {
    const id = current?.id;
    if (!id || !current.valid) {
      setStats(null);
      return;
    }
    let disposed = false;
    invoke<VersionStats>("version_stats", { versionId: id })
      .then((value) => {
        if (!disposed) setStats(value);
      })
      .catch(() => {
        if (!disposed) setStats(null);
      });
    return () => {
      disposed = true;
    };
  }, [current?.id, current?.valid, status?.phase, subPage, isolation?.gameDir]);

  const openFolder = async (folder: VersionFolderKey) => {
    if (!current) return;
    try {
      await invoke("version_open_folder", { versionId: current.id, folder });
    } catch (e) {
      toast(toAppError(e, "打开文件夹失败").message, "error");
    }
  };

  /** 改名：目录、JSON、子版本一起改，成功后切到新名字 */
  const renameVersion = async () => {
    if (!current) return;
    const target = current.id;
    const next = renameDraft.trim();
    if (!next || next === target) {
      setRenameOpen(false);
      return;
    }
    setSavingMeta(true);
    try {
      await invoke<string>("version_rename", { versionId: target, newId: next });
      await reloadVersions();
      setSelected(next);
      invalidate("version_list");
      toast(`已改名为 ${next}`, "success");
      setRenameOpen(false);
    } catch (e) {
      toast(toAppError(e, "改名失败").message, "error");
    } finally {
      setSavingMeta(false);
    }
  };

  const toggleFavorite = async () => {
    if (!current) return;
    try {
      await invoke("version_set_favorite", {
        versionId: current.id,
        favorite: !current.favorite,
      });
      await reloadVersions();
    } catch (e) {
      toast(toAppError(e, "收藏失败").message, "error");
    }
  };

  /** 选一张图当图标：先在 canvas 里缩到 128×128，再交给后端存盘 */
  const pickIcon = () => iconInput.current?.click();

  const applyIcon = async (event: ChangeEvent<HTMLInputElement>) => {
    const file = event.target.files?.[0];
    // 同一个文件选两次也要能触发，先把 input 清空
    event.target.value = "";
    if (!file || !current) return;
    try {
      const dataUrl = await toIconDataUrl(file);
      await invoke("version_set_icon", { versionId: current.id, dataUrl });
      await reloadVersions();
      invalidate("version_list");
      toast("图标已更新", "success");
    } catch (e) {
      toast(toAppError(e, "设置图标失败").message, "error");
    }
  };

  const clearIcon = async () => {
    if (!current) return;
    try {
      await invoke("version_clear_icon", { versionId: current.id });
      await reloadVersions();
      invalidate("version_list");
    } catch (e) {
      toast(toAppError(e, "恢复默认图标失败").message, "error");
    }
  };

  /** 删除当前版本：真删 `versions/<id>`，删完顺手把选中项挪到下一个 */
  const removeVersion = async () => {
    if (!current) return;
    const target = current.id;
    setDeleting(true);
    try {
      await invoke("version_delete", { versionId: target });
      const list = await reloadVersions();
      setSelected((prev) =>
        prev === target ? (list.find((item) => item.launchable) ?? list[0])?.id ?? null : prev,
      );
      // 下载页里的「已安装」标记跟着失效
      invalidate("version_list");
      toast(`已删除 ${target}`, "success");
      setDeleteOpen(false);
    } catch (e) {
      toast(toAppError(e, "删除版本失败").message, "error");
    } finally {
      setDeleting(false);
    }
  };

  // 启动页主界面：两栏（登录侧栏 + 版本列表 / 版本详情）
  const main = (
    <Box sx={{ flex: 1, minHeight: 0, display: "flex" }}>
      {/* 左：登录侧边栏。宽度 = 比例，外包一层负责定位 */}
      <Box
        data-page-block="left"
        sx={{
          width: `${layout.sidebar * 100}%`,
          minWidth: 200,
          flexShrink: 0,
          display: "flex",
          minHeight: 0,
        }}
      >
        <LoginSidebar
          selected={current}
          account={account}
          accounts={accounts}
          onAccountsChanged={reloadAccounts}
          onManageAccounts={() => openSubPage("accounts")}
        />
      </Box>

      <Splitter
        orientation="vertical"
        value={layout.sidebar}
        min={SIDEBAR_MIN}
        max={SIDEBAR_MAX}
        onChange={setSidebarRatio}
        resetValue={LAYOUT_DEFAULT.sidebar}
      />

      {/* 右：上半版本列表，下半版本信息，中间也能拖。
          这里不打块标记：标记会让整列当成一块落下，里面的版本卡就不动了。 */}
      <Box
        sx={{
          flex: 1,
          minWidth: 0,
          minHeight: 0,
          display: "flex",
          flexDirection: "column",
        }}
      >
        {/* 上半：我的版本（不要标头，列表自己就是内容） */}
        <Box
          sx={{
            height: `${layout.detail * 100}%`,
            minHeight: 0,
            display: "flex",
            flexDirection: "column",
            // 不铺底色：版本卡自己就是一张卡，直接排在背景上
            overflow: "hidden",
          }}
        >
          <Box
            ref={attachVersionsScroll}
            sx={{
              flex: 1,
              minHeight: 0,
              overflow: "auto",
              scrollbarGutter: "stable",
              px: 1.5,
              py: 1.5,
            }}
          >
            {!loading && versions.length === 0 && (
              <Typography
                sx={{ fontSize: 12, color: "text.secondary", px: 0.5, lineHeight: 1.6 }}
              >
                还没有安装任何版本。去「下载」页装一个。
              </Typography>
            )}
            {/* 每条版本各自算一块：进页面时一条接一条落下来（+25ms 错峰） */}
            <List ref={versionListRef} disablePadding data-page-stagger>
              {versions.map((item) => (
                <VersionRow
                  key={item.id}
                  item={item}
                  active={selected === item.id}
                  running={running && status?.versionId === item.id}
                  onSelect={() => selectVersion(item.id)}
                />
              ))}
            </List>
          </Box>
        </Box>

        <Splitter
          orientation="horizontal"
          value={layout.detail}
          min={DETAIL_MIN}
          max={DETAIL_MAX}
          onChange={setDetailRatio}
          resetValue={LAYOUT_DEFAULT.detail}
        />

        {/* 下半：版本信息（整块一个容器） */}
        <Box
          sx={{
            flex: 1,
            minWidth: 0,
            minHeight: 0,
            display: "flex",
            flexDirection: "column",
            p: 2,
            overflow: "hidden",
          }}
        >
          {!current ? (
            <Paper
              variant="outlined"
              sx={{ flex: 1, minHeight: 0, display: "grid", placeItems: "center" }}
            >
              {loading ? (
                <Loading />
              ) : (
                <Box sx={{ textAlign: "center" }}>
                  <Box sx={{ fontSize: 14, fontWeight: 600, mb: 0.5 }}>选一个版本</Box>
                  <Box sx={{ fontSize: 12.5, color: "text.secondary" }}>
                    上面列出的是本机已经装好的版本
                  </Box>
                </Box>
              )}
            </Paper>
          ) : (
            <Paper
              variant="outlined"
              sx={{
                flex: 1,
                minHeight: 0,
                display: "flex",
                flexDirection: "column",
                overflow: "hidden",
              }}
            >
              {/* 标题栏：身份 + 标签，日志只留图标 */}
              <Stack
                direction="row"
                spacing={1.25}
                sx={{
                  alignItems: "center",
                  px: 2,
                  py: 1.25,
                  flexShrink: 0,
                  borderBottom: "1px solid",
                  borderColor: "divider",
                  // 一行到底，不换行：空间不够就把版本名压成省略号，
                  // 按钮不许掉到第二行（掉下去之后整张卡的高度会跳）。
                  flexWrap: "nowrap",
                  minWidth: 0,
                }}
              >
                <Tooltip title={current.icon ? "更换图标" : "设置图标"}>
                  <ButtonBase
                    onClick={(e) => {
                      // 已经有自定义图标时先给个菜单：换一张 / 恢复默认
                      if (current.icon) setIconMenu(e.currentTarget);
                      else pickIcon();
                    }}
                    sx={{ borderRadius: 1, p: 0.25, "&:hover": { bgcolor: "action.hover" } }}
                  >
                    <VersionIcon kind={current.kind} size={24} icon={current.icon} />
                  </ButtonBase>
                </Tooltip>
                <ButtonBase
                  onClick={() => {
                    setRenameDraft(current.id);
                    setRenameOpen(true);
                  }}
                  sx={{ borderRadius: 1, px: 0.5, minWidth: 0, flexShrink: 1, overflow: "hidden" }}
                >
                  <Typography
                    noWrap
                    sx={{
                      fontSize: 15,
                      fontWeight: 700,
                      minWidth: 0,
                      overflow: "hidden",
                      textOverflow: "ellipsis",
                      "&:hover": { textDecoration: "underline" },
                    }}
                  >
                    {current.id}
                  </Typography>
                </ButtonBase>
                <Tooltip title="改版本名">
                  <IconButton
                    size="small"
                    aria-label="改版本名"
                    onClick={() => {
                      setRenameDraft(current.id);
                      setRenameOpen(true);
                    }}
                  >
                    <EditOutlinedIcon sx={{ fontSize: 16 }} />
                  </IconButton>
                </Tooltip>
                <VersionTypeChip type={current.versionType} />
                {current.javaMajor != null && (
                  <Chip
                    size="small"
                    variant="outlined"
                    label={`Java ${current.javaMajor}`}
                    sx={{ height: 18, fontSize: 9.5, flexShrink: 0 }}
                  />
                )}
                {/* 弹性空白不参与压缩：空间不够时只压版本名，别把间隙也吃掉 */}
                <Box sx={{ flex: 1, minWidth: 8, flexShrink: 0 }} />
                <Tooltip title={current.favorite ? "取消收藏" : "收藏，列表里排最前"}>
                  <IconButton
                    size="small"
                    aria-label={current.favorite ? "取消收藏" : "收藏"}
                    onClick={() => void toggleFavorite()}
                  >
                    <StarRoundedIcon
                      sx={{
                        fontSize: 18,
                        color: current.favorite ? "primary.dark" : "text.disabled",
                      }}
                    />
                  </IconButton>
                </Tooltip>
                {compactSettings ? (
                  <Tooltip title="版本设置">
                    <IconButton
                      size="small"
                      aria-label="版本设置"
                      onClick={() => openSubPage("settings")}
                    >
                      <TuneOutlinedIcon sx={{ fontSize: 18 }} />
                    </IconButton>
                  </Tooltip>
                ) : (
                  <Button
                    size="small"
                    variant="outlined"
                    startIcon={<TuneOutlinedIcon sx={{ fontSize: 16 }} />}
                    onClick={() => openSubPage("settings")}
                    // 文字绝不折行：之前窗口一窄，「版本设置」被压成
                    // 「版本 / 设置」两行，按钮高度翻倍、整行都歪了
                    sx={{ flexShrink: 0, whiteSpace: "nowrap" }}
                  >
                    版本设置
                  </Button>
                )}
                {compactDelete ? (
                  // 包一层 span：按钮 disabled 时 pointer-events 被关掉，
                  // Tooltip 收不到事件就永远不出现
                  <Tooltip title="删除版本">
                    <span>
                      <IconButton
                        size="small"
                        color="error"
                        aria-label="删除版本"
                        disabled={deleting || (running && status?.versionId === current.id)}
                        onClick={() => setDeleteOpen(true)}
                      >
                        <DeleteOutlinedIcon sx={{ fontSize: 18 }} />
                      </IconButton>
                    </span>
                  </Tooltip>
                ) : (
                  <Button
                    size="small"
                    color="error"
                    variant="outlined"
                    startIcon={<DeleteOutlinedIcon sx={{ fontSize: 16 }} />}
                    disabled={deleting || (running && status?.versionId === current.id)}
                    onClick={() => setDeleteOpen(true)}
                    sx={{ flexShrink: 0, whiteSpace: "nowrap" }}
                  >
                    删除
                  </Button>
                )}
                <Tooltip title="查看日志">
                  <IconButton size="small" aria-label="查看日志" onClick={() => setLogOpen(true)}>
                    <ArticleOutlinedIcon sx={{ fontSize: 18 }} />
                  </IconButton>
                </Tooltip>
              </Stack>

              {/* 中间：只读看板 —— 想改什么去「版本设置」，这里不摆编辑入口 */}
              <Box
                sx={{
                  flex: 1,
                  minHeight: 0,
                  overflow: "auto",
                  scrollbarGutter: "stable",
                  px: 2,
                  py: 1.5,
                }}
              >
                <Stack spacing={1}>
                  {/* 启动状态压在最上面：有状态时从 0 高度展开，信息整体往下推 */}
                  <LaunchStatusBlock
                    status={
                      status && (starting === current.id || status.versionId === current.id)
                        ? status
                        : null
                    }
                    starting={starting === current.id}
                  />

                  {current.description && (
                    <Typography
                      sx={{
                        fontSize: 12.5,
                        color: "text.secondary",
                        lineHeight: 1.7,
                        whiteSpace: "pre-wrap",
                      }}
                    >
                      {current.description}
                    </Typography>
                  )}

                  {/* 环境事实：扫出来的东西都摆在这儿，只读 —— 改不了，但一眼能看全 */}
                  <Box sx={{ pt: 0.5 }}>
                    <DetailRow label="版本名" value={current.id} />
                    <DetailRow
                      label="版本类型"
                      value={
                        current.inheritsFrom
                          ? `${versionTypeLabel(current.versionType)} · 继承自 ${current.inheritsFrom}`
                          : versionTypeLabel(current.versionType)
                      }
                    />
                    <DetailRow
                      label="Java 要求"
                      value={current.javaMajor != null ? `Java ${current.javaMajor}` : "按游戏 JSON"}
                    />
                    <DetailRow label="版本目录" value={`versions/${current.id}`} />
                    <DetailRow
                      label="游戏目录"
                      value={
                        isolation
                          ? isolation.enabled
                            ? isolation.gameDirLabel
                            : `${isolation.gameDirLabel}（共享）`
                          : "读取中"
                      }
                    />
                    {/* 统计是异步的：没回来就先不显示这两行，不写「统计中」占位 */}
                    {stats && (
                      <>
                        <DetailRow label="占用空间" value={formatBytes(stats.sizeBytes)} />
                        <DetailRow
                          label="上次启动"
                          value={formatRelativeTime(stats.lastPlayedMs)}
                        />
                      </>
                    )}
                  </Box>

                  {!current.launchable && (
                    <ErrorState
                      variant="inline"
                      severity="warning"
                      title="这个版本还不能启动"
                      message={
                        current.error ??
                        "缺少必要的文件，先去下载页对它点「校验并修复」"
                      }
                    />
                  )}

                  {/* 缺 Java：给一个「下载」入口，下完自动接着启动 */}
                  {javaFix && status?.versionId === current.id && (
                    <ErrorState
                      variant="banner"
                      severity="warning"
                      title={`这个版本需要 ${javaFix.label}`}
                      message="启动时会自动下载，这里也可以手动重试。"
                      action={
                        javaTask?.phase === "running" ? (
                          <Button
                            size="small"
                            onClick={() => cancelDownload(javaTask.taskId)}
                          >
                            取消
                          </Button>
                        ) : (
                          <Button
                            size="small"
                            variant="contained"
                            onClick={() => void downloadJava()}
                          >
                            {javaTask?.phase === "failed" ? "重试下载" : "下载"}
                          </Button>
                        )
                      }
                    >
                      {javaTask && (
                        <Box sx={{ mt: 1 }}>
                          <TaskProgress task={javaTask} />
                        </Box>
                      )}
                    </ErrorState>
                  )}

                  {/* 隔离状态在上一行已经写了；要改去「版本设置 → 隔离与目录」 */}
                </Stack>
              </Box>

              {/* 底部：每个文件夹一个入口 */}
              <Box
                sx={{
                  flexShrink: 0,
                  borderTop: "1px solid",
                  borderColor: "divider",
                  px: 1.5,
                  py: 1,
                  display: "grid",
                  gap: 0.75,
                  // 永远一排：按钮数量是变的（隔离关掉会多一个「游戏目录」），
                  // 写死列数会让第 5 个掉到第二排。用 auto-flow 让它自己挤。
                  gridAutoFlow: "column",
                  gridAutoColumns: "minmax(0, 1fr)",
                }}
              >
                {foldersFor(current, isolation?.enabled === true).map(({ key, label, icon: Icon }) => (
                  <Button
                    key={key}
                    size="small"
                    variant="outlined"
                    startIcon={<Icon sx={{ fontSize: 16 }} />}
                    disabled={!current.valid && key !== "version"}
                    onClick={() => void openFolder(key)}
                    sx={{
                      fontSize: 13,
                      fontWeight: 500,
                      minHeight: 38,
                      minWidth: 0,
                      py: 0.75,
                      px: 0.5,
                      justifyContent: "center",
                      // 一排挤下五个时标签不许换行，宁可让容器裁掉
                      whiteSpace: "nowrap",
                      "& .MuiButton-startIcon": { mr: 0.5 },
                    }}
                  >
                    {label}
                  </Button>
                ))}
              </Box>
            </Paper>
          )}
        </Box>
      </Box>
    </Box>
  );

  return (
    <>
      {/* 整页子页：主界面 / 账号管理 / 版本设置。
          切换走内层 PageTransition（initial={false}，避免和外层页面动画叠加），
          标题栏的「← 返回 + 标题」由 useRegisterNestedPage 接管。 */}
      <PageTransition
        pageKey={subPage ?? "main"}
        initial={false}
        render={(key) => {
          if (key === "accounts") {
            return <AccountPage onChanged={() => reloadAccounts()} />;
          }
          if (key === "settings" && current) {
            return (
              <VersionSettingsPage
                version={current}
                isolation={isolation}
                onChanged={() => void reloadVersions()}
                onIsolationChanged={setIsolation}
              />
            );
          }
          return main;
        }}
      />

      <LogDialog
        open={logOpen}
        onClose={() => setLogOpen(false)}
        logs={logs}
        onClear={clearLogs}
      />

      <Dialog open={deleteOpen} onClose={() => setDeleteOpen(false)} maxWidth="xs" fullWidth>
        <DialogTitle sx={{ fontSize: 16, fontWeight: 700 }}>
          删除 {current?.id}？
        </DialogTitle>
        <DialogContent>
          <DialogContentText sx={{ fontSize: 13 }}>
            会把这个版本整个目录删掉——jar、natives、mods、
            <strong>存档</strong>和版本设置都在里面。共享的库和资源不动。
          </DialogContentText>
          <DialogContentText sx={{ fontSize: 12.5, color: "error.main", mt: 1 }}>
            删掉就是真没了，不进回收站。
          </DialogContentText>
        </DialogContent>
        <DialogActions sx={{ px: 3, pb: 2 }}>
          <Button onClick={() => setDeleteOpen(false)} disabled={deleting}>
            取消
          </Button>
          <Button
            variant="contained"
            color="error"
            disabled={deleting}
            onClick={() => void removeVersion()}
          >
            {deleting ? "正在删除…" : "删除"}
          </Button>
        </DialogActions>
      </Dialog>

      {/* 改名：会同时改目录名、JSON 里的 id、子版本的 inheritsFrom */}
      <Dialog open={renameOpen} onClose={() => setRenameOpen(false)} maxWidth="xs" fullWidth>
        <DialogTitle sx={{ fontSize: 16, fontWeight: 700 }}>改版本名</DialogTitle>
        <DialogContent>
          <TextField
            autoFocus
            fullWidth
            size="small"
            label="版本名"
            value={renameDraft}
            disabled={savingMeta}
            onChange={(e) => setRenameDraft(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter") void renameVersion();
            }}
            sx={{ mt: 1 }}
          />
        </DialogContent>
        <DialogActions sx={{ px: 3, pb: 2 }}>
          <Button onClick={() => setRenameOpen(false)} disabled={savingMeta}>
            取消
          </Button>
          <Button
            variant="contained"
            disabled={savingMeta || !renameDraft.trim() || renameDraft.trim() === current?.id}
            onClick={() => void renameVersion()}
          >
            {savingMeta ? "正在保存…" : "保存"}
          </Button>
        </DialogActions>
      </Dialog>

      {/* 选图标的隐藏 input：真正缩放交给 canvas */}
      <input
        ref={iconInput}
        type="file"
        accept="image/png,image/jpeg,image/webp"
        hidden
        onChange={(e) => void applyIcon(e)}
      />

      <Menu
        anchorEl={iconMenu}
        open={Boolean(iconMenu)}
        onClose={() => setIconMenu(null)}
      >
        <MenuItem
          onClick={() => {
            setIconMenu(null);
            pickIcon();
          }}
        >
          换一张
        </MenuItem>
        <MenuItem
          onClick={() => {
            setIconMenu(null);
            void clearIcon();
          }}
        >
          恢复默认图标
        </MenuItem>
      </Menu>
    </>
  );
}

/** 选中的图片缩到 128×128、等比居中，转成数据 URL 交给后端存盘。 */
async function toIconDataUrl(file: File): Promise<string> {
  const bitmap = await createImageBitmap(file);
  const size = 128;
  const canvas = document.createElement("canvas");
  canvas.width = size;
  canvas.height = size;
  const ctx = canvas.getContext("2d");
  if (!ctx) throw new Error("拿不到画布");
  const scale = Math.min(size / bitmap.width, size / bitmap.height);
  const w = Math.max(1, Math.round(bitmap.width * scale));
  const h = Math.max(1, Math.round(bitmap.height * scale));
  ctx.clearRect(0, 0, size, size);
  ctx.drawImage(bitmap, (size - w) / 2, (size - h) / 2, w, h);
  bitmap.close();
  return canvas.toDataURL("image/png");
}

// 版本列表行 `VersionRow`、启动状态块 `LaunchStatusBlock`、信息行 `DetailRow`
// 都在 `./launch/` 下 —— 本文件只留页面状态机和两栏布局。


