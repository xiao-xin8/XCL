import type { SaveView } from "./types";

/**
 * 存档相关的固定映射和展示规则。
 *
 * 列表页和详情页都要把后端的枚举翻成中文，放这里一份，别各写一套。
 */

export const GAME_MODE_LABEL: Record<string, string> = {
  survival: "生存",
  creative: "创造",
  adventure: "冒险",
  spectator: "旁观",
};

export const DIFFICULTY_LABEL: Record<string, string> = {
  peaceful: "和平",
  easy: "简单",
  normal: "普通",
  hard: "困难",
};

/** 游戏版本后面跟着的「（快照）」，非快照就不加。 */
export function versionLabel(item: SaveView): string {
  if (!item.gameVersion) return "未识别";
  return item.snapshot ? `${item.gameVersion}（快照）` : item.gameVersion;
}
