import { useCallback, useEffect, useLayoutEffect, useRef, useState } from "react";
import {
  Box,
  ButtonBase,
  MenuItem,
  Select,
  Stack,
  Tab,
  Tabs,
  TextField,
  Tooltip,
  Typography,
  alpha,
} from "@mui/material";
import StarBorderRounded from "@mui/icons-material/StarBorderRounded";
import StarRounded from "@mui/icons-material/StarRounded";
import { invoke } from "@tauri-apps/api/core";
import { apiGet } from "../lib/api";
import { formatCount, formatDate } from "../lib/format";
import {
  toAppError,
  type AppError,
  type FavoriteEntry,
  type ModItem,
  type ModSearchView,
  type ModSort,
  type ModSource,
  type ResourceKind,
} from "../lib/types";
import { useRegisterNestedPage } from "../lib/nestedNav";
import { useScrollRestore } from "../lib/useScrollRestore";
import { platformLoaderLabels } from "../lib/loaders";
import { playEnter, playEnterElements } from "../motion/pageMotion";
import PageTransition from "../motion/PageTransition";
import Loading from "./Loading";
import ErrorState from "./ErrorState";
import ResourceDetailPage from "./ResourceDetailPage";
import ModpackDetailPage from "./ModpackDetailPage";
import { useToast } from "./Toast";

/**
 * 下载页 →「Mod」/「资源包」的共用列表页。
 *
 * 两种类型在平台侧只有 `project_type` / `classId` 不同，所以这里是同一份代码：
 * 同一个组件、同一套参数、同一套布局、同一套动画 —— 分别由
 * `ModPage` 与 `ResourcePackPage` 传 `kind` 进来，物理上不可能长得不一样。
 *
 * 结构：顶部一排固定（源切换 / 搜索 / 排序，只 `flexShrink:0`，不打任何动画标记）
 * + 下面一列卡片。搜索完成后整列逐条「掉下来」——每一条各自成块，
 * 由 `playEnter` 统一驱动，参数全在 `motion/tokens.ts`。
 *
 * 两个源互斥：一次只查一个，所以分页只有一份 offset，也没有跨源排序。
 */

const SOURCES: { key: ModSource; label: string }[] = [
  { key: "modrinth", label: "Modrinth" },
  { key: "curseforge", label: "CurseForge" },
];

const SORTS: { key: ModSort; label: string }[] = [
  { key: "relevance", label: "相关度" },
  { key: "downloads", label: "下载量" },
  { key: "updated", label: "最近更新" },
];

/** 加载器（平台）筛选：固定四项，和 Modrinth / CF 的取值一一对应 */
const PLATFORMS: { key: string; label: string }[] = [
  { key: "", label: "全部平台" },
  { key: "fabric", label: "Fabric" },
  { key: "forge", label: "Forge" },
  { key: "neoforge", label: "NeoForge" },
  { key: "quilt", label: "Quilt" },
];

/** 顶部那一排所有控件的统一高度：源胶囊 / 搜索框 / 下拉必须一样高，否则会多出一截 */
const CONTROL_H = 40;

/**
 * 三个下拉共用：只锁高度和字号，内部排版全交给 MUI。
 *
 * 不要再动 `.MuiSelect-select`（比如设成 flex + 固定高度）：MUI 的选中文字是
 * 文本节点，flex 化之后不再按 MUI 的省略号规则排版，文字会直接画到框外。
 */
const SELECT_SX = {
  height: CONTROL_H,
  flexShrink: 0,
  fontSize: 13,
} as const;

/** 输入停下多久才去搜（回车立刻搜） */
const DEBOUNCE_MS = 400;

/**
 * 收藏的唯一键，和 `favorites.toml` 里的三元组一致。
 *
 * 收藏页也用这一个，两边的键必须一样 —— 各拼各的字符串迟早会错开。
 */
export function favKey(entry: { source: string; kind: string; id: string }): string {
  return `${entry.source}:${entry.kind}:${entry.id}`;
}

/** 列表页支持的类型：三种都是同一套界面，只是查的东西不同 */
type Kind = ResourceKind;

/** 每个类型自己的文案。除了这几处，三种类型的界面完全一致。 */
const TEXT = {
  mod: { search: "搜索 Mod", empty: "没有找到匹配的 Mod" },
  resourcepack: { search: "搜索资源包", empty: "没有找到匹配的资源包" },
  shader: { search: "搜索光影", empty: "没有找到匹配的光影" },
  modpack: { search: "搜索整合包", empty: "没有找到匹配的整合包" },
} as const;

export default function ResourceListPage(props: { kind: Kind }) {
  const { kind } = props;
  const text = TEXT[kind];
  const toast = useToast();
  // 加载器筛选只对 Mod 有意义：资源包没有「跑在哪个加载器上」这回事
  const showPlatform = kind === "mod";

  const [source, setSource] = useState<ModSource>("modrinth");
  const [sort, setSort] = useState<ModSort>("downloads");
  /** 平台（加载器）筛选，空串 = 不限 */
  const [platform, setPlatform] = useState("");
  /** MC 大版本筛选（`1.20` 这条线），空串 = 不限 */
  const [gameVersion, setGameVersion] = useState("");
  /** 可选的版本线，来自后端（从版本清单算，不写死） */
  const [versionLines, setVersionLines] = useState<string[]>([]);
  /** 输入框里的字 */
  const [input, setInput] = useState("");
  /** 真正提交给后端的搜索词 */
  const [query, setQuery] = useState("");

  const [items, setItems] = useState<ModItem[]>([]);
  const [total, setTotal] = useState(0);
  const [hasMore, setHasMore] = useState(false);
  const [loading, setLoading] = useState(true);
  const [loadingMore, setLoadingMore] = useState(false);
  const [error, setError] = useState<AppError | null>(null);
  /** 翻页失败：挂在列表末尾的一条，不把已经看到的整列换成错误页 */
  const [moreError, setMoreError] = useState<AppError | null>(null);

  const listRef = useRef<HTMLDivElement>(null);
  // 滚动记忆按类型分开存，否则 Mod 列表的位置会被资源包列表拿去恢复
  const { ref: scrollRef, save: saveScroll, restore: restoreScroll } =
    useScrollRestore(`${kind}-list`);
  /** 点开的是哪个项目；`detailOpen` 控制内层页面切换 */
  const [detailItem, setDetailItem] = useState<ModItem | null>(null);
  const [detailOpen, setDetailOpen] = useState(false);
  /** 只认最后一次请求，避免快速切源时旧响应盖掉新结果 */
  const seq = useRef(0);
  /** 每次「一轮新搜索」加一，用来触发一次入场动画 */
  const [fadeToken, setFadeToken] = useState(0);
  /** 滚动事件一帧内会连发好几次，用 ref 挡住重复请求（state 是异步的） */
  const pulling = useRef(false);

  // 输入防抖：停下 400ms 才发请求
  useEffect(() => {
    const timer = window.setTimeout(() => {
      setQuery((prev) => (prev === input.trim() ? prev : input.trim()));
    }, DEBOUNCE_MS);
    return () => window.clearTimeout(timer);
  }, [input]);

  // 版本筛选的选项：后端按版本清单算出的大版本线（新的在前）
  useEffect(() => {
    let alive = true;
    void invoke<string[]>("mod_version_lines")
      .then((lines) => {
        if (alive) setVersionLines(lines);
      })
      .catch(() => {
        // 拿不到就只留「全部版本」，不影响搜索本身
      });
    return () => {
      alive = false;
    };
  }, []);

  const load = useCallback(
    async (offset: number) => {
      const mine = ++seq.current;
      if (offset === 0) {
        setLoading(true);
        setError(null);
        setMoreError(null);
      } else {
        pulling.current = true;
        setLoadingMore(true);
      }
      try {
        const view = await apiGet<ModSearchView>("mod_search", {
          source,
          query,
          offset,
          sort,
          gameVersion: gameVersion || null,
          loader: platform || null,
          kind,
        });
        if (seq.current !== mine) return;
        setItems((prev) => (offset === 0 ? view.items : [...prev, ...view.items]));
        setTotal(view.total);
        setHasMore(view.hasMore);
        if (offset === 0) setFadeToken((n) => n + 1);
      } catch (e: unknown) {
        if (seq.current !== mine) return;
        if (offset === 0) {
          setError(toAppError(e, "搜索失败"));
        } else {
          // 翻页失败：停在原地，把原因挂在列表末尾，别把已经看到的一整列换掉
          setMoreError(toAppError(e, "加载下一页失败"));
          setHasMore(false);
        }
      } finally {
        if (seq.current === mine) {
          setLoading(false);
          setLoadingMore(false);
        }
        pulling.current = false;
      }
    },
    [source, query, sort, gameVersion, platform, kind],
  );

  // 换源 / 换排序 / 换搜索词 → 从第一页重来
  useEffect(() => {
    void load(0);
  }, [load]);

  /**
   * 滑到底部就自动接下一页。
   *
   * 留 160px 提前量：等真到底才发请求，会先看到一段空白再突然补上。
   */
  const handleScroll = (el: HTMLDivElement) => {
    if (loading || error || !hasMore || pulling.current) return;
    const rest = el.scrollHeight - el.scrollTop - el.clientHeight;
    if (rest < 160) void load(items.length);
  };

  /**
   * 一页装不满一屏时（大窗口 / 结果很矮），没有滚动就没有 scroll 事件，
   * 会永远卡在第一页 —— 这里补一次「填满视口」的自动加载。
   */
  useEffect(() => {
    const el = scrollRef.current;
    if (!el || loading || !hasMore || pulling.current) return;
    if (el.scrollHeight <= el.clientHeight + 8) void load(items.length);
  }, [items, hasMore, loading, load, scrollRef]);

  /**
   * 入场动画分两种：
   * - 一轮新搜索（`fadeToken` 变了）→ 整列逐条落下；
   * - 追加下一页 → 只给新进来的那几条播，已渲染的不重播（不然整列会重跳一遍）。
   */
  const renderedRef = useRef(0);
  const lastFadeRef = useRef(0);
  useLayoutEffect(() => {
    const root = listRef.current;
    if (!root) {
      renderedRef.current = 0;
      return;
    }
    const children = Array.from(root.children).filter(
      (child): child is HTMLElement => child instanceof HTMLElement,
    );
    if (children.length === 0) {
      renderedRef.current = 0;
      return;
    }
    if (fadeToken !== lastFadeRef.current) {
      lastFadeRef.current = fadeToken;
      renderedRef.current = children.length;
      return playEnter(root);
    }
    if (children.length > renderedRef.current) {
      const fresh = children.slice(renderedRef.current);
      renderedRef.current = children.length;
      return playEnterElements(fresh);
    }
    renderedRef.current = children.length;
  }, [items, fadeToken]);

  /** 进详情页：先记滚动位置（列表马上随退场卸载），再切页 */
  const openDetail = useCallback(
    (item: ModItem) => {
      saveScroll();
      setDetailItem(item);
      setDetailOpen(true);
    },
    [saveScroll],
  );

  /**
   * 本地收藏集合（键是 `source:kind:id`）。
   *
   * 收藏状态是纯展示判断：这个 id 在不在我的收藏表里。所以前端读一次
   * `favorites_list`（本地查表，几毫秒）自己判，不让后端往搜索结果里塞标记 ——
   * 那样每收藏一次都得失效 `mod_search` 的缓存，为了颗星星浪费一次网络请求。
   *
   * 进页面时读一次就够：这个页面的收藏状态只有本页会改，改的时候同步更新集合。
   */
  const [faved, setFaved] = useState<Set<string>>(() => new Set());

  useEffect(() => {
    let alive = true;
    void apiGet<FavoriteEntry[]>("favorites_list", {}, { ttlMs: 0 })
      .then((list) => {
        if (alive) setFaved(new Set(list.map((entry) => favKey(entry))));
      })
      .catch(() => {
        // 读不到就当全没收藏，不影响搜索本身
      });
    return () => {
      alive = false;
    };
  }, []);

  /**
   * 收藏 / 取消收藏。
   *
   * 乐观更新：点下去星标要立刻响应，等一个 IPC 往返再亮会很钝。
   * 失败就回滚并给一条 toast —— 收藏丢了是能看出来的，不能静默。
   *
   * 快照只在收藏时上传：取关不需要画卡片，没必要传。
   */
  const toggleFavorite = useCallback(
    async (item: ModItem) => {
      const key = favKey({ source: item.source, kind, id: item.id });
      const on = !faved.has(key);
      const paint = (value: boolean) =>
        setFaved((prev) => {
          const next = new Set(prev);
          if (value) next.add(key);
          else next.delete(key);
          return next;
        });

      paint(on);
      try {
        await invoke<boolean>("favorite_set", {
          source: item.source,
          kind,
          id: item.id,
          info: on ? item : null,
          on,
        });
      } catch (error: unknown) {
        paint(!on);
        toast(toAppError(error, "收藏失败").message, "error");
      }
    },
    [faved, kind, toast],
  );

  const closeDetail = useCallback(() => setDetailOpen(false), []);

  // 标题栏左侧变「← 返回 + 项目名」。注册放在这一层而不是详情页里：
  // 点下去的瞬间标题就开始交叉换位，不用等页面切换的 110ms。
  useRegisterNestedPage(
    detailOpen && detailItem ? detailItem.title : null,
    closeDetail,
  );

  /**
   * 列表重新挂载后把滚动位置放回去。
   *
   * 用 ref 回调而不是 `useLayoutEffect`：列表要等退场播完（110ms）才挂回来，
   * effect 的时机对不上；回调是"节点真正出现"的那一刻，高度已经算好了。
   */
  const attachScroll = useCallback(
    (node: HTMLDivElement | null) => {
      scrollRef.current = node;
      if (node) restoreScroll();
    },
    [scrollRef, restoreScroll],
  );

  const list = (
    <Box sx={{ flex: 1, minHeight: 0, display: "flex", flexDirection: "column" }}>
      {/* 顶部一排：固定，不参与任何动画 */}
      <Stack
        direction="row"
        spacing={1}
        sx={{ flexShrink: 0, alignItems: "center", mb: 1.5, minWidth: 0 }}
      >
        {/* 源切换：胶囊 Tab。外层一个大容器，里面的高亮块跟着选中项左右滑 */}
        <Box
          sx={{
            flexShrink: 0,
            height: CONTROL_H,
            p: "3px",
            borderRadius: 999,
            // 胶囊条也是「面」：跟着界面不透明度一起淡，开了模糊一起毛玻璃
            bgcolor: (t) =>
              alpha(t.palette.text.primary, 0.06 * t.palette.surface.opacity),
            backdropFilter: (t) => t.palette.surface.filter,
            display: "flex",
            alignItems: "center",
          }}
        >
          <Tabs
            value={source}
            onChange={(_event, next: ModSource) => setSource(next)}
            variant="standard"
            sx={{
              minHeight: CONTROL_H - 6,
              "& .MuiTabs-flexContainer": { gap: 0 },
              "& .MuiTabs-indicator": {
                height: CONTROL_H - 6,
                top: 0,
                borderRadius: 999,
                bgcolor: "primary.main",
                // 淡黄和背景太近，靠一圈深金细边分出来
                boxShadow: (t) =>
                  `inset 0 0 0 1px ${alpha(t.palette.brand.ink, 0.22)}`,
              },
              "& .MuiTab-root": {
                minHeight: CONTROL_H - 6,
                height: CONTROL_H - 6,
                minWidth: 0,
                px: 2,
                zIndex: 1,
                borderRadius: 999,
                fontSize: 13,
                fontWeight: 500,
                textTransform: "none",
                whiteSpace: "nowrap",
                color: "text.secondary",
                transition: "color 200ms ease",
                "&.Mui-selected": { color: "primary.contrastText" },
              },
              "& .MuiTab-root:not(.Mui-selected):hover": {
                color: "text.primary",
                bgcolor: (t) => alpha(t.palette.text.primary, 0.06),
              },
            }}
          >
            {SOURCES.map((item) => (
              <Tab key={item.key} value={item.key} label={item.label} disableRipple />
            ))}
          </Tabs>
        </Box>

        {/* 浮标输入框：MUI 原生 TextField，平时占位、聚焦后标签浮到边框上 */}
        <TextField
          size="small"
          label={text.search}
          value={input}
          onChange={(event) => setInput(event.target.value)}
          onKeyDown={(event) => {
            // 回车立刻搜，不等防抖
            if (event.key === "Enter") setQuery(input.trim());
          }}
          sx={{
            flex: 1,
            minWidth: 0,
            "& .MuiInputBase-root": { height: CONTROL_H },
            // 只改字号，内边距/标签位置全用 MUI 默认：
            // 三个下拉（同为 13px）走的就是这套默认值，输入框跟着走才会和它们对齐。
            // 之前我把正文往下补 1px、标签补 2px，结果输入框比旁边高了一截，反而更歪。
            "& .MuiInputBase-input": { fontSize: 13 },
            "& .MuiInputLabel-root": { fontSize: 13 },
          }}
        />

        {/* 加载器筛选：只对 Mod 有意义，资源包不加这个控件 */}
        {showPlatform && (
          <Select
            size="small"
            displayEmpty
            value={platform}
            onChange={(event) => setPlatform(event.target.value)}
            sx={{ ...SELECT_SX, width: 96 }}
            slotProps={{ input: { "aria-label": "平台" } }}
          >
            {PLATFORMS.map((item) => (
              <MenuItem key={item.key || "all"} value={item.key} sx={{ fontSize: 13 }}>
                {item.label}
              </MenuItem>
            ))}
          </Select>
        )}

        <Select
          size="small"
          displayEmpty
          value={gameVersion}
          onChange={(event) => setGameVersion(event.target.value)}
          sx={{ ...SELECT_SX, width: 100 }}
          slotProps={{ input: { "aria-label": "游戏版本" } }}
        >
          <MenuItem value="" sx={{ fontSize: 13 }}>
            全部版本
          </MenuItem>
          {versionLines.map((version) => (
            <MenuItem key={version} value={version} sx={{ fontSize: 13 }}>
              {version}
            </MenuItem>
          ))}
        </Select>

        <Select
          size="small"
          value={sort}
          onChange={(event) => setSort(event.target.value as ModSort)}
          sx={{ ...SELECT_SX, width: 84 }}
          slotProps={{ input: { "aria-label": "排序" } }}
        >
          {SORTS.map((item) => (
            <MenuItem key={item.key} value={item.key} sx={{ fontSize: 13 }}>
              {item.label}
            </MenuItem>
          ))}
        </Select>
      </Stack>

      {/* 列表区：滚动容器本身不标记，里面的卡片才成块 */}
      <Box
        ref={attachScroll}
        onScroll={(event) => handleScroll(event.currentTarget)}
        sx={{ flex: 1, minHeight: 0, overflow: "auto", scrollbarGutter: "stable", pb: 0.5 }}
      >
        {loading && (
          <Box sx={{ display: "grid", placeItems: "center", py: 8 }}>
            <Loading />
          </Box>
        )}

        {!loading && error && (
          <ErrorState
            variant="page"
            error={error}
            onRetry={() => void load(0)}
            sx={{ py: 6 }}
          />
        )}

        {!loading && !error && items.length === 0 && (
          <Typography sx={{ fontSize: 11.5, color: "text.secondary", p: 1.5 }}>
            {text.empty}
          </Typography>
        )}

        {!loading && !error && items.length > 0 && (
          <>
            <Stack ref={listRef} data-page-stagger spacing={1}>
              {items.map((item) => (
                <ResourceCard
                  key={`${item.source}:${item.id}`}
                  item={item}
                  favorited={faved.has(
                    favKey({ source: item.source, kind, id: item.id }),
                  )}
                  onOpen={() => openDetail(item)}
                  onToggleFavorite={() => void toggleFavorite(item)}
                />
              ))}
            </Stack>

            {/* 下一页：和全软件一样只用一个转圈，不画进度线 */}
            {loadingMore && (
              <Box sx={{ display: "grid", placeItems: "center", py: 3 }}>
                <Loading />
              </Box>
            )}

            {/* 翻页失败：列表末尾挂一条，已经看到的内容不动 */}
            {moreError && (
              <ErrorState
                variant="inline"
                error={moreError}
                onRetry={() => void load(items.length)}
                sx={{ mt: 1 }}
              />
            )}

            {!hasMore && !moreError && (
              <Typography
                sx={{ fontSize: 11.5, color: "text.secondary", textAlign: "center", py: 1.5 }}
              >
                 {items.length >= total
                  ? `已显示全部 ${formatCount(total)} 个结果`
                  : `已显示 ${formatCount(items.length)} 个结果（共 ${formatCount(total)} 个）`}
              </Typography>
            )}
          </>
        )}
      </Box>
    </Box>
  );

  return (
    <PageTransition
      pageKey={detailOpen ? "detail" : "list"}
      initial={false}
      render={(key) =>
        key === "detail" && detailItem ? (
          // 整合包建的是新实例，和「塞进某个本地版本」完全两回事，单独一页
          kind === "modpack" ? (
            <ModpackDetailPage item={detailItem} />
          ) : (
            // 根详情页下载完成后停在原地（是否回列表由标题栏返回键决定）
            <ResourceDetailPage item={detailItem} kind={kind} />
          )
        ) : (
          list
        )
      }
    />
  );
}

/**
 * 一条资源。高度固定 72：两行文字 + 右侧收藏星标 + 下载量/更新时间。
 *
 * 整张卡是一个 `ButtonBase`，涟漪铺满整卡。星标做成它内部的
 * `<span role="button">` 而不是真的按钮 —— 真按钮套进 `ButtonBase` 就是
 * button 嵌 button（HTML 非法、点击行为也不可控）。用 span 的好处是：
 * 涟漪仍然属于整卡（点了卡就是整卡泛涟漪），而星标自己的点击靠
 * `stopPropagation` 挡掉，不会连带打开详情页。
 *
 * 星标还得挡 `mousedown`：`ButtonBase` 的涟漪是在按下那一刻起播的，
 * 只在 `click` 上拦，星星已经先泛一圈涟漪了。
 */
export function ResourceCard(props: {
  item: ModItem;
  /** 是否已收藏。由调用方判断（收藏页里恒为 true） */
  favorited: boolean;
  onOpen: () => void;
  onToggleFavorite: () => void;
}) {
  const { item, favorited, onOpen, onToggleFavorite } = props;
  return (
    <ButtonBase
      onClick={onOpen}
      sx={{
        width: "100%",
        height: 72,
        flexShrink: 0,
        minWidth: 0,
        overflow: "hidden",
        display: "flex",
        alignItems: "center",
        justifyContent: "flex-start",
        textAlign: "left",
        gap: 1.5,
        px: 1.5,
        // 和详情页的版本卡片同一套：描边 + 圆角 + surface 底（跟着界面不透明度透过去）
        border: "1px solid",
        borderColor: "divider",
        borderRadius: 1,
        bgcolor: "surface.paper",
        backdropFilter: (t) => t.palette.surface.filter,
        // 悬停底色渐进出现，别瞬间跳变
        transition: "background-color 160ms ease",
        "&:hover": { bgcolor: "action.hover" },
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
        {item.iconUrl ? (
          <Box
            component="img"
            src={item.iconUrl}
            alt=""
            loading="lazy"
            sx={{ width: "100%", height: "100%", objectFit: "cover" }}
          />
        ) : (
          <Typography sx={{ fontSize: 15, fontWeight: 700, color: "text.secondary" }}>
            {item.title.slice(0, 1).toUpperCase()}
          </Typography>
        )}
      </Box>

      <Box sx={{ flex: 1, minWidth: 0 }}>
        <Stack direction="row" spacing={0.75} sx={{ alignItems: "center", minWidth: 0 }}>
          <Typography noWrap sx={{ fontSize: 13.5, fontWeight: 600 }}>
            {item.title}
          </Typography>
          {item.author && (
            <Typography
              noWrap
              sx={{ fontSize: 11.5, color: "text.secondary", flexShrink: 0 }}
            >
              {item.author}
            </Typography>
          )}
          {/* 只显示认得的加载器：资源包的 `loaders` 是 `["minecraft"]`，
              原样渲染会在每张卡上多一个没信息量的小胶囊 */}
          {platformLoaderLabels(item.loaders).map((loader) => (
            <Typography
              key={loader}
              sx={{
                fontSize: 9.5,
                lineHeight: 1.6,
                px: 0.6,
                flexShrink: 0,
                borderRadius: 999,
                color: "text.secondary",
                bgcolor: (t) => alpha(t.palette.text.primary, 0.08),
              }}
            >
              {loader}
            </Typography>
          ))}
        </Stack>
        <Typography
          noWrap
          sx={{ fontSize: 11.5, color: "text.secondary", mt: 0.25 }}
        >
          {item.description || "没有简介"}
        </Typography>
      </Box>

      <Stack sx={{ alignItems: "flex-end", flexShrink: 0, pr: 0.25 }}>
        <Typography sx={{ fontSize: 12.5, fontVariantNumeric: "tabular-nums" }}>
          {formatCount(item.downloads)} 次下载
        </Typography>
        <Typography
          sx={{ fontSize: 11, color: "text.secondary", fontVariantNumeric: "tabular-nums" }}
        >
          {formatDate(item.updated)}
        </Typography>
      </Stack>

      {/* 收藏：只切换收藏，不进入详情。
          用 span + role 而不是 IconButton：卡片本身已经是 ButtonBase，
          再塞一个 <button> 就是非法嵌套。这样涟漪仍属于整卡，星标只负责
          挡掉自己的事件。 */}
      <Tooltip title={favorited ? "取消收藏" : "收藏"}>
        <Box
          component="span"
          role="button"
          tabIndex={0}
          aria-label={favorited ? "取消收藏" : "收藏"}
          onClick={(event) => {
            event.stopPropagation();
            onToggleFavorite();
          }}
          onMouseDown={(event) => event.stopPropagation()}
          // 触屏 / 触控板走的是 pointerdown：`ButtonBase` 的涟漪在按下那一刻起播，
          // 只拦 mouse 事件的话，手指点星星还是会先泛一圈整卡涟漪
          onPointerDown={(event) => event.stopPropagation()}
          onKeyDown={(event) => {
            if (event.key !== "Enter" && event.key !== " ") return;
            event.preventDefault();
            event.stopPropagation();
            onToggleFavorite();
          }}
          sx={{
            flexShrink: 0,
            width: 32,
            height: 32,
            ml: 0.25,
            mr: 0.5,
            borderRadius: "50%",
            display: "grid",
            placeItems: "center",
            cursor: "pointer",
            transition: "background-color 160ms ease",
            "&:hover": { bgcolor: (t) => alpha(t.palette.text.primary, 0.08) },
            "&:focus-visible": {
              outline: "2px solid",
              outlineColor: "primary.main",
              outlineOffset: -2,
            },
          }}
        >
          {favorited ? (
            <StarRounded sx={{ fontSize: 20, color: "primary.dark" }} />
          ) : (
            <StarBorderRounded sx={{ fontSize: 20, color: "text.secondary" }} />
          )}
        </Box>
      </Tooltip>
    </ButtonBase>
  );
}
