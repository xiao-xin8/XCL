import { Suspense, lazy, useEffect, useRef, useState } from "react";
import { Box, Chip, alpha } from "@mui/material";
import type { AccountView, SkinRender } from "../lib/types";
import Loading from "./Loading";

// 和 LoginSidebar 一样：skinview3d 拖着一整个 three.js，只能按需拉。
const SkinViewer3D = lazy(() => import("./SkinViewer3D"));

/** 上半区的皮肤预览：容器撑满半区，高度靠实测喂给 skinview3d。 */
export function PreviewBox(props: { account: AccountView; skin: SkinRender | null }) {
  const { account, skin } = props;
  const boxRef = useRef<HTMLDivElement>(null);
  const [height, setHeight] = useState(0);

  // 预览盒是 flex 撑开的，没有固定像素高；skinview3d 只吃数字，
  // 所以量一下容器实际高度再喂给它（首帧用 220 兜底）。
  useEffect(() => {
    const box = boxRef.current;
    if (!box) return;
    const observer = new ResizeObserver(() => setHeight(box.clientHeight));
    observer.observe(box);
    setHeight(box.clientHeight);
    return () => observer.disconnect();
  }, []);

  return (
    <Box
      sx={{
        flex: 1,
        minHeight: 0,
        borderRadius: 1,
        overflow: "hidden",
        position: "relative",
        bgcolor: (t) => alpha(t.palette.primary.main, 0.14),
      }}
    >
      <Box ref={boxRef} sx={{ position: "absolute", inset: 0 }}>
        {skin ? (
          <Suspense
            fallback={
              <Box sx={{ height: "100%", display: "grid", placeItems: "center" }}>
                <Loading />
              </Box>
            }
          >
            <SkinViewer3D
              key={account.id}
              skin={skin.skin}
              cape={skin.cape}
              model={skin.model}
              height={height || 220}
            />
          </Suspense>
        ) : (
          <Box sx={{ height: "100%", display: "grid", placeItems: "center" }}>
            <Loading />
          </Box>
        )}
      </Box>
    </Box>
  );
}

/** 预览占位：皮肤方块脸图标（不是把整张贴图铺出来）。
 *
 *  没有账号 / 皮肤没拉好 / 正在拖分栏手柄时显示。
 *  从皮肤贴图里裁「脸 (8,8)」打底、「头发 overlay (32,8)」盖上，叠成
 *  Minecraft 风味的方块脸 —— 一眼能看出是皮肤相关的占位，而不是一张图。
 *  背景同一张 steve.png，靠 background-position 取不同区域，零额外请求。 */
export function SkinPlaceholder(props: { badge?: string }) {
  /** 方块脸边长（px）。脸是 8×8 的格子，放大 12 倍 = 96。 */
  const FACE = 96;
  /** 8px 一格在放大图里的位移 = 8 × 12 = 96；64px 整图 × 12 = 768。 */
  const STEP = FACE / 8;
  return (
    <Box
      sx={{
        flex: 1,
        minHeight: 0,
        borderRadius: 1,
        overflow: "hidden",
        position: "relative",
        bgcolor: (t) => alpha(t.palette.primary.main, 0.14),
        display: "grid",
        placeItems: "center",
      }}
    >
      <Box
        sx={{
          width: FACE,
          height: FACE,
          position: "relative",
          imageRendering: "pixelated",
          // 占位感：比 3D 预览淡一截
          opacity: 0.7,
          pointerEvents: "none",
          userSelect: "none",
          "& > *": {
            position: "absolute",
            inset: 0,
            backgroundImage: "url(/skins/steve.png)",
            backgroundSize: `${STEP * 64}px ${STEP * 64}px`,
          },
        }}
      >
        {/* 脸 */}
        <Box sx={{ backgroundPosition: `-${STEP * 8}px -${STEP * 8}px` }} />
        {/* 头发（皮肤第二层 overlay） */}
        <Box sx={{ backgroundPosition: `-${STEP * 32}px -${STEP * 8}px` }} />
      </Box>
      {props.badge && (
        <Chip
          size="small"
          label={props.badge}
          sx={{
            position: "absolute",
            left: 8,
            top: 8,
            height: 18,
            fontSize: 10,
            // 小徽标要保持可读，不跟界面的透明度一起变
            bgcolor: "surface.solid",
            color: "text.secondary",
          }}
        />
      )}
    </Box>
  );
}
