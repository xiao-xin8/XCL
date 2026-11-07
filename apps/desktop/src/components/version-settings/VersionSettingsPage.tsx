import { useCallback, useEffect, useRef, useState } from "react";
import { Box, List, ListItemButton, ListItemText } from "@mui/material";
import { invoke } from "@tauri-apps/api/core";
import PageTransition from "../../motion/PageTransition";
import { playEnter } from "../../motion/pageMotion";
import {
  toAppError,
  type IsolationView,
  type LaunchOverride,
  type LocalVersion,
  type VersionSettingsView,
  type VersionStats,
} from "../../lib/types";
import { useToast } from "../Toast";
import PageLoaderGate from "../PageLoaderGate";
import { PANEL_SURFACE_SX } from "../../lib/surfaces";
import InfoSection from "./InfoSection";
import LaunchSection from "./LaunchSection";
import IsolationSection from "./IsolationSection";
import DataSection from "./DataSection";

type SectionKey = "info" | "launch" | "isolation" | "data";

const SECTIONS: { key: SectionKey; label: string }[] = [
  { key: "info", label: "说明" },
  { key: "launch", label: "启动" },
  { key: "isolation", label: "隔离与目录" },
  { key: "data", label: "数据" },
];

/**
 * 单个版本的设置（整页子页，从版本详情点「版本设置」进入，标题栏给返回键）。
 *
 * 和设置页完全同一套结构，只是数据源换成 `versions/<id>/XCL/instance.toml`：
 * - 左栏贴边（`data-page-stagger="left"` 逐项滑入）
 * - 右栏 `SettingGroup`（就是 `Paper`）自动成块，逐块落下
 * - 右栏滚动容器不标记，一标扫描就停；切分类走嵌套 `PageTransition initial={false}`
 *
 * 页面进出场由外层 `LaunchPage` 的 `PageTransition` 负责；右栏是
 * 设置读回来才挂载，所以用 ref 回调补一次 `playEnter`（同 Mod 详情页）。
 * 顶栏（图标 / 改名 / 收藏 / 删除）留在详情卡上，这里不再重复做一遍。
 */
export default function VersionSettingsPage(props: {
  version: LocalVersion | null;
  isolation: IsolationView | null;
  /** 说明这类会改变版本元信息的改动，改完让外面重扫列表 */
  onChanged: () => void;
  onIsolationChanged: (view: IsolationView) => void;
}) {
  const { version, isolation, onChanged, onIsolationChanged } = props;
  const toast = useToast();
  const [section, setSection] = useState<SectionKey>("info");
  const [settings, setSettings] = useState<VersionSettingsView | null>(null);
  const [stats, setStats] = useState<VersionStats | null>(null);

  const versionId = version?.id ?? null;

  // 进页面 / 换版本都重新拉一遍：外面可能刚改过说明、刚装完
  useEffect(() => {
    if (!versionId) return;
    let disposed = false;
    setSettings(null);
    setStats(null);
    invoke<VersionSettingsView>("version_settings_get", { versionId })
      .then((view) => {
        if (disposed) return;
        setSettings(view);
      })
      .catch((e: unknown) => {
        if (!disposed) toast(toAppError(e, "读取版本设置失败").message, "error");
      });
    invoke<VersionStats>("version_stats", { versionId })
      .then((value) => {
        if (!disposed) setStats(value);
      })
      .catch(() => {
        // 统计失败只是少几行数字，不打扰
      });
    return () => {
      disposed = true;
    };
  }, [versionId, toast]);

  /**
   * 设置回来时右栏才挂载 —— 这一刻就是「转圈换内容」的那一刻。
   * 挂 ref 回调直接补入场，不靠 effect 追状态（effect 会被后续 setState 吞掉）。
   */
  const playedFor = useRef<string | null>(null);
  const attachContent = useCallback(
    (node: HTMLDivElement | null) => {
      if (!node || !versionId) return;
      if (playedFor.current === versionId) return;
      playedFor.current = versionId;
      playEnter(node);
    },
    [versionId],
  );

  /** 局部改：只传变的字段，`null` 表示恢复跟随全局 */
  const patch = useCallback(
    async (fields: Partial<LaunchOverride>) => {
      if (!versionId) return;
      // 乐观更新：开关和输入立刻有反馈，回包再以后端归一值为准
      setSettings((prev) =>
        prev ? { ...prev, overrides: { ...prev.overrides, ...fields } } : prev,
      );
      try {
        const next = await invoke<VersionSettingsView>("version_settings_patch", {
          versionId,
          patch: fields,
        });
        setSettings(next);
      } catch (e: unknown) {
        toast(toAppError(e, "保存版本设置失败").message, "error");
        // 失败了就把真实状态拉回来，别让界面停在假的乐观值上
        try {
          const actual = await invoke<VersionSettingsView>("version_settings_get", {
            versionId,
          });
          setSettings(actual);
        } catch {
          /* 拉不回来就保持原样，下次打开会看到真值 */
        }
      }
    },
    [versionId, toast],
  );

  const saveDescription = useCallback(
    async (text: string) => {
      if (!versionId) return;
      try {
        await invoke("version_set_description", { versionId, description: text });
        onChanged();
      } catch (e: unknown) {
        toast(toAppError(e, "保存说明失败").message, "error");
      }
    },
    [versionId, onChanged, toast],
  );

  const refreshStats = useCallback(async () => {
    if (!versionId) return;
    try {
      setStats(await invoke<VersionStats>("version_stats", { versionId }));
    } catch (e: unknown) {
      toast(toAppError(e, "统计版本占用失败").message, "error");
    }
  }, [versionId, toast]);

  const renderSection = (key: string) => {
    if (!settings || !version) return null;
    if (key === "info") {
      return (
        <InfoSection
          version={version}
          onSaveDescription={(text) => void saveDescription(text)}
        />
      );
    }
    if (key === "launch") return <LaunchSection settings={settings} patch={patch} />;
    if (key === "isolation") {
      return (
        <IsolationSection
          version={version}
          isolation={isolation}
          onIsolationChanged={onIsolationChanged}
        />
      );
    }
    return <DataSection stats={stats} onRefresh={() => void refreshStats()} />;
  };

  const ready = settings !== null;

  return (
    <Box sx={{ flex: 1, minHeight: 0, display: "flex" }}>
      {/* 左栏：和设置页同一个长相 */}
      <Box
        sx={{
          width: 188,
          flexShrink: 0,
          borderRight: "1px solid",
          borderColor: "divider",
          ...PANEL_SURFACE_SX,
          overflow: "auto",
          py: 1.5,
          px: 1,
        }}
      >
        <List disablePadding data-page-stagger="left">
          {SECTIONS.map((item) => {
            const active = item.key === section;
            return (
              <ListItemButton
                key={item.key}
                selected={active}
                onClick={() => setSection(item.key)}
                sx={{
                  borderRadius: 1,
                  height: 34,
                  px: 1.25,
                  mb: 0.5,
                  bgcolor: "transparent",
                  "&.Mui-selected": {
                    bgcolor: "primary.main",
                    color: "primary.contrastText",
                    "&:hover": { bgcolor: "primary.main" },
                    "& .MuiListItemText-primary": { fontWeight: 600 },
                  },
                  "&:hover": { bgcolor: "action.hover" },
                }}
              >
                <ListItemText
                  primary={item.label}
                  slotProps={{
                    primary: { sx: { fontSize: 13.5, fontWeight: active ? 600 : 400 } },
                  }}
                />
              </ListItemButton>
            );
          })}
        </List>
      </Box>

      {/* 右栏：卡片直接铺，不放标头 */}
      <Box
        sx={{
          flex: 1,
          minWidth: 0,
          minHeight: 0,
          display: "flex",
          flexDirection: "column",
          p: 2,
        }}
      >
        <PageLoaderGate loading={!ready}>
          {ready && version ? (
            <Box
              ref={attachContent}
              sx={{ flex: 1, minHeight: 0, display: "flex", flexDirection: "column" }}
            >
              <PageTransition
                pageKey={section}
                initial={false}
                render={(key) => (
                  <Box
                    sx={{
                      flex: 1,
                      minHeight: 0,
                      overflow: "auto",
                      scrollbarGutter: "stable",
                      display: "flex",
                      flexDirection: "column",
                      gap: 1.5,
                      pr: 0.5,
                      pb: 1,
                      // 列方向的卡片默认会被压扁到容器高度，内容就截断了
                      "& > *": { flexShrink: 0 },
                    }}
                  >
                    {renderSection(key)}
                  </Box>
                )}
              />
            </Box>
          ) : null}
        </PageLoaderGate>
      </Box>
    </Box>
  );
}
