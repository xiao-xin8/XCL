import { useEffect, useRef, useState } from "react";
import {
  Box,
  Button,
  ButtonBase,
  Popover,
  Stack,
  Typography,
} from "@mui/material";
import { invoke } from "@tauri-apps/api/core";
import type { ConfigPatch } from "../../lib/config";
import { setAppearance, useAppearance } from "../../lib/appearance";
import { toAppError } from "../../lib/types";
import { readableTextOn } from "../../lib/color";
import { ACCENT_OPTIONS, ACCENT_SWATCH, type AccentKey } from "../../theme";
import { useToast } from "../Toast";
import SettingGroup from "./SettingGroup";
import SettingRow from "./SettingRow";
import ColorPicker from "./ColorPicker";
import { ActionRow, SelectRow, SliderRow } from "./controls";

const THEME_OPTIONS = [
  { value: "system", label: "跟随系统" },
  { value: "light", label: "浅色" },
  { value: "dark", label: "深色" },
] as const;

const ANIMATION_OPTIONS = [
  { value: "system", label: "跟随系统" },
  { value: "on", label: "全部开启" },
  { value: "off", label: "全部关闭" },
] as const;

/** 取色器拖动时会连续触发，落盘防抖这么久 */
const CUSTOM_SAVE_MS = 400;

/** 胶囊 tab 的共用样式：选中时整颗填成那个颜色 */
function pillSx(active: boolean, color?: string) {
  return {
    borderRadius: 999,
    height: 30,
    px: 1.5,
    gap: 0.75,
    flexShrink: 0,
    border: "1px solid",
    borderColor: active ? "transparent" : "divider",
    bgcolor: active && color ? color : "transparent",
    color: active && color ? readableTextOn(color) : "text.secondary",
    transition: "background-color 160ms ease, color 160ms ease",
    "&:hover": { bgcolor: active && color ? color : "action.hover" },
  } as const;
}

/**
 * 个性化：只影响「好不好看」，不改变任何行为。
 *
 * 改完立刻生效：先写运行时 store（顶层 `ThemeProvider` 会重建主题），
 * 再落盘。落盘失败由 `useConfig` 回滚并 toast，这里只管即时反馈。
 */
export default function PersonalizationSection(props: {
  patch: (patch: ConfigPatch) => void;
}) {
  const { patch } = props;
  // 显示值取自运行时 store 而不是 config：标题栏那个太阳键也在改同一批值，
  // 用 store 才能保证两处入口看到的是同一个现状（config 是持久化用的副本）
  const appearance = useAppearance();
  const toast = useToast();

  /** 取色器的落盘防抖；卸载时要清掉，免得定时器在别人页面里写盘 */
  const saveTimer = useRef<number | null>(null);
  useEffect(
    () => () => {
      if (saveTimer.current !== null) window.clearTimeout(saveTimer.current);
    },
    [],
  );

  const [pickerAnchor, setPickerAnchor] = useState<HTMLElement | null>(null);
  const [savingBackground, setSavingBackground] = useState(false);

  /** 取色器打开时的起始颜色：有自定义就用它，否则用当前预设的色块 */
  const pickerValue =
    appearance.accentCustom ?? ACCENT_SWATCH[appearance.accent];

  const selectPreset = (key: AccentKey) => {
    setAppearance({ accent: key, accentCustom: null });
    patch({ ui: { accent: key, accentCustom: null } });
  };

  const applyCustom = (hex: string) => {
    setAppearance({ accentCustom: hex });
    if (saveTimer.current !== null) window.clearTimeout(saveTimer.current);
    saveTimer.current = window.setTimeout(() => {
      saveTimer.current = null;
      patch({ ui: { accentCustom: hex } });
    }, CUSTOM_SAVE_MS);
  };

  const pickBackground = async () => {
    setSavingBackground(true);
    try {
      const url = await invoke<string | null>("background_pick");
      if (url) {
        setAppearance({ backgroundImage: url });
        toast("背景已更新", "success");
      }
    } catch (e: unknown) {
      const err = toAppError(e, "设置背景图失败");
      toast(`${err.title}：${err.message}`, "error");
    } finally {
      setSavingBackground(false);
    }
  };

  const clearBackground = async () => {
    setSavingBackground(true);
    try {
      await invoke("background_clear");
      setAppearance({ backgroundImage: null });
      toast("背景已移除", "success");
    } catch (e: unknown) {
      const err = toAppError(e, "移除背景图失败");
      toast(`${err.title}：${err.message}`, "error");
    } finally {
      setSavingBackground(false);
    }
  };

  return (
    <>
      <SettingGroup title="主题">
        <SelectRow
          title="主题模式"
          desc="跟随系统会随操作系统的浅色/深色自动切换。标题栏右上角那个按钮改的也是这一项。"
          value={appearance.themeMode}
          options={[...THEME_OPTIONS]}
          onChange={(next) => {
            setAppearance({ themeMode: next });
            patch({ ui: { themeMode: next } });
          }}
          width={160}
        />

        <SettingRow
          title="强调色"
          desc="淡黄是原厂配色。换成别的也不会有人拦你。"
          block
        >
          <Box sx={{ display: "flex", flexWrap: "wrap", gap: 0.75 }}>
            {ACCENT_OPTIONS.map((option) => {
              const swatch = ACCENT_SWATCH[option.value];
              const active =
                !appearance.accentCustom && appearance.accent === option.value;
              return (
                <ButtonBase
                  key={option.value}
                  onClick={() => selectPreset(option.value)}
                  sx={pillSx(active, swatch)}
                >
                  {!active && (
                    <Box
                      sx={{
                        width: 12,
                        height: 12,
                        flexShrink: 0,
                        borderRadius: "50%",
                        bgcolor: swatch,
                        border: "1px solid",
                        borderColor: "divider",
                      }}
                    />
                  )}
                  <Typography
                    sx={{ fontSize: 12, fontWeight: active ? 600 : 400 }}
                  >
                    {option.label}
                  </Typography>
                </ButtonBase>
              );
            })}

            {/* 自定义：选中时整颗胶囊填成当前颜色 */}
            <ButtonBase
              onClick={(event) => setPickerAnchor(event.currentTarget)}
              sx={pillSx(Boolean(appearance.accentCustom), pickerValue)}
            >
              <Box
                sx={{
                  width: 12,
                  height: 12,
                  flexShrink: 0,
                  borderRadius: "50%",
                  border: "1px solid",
                  borderColor: appearance.accentCustom
                    ? "transparent"
                    : "divider",
                  background: appearance.accentCustom
                    ? readableTextOn(pickerValue)
                    : "conic-gradient(#f66, #fd6, #6d8, #6bd, #86f, #f6c, #f66)",
                }}
              />
              <Typography
                sx={{
                  fontSize: 12,
                  fontWeight: appearance.accentCustom ? 600 : 400,
                }}
              >
                自定义
              </Typography>
            </ButtonBase>
          </Box>
        </SettingRow>

        <Popover
          open={Boolean(pickerAnchor)}
          anchorEl={pickerAnchor}
          onClose={() => setPickerAnchor(null)}
          anchorOrigin={{ vertical: "bottom", horizontal: "left" }}
          transformOrigin={{ vertical: "top", horizontal: "left" }}
        >
          <ColorPicker value={pickerValue} onChange={applyCustom} />
        </Popover>
      </SettingGroup>

      <SettingGroup
        title="背景"
        desc="换一张图铺满窗口。图片会复制到启动器自己的目录里，只保留一张，原图删掉也不影响。"
      >
        <ActionRow
          title="背景图"
          desc={appearance.backgroundImage ? "已设置一张自定义背景。" : "还没有设置，现在是纯色背景。"}
        >
          <Stack direction="row" spacing={1}>
            <Button
              size="small"
              variant="outlined"
              disabled={savingBackground}
              onClick={() => void pickBackground()}
            >
              选择图片
            </Button>
            {appearance.backgroundImage && (
              <Button
                size="small"
                disabled={savingBackground}
                onClick={() => void clearBackground()}
              >
                移除
              </Button>
            )}
          </Stack>
        </ActionRow>

        {/* 只有设了图才有意义，没图时先不摆两根空滑杆 */}
        {appearance.backgroundImage && (
          <>
            <SliderRow
              title="背景蒙版"
              desc="0 是原图，100 就完全被主题底色盖住。文字看不清时往上加。"
              value={appearance.backgroundOverlay}
              min={0}
              max={100}
              step={5}
              format={(value) => `${value}%`}
              onChange={(next) => {
                setAppearance({ backgroundOverlay: next });
                patch({ ui: { backgroundOverlay: next } });
              }}
            />
            <SliderRow
              title="背景模糊"
              desc="只糊背景图本身。卡片自己的毛玻璃在上面的「透明度与质感」里调。"
              value={appearance.backgroundBlur}
              min={0}
              max={20}
              step={2}
              format={(value) => (value === 0 ? "关闭" : `${value} px`)}
              onChange={(next) => {
                setAppearance({ backgroundBlur: next });
                patch({ ui: { backgroundBlur: next } });
              }}
            />
          </>
        )}
      </SettingGroup>

      <SettingGroup
        title="透明度与质感"
        desc="一套值管所有面：卡片、侧栏、分栏面板、菜单和对话框。"
      >
        <SliderRow
          title="界面不透明度"
          desc="往下拉能透出底下的背景和图片。0% 就是全透，看不清是你自己的选择。"
          value={appearance.surfaceOpacity}
          min={0}
          max={100}
          step={5}
          format={(value) => `${value}%`}
          onChange={(next) => {
            setAppearance({ surfaceOpacity: next });
            patch({ ui: { surfaceOpacity: next } });
          }}
        />

        <SliderRow
          title="毛玻璃模糊"
          desc="面和背景之间的模糊。要配合上面的不透明度才看得出效果。"
          value={appearance.surfaceBlur}
          min={0}
          max={20}
          step={2}
          format={(value) => (value === 0 ? "关闭" : `${value} px`)}
          onChange={(next) => {
            setAppearance({ surfaceBlur: next });
            patch({ ui: { surfaceBlur: next } });
          }}
        />
      </SettingGroup>

      <SettingGroup title="动效">
        <SelectRow
          title="动画强度"
          desc="关掉之后页面直接跳过去。不会更快，只是更朴素；开启则忽略系统的「减少动态效果」。"
          value={appearance.animation}
          options={[...ANIMATION_OPTIONS]}
          onChange={(next) => {
            setAppearance({ animation: next });
            patch({ ui: { animation: next } });
          }}
          width={160}
        />
      </SettingGroup>
    </>
  );
}
