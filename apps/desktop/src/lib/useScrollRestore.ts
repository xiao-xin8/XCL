import { useCallback, useLayoutEffect, useRef } from "react";

/**
 * 列表滚动位置的记忆与恢复。
 *
 * 用在「进详情页 → 返回列表」：返回时列表会重新挂载（旧节点已经卸载了），
 * 不记 `scrollTop` 就会跳回顶部。存的位置放在模块级 Map，key 由调用方给
 * （比如 `mod:<source>:<query>`），这样不同搜索词各自记各自的。
 *
 * 必须在内容已经渲染出来之后恢复：列表高度为 0 时设 `scrollTop` 会被浏览器
 * 夹成 0。所以默认在 `useLayoutEffect`（挂载、布局完成、未绘制）里恢复；
 * 数据是异步来的情况，调用方拿到数据后再调一次 `restore()`。
 */
const positions = new Map<string, number>();

export function useScrollRestore(id: string) {
  const ref = useRef<HTMLDivElement | null>(null);

  /** 离开之前调：把当前滚动位置存下来。 */
  const save = useCallback(() => {
    const el = ref.current;
    if (el) positions.set(id, el.scrollTop);
  }, [id]);

  /** 手动恢复（数据晚到时用）。 */
  const restore = useCallback(() => {
    const el = ref.current;
    const top = positions.get(id);
    if (el && top !== undefined) el.scrollTop = top;
  }, [id]);

  useLayoutEffect(() => {
    restore();
  }, [restore]);

  return { ref, save, restore };
}

/** 丢掉某个列表的记忆（比如它被彻底删掉了）。 */
export function forgetScroll(id: string): void {
  positions.delete(id);
}
