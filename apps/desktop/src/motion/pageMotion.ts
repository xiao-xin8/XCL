import { DISTANCE, DURATION, EASE } from "./tokens";
import { animationsEnabled } from "../lib/appearance";

/**
 * 页面动画的作用对象是「块」，不是整页：
 * 只对卡片 / 列表项 / 提示条这类控件逐个做动画，页面背景和容器不动。
 *
 * - `data-page-block`：普通内容块，从上方落下。
 * - `data-page-block="left"`：侧边栏 / 左栏整体，从左侧滑入。
 * - `data-page-stagger`：容器，每个直接子项各算一块，纵向逐项入场。
 * - `data-page-stagger="left"`：容器，每个直接子项按递减错峰入场。
 *
 * 没有标记时把卡片（Paper）和提示条（Alert）当作普通内容块。
 */
const BLOCK_SELECTOR = ".MuiPaper-root, .MuiAlert-root";
const BLOCK_ATTR = "data-page-block";
const STAGGER_ATTR = "data-page-stagger";

type Axis = "x" | "y";

type Unit = {
  el: HTMLElement;
  axis: Axis;
  /** 来自 data-page-stagger 的子项：左栏按递减错峰，普通块不区分。 */
  item: boolean;
};

function prefersReducedMotion(): boolean {
  // 设置页的「动画强度」优先：强制关闭 / 强制开启 / 跟随系统
  return !animationsEnabled();
}

function isRendered(el: HTMLElement): boolean {
  const style = getComputedStyle(el);
  return (
    style.display !== "none" &&
    style.visibility !== "hidden" &&
    style.opacity !== "0"
  );
}

function axisOf(el: HTMLElement, attr: string): Axis {
  return el.getAttribute(attr) === "left" ? "x" : "y";
}

export function collectUnits(root: HTMLElement): Unit[] {
  const units: Unit[] = [];

  /**
   * 根节点自己带 `data-page-stagger` 时也要认。
   *
   * `playEnter(列表容器)` 是最常见的写法（列表数据回来时补一次入场），
   * 只从子节点里找标记的话，这种调用会一个块都收不到 —— 列表静悄悄地出现。
   */
  if (root.hasAttribute(STAGGER_ATTR)) {
    const axis = axisOf(root, STAGGER_ATTR);
    for (const item of Array.from(root.children)) {
      if (item instanceof HTMLElement && isRendered(item)) {
        units.push({ el: item, axis, item: true });
      }
    }
    return units;
  }

  const walk = (el: Element) => {
    for (const child of Array.from(el.children)) {
      if (!(child instanceof HTMLElement) || !isRendered(child)) continue;

      if (child.hasAttribute(STAGGER_ATTR)) {
        const axis = axisOf(child, STAGGER_ATTR);
        for (const item of Array.from(child.children)) {
          if (item instanceof HTMLElement && isRendered(item)) {
            units.push({ el: item, axis, item: true });
          }
        }
        continue;
      }

      if (child.hasAttribute(BLOCK_ATTR) || child.matches(BLOCK_SELECTOR)) {
        units.push({
          el: child,
          axis: child.hasAttribute(BLOCK_ATTR) ? axisOf(child, BLOCK_ATTR) : "y",
          item: false,
        });
        continue;
      }

      walk(child);
    }
  };

  walk(root);
  return units;
}

function cancelOnFinish(animation: Animation) {
  animation.finished.then(() => animation.cancel()).catch(() => undefined);
}

/** 上一次给某个元素播入场的时间，用来挡住重复播放。 */
const lastEnterAt = new WeakMap<HTMLElement, number>();
const ENTER_DEDUPE_MS = 80;

/**
 * 清掉元素上还挂着的旧动画。
 *
 * 退场动画用 `fill: "both"` 留在元素上（保持透明，等换页把旧节点卸掉）。
 * 在 110ms 的换页窗口里又切回来时，同一个元素会被再挂一次入场动画；
 * 入场播完后 `cancelOnFinish` 把它取消，露出来的却是那条仍在 fill 的退场动画 ——
 * 表现就是「快速切回来没有动画」，重一点的甚至直接一片空白。
 */
function clearAnimations(el: HTMLElement) {
  for (const animation of el.getAnimations()) animation.cancel();
}

function moveKeyframes(axis: Axis, distance: number): Keyframe[] {
  const property = axis === "x" ? "translateX" : "translateY";
  return [
    { transform: `${property}(${-distance}px)` },
    { transform: `${property}(0)` },
  ];
}

/**
 * 入场延迟（每块一个 Delay，累加得出）：
 * - 纵向：每块 +25ms，普通块和逐项子项同一节奏；
 * - 左栏整体：0，不占左栏子项的位置；
 * - 左栏子项：`Delay += max(15 - Id, 7) * 2`，即 0 / 30 / 58 / 84 …（间隔递减，最少 14）。
 */
function enterDelays(units: Unit[]): number[] {
  let nextY = 0;
  let nextX = 0;
  let xIndex = 0;

  return units.map((unit) => {
    if (unit.axis === "y") {
      const delay = nextY;
      nextY += DURATION.enterStagger;
      return delay;
    }
    if (!unit.item) return 0;

    const delay = nextX;
    nextX += Math.max(15 - xIndex, 7) * 2;
    xIndex += 1;
    return delay;
  });
}

/**
 * 退场延迟：
 * - 纵向：每块 +15ms；
 * - 左栏整体：0；
 * - 左栏子项：`70 / Count * Id`，在 70ms 内均匀铺开。
 */
function exitDelays(units: Unit[]): number[] {
  const xCount = units.filter((unit) => unit.axis === "x" && unit.item).length;
  let nextY = 0;
  let xIndex = 0;

  return units.map((unit) => {
    if (unit.axis === "y") {
      const delay = nextY;
      nextY += DURATION.exitStagger;
      return delay;
    }
    if (!unit.item) return 0;

    const delay = xCount > 0 ? (DURATION.leftExitSpread / xCount) * xIndex : 0;
    xIndex += 1;
    return delay;
  });
}

/**
 * 逐块入场：
 * - 普通块从上方 16px 落回原位，位移 350ms（outBack 回弹）；
 * - 左栏从左侧 25px 滑回原位，位移 300ms；
 * - 都是淡入 100ms。
 */
export function playEnter(root: HTMLElement): () => void {
  if (prefersReducedMotion()) return () => undefined;
  return animateEnter(collectUnits(root));
}

/**
 * 只给指定的几个元素播入场。
 *
 * 列表「追加下一页」时用：已经渲染好的条目不能再挂一遍动画（重播会让整列重跳），
 * 只让新进来的那几条依次落下。
 */
export function playEnterElements(elements: HTMLElement[]): () => void {
  const units: Unit[] = elements
    .filter(isRendered)
    .map((el) => ({ el, axis: "y" as Axis, item: true }));
  return animateEnter(units);
}

function animateEnter(units: Unit[]): () => void {
  if (prefersReducedMotion() || units.length === 0) return () => undefined;

  /*
   * 同一次「内容出现」可能有两个调用方：页面自己补一次 `playEnter`，
   * `PageTransition` 又在块出现后补一次。两边都播会把卡片重新弹一遍，
   * 看着像抖了一下。80ms 内播过的元素直接跳过。
   */
  const now = performance.now();
  const fresh = units.filter(({ el }) => {
    const last = lastEnterAt.get(el);
    return last == null || now - last > ENTER_DEDUPE_MS;
  });
  if (fresh.length === 0) return () => undefined;
  for (const { el } of fresh) lastEnterAt.set(el, now);

  const delays = enterDelays(fresh);
  const animations: Animation[] = [];

  fresh.forEach(({ el, axis }, index) => {
    const delay = delays[index] ?? 0;
    const isLeft = axis === "x";

    clearAnimations(el);

    const fade = el.animate([{ opacity: 0 }, { opacity: 1 }], {
      duration: DURATION.enterFade,
      delay,
      easing: EASE.outFluent,
      fill: "both",
    });
    const move = el.animate(
      moveKeyframes(axis, isLeft ? DISTANCE.enterLeft : DISTANCE.enter),
      {
        duration: isLeft ? DURATION.enterMoveLeft : DURATION.enterMove,
        delay,
        easing: EASE.outBack,
        fill: "both",
      },
    );

    cancelOnFinish(fade);
    cancelOnFinish(move);
    animations.push(fade, move);
  });

  return () => animations.forEach((animation) => animation.cancel());
}

/**
 * 所有块同时退场（不做逐条错峰）。
 *
 * 用在「同一个页面的两套列表互换」（详情页的自动模式 ↔ 手动模式）：
 * 那里换的是内容而不是页面，逐条 70ms / +15ms 会让 20 条拖到 355ms，
 * 看着像在等什么。这里统一 70ms 一起淡出，紧接着新列表 `playEnter` 逐条落下。
 *
 * 换页仍然用 `playExit`（逐块错开是页面切换语义，别混用）。
 */
export function playExitAll(root: HTMLElement): () => void {
  if (prefersReducedMotion()) return () => undefined;

  const animations = collectUnits(root).map(({ el, axis }) => {
    const property = axis === "x" ? "translateX" : "translateY";
    const distance = axis === "x" ? DISTANCE.exitLeft : DISTANCE.exit;

    clearAnimations(el);

    return el.animate(
      [
        { opacity: 1, transform: `${property}(0)` },
        { opacity: 0, transform: `${property}(${-distance}px)` },
      ],
      {
        duration: axis === "x" ? DURATION.exitLeft : DURATION.exit,
        easing: "linear",
        fill: "both",
      },
    );
  });

  return () => animations.forEach((animation) => animation.cancel());
}

/**
 * 逐块退场：
 * - 普通块 70ms 淡出并上移 6px；
 * - 左栏 50ms 淡出并向左收 6px；
 * - 延迟见 exitDelays。结束后保持透明，直到换页把旧节点卸掉。
 */
export function playExit(root: HTMLElement): () => void {
  if (prefersReducedMotion()) return () => undefined;

  const units = collectUnits(root);
  const delays = exitDelays(units);

  const animations = units.map(({ el, axis }, index) => {
    const isLeft = axis === "x";
    const property = isLeft ? "translateX" : "translateY";

    clearAnimations(el);

    return el.animate(
      [
        { opacity: 1, transform: `${property}(0)` },
        {
          opacity: 0,
          transform: `${property}(${
            -(isLeft ? DISTANCE.exitLeft : DISTANCE.exit)
          }px)`,
        },
      ],
      {
        duration: isLeft ? DURATION.exitLeft : DURATION.exit,
        delay: delays[index] ?? 0,
        easing: "linear",
        fill: "both",
      },
    );
  });

  return () => animations.forEach((animation) => animation.cancel());
}
