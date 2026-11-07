import { useCallback, useEffect, useLayoutEffect, useRef, useState } from "react";
import { Box, LinearProgress } from "@mui/material";
import { apiGet, peek } from "../lib/api";
import { toAppError, type AppError, type VersionListView } from "../lib/types";
import ErrorState from "./ErrorState";
import Loading from "./Loading";
import VersionTree from "./VersionTree";
import VersionDetail from "./VersionDetail";
import { playEnter } from "../motion/pageMotion";
import { useDoneSignal } from "../lib/downloadStore";

/**
 * 下载页的「游戏版本」主体。
 *
 * 这里只做三件事：拉清单、管「当前选中哪个版本」、把两块内容拼成一栏列表 + 一栏详情。
 * 列表的折叠逻辑在 `VersionTree`，详情的清单与安装逻辑在 `VersionDetail`。
 */
export default function VersionList() {
  // 先用缓存里的旧数据渲染，避免切页回来白屏转圈
  const cachedView = peek<VersionListView>("version_list");

  const [view, setView] = useState<VersionListView | null>(cachedView ?? null);
  const [error, setError] = useState<AppError | null>(null);
  const [initialLoading, setInitialLoading] = useState(!cachedView);
  const [refreshing, setRefreshing] = useState(false);
  const [selected, setSelected] = useState<string | null>(null);

  const hasDataRef = useRef(!!cachedView);
  const contentRef = useRef<HTMLDivElement>(null);
  /** 首次是转圈还是缓存内容：只有从转圈换成内容时才补一次入场动画 */
  const startedWithSpinner = useRef(!cachedView);
  const contentAnimated = useRef(false);

  const load = useCallback((force = false) => {
    setError(null);
    if (hasDataRef.current) setRefreshing(true);
    else setInitialLoading(true);

    apiGet<VersionListView>("version_list", {}, { force })
      .then((data) => {
        hasDataRef.current = true;
        setView(data);
        setInitialLoading(false);
      })
      .catch((e: unknown) => {
        setError(toAppError(e, "加载失败"));
        setInitialLoading(false);
      })
      .finally(() => setRefreshing(false));
  }, []);

  useEffect(() => {
    load();
  }, [load]);

  /**
   * 有任务装完就重拉一次清单。
   *
   * 不重拉的话：任务一结束（队列里的卡片随后被清掉），`installed` 还是装之前的
   * 旧值，按钮会从「校验并修复」弹回「安装」，非得切页再回来才对。
   *
   * 只订阅「完成信号」而不是整份任务表 —— 这样下载进度再频繁也不会带着
   * 版本列表一起重渲染。水位初值取挂载时的值，历史任务不会触发多余重拉。
   */
  const doneSignal = useDoneSignal();
  const seenDone = useRef(doneSignal);
  useEffect(() => {
    if (doneSignal === seenDone.current) return;
    seenDone.current = doneSignal;
    load(true);
  }, [doneSignal, load]);

  // 转圈换成清单时，内容块走一次逐个入场；缓存直接出内容时由外层页面动画负责
  useLayoutEffect(() => {
    const root = contentRef.current;
    if (!view || !root || contentAnimated.current) return;
    contentAnimated.current = true;
    if (!startedWithSpinner.current) return;
    return playEnter(root);
  }, [view]);

  // 加载 / 出错都直接摊在页面背景上（和 Mod 页一致），不套白色容器
  if (initialLoading && !view) {
    return (
      <Box sx={{ flex: 1, minHeight: 0, display: "grid", placeItems: "center" }}>
        <Loading />
      </Box>
    );
  }

  if (error && !view) {
    return (
      <ErrorState
        variant="page"
        error={error}
        onRetry={() => load(true)}
        sx={{ flex: 1, minHeight: 0 }}
      />
    );
  }

  if (!view) return null;

  return (
    <Box ref={contentRef} sx={{ display: "flex", flex: 1, minHeight: 0 }}>
      <Box
        sx={{
          flex: 1,
          minWidth: 0,
          minHeight: 0,
          display: "flex",
          flexDirection: "column",
        }}
      >
        {/* 后台刷新只在顶部走一条 2px 进度，不占一行、不写文字 */}
        <Box sx={{ height: 2, flexShrink: 0 }}>
          {refreshing && <LinearProgress sx={{ height: 2 }} />}
        </Box>

        {/* 分类各自是一个 Paper（见 VersionTree），这里只负责滚：
            容器本身不标记，里面的卡片才会被当成块逐个落下 */}
        <Box
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
          <VersionTree view={view} selected={selected} onSelect={setSelected} />
        </Box>
      </Box>

      <VersionDetail
        open={Boolean(selected)}
        versionId={selected}
        installed={isInstalled(view, selected)}
        onClose={() => setSelected(null)}
      />
    </Box>
  );
}

/** 在清单里查这个版本装没装。后端每次查询都实时算，所以这里直接用。 */
function isInstalled(view: VersionListView, id: string | null): boolean {
  if (!id) return false;
  for (const group of [
    ...view.releases,
    ...view.snapshots,
    ...view.old,
    ...view.fools,
  ]) {
    if (group.items.some((item) => item.id === id && item.installed)) {
      return true;
    }
  }
  return false;
}
