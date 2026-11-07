/**
 * 启动页两栏 / 上下分栏的比例、钳制范围，以及 localStorage 读写。
 *
 * 抽出来是为了让「布局」和「页面状态机」分开：比例这东西只跟窗口有关，
 * 和启动、版本列表没有任何关系。
 */

export const LAYOUT_STORAGE_KEY = "xcl.launchLayout";

export type LaunchLayout = {
  /** 左侧栏占整页宽度的比例 */
  sidebar: number;
  /** 右侧上半（版本列表）占右栏高度的比例 */
  detail: number;
};

export const LAYOUT_DEFAULT: LaunchLayout = { sidebar: 0.4, detail: 0.5 };

/** 侧边栏钳制：下限按「900px 最小窗口下侧栏仍有 ~200px」给（配 minWidth 200），
 *  上限放到 0.7 —— 要能自由拖，这里只兜「别把哪格拖没」 */
export const SIDEBAR_MIN = 0.22;
export const SIDEBAR_MAX = 0.7;

/** 上下面板钳制：同理放宽，版本信息整卡可滚动，压到 15% 也看得见 */
export const DETAIL_MIN = 0.15;
export const DETAIL_MAX = 0.85;

/** 读出来的比例：越界 / 坏数据一律退回默认，绝不拦着页面渲染。 */
export function readLayout(): LaunchLayout {
  try {
    const raw = localStorage.getItem(LAYOUT_STORAGE_KEY);
    if (!raw) return LAYOUT_DEFAULT;
    const parsed = JSON.parse(raw) as Partial<LaunchLayout>;
    const sane = (v: unknown, fallback: number, min: number, max: number) =>
      typeof v === "number" && Number.isFinite(v) ? Math.min(max, Math.max(min, v)) : fallback;
    return {
      sidebar: sane(parsed.sidebar, LAYOUT_DEFAULT.sidebar, SIDEBAR_MIN, SIDEBAR_MAX),
      detail: sane(parsed.detail, LAYOUT_DEFAULT.detail, DETAIL_MIN, DETAIL_MAX),
    };
  } catch {
    // 存储被写坏（手改过 etc.）就当默认值，不拦着启动
    return LAYOUT_DEFAULT;
  }
}
