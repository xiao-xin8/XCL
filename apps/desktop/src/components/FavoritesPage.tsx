import { useCallback, useEffect, useLayoutEffect, useRef, useState } from "react";
import { Box, MenuItem, Select, Stack, TextField, Typography } from "@mui/material";
import { invoke } from "@tauri-apps/api/core";
import { apiGet } from "../lib/api";
import {
  toAppError,
  type AppError,
  type FavoriteEntry,
  type ResourceKind,
} from "../lib/types";
import { useRegisterNestedPage } from "../lib/nestedNav";
import { useScrollRestore } from "../lib/useScrollRestore";
import { CARD_MOTION, EASE } from "../motion/tokens";
import { playEnter } from "../motion/pageMotion";
import PageTransition from "../motion/PageTransition";
import ErrorState from "./ErrorState";
import ModpackDetailPage from "./ModpackDetailPage";
import ResourceDetailPage from "./ResourceDetailPage";
import { favKey, ResourceCard } from "./ResourceListPage";
import { useToast } from "./Toast";

/**
 * 下载页 →「收藏」。
 *
 * 把四种资源（整合包 / Mod / 资源包 / 光影）的收藏聚在一页里：
 * 搜索、筛类型、点进去分别是各自原来的详情页。
 *
 * 三个刻意的减法（和旁边的搜索列表页不同）：
 * - 没有下载源切换：收藏是本地记录，跟 Modrinth / CurseForge 无关；
 * - 没有排序下拉：固定按收藏时间倒序，最近收的在最前；
 * - 不发任何网络请求：数据全在 `favorites.toml` 里（含卡片快照），
 *   所以断网也能打开，搜索也是搜本地的名字。
 */

/** 顶部控件高度：和旁边的资源列表页保持一致，切分类时不跳。 */
const CONTROL_H = 40;

const KINDS: { key: "" | ResourceKind; label: string }[] = [
  { key: "", label: "全部类型" },
  { key: "modpack", label: "整合包" },
  { key: "mod", label: "Mod" },
  { key: "resourcepack", label: "资源包" },
  { key: "shader", label: "光影" },
];

/** 输入停下多久才筛（本地过滤，只为别每敲一个字就重排一次列表） */
const DEBOUNCE_MS = 200;

/** 一条收藏的渲染槽位。`leaving` 期间卡片还在 DOM 里播退场。 */
type Slot = { entry: FavoriteEntry; leaving: boolean };

function sameEntry(a: FavoriteEntry, b: FavoriteEntry): boolean {
  return a.source === b.source && a.kind === b.kind && a.id === b.id;
}

export default function FavoritesPage() {
  const toast = useToast();

  const [kind, setKind] = useState<"" | ResourceKind>("");
  const [input, setInput] = useState("");
  const [query, setQuery] = useState("");

  const [slots, setSlots] = useState<Slot[]>([]);
  /** 第一次读盘回来没有。回来之前不显示空态 —— 否则会先闪一下「还没有收藏」。 */
  const [loaded, setLoaded] = useState(false);
  const [error, setError] = useState<AppError | null>(null);
  const [reload, setReload] = useState(0);

  const [detail, setDetail] = useState<FavoriteEntry | null>(null);
  /**
   * 详情页是否打开，和 `detail` 分开存。
   *
   * `PageTransition` 在退场那 110ms 里仍然按旧 key 渲染，所以这时候
   * `detail` 必须还在 —— 否则返回的瞬间就换成列表内容，退场动画播的是错的东西，
   * 看着就是「返回时闪一下」。资源列表页也是这么分的（`detailOpen` + `detailItem`）。
   *
   * 关闭时只置 false、不清空 detail，等下次点开再覆盖。
   */
  const [detailOpen, setDetailOpen] = useState(false);
  const [fadeToken, setFadeToken] = useState(0);
  const listRef = useRef<HTMLDivElement>(null);
  // 进详情再返回时，回到原来的滚动位置
  const { ref: scrollRef, save: saveScroll, restore: restoreScroll } =
    useScrollRestore("favorite-list");
  /** 只认最后一次请求：快速换筛选条件时，旧响应不能盖掉新的 */
  const seq = useRef(0);

  // 输入防抖
  useEffect(() => {
    const timer = window.setTimeout(() => {
      setQuery((prev) => (prev === input.trim() ? prev : input.trim()));
    }, DEBOUNCE_MS);
    return () => window.clearTimeout(timer);
  }, [input]);

  // 取数据：纯本地，后端只读 favorites.toml
  useEffect(() => {
    const mine = ++seq.current;
    void (async () => {
      try {
        const items = await apiGet<FavoriteEntry[]>(
          "favorites_list",
          { kind: kind || null, query: query || null },
          // 这一页不走缓存。它是纯本地读（后端只是查内存里的映射），
          // 根本没有网络成本；进缓存唯一的结果是把刚取消的那条又还回来。
          // 缓存是给「要联网、慢」的接口用的，不是给本地查表的。
          { ttlMs: 0 },
        );
        if (seq.current !== mine) return;
        setSlots(items.map((entry) => ({ entry, leaving: false })));
        setError(null);
        setLoaded(true);
        setFadeToken((n) => n + 1);
      } catch (e: unknown) {
        if (seq.current !== mine) return;
        setError(toAppError(e, "读取收藏失败"));
        setLoaded(true);
      }
    })();
  }, [kind, query, reload]);

  // 入场：一轮新的筛选结果整列逐条落下（和搜索列表页同一个函数、同一套参数）
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
    // 只有「换了筛选条件」才重播；取消收藏导致条数变少时不动，
    // 那条卡片正在自己的退场动画里
    if (fadeToken !== lastFadeRef.current) {
      lastFadeRef.current = fadeToken;
      renderedRef.current = children.length;
      return playEnter(root);
    }
    renderedRef.current = children.length;
  }, [slots, fadeToken]);

  /**
   * 取消收藏。
   *
   * 不立刻从列表里抽走：先等后端确认（失败就什么都不动），再把卡片标成
   * `leaving` —— 高度 `1fr→0fr` 收回、同时淡出，下面的卡片跟着平滑上移，
   * 收完才真正卸载。直接 `filter` 掉会「啪」一下消失，还会让整列瞬间跳一下。
   */
  const unfavorite = useCallback(
    async (entry: FavoriteEntry) => {
      try {
        await invoke<boolean>("favorite_set", {
          source: entry.source,
          kind: entry.kind,
          id: entry.id,
          info: null,
          on: false,
        });
      } catch (e: unknown) {
        toast(toAppError(e, "取消收藏失败").message, "error");
        return;
      }

      setSlots((prev) =>
        prev.map((slot) =>
          sameEntry(slot.entry, entry) ? { ...slot, leaving: true } : slot,
        ),
      );

      // 动画播完再摘：时长取 `CARD_MOTION.collapse`，和过渡完全同步，
      // 多留 40ms 缓冲，避免刚好卡在最后一帧被截断
      window.setTimeout(() => {
        setSlots((prev) => prev.filter((slot) => !sameEntry(slot.entry, entry)));
      }, CARD_MOTION.collapse + 40);
    },
    [toast],
  );

  const openDetail = useCallback(
    (entry: FavoriteEntry) => {
      // 先进详情页再回来，列表是重新挂载的 —— 不记位置就会跳回顶部
      saveScroll();
      setDetail(entry);
      setDetailOpen(true);
    },
    [saveScroll],
  );
  const closeDetail = useCallback(() => setDetailOpen(false), []);

  /**
   * 列表滚动位置。用 ref 回调而不是 effect：列表要等退场播完（110ms）
   * 才挂回来，effect 的时机对不上；回调是「节点真正出现」的那一刻。
   */
  const attachScroll = useCallback(
    (node: HTMLDivElement | null) => {
      scrollRef.current = node;
      if (node) restoreScroll();
    },
    [scrollRef, restoreScroll],
  );

  // 标题栏左侧变「← 返回 + 项目名」，和资源列表页一个套路。
  // 注册放在打开的那一刻，标题就开始交叉换位，不用等页面切换的 110ms。
  useRegisterNestedPage(detailOpen && detail ? detail.info.title : null, closeDetail);

  const filtered = kind !== "" || query !== "";
  const empty = slots.length === 0;

  const list = (
    <Box sx={{ flex: 1, minHeight: 0, display: "flex", flexDirection: "column" }}>
      {/* 顶部一排：固定，不参与任何动画 */}
      <Stack
        direction="row"
        spacing={1}
        sx={{ flexShrink: 0, alignItems: "center", mb: 1.5, minWidth: 0 }}
      >
        <TextField
          size="small"
          label="搜索收藏"
          value={input}
          onChange={(event) => setInput(event.target.value)}
          sx={{
            flex: 1,
            minWidth: 0,
            "& .MuiInputBase-root": { height: CONTROL_H },
            "& .MuiInputBase-input": { fontSize: 13 },
            "& .MuiInputLabel-root": { fontSize: 13 },
          }}
        />

        <Select
          size="small"
          displayEmpty
          value={kind}
          onChange={(event) =>
            setKind(event.target.value as "" | ResourceKind)
          }
          sx={{ height: CONTROL_H, flexShrink: 0, fontSize: 13, width: 110 }}
          slotProps={{ input: { "aria-label": "类型" } }}
        >
          {KINDS.map((item) => (
            <MenuItem key={item.key || "all"} value={item.key} sx={{ fontSize: 13 }}>
              {item.label}
            </MenuItem>
          ))}
        </Select>
      </Stack>

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
        {error && (
          <ErrorState
            variant="page"
            error={error}
            onRetry={() => setReload((n) => n + 1)}
            sx={{ py: 6 }}
          />
        )}

        {!error && loaded && empty && (
          <Typography sx={{ fontSize: 11.5, color: "text.secondary", p: 1.5 }}>
            {filtered
              ? "没有符合条件的收藏"
              : "还没有收藏。在整合包 / Mod / 资源包 / 光影的列表里点右边的星星就能收藏。"}
          </Typography>
        )}

        {!error && !empty && (
          <Box ref={listRef} data-page-stagger>
            {slots.map(({ entry, leaving }) => (
              <Box
                key={favKey(entry)}
                sx={{
                  display: "grid",
                  // 退场：高度收到 0 + 淡出 + 间距一起收，下面的卡片平滑上移
                  gridTemplateRows: leaving ? "0fr" : "1fr",
                  opacity: leaving ? 0 : 1,
                  mb: leaving ? 0 : 1,
                  pointerEvents: leaving ? "none" : "auto",
                  transition: [
                    `grid-template-rows ${CARD_MOTION.collapse}ms ${EASE.outFluent}`,
                    `opacity ${CARD_MOTION.collapse}ms ${EASE.outFluent}`,
                    `margin-bottom ${CARD_MOTION.collapse}ms ${EASE.outFluent}`,
                  ].join(", "),
                  // 收高度靠这层：`0fr` 只有在子项 `min-height:0` 时才会真正压到 0
                  "& > div": { minHeight: 0, overflow: "hidden" },
                }}
              >
                <div>
                  <ResourceCard
                    item={entry.info}
                    // 收藏页里的每一条当然都是已收藏的
                    favorited
                    onOpen={() => openDetail(entry)}
                    onToggleFavorite={() => void unfavorite(entry)}
                  />
                </div>
              </Box>
            ))}
          </Box>
        )}
      </Box>
    </Box>
  );

  return (
    <PageTransition
      pageKey={detailOpen ? "detail" : "list"}
      initial={false}
      render={(key) =>
        key === "detail" && detail ? (
          // 整合包建的是新实例，和「塞进某个本地版本」两回事，分开走
          detail.kind === "modpack" ? (
            <ModpackDetailPage item={detail.info} />
          ) : (
            <ResourceDetailPage item={detail.info} kind={detail.kind} />
          )
        ) : (
          list
        )
      }
    />
  );
}
