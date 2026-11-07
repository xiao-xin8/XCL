/**
 * 「当前是否处于子页面」的纯 store（没有 React、没有 import）。
 *
 * 拆出来是为了能单测：Node 的类型剥离只剥类型、不解析依赖，
 * 带 `react` 运行时导入的模块在 `node --test` 里根本跑不起来。
 * React 那一层见 `nestedNav.ts`。
 *
 * 栈语义：主页面 → 详情页 → 依赖 Mod 详情页 → … 可以一层层压进去，
 * 返回键永远执行栈顶那条；某个子页卸载时只把自己那层摘掉，
 * 下面的页面（连同它的返回键）自动恢复。
 * 早先是"后来者覆盖前者"，做依赖跳转时下层一卸载就把整栈清空了。
 */

export type NestedEntry = {
  /** 标题栏上显示的项目名 */
  title: string;
  /** 「← 返回」按下去做什么 */
  onBack: () => void;
  /** 归属标记：注销时只有自己的 token 才清得掉自己的注册 */
  token: symbol;
};

let stack: NestedEntry[] = [];
const listeners = new Set<() => void>();

function emit(): void {
  listeners.forEach((listener) => listener());
}

export function subscribeNestedPage(listener: () => void): () => void {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

/** 只读快照：栈顶。引用稳定（只有注册 / 注销时才换），可以直接喂给 `useSyncExternalStore`。 */
export function getNestedPage(): NestedEntry | null {
  return stack.length > 0 ? stack[stack.length - 1] : null;
}

export function registerNestedPage(
  token: symbol,
  title: string,
  onBack: () => void,
): void {
  const index = stack.findIndex((entry) => entry.token === token);
  if (index >= 0) {
    // 同一层只改标题时不必换引用，省一次全树重渲染
    if (stack[index].title === title) return;
    stack[index] = { token, title, onBack };
    emit();
    return;
  }
  stack.push({ token, title, onBack });
  emit();
}

export function unregisterNestedPage(token: symbol): void {
  const index = stack.findIndex((entry) => entry.token === token);
  if (index < 0) return;
  stack.splice(index, 1);
  emit();
}

/** 测试用：清空。 */
export function resetNestedPage(): void {
  if (stack.length === 0) return;
  stack = [];
  emit();
}
