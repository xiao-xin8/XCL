import DescriptionOutlinedIcon from "@mui/icons-material/DescriptionOutlined";
import ExtensionOutlinedIcon from "@mui/icons-material/ExtensionOutlined";
import FolderOpenOutlinedIcon from "@mui/icons-material/FolderOpenOutlined";
import FolderOutlinedIcon from "@mui/icons-material/FolderOutlined";
import PaletteOutlinedIcon from "@mui/icons-material/PaletteOutlined";
import SaveOutlinedIcon from "@mui/icons-material/SaveOutlined";
import type { LocalVersion } from "./types";

/**
 * 版本目录的几个入口。
 *
 * `key` 是后端 `VersionFolder` 的序列化值，改这里要跟着改后端枚举。
 * 启动页详情和版本设置弹窗共用这一份，避免两处慢慢走偏。
 */
export const VERSION_FOLDERS = [
  { key: "version", label: "版本目录", icon: FolderOutlinedIcon },
  { key: "game", label: "游戏目录", icon: FolderOpenOutlinedIcon },
  { key: "saves", label: "存档", icon: SaveOutlinedIcon },
  { key: "mods", label: "模组", icon: ExtensionOutlinedIcon },
  { key: "resourcepacks", label: "资源包", icon: PaletteOutlinedIcon },
  { key: "texturepacks", label: "材质包", icon: PaletteOutlinedIcon },
  { key: "logs", label: "日志", icon: DescriptionOutlinedIcon },
] as const;

export type VersionFolderKey = (typeof VERSION_FOLDERS)[number]["key"];

/** 只给版本真正支持的文件夹出入口：纯净版没有 Mod，老版本没有资源包 / logs。 */
export function foldersFor(version: LocalVersion, isolated: boolean) {
  return VERSION_FOLDERS.filter((item) => {
    // 隔离后游戏目录就是版本目录，两个按钮指同一个地方只会让人以为点错了
    if (item.key === "game") return !isolated;
    if (item.key === "mods") return version.modable;
    if (item.key === "resourcepacks") return !version.texturePacks;
    if (item.key === "texturepacks") return version.texturePacks;
    if (item.key === "logs") return version.hasLogs;
    return true;
  });
}
