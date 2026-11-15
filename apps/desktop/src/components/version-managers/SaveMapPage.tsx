import { useEffect, useRef, useState } from "react";
import { Box, Typography } from "@mui/material";
import { invoke } from "@tauri-apps/api/core";
import L from "leaflet";
import "leaflet/dist/leaflet.css";
import { toAppError, type AppError, type MapTile, type SaveView } from "../../lib/types";
import ErrorState from "../ErrorState";

/** 瓦片边长，和后端 `TILE_PIXELS` 必须一致。 */
const TILE = 256;
/** 最远一级：1 像素 = 4 方块（后端 zoom 2）。 */
const MIN_ZOOM = -2;
/** 最近一级：1 方块 = 16 像素。再往上就是无意义的马赛克。 */
const MAX_ZOOM = 4;
/** 后端真正渲染的最深层级，之后靠 Leaflet 放大已有瓦片。 */
const MAX_NATIVE_ZOOM = 0;

/**
 * 世界坐标直接当地图坐标用的 CRS。
 *
 * `L.CRS.Simple` 默认 y 轴向「北」，而 Minecraft 的 z 轴向「南」（屏幕下），
 * 所以把 transformation 的 y 系数翻回来 —— 这样 `[z, x]` 就是方块坐标本身。
 */
const XclCRS = L.extend({}, L.CRS.Simple, {
  transformation: new L.Transformation(1, 0, 1, 0),
});

/**
 * 逐张 IPC 要瓦片的 TileLayer。
 *
 * 为什么不用 `L.tileLayer(url)` 直接给个地址：我们没有一个能被 WebView 直接
 * 请求的 HTTP 端点，瓦片要经过 Tauri 的命令拿。交给 `createTile` 自己填 `src`
 * 之后，Leaflet 的淡入、缩放过渡、瓦片保留全都照常生效 —— 它只关心
 * 这个 `<img>` 什么时候触发 load。
 */
function createTileLayer(versionId: string, folderName: string): L.TileLayer {
  const IpcTiles = L.TileLayer.extend({
    createTile(this: L.TileLayer, coords: L.Coords, done: L.DoneCallback) {
      const tile = document.createElement("img");
      tile.alt = "";
      tile.setAttribute("role", "presentation");

      // Leaflet 的 z 越小越远（0 = 1 像素 1 方块），后端 zoom 正好相反。
      const zoom = Math.max(0, -coords.z);

      invoke<MapTile>("save_map_tile", {
        versionId,
        folderName,
        zoom,
        tileX: coords.x,
        tileZ: coords.y,
      })
        .then((result) => {
          if (!result.dataUrl) {
            // 没有区块：不设 src，保持透明，也算这张瓦片「加载完成」
            done(undefined, tile);
            return;
          }
          tile.onload = () => done(undefined, tile);
          tile.onerror = () => done(new Error("地图瓦片解码失败"), tile);
          tile.src = result.dataUrl;
        })
        .catch((error: unknown) => {
          done(error instanceof Error ? error : new Error(String(error)), tile);
        });

      return tile;
    },
  });

  const Ctor = IpcTiles as unknown as new (
    url: string,
    options: L.TileLayerOptions,
  ) => L.TileLayer;

  return new Ctor("", {
    tileSize: TILE,
    minZoom: MIN_ZOOM,
    maxZoom: MAX_ZOOM,
    maxNativeZoom: MAX_NATIVE_ZOOM,
    // 缩放过程中就补瓦片，别等停下来
    updateWhenZooming: true,
    updateWhenIdle: false,
    // 视口外多留两圈，拖回来时不用重新请求
    keepBuffer: 2,
    noWrap: true,
  });
}

/**
 * 整页地图：Leaflet + 按需渲染的瓦片。
 *
 * 三件事交给 Leaflet，不自己写 canvas：
 * - 瓦片淡入（`fadeAnimation`，默认 200ms）
 * - 缩放时保留旧层级瓦片，新瓦片盖上去（不会一下白屏）
 * - 超过 `maxNativeZoom` 之后放大已有瓦片（像素块，但看得清）
 */
export default function SaveMapPage(props: { versionId: string; item: SaveView }) {
  const { versionId, item } = props;

  const containerRef = useRef<HTMLDivElement>(null);
  const [error, setError] = useState<AppError | null>(null);
  const [zoom, setZoom] = useState(0);

  useEffect(() => {
    const container = containerRef.current;
    if (!container) return;

    try {
      const map = L.map(container, {
        crs: XclCRS,
        center: [item.spawnZ ?? 0, item.spawnX ?? 0],
        zoom: 0,
        minZoom: MIN_ZOOM,
        maxZoom: MAX_ZOOM,
        zoomControl: false,
        attributionControl: false,
        zoomSnap: 1,
        zoomDelta: 1,
        wheelPxPerZoomLevel: 90,
        inertia: true,
        // 瓦片淡入：新瓦片从透明渐变进来，不是啪一下贴上。
        // 这是 Map 层开关，TileLayer 继承它。
        fadeAnimation: true,
        zoomAnimation: true,
      });

      createTileLayer(versionId, item.folderName).addTo(map);
      map.on("zoomend", () => setZoom(map.getZoom()));

      // 页面切换 / 侧栏收起时容器尺寸会变，让 Leaflet 重算一次
      const observer = new ResizeObserver(() => map.invalidateSize());
      observer.observe(container);

      return () => {
        observer.disconnect();
        map.remove();
      };
    } catch (err: unknown) {
      setError(toAppError(err, "打开地图失败"));
      return undefined;
    }
  }, [item.folderName, item.spawnX, item.spawnZ, versionId]);

  if (error) {
    return (
      <Box sx={{ flex: 1, minHeight: 0, display: "flex", flexDirection: "column", p: 2 }}>
        <ErrorState variant="page" error={error} />
      </Box>
    );
  }

  return (
    <Box sx={{ flex: 1, minHeight: 0, display: "flex", flexDirection: "column", p: 2 }}>
      <Box
        data-page-block
        sx={{
          flex: 1,
          minHeight: 0,
          position: "relative",
          overflow: "hidden",
          borderRadius: 1,
          border: "1px solid",
          borderColor: "divider",
          bgcolor: "surface.paper",
          // Leaflet 自带的白底和默认字体在这里统一压掉
          "& .leaflet-container": {
            bgcolor: "surface.paper",
            fontFamily: "inherit",
            outline: "none",
            cursor: "grab",
          },
          "& .leaflet-dragging .leaflet-container": { cursor: "grabbing" },
          "& .leaflet-tile": {
            // 放大超过原生层级时是像素块，别让浏览器抹平
            imageRendering: "pixelated",
          },
          "& .leaflet-control-attribution": { display: "none" },
        }}
      >
        <Box ref={containerRef} sx={{ width: 1, height: 1 }} />

        <Box
          sx={{
            position: "absolute",
            left: 8,
            bottom: 8,
            px: 0.9,
            py: 0.2,
            borderRadius: 999,
            bgcolor: (t) => t.palette.surface.paper,
            backdropFilter: (t) => t.palette.surface.filter,
            pointerEvents: "none",
            zIndex: 500,
          }}
        >
          <Typography sx={{ fontSize: 10.5, color: "text.secondary" }}>
            {scaleLabel(zoom)}
          </Typography>
        </Box>

        <Box
          sx={{
            position: "absolute",
            right: 8,
            bottom: 8,
            px: 0.9,
            py: 0.2,
            borderRadius: 999,
            bgcolor: (t) => t.palette.surface.paper,
            backdropFilter: (t) => t.palette.surface.filter,
            pointerEvents: "none",
            zIndex: 500,
          }}
        >
          <Typography sx={{ fontSize: 10.5, color: "text.secondary" }}>
            拖动平移 · 滚轮缩放
          </Typography>
        </Box>
      </Box>
    </Box>
  );
}

/** zoom 0 = 1 像素 1 方块；正数表示一个方块占几个像素，负数表示一个像素几个方块。 */
function scaleLabel(zoom: number): string {
  if (zoom === 0) return "1 像素 = 1 方块";
  if (zoom > 0) return `1 方块 = ${1 << zoom} 像素`;
  return `1 像素 = ${1 << -zoom} 方块`;
}
