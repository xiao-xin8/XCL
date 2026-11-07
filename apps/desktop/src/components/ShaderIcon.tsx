import SvgIcon, { type SvgIconProps } from "@mui/material/SvgIcon";

/**
 * 「光影」分类的图标：两层对角错开的光晕 + 中心光源。
 *
 * 为什么不用现成的：
 * - `BlurOn` 是一团糊掉的墨（之前用的，太丑）；
 * - `WbSunny` / `LightMode` 是天气；
 * - `Gradient` / `Tonality` 偏"色彩"，不偏"光"。
 *
 * 光影包给人的直观感受就是镜头光斑 —— 一个亮点外面套两层光晕，两层各自
 * 只画对角的两段弧，缺口错开，看起来像光在扩散，而不是被框住的太阳。
 *
 * 线宽 1.6 / 圆头端点，和 MUI outlined 那批图标放在一起不会显得突兀。
 */
export default function ShaderIcon(props: SvgIconProps) {
  return (
    <SvgIcon {...props} viewBox="0 0 24 24">
      {/* 光源本体：一个小实心点，靠它把视觉重心压在正中 */}
      <circle cx="12" cy="12" r="1.7" fill="currentColor" stroke="none" />

      {/* 外圈光晕：右上 + 左下两段，缺口留成对角 */}
      <path
        d="M12 4.6a7.4 7.4 0 0 1 7.4 7.4M11.99 19.4A7.4 7.4 0 0 1 4.6 12"
        fill="none"
        stroke="currentColor"
        strokeWidth="1.6"
        strokeLinecap="round"
      />

      {/* 内圈光晕：方向和外圈错开，形成层次 */}
      <path
        d="M12 7.9a4.1 4.1 0 0 1 4.1 4.1M11.99 16.1A4.1 4.1 0 0 1 7.9 12"
        fill="none"
        stroke="currentColor"
        strokeWidth="1.6"
        strokeLinecap="round"
      />
    </SvgIcon>
  );
}
