import AutoAwesomeOutlinedIcon from "@mui/icons-material/AutoAwesomeOutlined";
import DescriptionOutlinedIcon from "@mui/icons-material/DescriptionOutlined";
import ExtensionOutlinedIcon from "@mui/icons-material/ExtensionOutlined";
import FolderOpenOutlinedIcon from "@mui/icons-material/FolderOpenOutlined";
import FolderOutlinedIcon from "@mui/icons-material/FolderOutlined";
import PaletteOutlinedIcon from "@mui/icons-material/PaletteOutlined";
import SaveOutlinedIcon from "@mui/icons-material/SaveOutlined";
import type { ContentKind, LocalVersion } from "./types";

/**
 * 版本目录的几个入口。
 *
 * `key` 是后端 `VersionFolder` 的序列化值，改这里要跟着改后端枚举。
 * 带 `page` 的条目不再打开资源管理器，而是进入整页管理器；
 * 模组 / 资源包 / 光影 / 材质包都在管理器里保留「打开文件夹」。
 */
export type VersionFolderKey =
  | "version"
  | "game"
  | "saves"
  | "mods"
  | "resourcepacks"
  | "shaderpacks"
  | "texturepacks"
  | "logs";

/** 版本管理器的整页子页 key。 */
export type VersionManagerKey = ContentKind;

export type VersionFolderEntry = {
  key: VersionFolderKey;
  label: string;
  icon: typeof FolderOutlinedIcon;
  /** 有值时点按钮进整页管理器，而不是打开文件夹 */
  page?: VersionManagerKey;
};

export const VERSION_FOLDERS: readonly VersionFolderEntry[] = [
  { key: "version", label: "版本目录", icon: FolderOutlinedIcon },
  { key: "game", label: "游戏目录", icon: FolderOpenOutlinedIcon },
  { key: "saves", label: "存档", icon: SaveOutlinedIcon },
  { key: "mods", label: "模组", icon: ExtensionOutlinedIcon, page: "mod" },
  {
    key: "resourcepacks",
    label: "资源包",
    icon: PaletteOutlinedIcon,
    page: "resourcepack",
  },
  {
    key: "shaderpacks",
    label: "光影",
    icon: AutoAwesomeOutlinedIcon,
    page: "shader",
  },
  {
    key: "texturepacks",
    label: "材质包",
    icon: PaletteOutlinedIcon,
    page: "texturepack",
  },
  { key: "logs", label: "日志", icon: DescriptionOutlinedIcon },
];

/** 只给版本真正支持的文件夹出入口：纯净版没有 Mod / 光影，老版本没有资源包 / logs。 */
export function foldersFor(version: LocalVersion, isolated: boolean): VersionFolderEntry[] {
  return VERSION_FOLDERS.filter((item) => {
    // 隔离后游戏目录就是版本目录，两个按钮指同一个地方只会让人以为点错了
    if (item.key === "game") return !isolated;
    if (item.key === "mods") return version.modable;
    if (item.key === "resourcepacks") return !version.texturePacks;
    // 光影要模组加载器才能装 Iris / OptiFine；和下载页的候选规则保持一致
    if (item.key === "shaderpacks") return version.modable;
    if (item.key === "texturepacks") return version.texturePacks;
    if (item.key === "logs") return version.hasLogs;
    return true;
  });
}
