import { Chip, alpha, type SxProps, type Theme } from "@mui/material";

/**
 * 版本类型徽标 —— 全项目只此一个。
 *
 * 它同时服务两类值，因为它们本来就是同一个维度（成熟度）：
 * - 资源文件的发行类型：`release` / `beta` / `alpha`
 * - MC 版本本身的类型：`release` / `snapshot` / `old_beta` / `old_alpha`
 *
 * 以前这些地方都是 `label={type}` 直接渲染，界面上就冒出英文 `release`、
 * `snapshot`。现在统一走这里：中文标签 + 按风险配色。
 *
 * 配色：正式版跟主色走（金），预览 / 测试橙，早期测试红，远古灰。
 * 只染底纹和字，不加边框，和全局「状态色只进底纹」的规矩一致。
 */
const LABEL: Record<string, string> = {
  release: "正式版",
  snapshot: "预览版",
  beta: "测试版",
  alpha: "早期测试版",
  old_beta: "远古 Beta",
  old_alpha: "远古 Alpha",
  // 版本 JSON 读不出来时后端给的是这个
  unknown: "未知",
};

const STYLE: Record<string, SxProps<Theme>> = {
  release: {
    bgcolor: (t: Theme) => alpha(t.palette.primary.main, 0.3),
    color: (t: Theme) => t.palette.brand.ink,
  },
  snapshot: {
    bgcolor: (t: Theme) => alpha(t.palette.warning.main, 0.18),
    color: (t: Theme) => t.palette.warning.dark,
  },
  beta: {
    bgcolor: (t: Theme) => alpha(t.palette.warning.main, 0.18),
    color: (t: Theme) => t.palette.warning.dark,
  },
  alpha: {
    bgcolor: (t: Theme) => alpha(t.palette.error.main, 0.16),
    color: (t: Theme) => t.palette.error.dark,
  },
  old_beta: {
    bgcolor: (t: Theme) => alpha(t.palette.text.primary, 0.08),
    color: "text.secondary",
  },
  old_alpha: {
    bgcolor: (t: Theme) => alpha(t.palette.text.primary, 0.08),
    color: "text.secondary",
  },
  unknown: {
    bgcolor: (t: Theme) => alpha(t.palette.text.primary, 0.08),
    color: "text.secondary",
  },
};

/** 类型值 → 中文名。文案里要用（比如「版本类型：正式版」）就调这个。 */
export function versionTypeLabel(type: string): string {
  return LABEL[type] ?? type;
}

export default function VersionTypeChip(props: { type: string }) {
  const { type } = props;
  return (
    <Chip
      size="small"
      label={versionTypeLabel(type)}
      sx={{
        height: 18,
        fontSize: 9.5,
        flexShrink: 0,
        fontWeight: 600,
        ...(STYLE[type] ?? STYLE.unknown),
      }}
    />
  );
}
