/**
 * 从一个自定义色推导整套主题配色。
 *
 * 预设色（淡黄 / 天青 / 薄荷 / 樱粉 / 石墨）是逐档调出来的，不走这里；
 * 这里只服务「自定义颜色」：给一个 `#rrggbb`，我们算出
 *
 * - `main / light / dark`：面、浅面、描边；
 * - `contrast`：压在 main 上的文字；
 * - `ink`：浅底上的可交互文字（必须和白底拉开对比度）；
 * - 背景三件套：页面底、卡片底、分隔线（同色相、低饱和）。
 *
 * 规则只有一条：文字对比度不许低于 4.5。达不到就沿着明度方向推，
 * 推到头还不行才退回纯黑/纯白。
 */

export type Brand = {
  main: string;
  light: string;
  dark: string;
  contrast: string;
  ink: string;
};

export type Bg = {
  bgDefault: string;
  bgPaper: string;
  divider: string;
};

/** 统一成小写 `#rrggbb`；不合法返回 `null`（和后端 `is_hex_color` 同一套规则）。 */
export function normalizeHex(input: string): string | null {
  const text = input.trim().replace(/^#/, "");
  if (!/^[0-9a-fA-F]{6}$/.test(text)) return null;
  return `#${text.toLowerCase()}`;
}

function clamp(value: number, min: number, max: number): number {
  return Math.min(max, Math.max(min, value));
}

export type Hsv = { h: number; s: number; v: number };

/** HEX → HSV（h 0–360，s/v 0–1）。取色器用它定位。 */
export function hexToHsv(hex: string): Hsv {
  const [r, g, b] = toRgb(hex).map((value) => value / 255) as [
    number,
    number,
    number,
  ];
  const max = Math.max(r, g, b);
  const min = Math.min(r, g, b);
  const delta = max - min;
  let h = 0;
  if (delta !== 0) {
    if (max === r) h = ((g - b) / delta + (g < b ? 6 : 0)) * 60;
    else if (max === g) h = ((b - r) / delta + 2) * 60;
    else h = ((r - g) / delta + 4) * 60;
  }
  return { h, s: max === 0 ? 0 : delta / max, v: max };
}

/** HSV → HEX。h 会自动绕回 0–360。 */
export function hsvToHex(h: number, s: number, v: number): string {
  const hue = ((h % 360) + 360) % 360;
  const saturation = clamp(s, 0, 1);
  const value = clamp(v, 0, 1);
  const c = value * saturation;
  const x = c * (1 - Math.abs(((hue / 60) % 2) - 1));
  const m = value - c;
  let rgb: [number, number, number];
  if (hue < 60) rgb = [c, x, 0];
  else if (hue < 120) rgb = [x, c, 0];
  else if (hue < 180) rgb = [0, c, x];
  else if (hue < 240) rgb = [0, x, c];
  else if (hue < 300) rgb = [x, 0, c];
  else rgb = [c, 0, x];
  return toHex([(rgb[0] + m) * 255, (rgb[1] + m) * 255, (rgb[2] + m) * 255]);
}

/** 压在这个底色上的文字用黑还是白（取对比度高的那个）。 */
export function readableTextOn(background: string): string {
  return contrastRatio("#ffffff", background) >= contrastRatio("#161616", background)
    ? "#ffffff"
    : "#161616";
}

function toRgb(hex: string): [number, number, number] {
  const value = normalizeHex(hex) ?? "#000000";
  return [
    parseInt(value.slice(1, 3), 16),
    parseInt(value.slice(3, 5), 16),
    parseInt(value.slice(5, 7), 16),
  ];
}

function toHex(rgb: [number, number, number]): string {
  const part = (value: number) =>
    clamp(Math.round(value), 0, 255).toString(16).padStart(2, "0");
  return `#${part(rgb[0])}${part(rgb[1])}${part(rgb[2])}`;
}

function toHsl(hex: string): { h: number; s: number; l: number } {
  const [r, g, b] = toRgb(hex).map((value) => value / 255) as [
    number,
    number,
    number,
  ];
  const max = Math.max(r, g, b);
  const min = Math.min(r, g, b);
  const l = (max + min) / 2;
  if (max === min) return { h: 0, s: 0, l };

  const d = max - min;
  const s = l > 0.5 ? d / (2 - max - min) : d / (max + min);
  let h: number;
  if (max === r) h = (g - b) / d + (g < b ? 6 : 0);
  else if (max === g) h = (b - r) / d + 2;
  else h = (r - g) / d + 4;
  return { h: h * 60, s, l };
}

function fromHsl(h: number, s: number, l: number): string {
  const c = (1 - Math.abs(2 * l - 1)) * s;
  const x = c * (1 - Math.abs(((h / 60) % 2) - 1));
  const m = l - c / 2;
  let rgb: [number, number, number];
  if (h < 60) rgb = [c, x, 0];
  else if (h < 120) rgb = [x, c, 0];
  else if (h < 180) rgb = [0, c, x];
  else if (h < 240) rgb = [0, x, c];
  else if (h < 300) rgb = [x, 0, c];
  else rgb = [c, 0, x];
  return toHex([(rgb[0] + m) * 255, (rgb[1] + m) * 255, (rgb[2] + m) * 255]);
}

/** `t = 0` 取 `a`，`t = 1` 取 `b`。 */
function mix(a: string, b: string, t: number): string {
  const from = toRgb(a);
  const to = toRgb(b);
  return toHex([
    from[0] + (to[0] - from[0]) * t,
    from[1] + (to[1] - from[1]) * t,
    from[2] + (to[2] - from[2]) * t,
  ]);
}

function channel(value: number): number {
  const c = value / 255;
  return c <= 0.03928 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4;
}

function relativeLuminance(hex: string): number {
  const [r, g, b] = toRgb(hex);
  return 0.2126 * channel(r) + 0.7152 * channel(g) + 0.0722 * channel(b);
}

/** WCAG 对比度，1 ~ 21。 */
export function contrastRatio(a: string, b: string): number {
  const first = relativeLuminance(a);
  const second = relativeLuminance(b);
  const [high, low] = first >= second ? [first, second] : [second, first];
  return (high + 0.05) / (low + 0.05);
}

/** 沿明度方向推，直到和 `against` 的对比度达标。 */
function ensureContrast(
  base: string,
  against: string,
  target: number,
  direction: "darken" | "lighten",
): string {
  const { h, s, l } = toHsl(base);
  let current = l;
  for (let step = 0; step < 60; step += 1) {
    const candidate = fromHsl(h, s, current);
    if (contrastRatio(candidate, against) >= target) return candidate;
    current = clamp(current + (direction === "darken" ? -0.02 : 0.02), 0, 1);
  }
  return direction === "darken" ? "#000000" : "#ffffff";
}

/** 压在 `background` 上的文字：优先用品牌深色，不够黑就换纯白/纯黑。 */
function bestTextOn(background: string, preferred: string): string {
  if (contrastRatio(preferred, background) >= 4.5) return preferred;
  return contrastRatio("#ffffff", background) >= contrastRatio("#161616", background)
    ? "#ffffff"
    : "#161616";
}

/**
 * 从自定义色推导两套配色（浅色 / 深色）。
 *
 * 灰色（饱和度 < 0.08）不做加饱和处理 —— 否则 `#808080` 会被染成偏红。
 */
export function deriveAccent(input: string): {
  light: Brand;
  dark: Brand;
  bg: { light: Bg; dark: Bg };
} {
  const base = normalizeHex(input) ?? "#f6de8d";
  const { h, s, l } = toHsl(base);
  const neutral = s < 0.08;
  const sat = (value: number, min: number, max: number) =>
    neutral ? 0 : clamp(value, min, max);

  // 浅色面
  const lightBg: Bg = {
    bgDefault: mix(base, "#ffffff", 0.94),
    bgPaper: mix(base, "#ffffff", 0.985),
    divider: mix(base, "#ffffff", 0.8),
  };
  const lightInk = ensureContrast(
    fromHsl(h, sat(s * 0.9 + 0.1, 0.4, 0.9), 0.3),
    lightBg.bgPaper,
    4.5,
    "darken",
  );
  const lightDark = fromHsl(h, sat(s * 0.9, 0.3, 0.85), clamp(l - 0.28, 0.35, 0.62));

  // 深色面
  const darkBg: Bg = {
    bgDefault: mix(base, "#101010", 0.92),
    bgPaper: mix(base, "#101010", 0.88),
    divider: mix(base, "#101010", 0.7),
  };
  const darkInk = ensureContrast(
    fromHsl(h, sat(s, 0.35, 0.9), 0.78),
    darkBg.bgPaper,
    4.5,
    "lighten",
  );

  return {
    light: {
      main: base,
      light: mix(base, "#ffffff", 0.72),
      dark: lightDark,
      contrast: bestTextOn(base, lightInk),
      ink: lightInk,
    },
    dark: {
      main: base,
      light: fromHsl(h, sat(s * 0.6, 0, 1), 0.2),
      dark: fromHsl(h, sat(s * 0.8, 0, 1), 0.42),
      contrast: bestTextOn(base, darkInk),
      ink: darkInk,
    },
    bg: { light: lightBg, dark: darkBg },
  };
}
