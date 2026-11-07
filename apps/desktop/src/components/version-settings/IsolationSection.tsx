import { Box, Button, ToggleButton, ToggleButtonGroup, Typography } from "@mui/material";
import { invoke } from "@tauri-apps/api/core";
import { foldersFor } from "../../lib/versionFolders";
import { toAppError, type IsolationView, type LocalVersion } from "../../lib/types";
import { useToast } from "../Toast";
import SettingGroup from "../settings/SettingGroup";
import { InfoRow } from "../settings/controls";

/**
 * 隔离与目录。
 *
 * 隔离只换游戏目录（存档 / mods / 配置 / 日志），versions、libraries、
 * assets、Java 始终共享 —— 所以这里只有游戏目录会跟着变。
 */
export default function IsolationSection(props: {
  version: LocalVersion;
  isolation: IsolationView | null;
  onIsolationChanged: (view: IsolationView) => void;
}) {
  const { version, isolation, onIsolationChanged } = props;
  const toast = useToast();

  const mode = !isolation
    ? "follow"
    : isolation.source === "user"
      ? isolation.enabled
        ? "on"
        : "off"
      : "follow";

  const apply = async (next: "follow" | "on" | "off") => {
    if (!version) return;
    try {
      const view = await invoke<IsolationView>("version_set_isolation", {
        versionId: version.id,
        isolate: next === "follow" ? null : next === "on",
      });
      onIsolationChanged(view);
    } catch (e: unknown) {
      toast(toAppError(e, "设置版本隔离失败").message, "error");
    }
  };

  const openFolder = async (folder: string) => {
    try {
      await invoke("version_open_folder", { versionId: version.id, folder });
    } catch (e: unknown) {
      toast(toAppError(e, "打开文件夹失败").message, "error");
    }
  };

  return (
    <>
      <SettingGroup
        title="版本隔离"
        desc={
          isolation
            ? `当前：${isolation.enabled ? "已隔离" : "未隔离"} · ${isolation.sourceLabel}`
            : "读取中"
        }
      >
        <Box sx={{ px: 2, py: 1.5 }}>
          <ToggleButtonGroup
            exclusive
            size="small"
            value={mode}
            onChange={(_, next: "follow" | "on" | "off" | null) => {
              if (next) void apply(next);
            }}
            aria-label="版本隔离"
          >
            <ToggleButton value="follow" sx={{ px: 2, fontSize: 12.5 }}>
              跟随全局
            </ToggleButton>
            <ToggleButton value="on" sx={{ px: 2, fontSize: 12.5 }}>
              隔离
            </ToggleButton>
            <ToggleButton value="off" sx={{ px: 2, fontSize: 12.5 }}>
              不隔离
            </ToggleButton>
          </ToggleButtonGroup>
          <Typography sx={{ mt: 1, fontSize: 11.5, color: "text.secondary", lineHeight: 1.7 }}>
            隔离之后存档、mods、配置都住在版本目录里，删版本它们是跟着走的；
            全局策略现在是「{isolation?.policyLabel ?? "读取中"}」。
          </Typography>
        </Box>
        <InfoRow
          title="游戏目录"
          desc={isolation?.gameDir}
          value={isolation?.gameDirLabel ?? "读取中"}
        />
      </SettingGroup>

      <SettingGroup title="文件夹" desc="装完 mod 想看看放哪了，从这里进最省事。">
        <Box
          sx={{
            px: 1.5,
            py: 1.5,
            display: "grid",
            gap: 0.75,
            gridTemplateColumns: "repeat(2, minmax(0, 1fr))",
          }}
        >
          {foldersFor(version, isolation?.enabled === true).map(
            ({ key, label, icon: Icon }) => (
              <Button
                key={key}
                size="small"
                variant="outlined"
                startIcon={<Icon sx={{ fontSize: 16 }} />}
                disabled={!version.valid && key !== "version"}
                onClick={() => void openFolder(key)}
                sx={{ minHeight: 38, fontSize: 13, justifyContent: "flex-start" }}
              >
                {label}
              </Button>
            ),
          )}
        </Box>
      </SettingGroup>
    </>
  );
}
