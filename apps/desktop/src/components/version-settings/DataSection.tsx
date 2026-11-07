import { Button, Typography } from "@mui/material";
import { formatBytes, formatRelativeTime } from "../../lib/format";
import type { VersionStats } from "../../lib/types";
import SettingGroup from "../settings/SettingGroup";
import { ActionRow, InfoRow } from "../settings/controls";

/**
 * 数据：这个版本在磁盘上到底吃了多少。
 *
 * 数字是后端扫出来的（隔离版本扫版本目录，不隔离的扫数据根），
 * 文件特别多的时候有上限，所以这里只当「大概多大」看。
 */
export default function DataSection(props: {
  stats: VersionStats | null;
  onRefresh: () => void;
}) {
  const { stats, onRefresh } = props;

  return (
    <>
      <SettingGroup title="占用与内容" desc="统计的是游戏目录里的东西，不是共享的库和资源。">
        <InfoRow title="占用空间" value={stats ? formatBytes(stats.sizeBytes) : "统计中"} />
        <InfoRow title="Mod" value={stats ? `${stats.mods} 个` : "统计中"} />
        <InfoRow title="存档" value={stats ? `${stats.saves} 个世界` : "统计中"} />
        <InfoRow title="截图" value={stats ? `${stats.screenshots} 张` : "统计中"} />
        <InfoRow
          title="资源包 / 材质包"
          value={stats ? `${stats.resourcepacks} 个` : "统计中"}
        />
        <InfoRow
          title="上次启动"
          value={stats ? formatRelativeTime(stats.lastPlayedMs) : "统计中"}
        />
        <ActionRow title="重新统计" desc="刚下了几个 mod、删了几个存档，点一下重新算。">
          <Button size="small" onClick={onRefresh}>
            重新统计
          </Button>
        </ActionRow>
      </SettingGroup>

      {stats && (
        <Typography
          sx={{ px: 1, fontSize: 11.5, color: "text.secondary", lineHeight: 1.7, wordBreak: "break-all" }}
        >
          统计范围：{stats.gameDir}
        </Typography>
      )}
    </>
  );
}
