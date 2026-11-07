import { useEffect, useRef, useState } from "react";
import { Box, Stack, TextField } from "@mui/material";
import {
  hexToHsv,
  hsvToHex,
  normalizeHex,
  type Hsv,
} from "../../lib/color";

/**
 * 自绘取色器（MUI 没有官方颜色选择器）。
 *
 * 三块：饱和度/明度方块、色相条、色值输入。拖动时连续回调，
 * 外部拿到的是合法 `#rrggbb`。
 *
 * 不用浏览器原生的 `<input type="color">`：样式完全跟不了主题，弹出来还是
 * 系统那一套，放在设置页里像块补丁。
 */
export default function ColorPicker(props: {
  value: string;
  onChange: (hex: string) => void;
}) {
  const { value, onChange } = props;
  const [hsv, setHsv] = useState<Hsv>(() => hexToHsv(value));
  const [draft, setDraft] = useState(value);
  const dragging = useRef<"sv" | "hue" | null>(null);

  // 外部改色（点预设、用回预设）时同步进来；自己拖出来的值不会触发重置
  useEffect(() => {
    setHsv((prev) => {
      const next = hexToHsv(value);
      return hsvToHex(prev.h, prev.s, prev.v) === hsvToHex(next.h, next.s, next.v)
        ? prev
        : next;
    });
    setDraft((prev) => (normalizeHex(prev) === value ? prev : value));
  }, [value]);

  const apply = (next: Hsv) => {
    setHsv(next);
    const hex = hsvToHex(next.h, next.s, next.v);
    setDraft(hex);
    onChange(hex);
  };

  const pickSv = (event: React.PointerEvent<HTMLDivElement>) => {
    const rect = event.currentTarget.getBoundingClientRect();
    const s = Math.min(1, Math.max(0, (event.clientX - rect.left) / rect.width));
    const v = 1 - Math.min(1, Math.max(0, (event.clientY - rect.top) / rect.height));
    apply({ h: hsv.h, s, v });
  };

  const pickHue = (event: React.PointerEvent<HTMLDivElement>) => {
    const rect = event.currentTarget.getBoundingClientRect();
    const ratio = Math.min(1, Math.max(0, (event.clientX - rect.left) / rect.width));
    apply({ h: ratio * 360, s: hsv.s, v: hsv.v });
  };

  return (
    <Box sx={{ width: 240, p: 1.5 }}>
      <Box
        onPointerDown={(event) => {
          dragging.current = "sv";
          event.currentTarget.setPointerCapture(event.pointerId);
          pickSv(event);
        }}
        onPointerMove={(event) => {
          if (dragging.current === "sv") pickSv(event);
        }}
        onPointerUp={(event) => {
          dragging.current = null;
          event.currentTarget.releasePointerCapture(event.pointerId);
        }}
        onPointerCancel={() => {
          dragging.current = null;
        }}
        sx={{
          position: "relative",
          width: "100%",
          height: 118,
          borderRadius: 1,
          cursor: "crosshair",
          touchAction: "none",
          background:
            "linear-gradient(to top, #000, rgba(0,0,0,0)), " +
            `linear-gradient(to right, #fff, ${hsvToHex(hsv.h, 1, 1)})`,
        }}
      >
        <Box
          sx={{
            position: "absolute",
            left: `${hsv.s * 100}%`,
            top: `${(1 - hsv.v) * 100}%`,
            width: 12,
            height: 12,
            ml: "-6px",
            mt: "-6px",
            borderRadius: "50%",
            bgcolor: hsvToHex(hsv.h, hsv.s, hsv.v),
            border: "2px solid #fff",
            outline: "1px solid rgba(0,0,0,0.35)",
            pointerEvents: "none",
          }}
        />
      </Box>

      <Box
        onPointerDown={(event) => {
          dragging.current = "hue";
          event.currentTarget.setPointerCapture(event.pointerId);
          pickHue(event);
        }}
        onPointerMove={(event) => {
          if (dragging.current === "hue") pickHue(event);
        }}
        onPointerUp={(event) => {
          dragging.current = null;
          event.currentTarget.releasePointerCapture(event.pointerId);
        }}
        onPointerCancel={() => {
          dragging.current = null;
        }}
        sx={{
          position: "relative",
          height: 12,
          mt: 1.5,
          borderRadius: 999,
          cursor: "pointer",
          touchAction: "none",
          background:
            "linear-gradient(to right, #ff0000, #ffff00, #00ff00, #00ffff, #0000ff, #ff00ff, #ff0000)",
        }}
      >
        <Box
          sx={{
            position: "absolute",
            left: `${(hsv.h / 360) * 100}%`,
            top: -2,
            width: 8,
            height: 16,
            ml: "-4px",
            borderRadius: 999,
            bgcolor: hsvToHex(hsv.h, 1, 1),
            border: "2px solid #fff",
            outline: "1px solid rgba(0,0,0,0.35)",
            pointerEvents: "none",
          }}
        />
      </Box>

      <Stack direction="row" spacing={1} sx={{ alignItems: "center", mt: 1.5 }}>
        <Box
          sx={{
            width: 28,
            height: 28,
            flexShrink: 0,
            borderRadius: 0.75,
            bgcolor: value,
            border: "1px solid",
            borderColor: "divider",
          }}
        />
        <TextField
          size="small"
          value={draft}
          onChange={(event) => {
            const text = event.target.value;
            setDraft(text);
            const normalized = normalizeHex(text);
            if (normalized) {
              setHsv(hexToHsv(normalized));
              onChange(normalized);
            }
          }}
          onBlur={() => {
            if (!normalizeHex(draft)) setDraft(value);
          }}
          slotProps={{ htmlInput: { "aria-label": "颜色值" } }}
          sx={{
            flex: 1,
            minWidth: 0,
            "& .MuiInputBase-input": {
              fontSize: 12,
              fontFamily: "ui-monospace, Consolas, monospace",
            },
          }}
        />
      </Stack>
    </Box>
  );
}
