import AutoAwesomeOutlinedIcon from "@mui/icons-material/AutoAwesomeOutlined";
import ExtensionOutlinedIcon from "@mui/icons-material/ExtensionOutlined";
import PaletteOutlinedIcon from "@mui/icons-material/PaletteOutlined";
import type { ContentKind } from "./types";
import type { VersionFolderKey } from "./versionFolders";

/** 详情页能拉线上项目的类型；材质包在平台侧没有独立类型。 */
export type OnlineProjectKind = "mod" | "resourcepack" | "shader";

export type ContentKindConfig = {
  kind: ContentKind;
  /** 单个内容的称呼，用在按钮、开关和提示里 */
  label: string;
  /** 整页子页标题，如「模组管理」 */
  title: string;
  /** 打开文件夹用的后端目录 key */
  folder: VersionFolderKey;
  icon: typeof ExtensionOutlinedIcon;
  searchLabel: string;
  empty: string;
  noMatch: string;
  descriptionEmpty: string;
  externalSource: string;
  /** 没有来源记录时，详情页里说明能显示到什么程度 */
  localOnlyHint: string;
  onlineKind: OnlineProjectKind | null;
  /** 详情页「基本信息」里的 ID 行标题；没有 ID 概念的类型为 null */
  idLabel: string | null;
  /** 详情页里的格式行标题；没有格式概念的类型为 null */
  formatLabel: string | null;
  /** 重复标记文案；只有 Mod 有稳定 ID 可判重 */
  duplicateLabel: string | null;
};

/**
 * 四类本地内容的差异都收在这里。
 *
 * 管理器只认这个配置，不再按类型写分支 —— 加一种内容只改配置和后端
 * `ContentKind`，别在页面里再抄一份标签。
 */
export const CONTENT_KINDS: Record<ContentKind, ContentKindConfig> = {
  mod: {
    kind: "mod",
    label: "Mod",
    title: "模组管理",
    folder: "mods",
    icon: ExtensionOutlinedIcon,
    searchLabel: "搜索名称 / Mod ID / 文件名",
    empty: "这个版本还没有 Mod。点「从本地安装」放一个进来。",
    noMatch: "没有匹配的 Mod。",
    descriptionEmpty: "这个 Mod 没有写简介。",
    externalSource: "未记录（外部 Mod）",
    localOnlyHint: "只能显示 jar 自带元数据；哈希识别和线上更新还未接入。",
    onlineKind: "mod",
    idLabel: "Mod ID",
    formatLabel: null,
    duplicateLabel: "重复 ID",
  },
  resourcepack: {
    kind: "resourcepack",
    label: "资源包",
    title: "资源包管理",
    folder: "resourcepacks",
    icon: PaletteOutlinedIcon,
    searchLabel: "搜索名称 / 文件名",
    empty: "这个版本还没有资源包。点「从本地安装」放一个进来。",
    noMatch: "没有匹配的资源包。",
    descriptionEmpty: "这个资源包没有写简介。",
    externalSource: "未记录（外部资源包）",
    localOnlyHint: "只能显示包内元数据；哈希识别和线上更新还未接入。",
    onlineKind: "resourcepack",
    idLabel: null,
    formatLabel: "资源包格式",
    duplicateLabel: null,
  },
  shader: {
    kind: "shader",
    label: "光影",
    title: "光影管理",
    folder: "shaderpacks",
    icon: AutoAwesomeOutlinedIcon,
    searchLabel: "搜索名称 / 文件名",
    empty: "这个版本还没有光影。点「从本地安装」放一个进来。",
    noMatch: "没有匹配的光影。",
    descriptionEmpty: "这个光影没有可读取的简介。",
    externalSource: "未记录（外部光影）",
    localOnlyHint: "只能显示本地文件信息；哈希识别和线上更新还未接入。",
    onlineKind: "shader",
    idLabel: null,
    formatLabel: "格式",
    duplicateLabel: null,
  },
  texturepack: {
    kind: "texturepack",
    label: "材质包",
    title: "材质包管理",
    folder: "texturepacks",
    icon: PaletteOutlinedIcon,
    searchLabel: "搜索名称 / 文件名",
    empty: "这个版本还没有材质包。点「从本地安装」放一个进来。",
    noMatch: "没有匹配的材质包。",
    descriptionEmpty: "这个材质包没有写简介。",
    externalSource: "未记录（外部材质包）",
    localOnlyHint: "只能显示包内元数据；平台侧没有独立的材质包类型。",
    onlineKind: null,
    idLabel: null,
    formatLabel: "材质包格式",
    duplicateLabel: null,
  },
};

export function contentConfig(kind: ContentKind): ContentKindConfig {
  return CONTENT_KINDS[kind];
}

export function isContentKind(value: string): value is ContentKind {
  return (
    value === "mod" ||
    value === "resourcepack" ||
    value === "shader" ||
    value === "texturepack"
  );
}
