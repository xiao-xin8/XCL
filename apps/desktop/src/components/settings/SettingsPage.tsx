import { useEffect, useRef, useState } from "react";
import {
  Box,
  List,
  ListItemButton,
  ListItemIcon,
  ListItemText,
} from "@mui/material";
import RocketLaunchOutlined from "@mui/icons-material/RocketLaunchOutlined";
import DesktopWindowsOutlined from "@mui/icons-material/DesktopWindowsOutlined";
import PaletteOutlined from "@mui/icons-material/PaletteOutlined";
import DownloadOutlined from "@mui/icons-material/DownloadOutlined";
import CoffeeOutlined from "@mui/icons-material/CoffeeOutlined";
import TuneOutlined from "@mui/icons-material/TuneOutlined";
import SystemUpdateAltOutlined from "@mui/icons-material/SystemUpdateAltOutlined";
import InfoOutlined from "@mui/icons-material/InfoOutlined";
import PageTransition from "../../motion/PageTransition";
import { playEnter } from "../../motion/pageMotion";
import { useConfig } from "../../lib/config";
import { PANEL_SURFACE_SX } from "../../lib/surfaces";
import ErrorState from "../ErrorState";
import LaunchSection from "./LaunchSection";
import UiSection from "./UiSection";
import PersonalizationSection from "./PersonalizationSection";
import DownloadSection from "./DownloadSection";
import JavaSection from "./JavaSection";
import AdvancedSection from "./AdvancedSection";
import UpdateSection from "./UpdateSection";
import AboutSection from "./AboutSection";

type SectionKey =
  | "launch"
  | "ui"
  | "personalize"
  | "download"
  | "java"
  | "advanced"
  | "update"
  | "about";

/**
 * 侧栏分类。图标全部用 MUI 自带的 outlined 图标，和文字同一行；
 * 图标放在 `ListItemButton` 里面，左栏的入场动画按的是直接子项（按钮本身），
 * 所以加图标不会多出动画块、也不会改变错峰节奏。
 */
const SECTIONS: { key: SectionKey; label: string; icon: typeof InfoOutlined }[] = [
  { key: "launch", label: "启动", icon: RocketLaunchOutlined },
  { key: "ui", label: "界面", icon: DesktopWindowsOutlined },
  { key: "personalize", label: "个性化", icon: PaletteOutlined },
  { key: "download", label: "下载", icon: DownloadOutlined },
  { key: "java", label: "Java", icon: CoffeeOutlined },
  // 「高级」只放能操作的东西；「关于」是官网式介绍，两边不混在一起
  { key: "advanced", label: "高级", icon: TuneOutlined },
  // 更新是低频但必须找得到的事，单独一类；不开顶部页签（TABS.md 定过）
  { key: "update", label: "更新", icon: SystemUpdateAltOutlined },
  { key: "about", label: "关于", icon: InfoOutlined },
];

/**
 * 设置页。
 *
 * 布局照下载页：左栏贴边（`background.default` + 右侧分隔线），右栏直接铺内容卡片，
 * 没有标头（分类名在左栏已经写了，再顶一行「启动 + 说明 + 即时保存」是废话）。
 *
 * 动画：
 * - 左栏用 `data-page-stagger="left"`，每个分类按递减错峰滑入。
 * - 右栏每张 `SettingGroup`（`Paper`）自动算一块，逐块从上方落下。
 * - 右栏滚动容器不标记，一标扫描就停，里面的卡全都不动。
 * - 配置是异步拉的：卡片在配置回来之后才出现，那时外层的入场动画早已播完，
 *   所以这里补一次 `playEnter`（同 `VersionList` 的「转圈换内容」处理）。
 */
export default function SettingsPage() {
  const [section, setSection] = useState<SectionKey>("launch");
  const { config, error, patch, reset, reload } = useConfig();

  const rightRef = useRef<HTMLDivElement>(null);
  const enteredRef = useRef(false);

  // 配置到达后补一次入场：不补的话卡片是「凭空出现」的
  useEffect(() => {
    if (!config || enteredRef.current) return;
    enteredRef.current = true;
    if (rightRef.current) playEnter(rightRef.current);
  }, [config]);

  const renderSection = (key: string) => {
    if (!config) return null;
    if (key === "launch") return <LaunchSection config={config} patch={patch} />;
    if (key === "ui") return <UiSection config={config} patch={patch} />;
    if (key === "personalize") return <PersonalizationSection patch={patch} />;
    if (key === "download") return <DownloadSection config={config} patch={patch} />;
    if (key === "java") return <JavaSection config={config} patch={patch} />;
    if (key === "advanced") {
      return (
        <AdvancedSection
          config={config}
          patch={patch}
          onReset={() => void reset()}
          onReload={() => void reload()}
        />
      );
    }
    if (key === "update") {
      return <UpdateSection config={config} patch={patch} />;
    }
    return <AboutSection />;
  };

  return (
    <Box sx={{ flex: 1, minHeight: 0, display: "flex" }}>
      {/* 左栏：贴边、不套容器，和下载页的侧边栏保持一致 */}
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
                  // 图标跟着行的文字色走，未选中时降一档
                  "& .MuiListItemIcon-root": {
                    minWidth: 26,
                    color: "text.secondary",
                  },
                  "&.Mui-selected": {
                    bgcolor: "primary.main",
                    color: "primary.contrastText",
                    "&:hover": { bgcolor: "primary.main" },
                    "& .MuiListItemIcon-root": { color: "primary.contrastText" },
                    "& .MuiListItemText-primary": { fontWeight: 600 },
                  },
                  "&:hover": { bgcolor: "action.hover" },
                }}
              >
                <ListItemIcon>
                  <item.icon sx={{ fontSize: 18 }} />
                </ListItemIcon>
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

      {/* 右栏：直接铺卡片，不放标头 */}
      <Box
        ref={rightRef}
        sx={{
          flex: 1,
          minWidth: 0,
          minHeight: 0,
          display: "flex",
          flexDirection: "column",
          p: 2,
        }}
      >
        {error && !config ? (
          <ErrorState
            variant="page"
            error={error}
            onRetry={() => void reload()}
            sx={{ flex: 1, minHeight: 0 }}
          />
        ) : (
          <PageTransition
            pageKey={section}
            initial={false}
            render={(key) => (
              // 滚动容器：不标记、不参与动画，只负责滚。
              // 子项必须 `flexShrink: 0` —— 否则列方向的卡片会被压扁到容器高度，
              // 内容就「截断」了，而且永远滚不到下面。
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
                  "& > *": { flexShrink: 0 },
                }}
              >
                {renderSection(key)}
              </Box>
            )}
          />
        )}
      </Box>
    </Box>
  );
}
