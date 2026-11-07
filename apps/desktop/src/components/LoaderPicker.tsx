import { useCallback, useEffect, useRef, useState } from "react";
import {
  Box,
  Chip,
  ListItemButton,
  MenuItem,
  Select,
  Stack,
  Typography,
  alpha,
} from "@mui/material";
import { apiGet, peek } from "../lib/api";
import { animationsEnabled } from "../lib/appearance";
import { CONTENT_FADE } from "../motion/tokens";
import { loaderIconKind, loaderLabel, useLoaderKinds } from "../lib/loaderKinds";
import {
  toAppError,
  type AppError,
  type LoaderKind,
  type LoaderListView,
  type LoaderRef,
} from "../lib/types";
import CollapsibleSection from "./CollapsibleSection";
import ErrorState from "./ErrorState";
import Loading from "./Loading";
import VersionIcon from "./VersionIcon";

/**
 * 版本详情里的「加载器」选择。
 *
 * 两段式：先选加载器类型（原版 / Fabric / Forge / NeoForge），选完之后再选具体版本。
 * 可选项由后端给（`loader_kinds`），没接通安装流水线的类型不会出现在这里。
 *
 * 版本列表按 minor 分组折叠（`0.16.x` / `47.4.x` …），默认只展开含当前
 * 选中项的那一组 —— 几百个版本一次性铺开，谁也找不着。
 * 样式全部复用 `CollapsibleSection`（和下载页的版本树同源），不另起一套。
 *
 * 选加载器不要求原版已装：安装时会把原版与加载器合并成一份自包含版本，
 * 原版的东西一起装掉，但不会多出一条原版记录。
 */
export default function LoaderPicker(props: {
  value: LoaderRef | null;
  onChange: (next: LoaderRef | null) => void;
  /** 父版本（原版）id：用来算「这个加载器版本 + 这个原版」是否已装 */
  game: string;
  /** 外部要求刷新（例如刚装完一个实例，已安装标记要跟着变），变化即重新拉一次 */
  refreshKey?: number;
}) {
  const { value, onChange, game, refreshKey = 0 } = props;
  const kinds = useLoaderKinds();

  const kind: LoaderKind | "vanilla" = value?.kind ?? "vanilla";
  /** 列表属于哪个类型：切换类型时连数据一起换，不能拿上一个类型的列表接着显示 */
  const [view, setView] = useState<{ kind: LoaderKind; data: LoaderListView } | null>(null);
  const hasData = useRef(false);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<AppError | null>(null);
  /** 手动重试的计数器：失败后不会自己重来，只有点「重试」才加一 */
  const [attempt, setAttempt] = useState(0);

  // 只在真正选到某个加载器时才拉列表（停在原版时不必打网络）
  //
  // 依赖里不能放 `loading`：失败时 `loading` 从 true 回到 false 会再触发
  // 一次，请求一直失败就一直重发，界面上就是永远转圈 —— 看起来像「根本没拉过来」。
  // 依赖 `attempt` 而不是 `view`：这样「有缓存也照样在后台刷一次」，
  // 数据过期了会自己更新，但不会因为 setView 再触发一轮。
  useEffect(() => {
    if (kind === "vanilla") return;
    const args = { kind, game };
    // 和版本列表一样：命中前端缓存就先拿旧数据渲染，别让面板先空一下
    const cached = peek<LoaderListView>("loader_versions", args);
    let alive = true;
    if (cached) {
      hasData.current = true;
      setView({ kind, data: cached });
    } else {
      hasData.current = false;
    }
    if (!hasData.current) setLoading(true);
    setError(null);
    apiGet<LoaderListView>("loader_versions", args)
      .then((data) => {
        if (!alive) return;
        hasData.current = true;
        setView({ kind, data });
      })
      .catch((e: unknown) => {
        // 已经有内容时静默失败：留着旧列表，比把面板换成错误更有用
        if (alive && !hasData.current) {
          setError(toAppError(e, `拿不到 ${loaderLabel(kinds, kind)} 版本列表`));
        }
      })
      .finally(() => {
        if (alive) setLoading(false);
      });
    return () => {
      alive = false;
    };
  }, [kind, attempt, game, refreshKey, kinds]);

  // 列表回来之后落定默认版本（刚切过来时还不知道最新版是几）
  useEffect(() => {
    if (!value || !view || view.kind !== value.kind) return;
    const known = view.data.groups.some((group) =>
      group.items.some((item) => item.version === value.version),
    );
    if (known) return;
    if (view.data.latest) onChange({ kind: value.kind, version: view.data.latest });
  }, [view, value, onChange]);

  /** 展开的组。默认只展开含当前选中项的那一组，其余收起。 */
  const [openGroups, setOpenGroups] = useState<Set<string>>(new Set());
  useEffect(() => {
    if (!view || !value || view.kind !== value.kind) return;
    const group = view.data.groups.find((g) =>
      g.items.some((i) => i.version === value.version),
    );
    if (group) setOpenGroups(new Set([group.key]));
  }, [view, value]);

  const toggleGroup = (key: string) =>
    setOpenGroups((prev) => {
      const next = new Set(prev);
      if (next.has(key)) next.delete(key);
      else next.add(key);
      return next;
    });

  /**
   * 列表「转圈 → 内容」的淡入。
   *
   * 挂在 ref 回调上：这个列表块是「数据有了才渲染」，挂载即代表内容出现，
   * 在这里起播不依赖 effect 时序，绝不会被别的渲染吞掉。
   * 时长/曲线用全局的 `CONTENT_FADE`（180ms、只 opacity、不位移）。
   *
   * 按「类型 + 版本」各淡一次：换一个加载器类型值得重播（内容整个换了），
   * 同一类型里换版本号只是数字变了，不重播。
   */
  const fadedKey = useRef<string | null>(null);
  const attachList = useCallback(
    (el: HTMLDivElement | null) => {
      if (!el) {
        // 卸载（切回原版）后允许下次挂载再淡一次
        fadedKey.current = null;
        return;
      }
      const key = `${game}:${kind}`;
      if (fadedKey.current === key) return;
      fadedKey.current = key;
      if (!animationsEnabled()) return;

      const fade = el.animate([{ opacity: 0 }, { opacity: 1 }], {
        duration: CONTENT_FADE.duration,
        easing: CONTENT_FADE.easing,
        fill: "both",
      });
      fade.finished.then(() => fade.cancel()).catch(() => undefined);
    },
    [game, kind],
  );

  const selectedLabel = kind === "vanilla" ? "" : loaderLabel(kinds, kind);

  return (
    <Stack spacing={1} sx={{ minWidth: 0 }}>
      {/* 加载器类型：和上面的信息行对齐（标签 72 宽） */}
      <Stack direction="row" sx={{ alignItems: "center", gap: 1, minWidth: 0 }}>
        <Typography
          sx={{ fontSize: 12.5, color: "text.secondary", width: 72, flexShrink: 0 }}
        >
          加载器
        </Typography>
        <Select
          size="small"
          value={kind}
          onChange={(e) => {
            const next = e.target.value as LoaderKind | "vanilla";
            if (next === "vanilla") {
              onChange(null);
              return;
            }
            // 立刻带上默认版本，按钮文案才是确定的；列表还没回来时先给空串
            onChange({ kind: next, version: view?.kind === next ? (view.data.latest ?? "") : "" });
          }}
          sx={{ flex: 1, minWidth: 0, fontSize: 13 }}
          slotProps={{ input: { "aria-label": "加载器" } }}
        >
          <MenuItem value="vanilla" sx={{ fontSize: 13 }}>
            原版
          </MenuItem>
          {kinds.map((item) => (
            <MenuItem key={item.kind} value={item.kind} sx={{ fontSize: 13 }}>
              {item.label}
            </MenuItem>
          ))}
        </Select>
      </Stack>

      {kind !== "vanilla" && (
        <>
          {loading && (
            <Box sx={{ display: "grid", placeItems: "center", py: 2 }}>
              <Loading />
            </Box>
          )}

          {error && !loading && (
            <ErrorState
              variant="inline"
              error={error}
              onRetry={() => setAttempt((n) => n + 1)}
            />
          )}

          {view && view.kind === kind && !loading && (
            <Box
              ref={attachList}
              sx={{
                // 外框圆角、里面一律直角：和「下载 → 游戏」的版本树、模组版本列表
                // 同一套（`borderRadius: 1` = 全局 16px）。`overflow: hidden` 会把
                // 折叠头的矩形底色裁成圆角，否则方角会顶出框外。
                border: "1px solid",
                borderColor: "divider",
                borderRadius: 1,
                overflow: "hidden",
                // 关键：绝不允许里面的长版本号把面板撑宽、把底部按钮挤走
                width: "100%",
                maxWidth: "100%",
                minWidth: 0,
              }}
            >
              {/* 官方根本没出过该加载器的版本（比如 1.12.2 没有 NeoForge），返回的是空分组 */}
              {view.data.groups.length === 0 && (
                <Typography
                  sx={{ fontSize: 11.5, color: "text.secondary", p: 1.5 }}
                >
                  该版本没有 {selectedLabel} 支持
                </Typography>
              )}
              {view.data.groups.map((group) => (
                <CollapsibleSection
                  key={group.key}
                  level={2}
                  open={openGroups.has(group.key)}
                  onToggle={() => toggleGroup(group.key)}
                  count={group.items.length}
                  header={
                    <Stack direction="row" spacing={1} sx={{ alignItems: "center" }}>
                      <Typography sx={{ fontSize: 13, fontWeight: 600 }}>
                        {group.key}
                      </Typography>
                      {group.latest && (
                        <Chip size="small" color="primary" label="最新" />
                      )}
                    </Stack>
                  }
                  renderBody={() =>
                    group.items.map((item) => {
                      const selected = value?.version === item.version;
                      return (
                        <ListItemButton
                          key={item.version}
                          selected={selected}
                          onClick={() =>
                            onChange({ kind: view.kind, version: item.version })
                          }
                          sx={{
                            borderRadius: 0,
                            minHeight: 32,
                            height: 32,
                            py: 0,
                            pl: 3,
                            pr: 2,
                            gap: 0.75,
                            bgcolor: "transparent",
                            overflow: "hidden",
                            "&.Mui-selected": {
                              bgcolor: (t) => alpha(t.palette.primary.main, 0.6),
                              "&:hover": {
                                bgcolor: (t) => alpha(t.palette.primary.main, 0.6),
                              },
                            },
                            "&:hover": { bgcolor: "action.hover" },
                          }}
                        >
                          {/* 和下载页的版本列表一样，每一条都挂一个方块图标 */}
                          <VersionIcon kind={loaderIconKind(view.kind)} size={16} />
                          <Typography
                            sx={{
                              fontSize: 13,
                              fontWeight: selected ? 600 : 400,
                              flexShrink: 0,
                              whiteSpace: "nowrap",
                            }}
                          >
                            {item.version}
                          </Typography>
                          {!item.stable && (
                            <Chip
                              size="small"
                              label="测试版"
                              sx={{
                                fontSize: 9.5,
                                height: 16,
                                flexShrink: 0,
                                bgcolor: (t) => alpha(t.palette.primary.dark, 0.16),
                                color: "text.secondary",
                              }}
                            />
                          )}
                          {item.installed && (
                            <Chip
                              size="small"
                              label="已安装"
                              sx={{
                                fontSize: 9.5,
                                height: 16,
                                flexShrink: 0,
                                // 和版本树里的「已安装 Forge」同款：淡黄底 + 深金字，不描边
                                bgcolor: (t) =>
                                  alpha(
                                    t.palette.primary.main,
                                    t.palette.mode === "dark" ? 0.28 : 0.55,
                                  ),
                                color: "brand.ink",
                              }}
                            />
                          )}
                          <Box sx={{ flex: 1, minWidth: 0 }} />
                        </ListItemButton>
                      );
                    })
                  }
                />
              ))}
            </Box>
          )}
        </>
      )}
    </Stack>
  );
}
