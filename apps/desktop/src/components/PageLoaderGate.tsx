import { Box } from "@mui/material";
import { useEffect, useRef, useState, type ReactNode } from "react";
import {
  gateNextTickIn,
  gateShowsLoader,
  idleGate,
  nextGate,
  type GateState,
} from "../lib/loaderGate";
import { LOADER_GATE } from "../motion/tokens";
import Loading from "./Loading";

/**
 * 加载环闸门：不是「loading 就转圈」，而是一套固定节奏。
 *
 * - 数据 400ms 内就回来 → 转圈根本不出现，直接出内容（消灭「闪一下」）；
 * - 超过 400ms 才显示环，且环至少停 400ms（消灭「转圈一闪而过」）。
 *
 * 两个阈值在 `motion/tokens.ts` 的 `LOADER_GATE`，逻辑在 `lib/loaderGate.ts`。
 * 注意这里只有转圈：把内容也一起收起 / 展开不归它管，
 * 内容出现时的入场动画由外层 `PageTransition` 的块动画负责。
 */
export default function PageLoaderGate(props: {
  loading: boolean;
  children: ReactNode;
}) {
  const { loading, children } = props;
  const [state, setState] = useState<GateState>(idleGate);
  const stateRef = useRef(state);
  stateRef.current = state;

  useEffect(() => {
    let timer: number | undefined;

    const step = () => {
      const now = performance.now();
      const next = nextGate(stateRef.current, loading, now, LOADER_GATE);
      if (next !== stateRef.current) {
        stateRef.current = next;
        setState(next);
      }
      const wait = gateNextTickIn(next, loading, now, LOADER_GATE);
      if (wait !== null) timer = window.setTimeout(step, wait);
    };

    step();
    return () => {
      if (timer !== undefined) window.clearTimeout(timer);
    };
  }, [loading]);

  if (gateShowsLoader(state)) {
    return (
      <Box
        sx={{
          flex: 1,
          minHeight: 0,
          display: "grid",
          placeItems: "center",
        }}
      >
        <Loading />
      </Box>
    );
  }

  return <>{children}</>;
}
