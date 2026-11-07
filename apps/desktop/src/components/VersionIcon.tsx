import { Box } from "@mui/material";
import type { VersionKind } from "../lib/types";
import grass from "../assets/version-icons/Grass.png";
import commandBlock from "../assets/version-icons/CommandBlock.png";
import cobbleStone from "../assets/version-icons/CobbleStone.png";
import anvil from "../assets/version-icons/Anvil.png";
import neoForge from "../assets/version-icons/NeoForge.png";
import fabric from "../assets/version-icons/Fabric.png";
import grassPath from "../assets/version-icons/GrassPath.png";
import egg from "../assets/version-icons/Egg.png";
import redstoneBlock from "../assets/version-icons/RedstoneBlock.png";

/**
 * 版本图标：方块素材（直接复用）。
 *
 * 分类由后端判定（下载清单看 Mojang 的 type，本地版本看 libraries / mainClass），
 * 这里只负责映射成图片。`kind` 理论上不会缺，但旧缓存可能没有这个字段，
 * 所以查不到就回落到红石块。
 */
const ICONS: Record<VersionKind, { src: string; label: string }> = {
  vanilla: { src: grass, label: "原版" },
  snapshot: { src: commandBlock, label: "预览版" },
  oldAlpha: { src: cobbleStone, label: "远古版" },
  oldBeta: { src: cobbleStone, label: "远古版" },
  forge: { src: anvil, label: "Forge" },
  neoForge: { src: neoForge, label: "NeoForge" },
  fabric: { src: fabric, label: "Fabric" },
  optiFine: { src: grassPath, label: "OptiFine" },
  liteLoader: { src: egg, label: "LiteLoader" },
  unknown: { src: redstoneBlock, label: "未知版本" },
};

export default function VersionIcon(props: {
  kind: VersionKind;
  size?: number;
  /** 自定义图标（data URL）。有就优先用它，没有才回落到方块素材 */
  icon?: string | null;
}) {
  const { kind, size = 20, icon } = props;
  const fallback = ICONS[kind] ?? ICONS.unknown;

  return (
    <Box
      component="img"
      src={icon ?? fallback.src}
      alt=""
      title={icon ? "自定义图标" : fallback.label}
      // 异步解码：展开一个几百行的分组时，别让图标解码占住主线程
      decoding="async"
      sx={{
        width: size,
        height: size,
        flexShrink: 0,
        display: "block",
        // 自定义图标按等比缩放塞进方块，不拉伸
        objectFit: icon ? "contain" : undefined,
        borderRadius: icon ? 0.5 : undefined,
      }}
    />
  );
}
