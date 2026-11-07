import { memo, useCallback, useRef, useState } from "react";
import {
  Box,
  Chip,
  ListItemButton,
  Paper,
  Stack,
  Typography,
  alpha,
} from "@mui/material";
import type { SxProps, Theme } from "@mui/material/styles";
import { formatDateTime } from "../lib/format";
import type { VersionItem, VersionListView } from "../lib/types";
import CollapsibleSection from "./CollapsibleSection";
import VersionIcon from "./VersionIcon";

/**
 * 三级折叠的版本列表（正式版 / 预览版 / 远古版 / 愚人节版 → 大版本 → 具体版本）。
 *
 * 折叠本身的样式与动画在 `CollapsibleSection`（和版本详情里的加载器列表共用）。
 *
 * 展开状态自己管：组件不会因为刷新而卸载，所以展开过什么，刷新后还在。
 * 默认全部收起：进入页面只挂四个分类头，避免一次构造上千行。
 */

const CATEGORIES = [
  { key: "releases", label: "正式版" },
  { key: "snapshots", label: "预览版" },
  { key: "old", label: "远古版" },
  { key: "fools", label: "愚人节版" },
] as const;

type CategoryKey = (typeof CATEGORIES)[number]["key"];

// ---------- 行样式 ----------
// 全部提到模块级：emotion 按对象引用缓存样式，行数多的时候每行每次渲染都
// 重新生成一份 sx 是纯粹的开销。选中态加粗写成 CSS 后代选择器，
// 标题样式因此也能是常量。

const ROW_SX: SxProps<Theme> = {
  // 列表行是通栏的，不能有圆角：32px 高配 16px 圆角会变成胶囊
  borderRadius: 0,
  minHeight: 32,
  height: 32,
  py: 0,
  pl: 3,
  pr: 2,
  gap: 1,
  bgcolor: "transparent",
  "&.Mui-selected": {
    bgcolor: (t) => alpha(t.palette.primary.main, 0.6),
    "&:hover": { bgcolor: (t) => alpha(t.palette.primary.main, 0.6) },
  },
  "&:hover": { bgcolor: "action.hover" },
  "&.Mui-selected .xcl-version-title": { fontWeight: 600 },
  // 视口外的行跳过布局与绘制（高度按 32px 占位，滚动进来再算）。
  // 展开一个几百行的分组时，这一步能砍掉绝大部分首帧布局开销
  contentVisibility: "auto",
  containIntrinsicSize: "auto 32px",
};

const TITLE_SX: SxProps<Theme> = {
  fontSize: 13,
  fontWeight: 400,
  // 状态胶囊可能有两个，窗口窄时让版本号先让步
  minWidth: 0,
  overflow: "hidden",
  textOverflow: "ellipsis",
};

const INSTALLED_CHIP_SX: SxProps<Theme> = {
  fontSize: 9.5,
  height: 16,
  flexShrink: 0,
  // 状态用极淡的底色 + 文字色，不做彩色描边
  bgcolor: (t) => alpha(t.palette.text.primary, 0.08),
  color: "text.secondary",
};

const LOADER_CHIP_SX: SxProps<Theme> = {
  fontSize: 9.5,
  height: 16,
  flexShrink: 0,
  // 和「已安装」拉开一点点：淡黄底 + 深金字，仍是状态色不做描边
  bgcolor: (t) =>
    alpha(t.palette.primary.main, t.palette.mode === "dark" ? 0.28 : 0.55),
  color: "brand.ink",
};

const SPACER_SX: SxProps<Theme> = { flex: 1 };

const TIME_SX: SxProps<Theme> = {
  fontSize: 11.5,
  color: "text.secondary",
  fontVariantNumeric: "tabular-nums",
  whiteSpace: "nowrap",
  flexShrink: 0,
};

/**
 * 一行版本。
 *
 * `memo` 是刻意的：展开一个分组会一次性渲染几十上百行，之后选中/刷新
 * 只会真正重渲染发生变化的那几行。
 */
const VersionRow = memo(function VersionRow(props: {
  item: VersionItem;
  selected: boolean;
  onSelect: (id: string) => void;
}) {
  const { item, selected, onSelect } = props;
  return (
    <ListItemButton
      selected={selected}
      onClick={() => onSelect(item.id)}
      sx={ROW_SX}
    >
      <VersionIcon kind={item.kind} />
      <Typography noWrap className="xcl-version-title" sx={TITLE_SX}>
        {item.id}
      </Typography>
      {/* 两种状态各标各的，不互相覆盖：
          「已安装」= 原版装过；「已安装 Forge」= 这个原版上有加载器版本。
          可能同时有几个（Forge 和 NeoForge 都装过），所以按数组铺开 */}
      {item.installed && (
        <Chip size="small" label="已安装" sx={INSTALLED_CHIP_SX} />
      )}
      {item.installedLoaders.map((label) => (
        <Chip
          key={label}
          size="small"
          label={`已安装 ${label}`}
          sx={LOADER_CHIP_SX}
        />
      ))}
      <Box sx={SPACER_SX} />
      <Typography sx={TIME_SX}>{formatDateTime(item.releaseTime)}</Typography>
    </ListItemButton>
  );
});

function toggleIn(
  setter: React.Dispatch<React.SetStateAction<Set<string>>>,
  key: string,
) {
  setter((prev) => {
    const next = new Set(prev);
    if (next.has(key)) next.delete(key);
    else next.add(key);
    return next;
  });
}

export default function VersionTree(props: {
  view: VersionListView;
  selected: string | null;
  onSelect: (id: string) => void;
}) {
  const { view, selected, onSelect } = props;

  // 父级每次渲染都可能传新的回调；用 ref 转成稳定引用，
  // 否则 `memo` 的行会因为 props 变化整片重渲染
  const onSelectRef = useRef(onSelect);
  onSelectRef.current = onSelect;
  const selectVersion = useCallback((id: string) => {
    onSelectRef.current(id);
  }, []);

  const [openCategories, setOpenCategories] = useState<Set<string>>(
    new Set(),
  );
  const [openGroups, setOpenGroups] = useState<Set<string>>(new Set());

  const counts: Record<CategoryKey, number> = {
    releases: view.releaseCount,
    snapshots: view.snapshotCount,
    old: view.oldCount,
    fools: view.foolCount,
  };

  return (
    <>
      {CATEGORIES.map((category) => {
        const groups = view[category.key];

        return (
          // 一个分类 = 一张卡片 = 一个动画块（未标记的 Paper 自动成块），
          // 于是切页进来时四个分类一块接一块落下。
          // 圆角只给这层容器（`borderRadius: 1` = 全局统一的 16px），
          // 里面的折叠头和版本行一律直角、通栏，不要再圆。
          <Paper
            key={category.key}
            variant="outlined"
            sx={{ borderRadius: 1, overflow: "hidden" }}
          >
            <CollapsibleSection
              level={1}
              open={openCategories.has(category.key)}
              onToggle={() => toggleIn(setOpenCategories, category.key)}
              count={counts[category.key]}
              header={
                <Typography sx={{ fontSize: 14, fontWeight: 700 }}>
                  {category.label}
                </Typography>
              }
              renderBody={() =>
                groups.map((group) => {
                  const groupKey = `${category.key}:${group.key}`;

                  return (
                    <CollapsibleSection
                      key={groupKey}
                      level={2}
                      open={openGroups.has(groupKey)}
                      onToggle={() => toggleIn(setOpenGroups, groupKey)}
                      count={group.items.length}
                      header={
                        <Stack
                          direction="row"
                          spacing={1}
                          sx={{ alignItems: "center" }}
                        >
                          <Typography sx={{ fontSize: 13, fontWeight: 600 }}>
                            {group.label}
                          </Typography>
                          {group.latest && (
                            <Chip size="small" color="primary" label="最新" />
                          )}
                        </Stack>
                      }
                      renderBody={() =>
                        group.items.map((item) => (
                          <VersionRow
                            key={item.id}
                            item={item}
                            selected={selected === item.id}
                            onSelect={selectVersion}
                          />
                        ))
                      }
                    />
                  );
                })
              }
            />
          </Paper>
        );
      })}
    </>
  );
}
