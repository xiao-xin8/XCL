/**
 * 版本列表行：ListItemButton（MUI 原生涟漪 / 焦点 / 键盘）。
 *
 * 纯展示：选中态、运行绿点、启动不了的警示 chip 都从 props 进来，
 * 点一下只回调 `onSelect`。重量级的状态（版本扫描、启动）在页面里。
 */
import {
  Box,
  Chip,
  ListItemButton,
  ListItemText,
  Stack,
  Typography,
  alpha,
} from "@mui/material";
import StarRoundedIcon from "@mui/icons-material/StarRounded";

import type { LocalVersion } from "../../lib/types";
import VersionIcon from "../VersionIcon";
import VersionTypeChip from "../VersionTypeChip";
import { formatDate } from "./format";

export function VersionRow(props: {
  item: LocalVersion;
  active: boolean;
  running: boolean;
  onSelect: () => void;
}) {
  const { item, active, running, onSelect } = props;
  return (
    <ListItemButton
      selected={active}
      onClick={onSelect}
      sx={{
        borderRadius: 1,
        px: 1.25,
        py: 1,
        mb: 0.75,
        alignItems: "center",
        gap: 1,
        border: "1px solid",
        borderColor: active ? "brand.ink" : "divider",
        bgcolor: "surface.paper",
        // 悬停 / 选中的底色淡入，不要瞬间跳变
        transition: "background-color 160ms ease, border-color 160ms ease",
        "&.Mui-selected": {
          bgcolor: (t) => alpha(t.palette.primary.main, 0.08),
          "&:hover": { bgcolor: (t) => alpha(t.palette.primary.main, 0.12) },
        },
        "&:hover": { bgcolor: "action.hover" },
      }}
    >
      <VersionIcon kind={item.kind} size={24} icon={item.icon} />
      <ListItemText
        primary={
          <Stack direction="row" spacing={0.75} sx={{ alignItems: "center" }}>
            {item.favorite && (
              <StarRoundedIcon sx={{ fontSize: 14, color: "primary.dark", flexShrink: 0 }} />
            )}
            <Typography
              sx={{ fontSize: 13, fontWeight: active ? 700 : 500, wordBreak: "break-all" }}
            >
              {item.id}
            </Typography>
            {item.versionType && <VersionTypeChip type={item.versionType} />}
            {item.javaMajor != null && (
              <Chip
                size="small"
                variant="outlined"
                label={`Java ${item.javaMajor}`}
                sx={{ fontSize: 9.5, height: 16, flexShrink: 0 }}
              />
            )}
            {!item.launchable && (
              <Chip
                size="small"
                color="warning"
                label={item.valid ? "缺少主文件" : "描述损坏"}
                sx={{ fontSize: 9.5, height: 16, flexShrink: 0 }}
              />
            )}
          </Stack>
        }
        secondary={
          item.description ? (
            <Typography sx={{ fontSize: 11, color: "text.secondary" }} noWrap>
              {item.description}
            </Typography>
          ) : undefined
        }
        slotProps={{
          primary: { component: "div" },
          secondary: { component: "div" },
        }}
        sx={{ my: 0, minWidth: 0 }}
      />
      {/* 状态与时间贴右端，给绿点留出固定位置，长版本名只会挤压中间 */}
      <Typography
        sx={{
          fontSize: 10.5,
          color: "text.secondary",
          whiteSpace: "nowrap",
          flexShrink: 0,
        }}
      >
        {item.launchable ? `可以启动 · ${formatDate(item.modifiedMs)}` : (item.error ?? "文件不完整")}
      </Typography>
      {running && (
        <Box
          sx={{
            width: 7,
            height: 7,
            borderRadius: "50%",
            bgcolor: "success.main",
            flexShrink: 0,
          }}
        />
      )}
    </ListItemButton>
  );
}
