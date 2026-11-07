import { useEffect, useRef, useSyncExternalStore } from "react";
import {
  getNestedPage,
  registerNestedPage,
  subscribeNestedPage,
  unregisterNestedPage,
} from "./nestedNavStore";

/**
 * 子页面导航状态（标题栏的返回键靠它）。
 *
 * 为什么需要它：详情页是页面级导航（不是弹窗），返回键在标题栏里，
 * 而标题栏在组件树顶端、和详情页隔着好几层。用 props 传要穿五层，
 * 用 context 又要处理 Provider 嵌套顺序 —— 所以和 `appearance` /
 * `splitterDrag` 一样走模块级广播。
 *
 * 状态本体在 `nestedNavStore.ts`（纯 store，可单测），这里只有 React 那一层。
 */

export type NestedPage = {
  /** 标题栏上显示的项目名 */
  title: string;
  /** 「← 返回」按下去做什么 */
  onBack: () => void;
};

/** 标题栏订阅这个：`null` = 主页面，非空 = 子页面。 */
export function useNestedPage(): NestedPage | null {
  return useSyncExternalStore(subscribeNestedPage, getNestedPage, getNestedPage);
}

/**
 * 子页调用：挂载即注册，`title` 传 `null` 表示"退回主页面"。
 *
 * `onBack` 每次渲染都可能是新函数，所以用 ref 兜住、注册时包一层稳定壳子 ——
 * 否则每次 render 都会重新注册，标题栏跟着白重渲染。
 */
export function useRegisterNestedPage(
  title: string | null,
  onBack: () => void,
): void {
  const token = useRef<symbol>(Symbol("nested-page"));
  const back = useRef(onBack);
  back.current = onBack;

  useEffect(() => {
    if (title === null) {
      unregisterNestedPage(token.current);
      return;
    }
    registerNestedPage(token.current, title, () => back.current());
  }, [title]);

  useEffect(() => {
    const mine = token.current;
    return () => unregisterNestedPage(mine);
  }, []);
}
