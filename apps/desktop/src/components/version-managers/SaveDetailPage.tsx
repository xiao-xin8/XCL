import { useCallback, useState } from "react";
import {
  Box,
  Button,
  ButtonBase,
  Chip,
  Paper,
  Stack,
  Typography,
  alpha,
} from "@mui/material";
import ChevronRightOutlinedIcon from "@mui/icons-material/ChevronRightOutlined";
import MapOutlinedIcon from "@mui/icons-material/MapOutlined";
import { invoke } from "@tauri-apps/api/core";
import { copyText } from "../../lib/clipboard";
import { formatBytes, formatDateTime, formatRelativeTime } from "../../lib/format";
import { DIFFICULTY_LABEL, GAME_MODE_LABEL, versionLabel } from "../../lib/saves";
import { toAppError, type SaveListView, type SaveView } from "../../lib/types";
import SettingGroup from "../settings/SettingGroup";
import { ActionRow, DangerRow, InfoRow } from "../settings/controls";
import { useToast } from "../Toast";

/**
 * 单个存档的详情页（从存档管理器点卡片进入，标题栏返回）。
 *
 * 数据全部来自列表已经算好的 `SaveView`，这一页不发任何请求 ——
 * 存档没有线上信息可拉，`level.dat` 也已经在列表阶段读完了。
 *
 * 地图只给一个入口，不在详情页里画缩略图：那要么糊，要么慢，还不如直接进
 * 可拖动缩放的那一页。
 *
 * 危险区只放删除。重命名世界要写 `level.dat`（NBT 字符串变长要整体重排），
 * 单独一批做，不混进这版。
 */
export default function SaveDetailPage(props: {
  versionId: string;
  item: SaveView;
  /** 点地图预览卡：进整页地图 */
  onOpenMap: () => void;
  /** 删除成功：父组件换掉整张表并关掉详情 */
  onDeleted: (next: SaveListView) => void;
}) {
  const { versionId, item, onOpenMap, onDeleted } = props;
  const toast = useToast();

  const [confirming, setConfirming] = useState(false);
  const [deleting, setDeleting] = useState(false);

  const subtitle = item.broken
    ? item.folderName
    : [
        versionLabel(item),
        item.lastPlayedMs
          ? `上次游玩 ${formatRelativeTime(item.lastPlayedMs)}`
          : "没有游玩记录",
        formatBytes(item.sizeBytes),
      ].join(" · ");

  const openFolder = useCallback(() => {
    void invoke("save_open_folder", { versionId, folderName: item.folderName }).catch(
      (error: unknown) => {
        toast(toAppError(error, "打开存档文件夹失败").message, "error");
      },
    );
  }, [item.folderName, toast, versionId]);

  const copySeed = useCallback(() => {
    if (!item.seed) return;
    void copyText(item.seed).then((ok) => {
      toast(ok ? "种子已复制" : "复制失败，请手动选择文本", ok ? "success" : "error");
    });
  }, [item.seed, toast]);

  const remove = useCallback(async () => {
    if (!confirming) {
      setConfirming(true);
      return;
    }
    setDeleting(true);
    try {
      const next = await invoke<SaveListView>("save_delete", {
        versionId,
        folderName: item.folderName,
      });
      onDeleted(next);
      toast("已移到版本的回收站", "success");
    } catch (error: unknown) {
      toast(toAppError(error, "删除存档失败").message, "error");
    } finally {
      setDeleting(false);
    }
  }, [confirming, item.folderName, onDeleted, toast, versionId]);

  const modeLabel = item.gameMode ? GAME_MODE_LABEL[item.gameMode] : null;
  const difficultyLabel = item.difficulty ? DIFFICULTY_LABEL[item.difficulty] : null;

  return (
    <Box sx={{ flex: 1, minHeight: 0, display: "flex", flexDirection: "column", p: 2 }}>
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
        <Paper
          variant="outlined"
          sx={{
            borderRadius: 1,
            p: 3,
            display: "flex",
            alignItems: "center",
            gap: 2,
          }}
        >
          <Box
            sx={{
              width: 64,
              height: 64,
              flexShrink: 0,
              borderRadius: 1,
              overflow: "hidden",
              bgcolor: "action.hover",
              display: "grid",
              placeItems: "center",
            }}
          >
            {item.icon ? (
              <Box
                component="img"
                src={item.icon}
                alt=""
                loading="lazy"
                sx={{
                  width: "100%",
                  height: "100%",
                  objectFit: "cover",
                  // 存档封面本来就是 64×64，放大要保住像素块
                  imageRendering: "pixelated",
                }}
              />
            ) : (
              <Typography sx={{ fontSize: 10, color: "text.secondary" }}>
                无封面
              </Typography>
            )}
          </Box>
          <Box sx={{ flex: 1, minWidth: 0 }}>
            <Typography
              sx={{ fontSize: 20, fontWeight: 700, letterSpacing: -0.3, lineHeight: 1.3 }}
            >
              {item.name}
            </Typography>
            <Typography noWrap sx={{ fontSize: 12, color: "text.secondary", mt: 0.5 }}>
              {subtitle}
            </Typography>
            <Stack direction="row" sx={{ gap: 0.75, mt: 1, flexWrap: "wrap" }}>
              {modeLabel && (
                <Chip size="small" label={modeLabel} sx={{ height: 18, fontSize: 10 }} />
              )}
              {difficultyLabel && (
                <Chip
                  size="small"
                  label={difficultyLabel}
                  sx={{ height: 18, fontSize: 10 }}
                />
              )}
              {item.hardcore && (
                <Chip
                  size="small"
                  label="硬核"
                  sx={{
                    height: 18,
                    fontSize: 10,
                    bgcolor: (t) => alpha(t.palette.warning.main, 0.18),
                    color: "warning.main",
                  }}
                />
              )}
              {item.broken && (
                <Chip
                  size="small"
                  label="读不出 level.dat"
                  sx={{
                    height: 18,
                    fontSize: 10,
                    bgcolor: (t) => alpha(t.palette.warning.main, 0.18),
                    color: "warning.main",
                  }}
                />
              )}
            </Stack>
          </Box>
        </Paper>

        {/* 地图只有一个入口：详情页里画缩略图没意义，那要么糊要么慢 */}
        <Paper variant="outlined" sx={{ borderRadius: 1, overflow: "hidden" }}>
          <ButtonBase
            onClick={onOpenMap}
            aria-label={`打开 ${item.name} 的地图`}
            sx={{
              width: 1,
              px: 2,
              py: 1.5,
              display: "flex",
              alignItems: "center",
              gap: 1.5,
              textAlign: "left",
            }}
          >
            <MapOutlinedIcon sx={{ fontSize: 21, color: "text.secondary" }} />
            <Box sx={{ flex: 1, minWidth: 0 }}>
              <Typography sx={{ fontSize: 13.5, fontWeight: 600 }}>地图</Typography>
              <Typography sx={{ fontSize: 11.5, color: "text.secondary", mt: 0.25 }}>
                拖动平移、滚轮缩放，按需渲染
              </Typography>
            </Box>
            <ChevronRightOutlinedIcon sx={{ fontSize: 20, color: "text.secondary" }} />
          </ButtonBase>
        </Paper>

        <SettingGroup title="基本信息">
          <InfoRow title="文件夹" value={item.folderName} />
          {item.broken ? (
            <Box sx={{ px: 2, py: 1.5 }}>
              <Typography
                sx={{ fontSize: 11.5, color: "text.secondary", lineHeight: 1.7 }}
              >
                读不出 level.dat，只能显示目录名和占用。世界可能来自更老的版本，或者文件
                已经损坏 —— 这时候照样可以打开目录或者删掉它。
              </Typography>
            </Box>
          ) : (
            <>
              <InfoRow title="游戏版本" value={versionLabel(item)} />
              <InfoRow
                title="数据版本"
                value={item.dataVersion != null ? String(item.dataVersion) : "未识别"}
              />
              <InfoRow title="游戏模式" value={modeLabel ?? "未识别"} />
              <InfoRow title="难度" value={difficultyLabel ?? "未识别"} />
              <InfoRow title="硬核" value={item.hardcore ? "是" : "否"} />
              <InfoRow title="允许作弊" value={item.cheats ? "是" : "否"} />
              {item.spawnX != null && item.spawnZ != null && (
                <InfoRow title="出生点" value={`${item.spawnX}, ${item.spawnZ}`} />
              )}
            </>
          )}
          <InfoRow title="大小" value={formatBytes(item.sizeBytes)} />
          {item.lastPlayedMs != null && (
            <InfoRow
              title="最后游玩"
              value={`${formatDateTime(new Date(item.lastPlayedMs).toISOString())}（${formatRelativeTime(item.lastPlayedMs)}）`}
            />
          )}
        </SettingGroup>

        <SettingGroup title="世界操作">
          {item.seed && (
            <InfoRow
              title="世界种子"
              desc="新建同种子的世界时用得上"
              value={item.seed}
              action={
                <Button size="small" variant="outlined" onClick={copySeed}>
                  复制
                </Button>
              }
            />
          )}
          <ActionRow title="存档位置" desc={`saves/${item.folderName}`}>
            <Button size="small" variant="outlined" onClick={openFolder}>
              打开文件夹
            </Button>
          </ActionRow>
        </SettingGroup>

        <SettingGroup title="危险操作" tone="danger">
          <DangerRow
            title="删除存档"
            desc={`整个世界文件夹会移到 versions/${versionId}/XCL/trash/saves，不会直接永久删除`}
            label="删除"
            confirmLabel={deleting ? "正在删除…" : "确认删除"}
            confirming={confirming}
            onAction={() => void remove()}
          />
        </SettingGroup>
      </Box>
    </Box>
  );
}
