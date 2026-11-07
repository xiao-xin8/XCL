/**
 * 加载环的开关闸门（LoaderWait / LoaderStayForce 两种状态）。
 *
 * 两个目的：
 * 1. 不闪 —— 请求 400ms 内就回来，转圈根本不该出现；
 * 2. 不闪而过 —— 环一旦出现，至少停 400ms，否则"亮一下就没"比不显示还难看。
 *
 * 这里只放纯状态机（没有 DOM、没有计时器、没有 import），计时交给
 * `components/PageLoaderGate.tsx`。阈值由调用方传（`motion/tokens.ts` 的
 * `LOADER_GATE`），这样这个文件能被 Node 直接跑单测 ——
 * Node 的类型剥离不解析无扩展名的 import，带运行时依赖就跑不起来。
 */

export type GateLimits = {
  /** 进来先等多久再决定显示环 */
  showDelay: number;
  /** 环一旦出现至少停多久 */
  minStay: number;
};

export type GatePhase =
  /** 什么都没发生 */
  | "idle"
  /** 正在等 `showDelay`，还没决定要不要显示环 */
  | "waiting"
  /** 环正在显示（可能数据已经好了，在补 `minStay`） */
  | "showing";

export type GateState = {
  phase: GatePhase;
  /** 进入 waiting 的时刻（`performance.now()`） */
  waitingSince: number | null;
  /** 开始显示环的时刻 */
  shownSince: number | null;
};

export function idleGate(): GateState {
  return { phase: "idle", waitingSince: null, shownSince: null };
}

/** 现在该不该显示加载环。 */
export function gateShowsLoader(state: GateState): boolean {
  return state.phase === "showing";
}

/**
 * 走一步。幂等：同一时刻反复调用不会重复迁移。
 *
 * @param loading 数据是否还在加载
 * @param now 当前时刻（`performance.now()`）
 */
export function nextGate(
  state: GateState,
  loading: boolean,
  now: number,
  limits: GateLimits,
): GateState {
  if (loading) {
    if (state.phase === "idle") {
      return { phase: "waiting", waitingSince: now, shownSince: null };
    }
    if (state.phase === "waiting") {
      const since = state.waitingSince ?? now;
      if (now - since < limits.showDelay) return state;
      return { phase: "showing", waitingSince: since, shownSince: now };
    }
    // showing：数据还没好，继续显示
    return state;
  }

  // 数据已经好了
  if (state.phase === "waiting") {
    // 还没到 showDelay 就完成了 → 环从未出现，直接收摊
    return idleGate();
  }
  if (state.phase === "showing") {
    const since = state.shownSince ?? now;
    if (now - since < limits.minStay) return state;
    return idleGate();
  }
  return state;
}

/**
 * 距下一次「自动迁移」还有多少毫秒；`null` 表示不会再自己动，等外部事件。
 *
 * 组件拿它排一个 `setTimeout`：到点再调一次 `nextGate` 就能完成迁移。
 * 调用前要先 `nextGate` 过一遍 —— 剩余量为 0 说明迁移已经该发生并且
 * 已经发生过了，这里返回 `null`，不排 0ms 的空转计时器。
 */
export function gateNextTickIn(
  state: GateState,
  loading: boolean,
  now: number,
  limits: GateLimits,
): number | null {
  const remaining = (() => {
    if (loading && state.phase === "waiting") {
      return limits.showDelay - (now - (state.waitingSince ?? now));
    }
    if (!loading && state.phase === "showing") {
      return limits.minStay - (now - (state.shownSince ?? now));
    }
    return null;
  })();

  return remaining !== null && remaining > 0 ? remaining : null;
}
