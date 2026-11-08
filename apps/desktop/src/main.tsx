import React from "react";
import ReactDOM from "react-dom/client";
import { invoke } from "@tauri-apps/api/core";
import App from "./App";
import { setAppearance } from "./lib/appearance";
import type { AppConfig } from "./lib/types";

/**
 * 挡掉「像网页」的那几个默认行为。
 *
 * Tauri 的窗口本质是一个内嵌 WebView2 —— 浏览器认为自己在浏览网页，所以：
 *   - 右键会弹出「重新加载 / 另存为 / 检查」这套菜单，在启动器里既没用又出戏；
 *   - F5 / Ctrl+R 会真的整页重载，前端状态全丢（Rust 那边的下载任务还在跑，
 *     界面却回到首页），这是会让人以为「卡死了」的那种问题；
 *   - Ctrl+P / Ctrl+U 更没意义。
 *
 * 但不该一刀切：输入框和可编辑区域要留住原生右键菜单，那是唯一能右键复制粘贴的地方。
 * F12 / Ctrl+Shift+I 故意不拦，dev 下还要开调试台。
 */
function blockBrowserDefaults(): void {
  window.addEventListener("contextmenu", (event) => {
    const target = event.target;
    const editable =
      target instanceof Element && target.closest('input, textarea, [contenteditable="true"]');
    if (!editable) event.preventDefault();
  });

  window.addEventListener("keydown", (event) => {
    if (event.key === "F5") {
      event.preventDefault();
      return;
    }
    if (!(event.ctrlKey || event.metaKey) || event.altKey || event.shiftKey) return;
    const key = event.key.toLowerCase();
    if (key === "r" || key === "p" || key === "u") event.preventDefault();
  });

  // 图片（皮肤、图标）默认能被拖出来，跟着光标飘一张幽灵图，很网页
  window.addEventListener("dragstart", (event) => event.preventDefault());

  // Ctrl+滚轮是浏览器的缩放，会把整个界面连布局一起缩小放大，必须掐掉
  window.addEventListener(
    "wheel",
    (event) => {
      if (event.ctrlKey) event.preventDefault();
    },
    { passive: false },
  );
}

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

// 先关掉浏览器的默认交互，再渲染 —— 顺序反了第一帧右键就能弹出来
blockBrowserDefaults();
void bootstrap();
