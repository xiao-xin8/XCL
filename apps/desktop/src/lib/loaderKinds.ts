import { useSyncExternalStore } from "react";
import { apiGet, peek } from "./api";
import type { LoaderKind, LoaderKindView, VersionKind } from "./types";

/**
 * 加载器类型的全局小 store。
 *
 * 为什么放在模块级而不是组件里：加载器下拉（`LoaderPicker`）和详情按钮文案
 * （`VersionDetail`）都要知道「Fabric / Forge / NeoForge 各叫什么」，而这两处
 * 不在同一棵子树的父子关系上。和 `splitterDrag.ts` 一样用 `useSyncExternalStore`
 * 广播，比把 label 一层层传下去干净。
 *
 * 类型清单由后端给（`loader_kinds`），这里只是缓存一份给两个地方共用。
 */

/** 后端没来得及回话时的兜底：至少 Fabric 的名字还在。 */
const FALLBACK: LoaderKindView[] = [{ kind: "fabric", label: "Fabric" }];

let kinds: LoaderKindView[] = peek<LoaderKindView[]>("loader_kinds") ?? FALLBACK;
const listeners = new Set<() => void>();

function emit() {
  listeners.forEach((listener) => listener());
}

function subscribe(listener: () => void): () => void {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

function getSnapshot(): LoaderKindView[] {
  return kinds;
}

// 模块被用到时拉一次就够：这份数据整个运行期不会变（改它等于改代码）。
// 失败不重试也不弹错：已经用兜底列表顶着，看不出差别。
void apiGet<LoaderKindView[]>("loader_kinds")
  .then((data) => {
    if (data.length === 0) return;
    kinds = data;
    emit();
  })
  .catch(() => undefined);

/** 当前可装的加载器类型（含展示名）。 */
export function useLoaderKinds(): LoaderKindView[] {
  return useSyncExternalStore(subscribe, getSnapshot, getSnapshot);
}

/**
 * 加载器类型 → 版本图标分类。
 *
 * 两套命名不一致：加载器是 `neoforge`（全小写），图标分类是 `neoForge`（驼峰）。
 * 直接拿前者查图标会查不到、回落到红石块 —— NeoForge 的狐狸就是这么丢的。
 */
export function loaderIconKind(kind: LoaderKind | string): VersionKind {
  return kind === "neoforge" ? "neoForge" : (kind as VersionKind);
}

/** 某个加载器类型的展示名；认不出来就原样显示，不编名字。 */
export function loaderLabel(kinds: LoaderKindView[], kind: LoaderKind | string): string {
  return kinds.find((item) => item.kind === kind)?.label ?? kind;
}
