import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useRef,
  useState,
  type PointerEvent as ReactPointerEvent,
  type ReactNode,
} from "react";
import { Box, IconButton, Typography, alpha, useTheme } from "@mui/material";
import type { Theme } from "@mui/material/styles";
import CloseIcon from "@mui/icons-material/Close";

/**
 * 全局轻提示。
 *
 * 形态：右上角一组长条卡片（小圆角，不是胶囊），左边关闭键，底部一条倒计时进度。
 *
 * 交互清单（每一条都是需求）：
 * - 多条同时堆叠显示，新来的插在最上面，不是排队一条一条播
 * - 从右侧滑入、向右侧滑出；新增 / 移除时高度带过渡，下面的卡片平滑让位
 * - 底部进度条跑完自动收起；鼠标停在上面时暂停，方便读完
 * - 可以按住往右甩出去（超过阈值就关，不到就弹回来）
 * - 内容不限行数，长文案自然换行，卡片自己长高
 * - 状态色很克制：底色永远是主题色，状态色只在它上面叠一层几乎看不见的
 *   底纹（外加底部进度条）；不做彩色描边 —— 满屏绿边黄边看着像警告灯，不是提示
 */

export type ToastSeverity = "success" | "info" | "warning" | "error";

type ToastItem = {
  id: number;
  message: string;
  severity: ToastSeverity;
};

type ShowToast = (message: string, severity?: ToastSeverity) => void;

const ToastContext = createContext<ShowToast>(() => {});

/** 任何组件里 `const toast = useToast(); toast("干完了", "success")` 即可。 */
export function useToast(): ShowToast {
  return useContext(ToastContext);
}

/** 存活时长。错误多留一会儿，往往要读完才知道出了什么事。 */
const HIDE_MS: Record<ToastSeverity, number> = {
  success: 3200,
  info: 3200,
  warning: 4200,
  error: 6000,
};

/** 同时最多堆几条。再多了会糊住右上角那片区域。 */
const MAX_VISIBLE = 4;

const ENTER_MS = 320;
const EXIT_MS = 200;
/** 高度展开 / 收起的时长，跟着卡片进出一起来。 */
const COLLAPSE_MS = 280;
/** 往右拖多少像素算「甩掉」。 */
const DISMISS_PX = 72;
const CARD_WIDTH = 360;
/** 标题栏 52 + 一点缝。 */
const TOP_OFFSET = 60;
/** 滑出时额外多走一点，确保完全离开视口。 */
const OFFSCREEN_GAP = 48;

const ENTER_EASE = "cubic-bezier(0.16, 1, 0.3, 1)";
const EXIT_EASE = "cubic-bezier(0.4, 0, 1, 1)";

function severityColor(theme: Theme, severity: ToastSeverity): string {
  switch (severity) {
    case "success":
      return theme.palette.success.main;
    case "warning":
      return theme.palette.warning.main;
    case "error":
      return theme.palette.error.main;
    default:
      return theme.palette.info.main;
  }
}

type ToastCardProps = {
  item: ToastItem;
  /** 卡片自己滑走了，请求从列表里移除。 */
  onDismiss: (id: number) => void;
};

function ToastCard({ item, onDismiss }: ToastCardProps) {
  const theme = useTheme();
  const color = severityColor(theme, item.severity);
  const total = HIDE_MS[item.severity];

  /** 已经站定（入场动画的目标位）。 */
  const [entered, setEntered] = useState(false);
  /** 正在往右滑出。 */
  const [leaving, setLeaving] = useState(false);
  /** 跟手位移，向右为正。 */
  const [offset, setOffset] = useState(0);
  const [dragging, setDragging] = useState(false);
  /** hover 或拖动时暂停倒计时。 */
  const [paused, setPaused] = useState(false);

  const barRef = useRef<HTMLDivElement>(null);
  const remaining = useRef(total);
  const dragOrigin = useRef(0);
  const leavingRef = useRef(false);
  const exitTimer = useRef<number | undefined>(undefined);

  // 入场：等一帧再改 transform，否则浏览器会把初始值和目标值合并，动画不播
  useEffect(() => {
    const frame = requestAnimationFrame(() => {
      requestAnimationFrame(() => setEntered(true));
    });
    return () => cancelAnimationFrame(frame);
  }, []);

  const leave = useCallback(() => {
    // 用 ref 防重入：连点两次关闭键不能推两条退场
    if (leavingRef.current) return;
    leavingRef.current = true;
    setLeaving(true);
    setPaused(true);
    // 等滑出 + 高度收拢都播完再摘掉，否则下面的卡片会突然跳一下
    exitTimer.current = window.setTimeout(
      () => onDismiss(item.id),
      Math.max(EXIT_MS, COLLAPSE_MS) + 40,
    );
  }, [onDismiss, item.id]);

  useEffect(
    () => () => {
      if (exitTimer.current) window.clearTimeout(exitTimer.current);
    },
    [],
  );

  /**
   * 倒计时。
   *
   * 直接改 DOM 的 `scaleX` 而不是走 state —— 每帧 setState 会把整张卡片
   * 重渲染 60 次/秒，没必要。
   */
  useEffect(() => {
    // 等入场动画开始后再计时，否则进度条一出来就已经少了一截
    if (!entered || paused || leaving) return;

    let frame = 0;
    let last = performance.now();
    const tick = (now: number) => {
      remaining.current -= now - last;
      last = now;
      const ratio = Math.max(0, Math.min(1, remaining.current / total));
      if (barRef.current) barRef.current.style.transform = `scaleX(${ratio})`;
      if (remaining.current <= 0) {
        leave();
        return;
      }
      frame = requestAnimationFrame(tick);
    };
    frame = requestAnimationFrame(tick);
    return () => cancelAnimationFrame(frame);
  }, [entered, paused, leaving, total, leave]);

  // ---------- 拖动甩出 ----------

  const handlePointerDown = (event: ReactPointerEvent<HTMLDivElement>) => {
    if (leaving) return;
    // 只接管主键 / 单指
    if (event.button !== 0) return;
    dragOrigin.current = event.clientX;
    setDragging(true);
    setPaused(true);
    event.currentTarget.setPointerCapture(event.pointerId);
  };

  const handlePointerMove = (event: ReactPointerEvent<HTMLDivElement>) => {
    if (!dragging) return;
    // 只允许往右拖，往左拖不动（左边是内容的阅读方向）
    setOffset(Math.max(0, event.clientX - dragOrigin.current));
  };

  const endDrag = () => {
    if (!dragging) return;
    setDragging(false);
    if (offset > DISMISS_PX) {
      leave();
      return;
    }
    // 没甩够：弹回去，继续倒计时
    setOffset(0);
    setPaused(false);
  };

  const offscreen = !entered || leaving;
  const shift = offscreen ? `calc(100% + ${OFFSCREEN_GAP}px)` : `${offset}px`;
  const fade = leaving
    ? 0
    : !entered
      ? 0
      : dragging
        ? Math.max(0.4, 1 - offset / 280)
        : 1;

  return (
    /**
     * 外层只负责高度：`grid-template-rows` 从 `0fr` 到 `1fr` 是可过渡的，
     * 新卡片因此在顶部"长出来"，下面的卡片被平滑推开，而不是直接跳位。
     */
    <Box
      sx={{
        display: "grid",
        gridTemplateRows: offscreen ? "0fr" : "1fr",
        transition: `grid-template-rows ${COLLAPSE_MS}ms ${
          offscreen ? EXIT_EASE : ENTER_EASE
        }`,
        // 收拢时要把内容裁掉，否则会盖住下面的卡片
        overflow: "hidden",
      }}
    >
      {/* minHeight: 0 是 grid 折叠动画能生效的前提 */}
      <Box sx={{ minHeight: 0, pb: 1 }}>
        <Box
          role="status"
          onPointerDown={handlePointerDown}
          onPointerMove={handlePointerMove}
          onPointerUp={endDrag}
          onPointerCancel={endDrag}
          onMouseEnter={() => setPaused(true)}
          onMouseLeave={() => {
            // 拖动中不恢复计时，松手时统一处理
            if (!dragging) setPaused(false);
          }}
          sx={{
            position: "relative",
            display: "flex",
            alignItems: "flex-start",
            gap: 0.5,
            pl: 0.5,
            pr: 1.75,
            py: 1,
            // 父容器是 pointer-events: none，这里要把可点性还回来
            pointerEvents: "auto",
            // 长条：小圆角，没有彩色描边，状态色只留一层极淡的底纹
            borderRadius: 1,
            // 扁平：一层 1px 描边划清边界，不投影 —— 投影会在画面上糊出一圈灰边
            border: "1px solid",
            borderColor: "divider",
            // 浮层要保持可读：用不透明底色，不跟着「界面不透明度」变
            backgroundColor: "surface.solid",
            // 两层叠加，不替换底色：
            //   上层 = 状态色（极淡，只做"这件事成功了/失败了"的暗示）
            //   下层 = 主题色（让提示条始终带品牌色，而不是一张白纸）
            // CSS 多层背景里第一个在最上面，所以顺序是「状态色压主题色」。
            backgroundImage: [
              `linear-gradient(${alpha(color, 0.07)}, ${alpha(color, 0.07)})`,
              `linear-gradient(${alpha(
                theme.palette.primary.main,
                0.16,
              )}, ${alpha(theme.palette.primary.main, 0.16)})`,
            ].join(", "),
            overflow: "hidden",
            cursor: dragging ? "grabbing" : "grab",
            userSelect: "none",
            touchAction: "pan-y",
            transform: `translateX(${shift})`,
            opacity: fade,
            transition: dragging
              ? "none"
              : `transform ${offscreen ? EXIT_MS : ENTER_MS}ms ${
                  offscreen ? EXIT_EASE : ENTER_EASE
                }, opacity ${EXIT_MS}ms ease`,
          }}
        >
          <IconButton
            size="small"
            aria-label="关闭提示"
            onPointerDown={(event) => event.stopPropagation()}
            onClick={leave}
            sx={{
              flexShrink: 0,
              width: 26,
              height: 26,
              mt: 0.125,
              borderRadius: 1,
              color: "text.secondary",
              "&:hover": { color: "text.primary", bgcolor: alpha(color, 0.12) },
            }}
          >
            <CloseIcon sx={{ fontSize: 16 }} />
          </IconButton>

          <Typography
            sx={{
              flex: 1,
              minWidth: 0,
              fontSize: 13,
              lineHeight: 1.5,
              // 正文用主题文字色，状态只体现在进度条和底纹上
              color: "text.primary",
              // 不截断：文案多长就多高，卡片自己撑开
              whiteSpace: "pre-wrap",
              wordBreak: "break-word",
              py: 0.25,
            }}
          >
            {item.message}
          </Typography>

          {/* 倒计时进度：贴底，这是状态色唯一的存在感 */}
          <Box
            sx={{
              position: "absolute",
              left: 0,
              right: 0,
              bottom: 0,
              height: 2,
              bgcolor: alpha(color, 0.16),
              pointerEvents: "none",
            }}
          >
            <Box
              ref={barRef}
              sx={{
                width: "100%",
                height: "100%",
                bgcolor: color,
                transformOrigin: "left center",
                willChange: "transform",
              }}
            />
          </Box>
        </Box>
      </Box>
    </Box>
  );
}

export function ToastProvider({ children }: { children: ReactNode }) {
  const [queue, setQueue] = useState<ToastItem[]>([]);
  const seq = useRef(0);

  const show = useCallback<ShowToast>((message, severity = "info") => {
    seq.current += 1;
    const item: ToastItem = { id: seq.current, message, severity };
    setQueue((prev) => {
      // 新的插在最上面：右上角的关键位置优先给最新消息
      const next = [item, ...prev];
      // 超出容量就丢最旧的（队尾），不是丢掉新的
      return next.slice(0, MAX_VISIBLE);
    });
  }, []);

  const dismiss = useCallback((id: number) => {
    setQueue((prev) => prev.filter((entry) => entry.id !== id));
  }, []);

  return (
    <ToastContext.Provider value={show}>
      {children}
      <Box
        sx={{
          position: "fixed",
          top: TOP_OFFSET,
          right: 16,
          zIndex: (t) => t.zIndex.snackbar,
          display: "flex",
          flexDirection: "column",
          width: CARD_WIDTH,
          maxWidth: "calc(100vw - 32px)",
          // 容器本身（卡片之间的空档）不拦点击，只有卡片本体拦
          pointerEvents: "none",
        }}
      >
        {queue.map((item) => (
          <ToastCard key={item.id} item={item} onDismiss={dismiss} />
        ))}
      </Box>
    </ToastContext.Provider>
  );
}
