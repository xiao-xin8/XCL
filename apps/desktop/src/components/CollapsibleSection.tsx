import { useEffect, useState, type ReactNode } from "react";
import { Box, ButtonBase, Collapse, alpha, useTheme } from "@mui/material";
import ExpandMoreIcon from "@mui/icons-material/ExpandMore";
import { CARD_MOTION, EASE } from "../motion/tokens";

/**
 * 通用折叠区块（一级 = 分类头，二级 = 分组头）。
 *
 * 从 `VersionTree` 里抽出来的原因：版本列表和版本详情里的「加载器版本」
 * 都要同一套折叠，各写一份迟早会长歪。改样式只改这里。
 *
 * 两条硬规矩：
 * - 折叠头用 `ButtonBase` 自己搭：MUI 9 的 `AccordionSummary` 默认
 *   `disableRipple`，点下去一点反馈都没有。
 * - `renderBody` 传函数而不是节点：收起时不构造子元素，
 *   否则三百个版本行会在展开前就全建出来。
 */

/** 数量胶囊：贴在本行右端、展开按钮左侧。 */
export function CountPill({ value }: { value: number }) {
  const theme = useTheme();
  return (
    <Box
      sx={{
        minWidth: 24,
        height: 20,
        px: 0.85,
        mr: 0.5,
        borderRadius: 999,
        display: "grid",
        placeItems: "center",
        bgcolor: alpha(theme.palette.text.primary, 0.09),
        color: "text.secondary",
        fontSize: 11.5,
        fontWeight: 600,
        fontVariantNumeric: "tabular-nums",
      }}
    >
      {value}
    </Box>
  );
}

/**
 * 按「行数 × 32px」估展开高度，套 MUI 的 auto 曲线后封顶。
 *
 * MUI `timeout="auto"` 的公式是 `min((4 + 15·(h/36)^0.25 + h/180)·10, 3000)`：
 * 1.3k 高的分组 ~470ms、6k ~900ms，大分组光等就等掉了。这里保留曲线
 * （小组快、大组稳）但把上限压到 `collapseEnterMax`，视觉节奏还是同一个，
 * 只是不再有长尾。
 */
function enterDuration(count: number): number {
  const height = Math.max(1, count * 32);
  const constant = height / 36;
  const auto = Math.round((4 + 15 * constant ** 0.25 + constant / 5) * 10);
  return Math.min(
    CARD_MOTION.collapseEnterMax,
    Math.max(CARD_MOTION.collapseEnterMin, auto),
  );
}

export default function CollapsibleSection(props: {
  level: 1 | 2;
  open: boolean;
  onToggle: () => void;
  header: ReactNode;
  count: number;
  renderBody: () => ReactNode;
}) {
  const { level, open, onToggle, header, count, renderBody } = props;
  const theme = useTheme();
  const isDark = theme.palette.mode === "dark";

  const [mounted, setMounted] = useState(open);
  useEffect(() => {
    if (open) setMounted(true);
  }, [open]);

  const level1 = level === 1;
  // 一级更深、二级稍浅，但都比正文深，形成层次
  const headBg = level1
    ? alpha(theme.palette.primary.main, isDark ? 0.18 : 0.45)
    : alpha(theme.palette.primary.main, isDark ? 0.09 : 0.2);

  return (
    <Box
      sx={{
        borderTop: level1 ? "none" : "1px solid",
        borderColor: "divider",
        // 列表里可能出现很长的版本号：绝不允许撑宽容器把外面的按钮挤走
        minWidth: 0,
        width: "100%",
        maxWidth: "100%",
      }}
    >
      <ButtonBase
        onClick={onToggle}
        aria-expanded={open}
        sx={{
          width: "100%",
          display: "flex",
          alignItems: "center",
          gap: 1,
          minHeight: level1 ? 46 : 38,
          pl: level1 ? 2 : 3,
          pr: 1.5,
          bgcolor: headBg,
          borderRadius: 0,
          textAlign: "left",
        }}
      >
        {header}
        <Box sx={{ flex: 1 }} />
        <CountPill value={count} />
        <ExpandMoreIcon
          sx={{
            fontSize: level1 ? 20 : 17,
            color: "text.secondary",
            flexShrink: 0,
            transition: `transform ${CARD_MOTION.collapse}ms ${EASE.outFluent}`,
            transform: open ? "rotate(180deg)" : "none",
          }}
        />
      </ButtonBase>

      {/* 高度交给 MUI 的 Collapse，但时长封顶（见 `enterDuration`）：
          曲线还是「大组从容、小组干脆」，不再让几千像素的分组拖近一秒。
          展开时高度每帧都在变，加 `contain: content` 把重排/重绘限制在这一块
          内部，外层卡片和滚动容器不用跟着重算 */}
      <Collapse
        in={open}
        timeout={{
          enter: enterDuration(count),
          exit: CARD_MOTION.collapseExit,
        }}
        onExited={() => setMounted(false)}
      >
        <Box sx={{ minHeight: 0, contain: "content" }}>
          {open || mounted ? renderBody() : null}
        </Box>
      </Collapse>
    </Box>
  );
}
