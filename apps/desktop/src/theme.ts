import { alpha, createTheme } from "@mui/material/styles";
import type { Theme } from "@mui/material/styles";
import { deriveAccent } from "./lib/color";
import { CARD_MOTION, EASE } from "./motion/tokens";

/**
 * 主题工厂。
 *
 * 配色只有一条规则：浅黄只做面，深金只做字/线/控件。
 * MUI 默认把 `primary.main` 同时当填充色和文字色用，而淡黄当文字在白底上
 * 几乎不可读，所以这里单独加了一个 `brand.ink`：只有「原本会是淡黄」的
 * 可交互文字、描边、勾选/进度/输入框焦点改走它；普通正文和次要文字的颜色
 * 保持原样，不做全局加深。
 */

export type ThemeMode = "light" | "dark";

/**
 * 强调色。淡黄是原厂配色，其余是给「个性化」用的预设。
 *
 * 每个色都有四件套：`main`（面）、`light`（浅面）、`dark`（描边/次级）、
 * `contrast`（面上的字），外加 `ink` —— 可交互文字与描边专用的深色。
 * 淡黄不能当文字色（白底上基本看不见），所以 ink 必须单独给，不能从 main 算。
 */
export type AccentKey = "yellow" | "sky" | "mint" | "sakura" | "graphite";

type Brand = {
  main: string;
  light: string;
  dark: string;
  contrast: string;
  ink: string;
};

const ACCENTS: Record<AccentKey, { light: Brand; dark: Brand }> = {
  yellow: {
    light: { main: "#F6DE8D", light: "#FCF2CB", dark: "#C9A227", contrast: "#3A3009", ink: "#6B5310" },
    dark: { main: "#E8CC7A", light: "#5A4A1E", dark: "#B9963C", contrast: "#231E08", ink: "#F0D585" },
  },
  sky: {
    light: { main: "#A8CDF0", light: "#E3EFFB", dark: "#5B9BD5", contrast: "#0B2B45", ink: "#1B5E8C" },
    dark: { main: "#7FB6E8", light: "#1E3A52", dark: "#4A87BF", contrast: "#06131F", ink: "#A9D2F2" },
  },
  mint: {
    light: { main: "#A9DFC4", light: "#E4F6EC", dark: "#4FA87E", contrast: "#0A2E1F", ink: "#1F6B4A" },
    dark: { main: "#7FCFAB", light: "#173A2B", dark: "#3E8D68", contrast: "#04150F", ink: "#A8E6C8" },
  },
  sakura: {
    light: { main: "#F3C0D1", light: "#FCEBF1", dark: "#CE7495", contrast: "#3A0E1E", ink: "#8E3B58" },
    dark: { main: "#E9A3BC", light: "#4A2230", dark: "#B96A88", contrast: "#1F0710", ink: "#F6C8D7" },
  },
  graphite: {
    light: { main: "#CFCDC4", light: "#EFEEE9", dark: "#8D8A80", contrast: "#23221E", ink: "#4A4842" },
    dark: { main: "#8C8980", light: "#33312C", dark: "#6A6862", contrast: "#12110F", ink: "#D5D2C9" },
  },
};

export const ACCENT_OPTIONS: { value: AccentKey; label: string }[] = [
  { value: "yellow", label: "淡黄" },
  { value: "sky", label: "天青" },
  { value: "mint", label: "薄荷" },
  { value: "sakura", label: "樱粉" },
  { value: "graphite", label: "石墨" },
];

/** 色块预览用（浅色模式的主色），设置页画小圆点用 */
export const ACCENT_SWATCH: Record<AccentKey, string> = {
  yellow: "#F6DE8D",
  sky: "#A8CDF0",
  mint: "#A9DFC4",
  sakura: "#F3C0D1",
  graphite: "#CFCDC4",
};

type Neutrals = {
  textPrimary: string;
  textSecondary: string;
};

const LIGHT: Neutrals = {
  textPrimary: "#2E2A1D",
  textSecondary: "#7B7362",
};

const DARK: Neutrals = {
  textPrimary: "#F0ECDF",
  textSecondary: "#A39D8C",
};

type Bg = {
  bgDefault: string;
  bgPaper: string;
  divider: string;
};

/**
 * 背景与分隔线跟着强调色走。
 *
 * 之前底色写死成米白，换强调色只有控件变、整页还是黄的，个性化等于没生效。
 * 这里每个强调色给一套同色相的低饱和底色；文字颜色则不跟着变
 * （正文对比度按原来的保证，不因为换色而漂）。
 */
const ACCENT_BG: Record<AccentKey, { light: Bg; dark: Bg }> = {
  yellow: {
    light: { bgDefault: "#FAF7EC", bgPaper: "#FFFFFF", divider: "#EAE0C6" },
    dark: { bgDefault: "#171614", bgPaper: "#201F1C", divider: "#332F26" },
  },
  sky: {
    light: { bgDefault: "#F3F8FD", bgPaper: "#FCFDFF", divider: "#DBE8F4" },
    dark: { bgDefault: "#14171A", bgPaper: "#1C2024", divider: "#29323A" },
  },
  mint: {
    light: { bgDefault: "#F3F9F5", bgPaper: "#FCFEFC", divider: "#DCEADF" },
    dark: { bgDefault: "#131715", bgPaper: "#1B211D", divider: "#26322A" },
  },
  sakura: {
    light: { bgDefault: "#FCF5F8", bgPaper: "#FFFCFD", divider: "#F1DFE6" },
    dark: { bgDefault: "#191416", bgPaper: "#231C1F", divider: "#33262B" },
  },
  graphite: {
    light: { bgDefault: "#F6F6F4", bgPaper: "#FEFEFD", divider: "#E3E2DD" },
    dark: { bgDefault: "#161615", bgPaper: "#1F1F1E", divider: "#2F2E2B" },
  },
};

declare module "@mui/material/styles" {
  interface Palette {
    brand: { ink: string };
    /**
     * 半透明面。
     *
     * - `default`：侧栏 / 分栏面板
     * - `paper`：卡片、菜单、对话框（同时写进 `background.paper`）
     * - `solid`：不参与透明度的手动底色（Toast 这类必须看清的浮层）
     * - `filter`：开了模糊时的 `backdrop-filter`，否则 `none`
     * - `opacity`：0–1 的系数，给「本来就有底色的条」按比例减淡（标题栏、胶囊条）
     */
    surface: {
      default: string;
      paper: string;
      solid: string;
      filter: string;
      opacity: number;
    };
  }
  interface PaletteOptions {
    brand?: { ink?: string };
    surface?: {
      default?: string;
      paper?: string;
      solid?: string;
      filter?: string;
      opacity?: number;
    };
  }
}

/**
 * 全局细滚动条：WebView2 是 Chromium，`::-webkit-scrollbar` 生效。
 *
 * 滑块跟着强调色走（取该色的 `dark` 档加透明度）：之前是写死的暖灰，
 * 换成天青 / 薄荷之后滚动条还是黄的，整页配色调不起来。
 */
function scrollbarOverrides(mode: ThemeMode, brand: Brand) {
  const isDark = mode === "dark";
  const thumb = alpha(brand.dark, isDark ? 0.5 : 0.4);
  const thumbHover = alpha(brand.dark, isDark ? 0.75 : 0.65);

  return {
    // 标准属性（Firefox）：Chromium 忽略但不报错
    "*": {
      scrollbarWidth: "thin",
      scrollbarColor: `${thumb} transparent`,
    },
    "*::-webkit-scrollbar": {
      width: 10,
      height: 10,
    },
    "*::-webkit-scrollbar-track": {
      background: "transparent",
    },
    // 2px 透明边框 + background-clip:content-box，视觉上只留约 5px 细条
    "*::-webkit-scrollbar-thumb": {
      backgroundColor: thumb,
      borderRadius: 999,
      border: "2px solid transparent",
      backgroundClip: "content-box",
    },
    "*::-webkit-scrollbar-thumb:hover": {
      backgroundColor: thumbHover,
    },
    "*::-webkit-scrollbar-corner": {
      background: "transparent",
    },
  };
}

export type ThemeOptions = {
  /** 预设色；`customAccent` 有值时会被它覆盖 */
  accent?: AccentKey;
  /** 自定义强调色（`#rrggbb`）。由 `lib/color` 推导整套配色 */
  customAccent?: string | null;
  /** 界面不透明度，0–100（卡片、侧栏、弹出层统一） */
  surfaceOpacity?: number;
  /** 毛玻璃模糊半径（px），0 = 关闭 */
  surfaceBlur?: number;
  /** 是否播放勾选 / 展开这类微动画；跟着动画偏好走 */
  motion?: boolean;
};

export function createAppTheme(mode: ThemeMode, options: ThemeOptions = {}): Theme {
  const {
    accent = "yellow",
    customAccent = null,
    surfaceOpacity = 100,
    surfaceBlur = 0,
    motion = true,
  } = options;
  const variant = mode === "dark" ? "dark" : "light";
  // 自定义色走推导，预设色走手调好的那套
  const custom = customAccent ? deriveAccent(customAccent) : null;
  const brand: Brand = custom
    ? custom[variant]
    : (ACCENTS[accent]?.[variant] ?? ACCENTS.yellow.light);
  const bg: Bg = custom
    ? custom.bg[variant]
    : (ACCENT_BG[accent]?.[variant] ?? ACCENT_BG.yellow.light);
  const c = mode === "dark" ? DARK : LIGHT;
  const ink = brand.ink;
  const disabledText = alpha(c.textPrimary, 0.38);
  const disabledBorder = alpha(c.textPrimary, 0.18);

  /**
   * 界面透明度：一套值统一给所有面。
   *
   * 关键点：直接改 `palette.background.paper`（Paper / 菜单 / 对话框的底色
   * 本来就取它），而不是在 `components.MuiPaper.styleOverrides` 里覆盖 ——
   * MUI 9 的组件样式打包顺序会让 styleOverrides 输给组件自带的基础样式，
   * 表现就是「怎么调都没反应」。模糊则用一条只声明 `backdrop-filter` 的
   * 全局规则补上（不碰底色，`sx` 里的自定义底色仍然优先）。
   */
  const opacity = Math.min(100, Math.max(0, surfaceOpacity)) / 100;
  const blur = Math.min(20, Math.max(0, surfaceBlur));
  const blurFilter = blur > 0 ? `blur(${blur}px) saturate(1.08)` : "none";
  const surface = {
    default: alpha(bg.bgDefault, opacity),
    paper: alpha(bg.bgPaper, opacity),
    // 给 Toast 这类浮层留一个不透明底，别跟着变透明
    solid: bg.bgPaper,
    filter: blurFilter,
    opacity,
  };

  return createTheme({
    palette: {
      mode,
      primary: {
        main: brand.main,
        light: brand.light,
        dark: brand.dark,
        contrastText: brand.contrast,
      },
      brand: { ink },
      surface,
      // 卡片 / 菜单 / 对话框的底色全部来自这里 —— 改它比覆盖组件样式可靠
      background: { default: bg.bgDefault, paper: surface.paper },
      divider: bg.divider,
      text: { primary: c.textPrimary, secondary: c.textSecondary },
    },
    // 全局统一圆角 16px：卡片 / 行 / 按钮 / 弹窗全部走这套。
    // 注意 sx 里的数字圆角是按这个值的倍数算的（borderRadius: 1 = 16px）。
    shape: { borderRadius: 16 },
    typography: {
      fontFamily:
        '"MiSans", "HarmonyOS Sans SC", "Microsoft YaHei UI", system-ui, sans-serif',
      button: { textTransform: "none" },
    },
    components: {
      MuiCssBaseline: {
        styleOverrides: {
          // 只补模糊，不碰底色：Paper 的底色来自 palette，sx 里的自定义底色照旧优先
          ...(blur > 0 ? { ".MuiPaper-root": { backdropFilter: blurFilter } } : {}),
          ...scrollbarOverrides(mode, brand),
        },
      },
      MuiButton: {
        defaultProps: { disableElevation: true },
        styleOverrides: {
          // MUI 9 没有 textPrimary / outlinedPrimary 这两个 class，
          // 只能用组合选择器盖掉「text/outlined + 主色」的默认淡黄文字。
          // 描边按钮的框太细会显得「飘」，统一 1.5px（含 hover / disabled）
          outlined: {
            borderWidth: 1.5,
            "&:hover": { borderWidth: 1.5 },
            "&.Mui-disabled": { borderWidth: 1.5 },
          },
          root: {
            "&.MuiButton-text.MuiButton-colorPrimary": {
              color: ink,
              "&:hover": { backgroundColor: alpha(ink, 0.07) },
              "&.Mui-disabled": { color: disabledText },
            },
            "&.MuiButton-outlined.MuiButton-colorPrimary": {
              color: ink,
              borderColor: alpha(ink, 0.55),
              "&:hover": {
                borderColor: ink,
                backgroundColor: alpha(ink, 0.07),
              },
              "&.Mui-disabled": {
                color: disabledText,
                borderColor: disabledBorder,
              },
            },
          },
        },
      },
      MuiIconButton: {
        styleOverrides: {
          colorPrimary: { color: ink },
        },
      },
      MuiCheckbox: {
        styleOverrides: {
          colorPrimary: {
            color: ink,
            "&.Mui-checked": { color: ink },
            "&.MuiCheckbox-indeterminate": { color: ink },
          },
          root: {
            "& .MuiSvgIcon-root": { transformOrigin: "center" },
            // 默认图标是整块替换，勾上瞬间太生硬：给填充一次短促的回弹。
            // 取消勾选不播，否则每次打开弹窗所有空框都会先闪一下。
            ...(motion
              ? {
                  "&.Mui-checked .MuiSvgIcon-root": {
                    animation: `xcl-check-fill ${CARD_MOTION.collapseEnterMin}ms ${EASE.outBack}`,
                  },
                  "@keyframes xcl-check-fill": {
                    from: { transform: "scale(0.55)", opacity: 0.4 },
                    to: { transform: "scale(1)", opacity: 1 },
                  },
                }
              : {}),
          },
        },
      },
      MuiRadio: {
        styleOverrides: {
          colorPrimary: {
            color: ink,
            "&.Mui-checked": { color: ink },
          },
        },
      },
      MuiSwitch: {
        styleOverrides: {
          colorPrimary: {
            "&.Mui-checked": {
              color: ink,
              "& + .MuiSwitch-track": {
                backgroundColor: alpha(ink, 0.55),
                opacity: 1,
              },
            },
          },
        },
      },
      // 进度条默认用淡黄铺满，轨道是更淡的黄，几乎看不出走到哪
      MuiLinearProgress: {
        styleOverrides: {
          bar1: { backgroundColor: ink },
          bar2: { backgroundColor: alpha(ink, 0.55) },
        },
      },
      MuiCircularProgress: {
        styleOverrides: {
          colorPrimary: { color: ink },
        },
      },
      MuiInputLabel: {
        styleOverrides: {
          root: {
            "&.Mui-focused": { color: ink },
          },
        },
      },
      MuiOutlinedInput: {
        styleOverrides: {
          root: {
            "&:hover .MuiOutlinedInput-notchedOutline": {
              borderColor: alpha(c.textPrimary, 0.45),
            },
            "&.Mui-focused .MuiOutlinedInput-notchedOutline": {
              borderColor: ink,
              borderWidth: 2,
            },
          },
        },
      },
      MuiLink: {
        styleOverrides: {
          root: { color: ink },
        },
      },
      MuiPaper: { defaultProps: { elevation: 0 } },
      MuiTooltip: { defaultProps: { enterDelay: 400 } },
      // Chip 源码把圆角写死成 16，正好等于统一值；显式再钉一遍，
      // 将后 MUI 改默认也不会破坏全局一致性。
      MuiChip: {
        styleOverrides: {
          root: {
            borderRadius: 16,
            "&.MuiChip-outlined.MuiChip-colorPrimary": {
              color: ink,
              borderColor: alpha(ink, 0.45),
            },
          },
        },
      },
    },
  });
}

/** 默认浅色主题，供不关心切换的场景直接引用。 */
export const theme = createAppTheme("light");
