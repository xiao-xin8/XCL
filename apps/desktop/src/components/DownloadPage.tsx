import { useState } from "react";
import {
  Box,
  List,
  ListItemButton,
  ListItemIcon,
  ListItemText,
  Paper,
} from "@mui/material";
import SportsEsportsOutlined from "@mui/icons-material/SportsEsportsOutlined";
import CoffeeOutlined from "@mui/icons-material/CoffeeOutlined";
import Inventory2Outlined from "@mui/icons-material/Inventory2Outlined";
import ExtensionOutlined from "@mui/icons-material/ExtensionOutlined";
import WallpaperOutlined from "@mui/icons-material/WallpaperOutlined";
import ShaderIcon from "./ShaderIcon";
import StarBorderOutlined from "@mui/icons-material/StarBorderOutlined";
import VersionList from "./VersionList";
import FavoritesPage from "./FavoritesPage";
import JavaRuntimePage from "./JavaRuntimePage";
import ModPage from "./ModPage";
import ModpackPage from "./ModpackPage";
import ResourcePackPage from "./ResourcePackPage";
import ShaderPage from "./ShaderPage";
import PageTransition from "../motion/PageTransition";
import { PANEL_SURFACE_SX } from "../lib/surfaces";

/**
 * 下载分类骨架。除「游戏」「Java」「Mod」「资源包」「光影」外都还没实现，
 * 先把结构占住。
 *
 * 资源包与光影是两个分类、两个落地目录（`resourcepacks/` 与 `shaderpacks/`），
 * 游戏只认各自的目录，塞错地方等于没装 —— 所以这里不合并成一项。
 * 光影额外要 Iris 才生效，这条由详情页的依赖卡去说明，不靠分类本身区分。
 *
 * 图标用 MUI 自带的 outlined 图标，放在 `ListItemButton` 内部 ——
 * 左栏动画认的是 `List` 的直接子项，加图标不会改变错峰节奏。
 */
const CATEGORIES = [
  { key: "game", label: "游戏", icon: SportsEsportsOutlined },
  { key: "java", label: "Java 运行环境", icon: CoffeeOutlined },
  { key: "modpack", label: "整合包", icon: Inventory2Outlined },
  { key: "mod", label: "Mod", icon: ExtensionOutlined },
  { key: "resourcepack", label: "资源包", icon: WallpaperOutlined },
  { key: "shader", label: "光影", icon: ShaderIcon },
  // 收藏放最后：它是前面几类的聚合，不是又一种可以下载的东西
  { key: "favorite", label: "收藏", icon: StarBorderOutlined },
] as const;

type CategoryKey = (typeof CATEGORIES)[number]["key"];

export default function DownloadPage() {
  const [category, setCategory] = useState<CategoryKey>("game");

  return (
    <Box sx={{ flex: 1, minHeight: 0, display: "flex" }}>
      {/* 左侧分类：贴边，不套容器、不占额外留白 */}
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
          {CATEGORIES.map((item) => {
            const active = category === item.key;
            return (
              <ListItemButton
                key={item.key}
                selected={active}
                onClick={() => setCategory(item.key)}
                sx={{
                  borderRadius: 1,
                  height: 34,
                  px: 1.25,
                  mb: 0.5,
                  bgcolor: "transparent",
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
                    primary: {
                      sx: { fontSize: 13.5, fontWeight: active ? 600 : 400 },
                    },
                  }}
                />
              </ListItemButton>
            );
          })}
        </List>
      </Box>

      {/* 右侧：容器里装具体内容 */}
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
        <PageTransition
          pageKey={category}
          initial={false}
          render={(key) =>
            key === "game" ? (
              <VersionList />
            ) : key === "modpack" ? (
              <ModpackPage />
            ) : key === "mod" ? (
              <ModPage />
            ) : key === "resourcepack" ? (
              <ResourcePackPage />
            ) : key === "shader" ? (
              <ShaderPage />
            ) : key === "java" ? (
              <JavaRuntimePage />
            ) : key === "favorite" ? (
              <FavoritesPage />
            ) : (
              <Paper
                variant="outlined"
                sx={{
                  flex: 1,
                  minHeight: 0,
                  display: "grid",
                  placeItems: "center",
                }}
              >
                <Box sx={{ textAlign: "center" }}>
                  <Box sx={{ fontSize: 14, fontWeight: 600, mb: 0.5 }}>
                    {CATEGORIES.find((item) => item.key === key)?.label}
                  </Box>
                  <Box sx={{ fontSize: 12.5, color: "text.secondary" }}>
                    这一分类还没实现
                  </Box>
                </Box>
              </Paper>
            )
          }
        />
      </Box>
    </Box>
  );
}
