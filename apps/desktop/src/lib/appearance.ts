import { useSyncExternalStore } from "react";
import type { AccentKey } from "../theme";
import type { AnimationPreference, ThemePreference } from "./types";

/**
 * 外观偏好的运行时副本。
 *
 * 权威来源是 `config.toml`（`ui.themeMode` / `ui.accent` / `ui.animation`），
 * 但主题必须挂在最外层 `ThemeProvider` 上，设置页则在很深的子树里 ——
 * 用 props 传五层不现实，用 context 又会把 Provider 套进 Provider 的顺序问题。
 * 所以这里用模块级 store（和 `splitterDrag` 同一套路）：设置页改完立刻广播，
 * 顶层订阅后重建主题。
 */

export type Appearance = {
  themeMode: ThemePreference;
  accent: AccentKey;
  /** 自定义强调色（`#rrggbb`）。有值时覆盖 `accent` 的预设色 */
  accentCustom: string | null;
  /** 界面不透明度 0–100（卡片、侧栏、弹出层统一） */
  surfaceOpacity: number;
  /** 毛玻璃模糊半径（px）0–20 */
  surfaceBlur: number;
  /** 自定义背景图（data URL）；null = 纯色背景 */
  backgroundImage: string | null;
  /** 背景图蒙版不透明度 0–100 */
  backgroundOverlay: number;
  /** 背景图自身模糊半径 0–20 px */
  backgroundBlur: number;
  animation: AnimationPreference;
};

const DEFAULTS: Appearance = {
  themeMode: "system",
  accent: "yellow",
  accentCustom: null,
  surfaceOpacity: 100,
  surfaceBlur: 0,
  backgroundImage: null,
  backgroundOverlay: 60,
  backgroundBlur: 0,
  animation: "system",
};

let current: Appearance = { ...DEFAULTS };
const listeners = new Set<() => void>();

export function getAppearance(): Appearance {
  return current;
}

export function setAppearance(next: Partial<Appearance>): void {
  const merged = { ...current, ...next };
  if (
    merged.themeMode === current.themeMode &&
    merged.accent === current.accent &&
    merged.accentCustom === current.accentCustom &&
    merged.surfaceOpacity === current.surfaceOpacity &&
    merged.surfaceBlur === current.surfaceBlur &&
    merged.backgroundImage === current.backgroundImage &&
    merged.backgroundOverlay === current.backgroundOverlay &&
    merged.backgroundBlur === current.backgroundBlur &&
    merged.animation === current.animation
  ) {
    return;
  }
  current = merged;
  for (const listener of listeners) listener();
}

function subscribe(listener: () => void): () => void {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

export function useAppearance(): Appearance {
  return useSyncExternalStore(subscribe, getAppearance, getAppearance);
}

/** 当前是否该播动画：强制开关优先，否则看系统的「减少动态效果」。 */
export function animationsEnabled(): boolean {
  if (current.animation === "off") return false;
  if (current.animation === "on") return true;
  return !window.matchMedia("(prefers-reduced-motion: reduce)").matches;
}
