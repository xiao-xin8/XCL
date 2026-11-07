/**
 * 页面动画参数。
 */
export const DURATION = {
  /** 块进入：淡入 100ms，相邻块间隔 25ms */
  enterFade: 100,
  enterStagger: 25,
  /** 普通块从上方落下 350ms；左栏从左侧滑入 300ms */
  enterMove: 350,
  enterMoveLeft: 300,
  /** 普通块退出 70ms；左栏退出 50ms */
  exit: 70,
  exitLeft: 50,
  exitStagger: 15,
  /** 左栏子项退场延迟 `70 / Count * Id` 的总跨度 */
  leftExitSpread: 70,
  /** 旧内容播完退场后换页（110ms） */
  swap: 110,
} as const;

export const EASE = {
  outFluent: "cubic-bezier(0.22, 1, 0.36, 1)",
  /** 带回弹的收尾 */
  outBack: "cubic-bezier(0.34, 1.56, 0.64, 1)",
  /**
   * 起步慢、收尾快。
   * 只用在「短距离 + 短时长」的退出上（标题栏旧区往右让位 12px / 150ms），
   * 位移方向是"被推走"，用 outFluent 会看着像自己冲出去。
   */
  inFluent: "cubic-bezier(0.64, 0, 0.78, 0)",
} as const;

/**
 * 标题栏在「主页面 ↔ 子页面」之间交叉换位。
 *
 * 对应主窗口标题栏的第一层动画：
 * - 旧区 opacity→0 + X 让开，150ms（`inFluent(weak)`）；
 * - 新区 opacity→1，150ms，delay 200；
 * - 新区位移 350ms `outBack`，从 `-18px` 回弹到位（主区返回时从 `+12px` 回来）。
 *
 * 200ms 的等待是刻意的：两段不重叠，视觉上才有"旧的先让位、新的再进来"的层次。
 */
export const TITLE_SWAP = {
  fade: 150,
  delay: 200,
  move: 350,
  /** 子页标题的起点偏移（从左侧滑入） */
  fromInner: 18,
  /** 主页品牌区被推开的距离（往右） */
  outMain: 12,
} as const;

/**
 * 加载环的闸门：
 * - `showDelay`：进来先等 400ms，这段时间内加载完就根本不显示环，
 *   避免"闪一下转圈"；
 * - `minStay`：环一旦出现就至少停 400ms（`LoaderStayForce`），
 *   避免"转圈一闪而过"。
 *
 * 两个数都是实测出来的，不是拍的。
 */
export const LOADER_GATE = {
  showDelay: 400,
  minStay: 400,
} as const;

/** 3D 皮肤预览：模型真正渲染出来后慢慢淡入，避免「一下跳出来」。 */
export const SKIN_FADE = {
  duration: 480,
  easing: EASE.outFluent,
} as const;

/**
 * 面板里「转圈换成内容」的淡入。
 *
 * 详情面板的清单是异步来的：加载完直接把内容贴上去会「啪」一下跳出来。
 * 从 0 关键帧起播（见 ANIMATION.md 第 6 节：首次显示的元素不能用 CSS
 * transition 做淡入，插入和揭示可能落在同一帧，浏览器补不上间）。
 */
export const CONTENT_FADE = {
  duration: 180,
  easing: EASE.outFluent,
} as const;

/**
 * 悬浮球（下载队列入口）：从 0.4 缩放弹进来、缩回 0.6 退场。
 * 跟页面块一样只动 `transform` / `opacity`。
 */
export const FAB_MOTION = {
  enterDuration: 280,
  enterFade: 140,
  enterScaleFrom: 0.4,
  exitDuration: 150,
  exitScaleTo: 0.6,
} as const;

/** 「正在启动」的呼吸：只动透明度，不转圈（状态块里不许出现 spinner）。 */
export const PULSE = {
  duration: 1400,
  /** 最暗到 0.45，别闪成看不见 */
  minOpacity: 0.45,
} as const;

/**
 * 卡片（下载队列里的任务卡）：
 * - 状态从「跑着」变成「结束」时，进度区块用 `grid-template-rows: 1fr→0fr` 收起来，
 *   高度变化跟着过渡，不是啪一下矮半截；
 * - 卡片退场向右滑出并淡出，滑完才真正卸载。
 */
export const CARD_MOTION = {
  /** 收起 / 展开进度区块 */
  collapse: 220,
  /**
   * 折叠列表的展开时长上限。
   *
   * MUI `Collapse` 的 `timeout="auto"` 按时长公式算，一个 1.3k 像素高的分组
   * 要 ~470ms、6k 像素要 ~900ms —— 大分组「慢」主要慢在这里。
   * 保留「小组干脆、大组从容」的曲线，但封顶，避免长尾等待。
   */
  collapseEnterMax: 420,
  /** 太小的分组也别快到像闪一下 */
  collapseEnterMin: 160,
  /** 收起统一短一点：已经决定不要了，不需要再演一遍 */
  collapseExit: 190,
  /** 退场：向右滑走的距离与耗时 */
  exitDuration: 210,
  exitDistance: 56,
  exitScale: 0.96,
} as const;

export const DISTANCE = {
  /** 普通块从 -16px 落回原位，退场再上移 6px */
  enter: 16,
  exit: 6,
  /** 左栏从 -25px 滑回原位，退场再向左收 6px */
  enterLeft: 25,
  exitLeft: 6,
} as const;

/**
 * 页面稳定后多久才去拉「活数据」。
 *
 * 不是动画时长，是给入场、配置到达、卡片补播留的余量：内存条这类会改布局的
 * 东西等这一段过去再出现，条子就能从空槽一次冲到实际宽度，不撞在动画里。
 */
export const SETTLE = {
  liveDataReveal: 1500,
} as const;
