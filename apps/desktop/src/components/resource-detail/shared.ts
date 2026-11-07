/**
 * 资源详情页的共用常量、文案表和样式片段。
 *
 * 这里只有数据和纯函数，不产生任何组件 —— 拆出来是因为
 * `ResourceDetailPage` 主文件、依赖区块、各种列表行都要用同一份，
 * 留在主文件里会让子组件反向 import 主文件（循环依赖 + chunk 变大）。
 */
import type { SxProps, Theme } from "@mui/material/styles";
import { alpha } from "@mui/material";

import type { ResourceKind, VersionKind } from "../../lib/types";

/** 右栏抽屉宽度。 */
export const PANEL_W = 328;

/**
 * 一个版本组一次最多渲染多少行。
 *
 * 不是"优化着玩"：实测 JEI 的 `26.3` 一个版本挂着 1844 个文件，一次全建出来
 * 就是近两千行、上万 DOM 节点 —— 点任何东西都要等它重排完。
 * 超过这个数就写「显示更多」，点一次再放 100 行。
 */
export const ROW_PAGE = 100;

/**
 * 右栏里那两种没有折叠标头的列表：外框自己圆角（16px = `borderRadius: 1`），
 * 里面的行保持直角。不加这一层就是一排方角行贴在面板里，看着"没有圆角"。
 */
export const panelListSx = {
  border: "1px solid",
  borderColor: "divider",
  borderRadius: 1,
  overflow: "hidden",
  bgcolor: "surface.paper",
  backdropFilter: (theme: Theme) => theme.palette.surface.filter,
} as const;

export const NUM = { fontVariantNumeric: "tabular-nums" } as const;

/**
 * 文案里该怎么称呼这类资源。
 *
 * 用 `Record<ResourceKind, ...>` 而不是三元：新增一种类型时 TS 会直接报缺 key，
 * 比 `kind === "resourcepack" ? ... : "Mod"` 安全 —— 后者在加了第三种类型之后
 * 会默默把新类型也叫成「Mod」。
 */
export const RESOURCE_NOUN: Record<ResourceKind, string> = {
  mod: "Mod",
  resourcepack: "资源包",
  shader: "光影",
  modpack: "整合包",
};

/**
 * 装完之后「还得去游戏里做一步」的提示。
 *
 * Mod 进 `mods/` 就生效，不需要提示；资源包要去「选项 → 资源包」里启用，
 * 光影要去「选项 → 视频设置 → 光影」里选。只有列在这里的类型才会显示。
 */
export const INSTALL_HINT: Partial<Record<ResourceKind, string>> = {
  resourcepack: "装好了。回游戏里「选项 → 资源包」点一下启用才会生效。",
  shader: "装好了。回游戏里「选项 → 视频设置 → 光影」选一下才会生效。",
};

export const LOADER_LABEL: Record<string, string> = {
  fabric: "Fabric",
  forge: "Forge",
  neoforge: "NeoForge",
  quilt: "Quilt",
  // 资源包的行上放的是本地版本自身的类型，不写这条就会显示成英文 `vanilla`
  vanilla: "原版",
};

export const RELATION_LABEL: Record<string, string> = {
  required: "必装",
  optional: "可选",
  incompatible: "不兼容",
};

/** 依赖页需要「同一个本地版本」：id + MC 版本 + 加载器。 */
export type TargetRef = {
  versionId: string;
  gameVersion: string;
  loader: string;
};

export function loaderLabel(loader: string): string {
  return LOADER_LABEL[loader] ?? loader;
}

/** 加载器名 → `VersionIcon` 的分类（注意 NeoForge 在前端类型里是驼峰 `neoForge`）。 */
export function iconKind(loader: string): VersionKind {
  switch (loader) {
    case "fabric":
      return "fabric";
    case "forge":
      return "forge";
    case "neoforge":
      return "neoForge";
    // 资源包的行上放的是本地版本自身的类型
    case "vanilla":
      return "vanilla";
    default:
      return "unknown";
  }
}

/**
 * 卡片外壳：一条一个 `Paper`，和 Mod 列表同款
 * （圆角 16、1px 描边、选中整块淡黄 —— 不额外加左边那道竖条）。
 */
export const cardSx = (active: boolean) => ({
  flexShrink: 0,
  borderRadius: 1,
  overflow: "hidden" as const,
  // 选中只给一层淡黄，描边保持原样 —— 不要额外的色边
  bgcolor: active ? "primary.light" : "surface.paper",
  borderColor: "divider",
  // 悬停反馈：和 Mod 列表的卡片一致（鼠标上去不能没反应），且要淡入
  transition: "background-color 160ms ease",
  "&:hover": { bgcolor: active ? "primary.light" : "action.hover" },
});

/** 侧栏里的一行：尺寸对齐 LoaderPicker 的列表行（32 高、选中 alpha .6）。 */
export const panelRowSx = (active: boolean): SxProps<Theme> => ({
  width: "100%",
  height: 32,
  minHeight: 32,
  maxHeight: 32,
  justifyContent: "flex-start",
  textAlign: "left" as const,
  px: 1.5,
  gap: 1,
  borderRadius: 0,
  // 和加载器列表的选中行同款：alpha(primary.main, .6)
  bgcolor: active ? (t: Theme) => alpha(t.palette.primary.main, 0.6) : "transparent",
  transition: "background-color 160ms ease",
  "&:hover": {
    bgcolor: active
      ? (t: Theme) => alpha(t.palette.primary.main, 0.6)
      : "action.hover",
  },
});
