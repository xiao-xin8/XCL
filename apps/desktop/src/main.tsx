import React from "react";
import ReactDOM from "react-dom/client";
import { invoke } from "@tauri-apps/api/core";
import App from "./App";
import { setAppearance } from "./lib/appearance";
import type { AppConfig } from "./lib/types";

/**
 * 先把配置读回来，再挂载界面。
 *
 * 之前是「先渲染启动页 → config_get 回来再 setNav」：第一帧必定是首页，
 * 然后播一次「首页退场 → 目标页进场」，会看到闪一下再切过去。
 * 配置是本地 TOML，读一次几毫秒 —— 挪到挂载前，第一帧就是目标页，
 * 顺带把主题 / 强调色也一次到位（不再「先默认色闪一下」）。
 */
async function bootstrap(): Promise<void> {
  const root = document.getElementById("root");
  if (!root) throw new Error("找不到 #root 挂载点");

  let config: AppConfig | null = null;
  try {
    config = await invoke<AppConfig>("config_get");
  } catch {
    // 纯浏览器环境 / 配置读不出来：用默认外观 + 启动页，不影响使用
    config = null;
  }

  if (config) {
    setAppearance({
      themeMode: config.ui.themeMode,
      accent: config.ui.accent,
      accentCustom: config.ui.accentCustom,
      surfaceOpacity: config.ui.surfaceOpacity,
      surfaceBlur: config.ui.surfaceBlur,
      backgroundOverlay: config.ui.backgroundOverlay,
      backgroundBlur: config.ui.backgroundBlur,
      animation: config.ui.animation,
    });
  }

  // 背景图单独取：可能是几 MB 的 data URL，不塞进 config_get
  try {
    const background = await invoke<string | null>("background_image");
    if (background) setAppearance({ backgroundImage: background });
  } catch {
    // 没有背景图 / 读失败都不影响启动
  }

  ReactDOM.createRoot(root).render(
    <React.StrictMode>
      <App bootstrap={config} />
    </React.StrictMode>,
  );
}

void bootstrap();
