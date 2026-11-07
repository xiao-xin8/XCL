import {
  Box,
  ButtonBase,
  Stack,
  Typography,
  alpha,
} from "@mui/material";
import ArrowBackIcon from "@mui/icons-material/ArrowBack";
import { useLayoutEffect, useRef } from "react";
import { animationsEnabled } from "../lib/appearance";
import { useNestedPage } from "../lib/nestedNav";
import { EASE, TITLE_SWAP } from "../motion/tokens";

/**
 * 标题栏左侧那块：主页面显示「图标 + XCL」，子页面显示「← 返回 + 项目名」。
 *
 * 动画对应主窗口标题栏的第一层：
 * - 旧的一层 opacity→0 并让开 150ms；
 * - 新的一层 opacity→1 也是 150ms，但 delay 200 —— 两段不重叠，
 *   才有「旧的先让位、新的再进来」的层次；
 * - 新的一层位移 350ms `outBack`，从 -18px（子页标题）或 +12px（品牌区）回弹到位。
 *
 * 两层用 grid 叠在同一格里，都常驻、不 Collapsed：
 * 播完把旧的那层折叠掉会让左区宽度变一次、把中间 Tab 顶歪，
 * 这里只动 opacity / transform，布局零抖动。
 */
export default function TitleBarTitle() {
  const nested = useNestedPage();
  const mainRef = useRef<HTMLDivElement>(null);
  const innerRef = useRef<HTMLDivElement>(null);
  const mounted = useRef(false);
  const wasNested = useRef(nested !== null);

  useLayoutEffect(() => {
    const main = mainRef.current;
    const inner = innerRef.current;
    if (!main || !inner) return;

    const isNested = nested !== null;
    const previous = wasNested.current;
    wasNested.current = isNested;

    // 首次挂载直接摆好：应用启动时标题栏不该动一下
    if (!mounted.current) {
      mounted.current = true;
      return;
    }
    // 只是子页换了标题（详情页 → 详情页），不做交叉换位
    if (previous === isNested) return;
    if (!animationsEnabled()) return;

    const animations: Animation[] = [];
    const fade = (el: HTMLElement, from: number, to: number, delay: number) =>
      el.animate([{ opacity: from }, { opacity: to }], {
        duration: TITLE_SWAP.fade,
        delay,
        easing: EASE.outFluent,
        fill: "both",
      });
    const move = (
      el: HTMLElement,
      from: number,
      to: number,
      delay: number,
      easing: string,
    ) =>
      el.animate(
        [
          { transform: `translateX(${from}px)` },
          { transform: `translateX(${to}px)` },
        ],
        {
          duration: TITLE_SWAP.move,
          delay,
          easing,
          fill: "both",
        },
      );

    if (isNested) {
      // 进子页：品牌区往右让开，子页标题从左边回弹进来
      animations.push(
        fade(main, 1, 0, 0),
        move(main, 0, TITLE_SWAP.outMain, 0, EASE.inFluent),
        fade(inner, 0, 1, TITLE_SWAP.delay),
        move(
          inner,
          -TITLE_SWAP.fromInner,
          0,
          TITLE_SWAP.delay,
          EASE.outBack,
        ),
      );
    } else {
      // 返回：子页标题向左收走，品牌区从右边回来
      animations.push(
        fade(inner, 1, 0, 0),
        move(inner, 0, -TITLE_SWAP.fromInner, 0, EASE.inFluent),
        fade(main, 0, 1, TITLE_SWAP.delay),
        move(main, TITLE_SWAP.outMain, 0, TITLE_SWAP.delay, EASE.outBack),
      );
    }

    // 全部播完再取消：取消后交回 static 样式，而 static 就等于动画终点，接缝看不出来
    Promise.all(animations.map((animation) => animation.finished))
      .then(() => animations.forEach((animation) => animation.cancel()))
      .catch(() => undefined);

    return () => animations.forEach((animation) => animation.cancel());
  }, [nested]);

  const brand = (
    <Stack
      ref={mainRef}
      direction="row"
      spacing={1.25}
      sx={{
        gridArea: "stack",
        alignItems: "center",
        minWidth: 0,
        opacity: nested ? 0 : 1,
        transform: nested ? `translateX(${TITLE_SWAP.outMain}px)` : "none",
        pointerEvents: nested ? "none" : "auto",
      }}
      aria-hidden={nested ? true : undefined}
    >
      {/* 图标本身就是拖动手柄：拖动区不向上查找，必须自己带标记；
          同时禁掉浏览器原生的图片拖放，否则按在图上拖的是图片而不是窗口。 */}
      <Box
        component="img"
        src="/xcl-icon.png"
        alt=""
        aria-hidden="true"
        draggable={false}
        data-tauri-drag-region
        sx={{
          width: 32,
          height: 32,
          flexShrink: 0,
          objectFit: "contain",
          userSelect: "none",
        }}
      />
      <Typography
        sx={{
          fontSize: 16.5,
          fontWeight: 700,
          letterSpacing: "0.02em",
          color: "text.primary",
          lineHeight: 1,
          whiteSpace: "nowrap",
        }}
      >
        XCL
      </Typography>
    </Stack>
  );

  const inner = (
    <Stack
      ref={innerRef}
      direction="row"
      spacing={0.75}
      sx={{
        gridArea: "stack",
        alignItems: "center",
        minWidth: 0,
        opacity: nested ? 1 : 0,
        transform: nested ? "none" : `translateX(-${TITLE_SWAP.fromInner}px)`,
        pointerEvents: nested ? "auto" : "none",
      }}
      aria-hidden={nested ? undefined : true}
    >
      <ButtonBase
        data-no-drag
        onClick={() => nested?.onBack()}
        aria-label="返回"
        sx={{
          width: 32,
          height: 32,
          flexShrink: 0,
          borderRadius: 1,
          color: "text.secondary",
          "&:hover": {
            bgcolor: (t) => alpha(t.palette.text.primary, 0.08),
            color: "text.primary",
          },
          "& .MuiTouchRipple-root": { borderRadius: "inherit" },
        }}
      >
        <ArrowBackIcon sx={{ fontSize: 19 }} />
      </ButtonBase>
      <Typography
        noWrap
        sx={{
          fontSize: 15,
          fontWeight: 600,
          color: "text.primary",
          minWidth: 0,
        }}
      >
        {nested?.title ?? ""}
      </Typography>
    </Stack>
  );

  return (
    <Box
      sx={{
        display: "grid",
        gridTemplateAreas: '"stack"',
        alignItems: "center",
        width: "100%",
        minWidth: 0,
        height: "100%",
        // 两个层叠着，宽度取较宽的那个；超出左区就裁掉，不去顶中间的 Tab
        overflow: "hidden",
      }}
    >
      {brand}
      {inner}
    </Box>
  );
}
