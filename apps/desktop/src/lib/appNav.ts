import type { NavKey } from "../components/TitleBar";

/**
 * 从任意深层组件请求切换顶部页签。
 *
 * 为什么不用 context：`App` 里的 `nav` 是唯一真源，但下载页在 `PageTransition`
 * 下面好几层，为这一个动作铺一层 Provider 不值当。这里只做「一个订阅者」的
 * 极简转发 —— 页签只有一个，本来也只该有一个 App 在监听。
 *
 * 切页动画由 `PageTransition` 负责（它认 `nav` 的变化），这里只管把意图递上去。
 */
type Listener = (nav: NavKey) => void;

let listener: Listener | null = null;

/** 请求切到某个页签。没有 App 在监听时静默忽略（组件卸载后触发不会崩）。 */
export function navigateTo(nav: NavKey): void {
  listener?.(nav);
}

/** `App` 挂载时绑定。返回解绑函数。 */
export function bindNavigate(fn: Listener): () => void {
  listener = fn;
  return () => {
    if (listener === fn) listener = null;
  };
}
