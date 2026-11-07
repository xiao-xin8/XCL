/**
 * 资源详情页的各种「行」。
 *
 * 全部是纯 props 组件：不读页面状态、不调后端，只把一条数据画成一行。
 * 拆出来是因为它们和主组件的状态机没有耦合 —— 主组件 1200 行里有一半是
 * 这些行，混在一起改哪一行都要在两千行里找。
 *
 * 两个细节别动：
 * - `ManualFileRow` 的 `memo` + 稳定 `onSelect`：一个组可能有几百行，
 *   点一下只该重绘"选中状态变了"的那两行（那就是"点按钮有延时"的来源）。
 * - `FileTaskChip` 按「文件 + 落点」订阅：别的文件在下载不会让这一行重渲染。
 */
import { memo } from "react";
import { Box, ButtonBase, Chip, Paper, Typography, alpha } from "@mui/material";

import { formatBytes, formatDate } from "../../lib/format";
import { isFilePairDone, useTaskForFile } from "../../lib/downloadStore";
import type { InstallTarget, LocalTarget, ResourceFile } from "../../lib/types";
import VersionTypeChip from "../VersionTypeChip";
import VersionIcon from "../VersionIcon";
import { NUM, cardSx, iconKind, loaderLabel, panelRowSx } from "./shared";

/** 自动模式一条：一个本地版本（卡片，和 Mod 列表一个样子）。 */
export function LocalTargetRow(props: {
  target: LocalTarget;
  active: boolean;
  onClick: () => void;
}) {
  const { target, active, onClick } = props;
  return (
    <Paper variant="outlined" sx={cardSx(active)}>
      <ButtonBase
        onClick={onClick}
        sx={{
          width: "100%",
          height: 68,
          display: "flex",
          alignItems: "center",
          justifyContent: "flex-start",
          textAlign: "left",
          gap: 1.5,
          px: 1.75,
          cursor: "pointer",
        }}
      >
        <VersionIcon kind={iconKind(target.loader)} size={26} />
        <Box sx={{ minWidth: 0, flex: 1 }}>
          <Typography noWrap sx={{ fontSize: 13.5, fontWeight: 600 }}>
            {target.gameVersion} · {loaderLabel(target.loader)}
          </Typography>
          <Typography noWrap sx={{ fontSize: 11.5, color: "text.secondary", mt: 0.25 }}>
            {target.installed ? "已安装 · 点一下查看文件与依赖" : "点一下挑文件版本"}
          </Typography>
        </Box>
        {target.installed && (
          <Chip size="small" label="已安装" sx={{ height: 20, fontSize: 10.5 }} />
        )}
      </ButtonBase>
    </Paper>
  );
}

/**
 * 一行文件自己的下载状态。
 *
 * 按「文件 + 落点」订阅：别的文件在下载、或者这个文件装到了另一个实例，
 * 都不会让这一行跟着重渲染。
 */
export function FileTaskChip(props: { fileId: string; target: string | null }) {
  const task = useTaskForFile(props.fileId, props.target);
  // 成功的任务 1.2s 后会被清掉，但这一行还该标着「已完成」
  const pairDone = isFilePairDone(props.fileId, props.target);
  if (!task && !pairDone) return null;

  const label = !task
    ? "已完成"
    : task.phase === "running"
      ? `下载中 ${Math.round(task.fraction * 100)}%`
      : task.phase === "done"
        ? "已完成"
        : task.phase === "failed"
          ? "失败"
          : "已取消";
  const failed = task?.phase === "failed";

  return (
    <Chip
      size="small"
      label={label}
      sx={{
        height: 16,
        fontSize: 9.5,
        flexShrink: 0,
        bgcolor: (t) => alpha(t.palette.text.primary, 0.08),
        color: failed ? "error.main" : "text.secondary",
      }}
    />
  );
}

/**
 * 手动模式一条：折叠组里的文件行（直角、缩进，和版本树的行一致）。
 *
 * `memo` + 稳定的 `onSelect`：一个组可能有几百行，点一下只该重绘"选中状态变了"
 * 的那两行，而不是把整个列表重新协调一遍（那就是"点按钮有延时"的来源）。
 */
export const ManualFileRow = memo(function ManualFileRow(props: {
  file: ResourceFile;
  /** 当前的落点（哪个本地版本 / 哪个自定义目录），这一行只显示自己那份进度 */
  target: string | null;
  active: boolean;
  onSelect: (fileId: string) => void;
}) {
  const { file, target, active, onSelect } = props;
  return (
    <ButtonBase
      onClick={() => onSelect(file.fileId)}
      sx={{
        width: "100%",
        height: 40,
        minHeight: 40,
        maxHeight: 40,
        justifyContent: "flex-start",
        textAlign: "left",
        gap: 1,
        pl: 3,
        pr: 2,
        borderRadius: 0,
        bgcolor: active ? (t) => alpha(t.palette.primary.main, 0.6) : "transparent",
        transition: "background-color 160ms ease",
        "&:hover": {
          bgcolor: active
            ? (t) => alpha(t.palette.primary.main, 0.6)
            : "action.hover",
        },
      }}
    >
      <Typography
        noWrap
        sx={{
          fontSize: 13,
          fontWeight: active ? 600 : 400,
          flex: 1,
          minWidth: 0,
        }}
      >
        {file.versionNumber}
      </Typography>
      <FileTaskChip fileId={file.fileId} target={target} />
      <VersionTypeChip type={file.releaseType} />
      <Typography sx={{ fontSize: 11.5, color: "text.secondary", flexShrink: 0, ...NUM }}>
        {formatBytes(file.size)}
      </Typography>
      <Typography
        sx={{
          fontSize: 11.5,
          color: "text.secondary",
          flexShrink: 0,
          width: 76,
          textAlign: "right",
          ...NUM,
        }}
      >
        {formatDate(file.published)}
      </Typography>
    </ButtonBase>
  );
});

/**
 * 一个版本组里的文件行（带「显示更多」）。
 *
 * 大模组一个版本可能挂一千多个文件：先放 `ROW_PAGE` 行，剩下的写一行
 * 「显示更多」，点一次再放一批。DOM 里同时存在的行数因此是有限的。
 */
export function ManualFileRowList(props: {
  items: ResourceFile[];
  shown: number;
  target: string | null;
  selected: string | null;
  onSelect: (fileId: string) => void;
  onMore: () => void;
}) {
  const { items, shown, target, selected, onSelect, onMore } = props;
  const rest = items.length - shown;

  return (
    <>
      {items.slice(0, shown).map((file) => (
        <ManualFileRow
          key={file.fileId}
          file={file}
          target={target}
          active={selected === file.fileId}
          onSelect={onSelect}
        />
      ))}
      {rest > 0 && (
        <ButtonBase
          onClick={onMore}
          sx={{
            width: "100%",
            height: 36,
            justifyContent: "flex-start",
            pl: 3,
            pr: 2,
            borderRadius: 0,
            transition: "background-color 160ms ease",
            "&:hover": { bgcolor: "action.hover" },
          }}
        >
          <Typography sx={{ fontSize: 12.5, fontWeight: 600, color: "brand.ink" }}>
            显示更多（还有 {rest} 个）
          </Typography>
        </ButtonBase>
      )}
    </>
  );
}

/** 侧栏里的文件一行（自动模式）：版本号 + 类型 + 体积，单行。 */
export function FileRow(props: {
  file: ResourceFile;
  target: string | null;
  active: boolean;
  onClick: () => void;
}) {
  const { file, target, active, onClick } = props;
  return (
    <ButtonBase onClick={onClick} sx={panelRowSx(active)}>
      <Typography
        noWrap
        sx={{ fontSize: 13, fontWeight: active ? 600 : 400, minWidth: 0 }}
      >
        {file.versionNumber}
      </Typography>
      <FileTaskChip fileId={file.fileId} target={target} />
      <VersionTypeChip type={file.releaseType} />
      <Box sx={{ flex: 1, minWidth: 0 }} />
      <Typography sx={{ fontSize: 11.5, color: "text.secondary", flexShrink: 0, ...NUM }}>
        {formatBytes(file.size)}
      </Typography>
    </ButtonBase>
  );
}

/** 侧栏里的「装到哪」一行（手动模式）。 */
export function InstallTargetRow(props: {
  target: InstallTarget;
  active: boolean;
  onClick: () => void;
}) {
  const { target, active, onClick } = props;
  return (
    <ButtonBase onClick={onClick} sx={panelRowSx(active)}>
      <VersionIcon kind={iconKind(target.loader)} size={16} />
      <Typography
        noWrap
        sx={{ fontSize: 13, fontWeight: active ? 600 : 400, minWidth: 0 }}
      >
        {target.gameVersion} · {loaderLabel(target.loader)}
      </Typography>
    </ButtonBase>
  );
}
