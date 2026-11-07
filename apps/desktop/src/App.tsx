import { lazy, Suspense, useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { Box, CssBaseline, ThemeProvider } from "@mui/material";
import { invoke } from "@tauri-apps/api/core";
import TitleBar, { NAV, type NavKey } from "./components/TitleBar";
import LaunchPage from "./components/LaunchPage";
import Loading from "./components/Loading";
import WindowResizeZones from "./components/WindowResizeZones";
import DownloadDock from "./components/DownloadDock";
import { DownloadProvider } from "./components/DownloadProvider";
import ErrorBoundary from "./components/ErrorBoundary";
import Wallpaper from "./components/Wallpaper";
import { LaunchProvider } from "./components/LaunchProvider";
import { UpdateProvider } from "./components/UpdateProvider";
import { useUpdateState } from "./lib/updateContext";
import { ToastProvider } from "./components/Toast";
import PageTransition from "./motion/PageTransition";
import { bindNavigate } from "./lib/appNav";
import { createAppTheme, type ThemeMode } from "./theme";
import { animationsEnabled, setAppearance, useAppearance } from "./lib/appearance";
import type { AppConfig, StartPage } from "./lib/types";

// 启动页首屏要用，保持静态；下载页和设置页切过去时再取（都是本地 chunk）
const DownloadPage = lazy(() => import("./components/DownloadPage"));
const ConnectPage = lazy(() => import("./components/ConnectPage"));
const SettingsPage = lazy(() => import("./components/settings/SettingsPage"));

const NAV_KEYS: NavKey[] = ["launch", "download", "connect", "setting"];

/** 懒加载页面的占位：整块居中，和全局加载指示器一个尺寸。 */
function PageLoading() {
  return (
    <Box sx={{ flex: 1, display: "grid", placeItems: "center" }}>
      <Loading />
    </Box>
  );
}

function isNavKey(value: string | null | undefined): value is NavKey {
  return typeof value === "string" && (NAV_KEYS as string[]).includes(value);
}

/** 起始页 / 上次页签 → 真的能显示的页签。认不出来（老配置里的「实例页」）就回启动页。 */
function resolveStart(startPage: StartPage, lastTab: string): NavKey {
  const wanted = startPage === "last" ? lastTab : (startPage as string);
  return isNavKey(wanted) ? wanted : "launch";
}

/**
 * 强制更新时把整个界面「冻住」。
 *
 * 只加了 `inert`，没有卸载主界面 —— 下载任务、启动中的游戏都还在跑，
 * 把他打开的页面拆掉既没必要也会丢状态。`inert` 会同时挡住鼠标、键盘和
 * 无障碍焦点，弹窗本身就盖在上面。
 *
 * 这里消费的是高频的 `UpdateView`，所以 `children` 必须原样透传（它是 props
 * 里的稳定引用），React 才会跳过整棵子树的重渲染 —— 否则下载时每秒 4 次
 * 重渲染会把启动页的 3D 预览拖卡。
 */
function UpdateGate({ children }: { children: ReactNode }) {
  const { view } = useUpdateState();
  const forced = view?.forced ?? false;
  return (
    <Box sx={{ display: "contents" }} inert={forced}>
      {children}
    </Box>
  );
}

export default function App(props: {
  /** 挂载前已经读到的配置；`null` = 读失败，走默认值 */
  bootstrap: AppConfig | null;
}) {
  const { bootstrap } = props;
  const appearance = useAppearance();
  /**
   * 第一帧就落在目标页签上。
   *
   * 起始页 / 上次页签在 `main.tsx` 里已经拿到，这里同步算出来 ——
   * 不再「先渲染启动页、等 IPC 回来再切」，那样会白闪一次切页动画。
   */
  const [nav, setNav] = useState<NavKey>(() =>
    bootstrap
      ? resolveStart(bootstrap.ui.startPage, bootstrap.ui.lastTab)
      : "launch",
  );
  const [systemDark, setSystemDark] = useState(
    () => window.matchMedia("(prefers-color-scheme: dark)").matches,
  );
  /** 已落盘的上次页签；`null` 表示配置没读回来，别急着往上写 */
  const lastSaved = useRef<NavKey | null>(bootstrap ? nav : null);

  // 设置里可以关掉「联机」页签。配置是启动时读的，所以改完下次生效（同「启动行为」那一组）。
  const showConnectTab = bootstrap?.ui.showConnectTab ?? true;
  const navItems = useMemo(
    () => NAV.filter((item) => showConnectTab || item.key !== "connect"),
    [showConnectTab],
  );
  // 页签被关掉时如果正停在那一页，退回启动页，别让界面空着
  useEffect(() => {
    if (!showConnectTab && nav === "connect") setNav("launch");
  }, [showConnectTab, nav]);

  /**
   * 让深层组件（整合包装完）能把界面送回启动页。
   *
   * 切页动画不用管：`PageTransition` 认的是 `nav` 的变化，这里只负责改状态。
   */
  useEffect(() => bindNavigate(setNav), []);

  // themeMode = system 时跟随操作系统
  useEffect(() => {
    const mq = window.matchMedia("(prefers-color-scheme: dark)");
    const onChange = (event: MediaQueryListEvent) => setSystemDark(event.matches);
    mq.addEventListener("change", onChange);
    return () => mq.removeEventListener("change", onChange);
  }, []);

  // 读不到配置时也要能记住页签，从第二页开始写
  useEffect(() => {
    if (lastSaved.current === null) lastSaved.current = nav;
  }, [nav]);

  // 记住上次停在哪一页。配置没回来之前不写，免得把默认值盖掉真实记录
  useEffect(() => {
    if (lastSaved.current === null || lastSaved.current === nav) return;
    lastSaved.current = nav;
    void invoke("config_patch", { patch: { ui: { lastTab: nav } } }).catch(() => undefined);
  }, [nav]);

  const resolvedMode: ThemeMode =
    appearance.themeMode === "system" ? (systemDark ? "dark" : "light") : appearance.themeMode;
  const theme = useMemo(
    () =>
      createAppTheme(resolvedMode, {
        accent: appearance.accent,
        customAccent: appearance.accentCustom,
        surfaceOpacity: appearance.surfaceOpacity,
        surfaceBlur: appearance.surfaceBlur,
        motion: animationsEnabled(),
      }),
    [
      resolvedMode,
      appearance.accent,
      appearance.accentCustom,
      appearance.surfaceOpacity,
      appearance.surfaceBlur,
      appearance.animation,
    ],
  );

  // 标题栏那个太阳/月亮按钮：切成明确的浅色或深色（不再跟随系统）
  const toggleMode = () => {
    const next: ThemeMode = resolvedMode === "light" ? "dark" : "light";
    setAppearance({ themeMode: next });
    void invoke("config_patch", { patch: { ui: { themeMode: next } } }).catch(() => undefined);
  };

  return (
    <ThemeProvider theme={theme}>
      <CssBaseline enableColorScheme />
      {/* 背景图独立一层：蒙版 / 模糊是两个旋钮，塞进 body 就调不动了 */}
      <Wallpaper />
      {/* Toast 在外层：下载任务的通知必须在切页之后仍然存在 */}
      <ToastProvider>
        <DownloadProvider>
          <UpdateProvider autoCheck={bootstrap?.update.autoCheck ?? true}>
            <LaunchProvider>
              {/* 渲染异常兜底：白屏换成统一的「页面出错了」 */}
              <ErrorBoundary>
                <UpdateGate>
                  <Box
                    sx={{
                      display: "flex",
                      flexDirection: "column",
                      height: "100vh",
                      overflow: "hidden",
                    }}
                  >
                    <TitleBar
                      current={nav}
                      onChange={setNav}
                      onToggleMode={toggleMode}
                      items={navItems}
                    />

                    {/* 下载页和启动页自己管理内边距（侧边栏要贴边），其他页统一 16px */}
                    <Box
                      sx={{
                        flex: 1,
                        minHeight: 0,
                        display: "flex",
                        flexDirection: "column",
                      }}
                    >
                      <PageTransition
                        pageKey={nav}
                        render={(key) => {
                          if (key === "download") {
                            return (
                              <Suspense fallback={<PageLoading />}>
                                <DownloadPage />
                              </Suspense>
                            );
                          }
                          if (key === "connect") {
                            return (
                              <Suspense fallback={<PageLoading />}>
                                <ConnectPage />
                              </Suspense>
                            );
                          }
                          if (key === "setting") {
                            return (
                              <Suspense fallback={<PageLoading />}>
                                <SettingsPage />
                              </Suspense>
                            );
                          }
                          return <LaunchPage />;
                        }}
                      />
                    </Box>
                  </Box>
                  {/* 窗口边缘缩放热区：自绘标题栏下把原生 resize 边框加宽到好瞄 */}
                  <WindowResizeZones />
                  {/* 下载任务统一入口：右下角悬浮球 + 右侧抽屉 */}
                  <DownloadDock />
                </UpdateGate>
              </ErrorBoundary>
            </LaunchProvider>
          </UpdateProvider>
        </DownloadProvider>
      </ToastProvider>
    </ThemeProvider>
  );
}
