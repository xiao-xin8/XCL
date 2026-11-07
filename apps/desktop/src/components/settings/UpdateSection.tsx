import { Box, Button, Chip, Stack, Typography, alpha } from "@mui/material";
import type { ConfigPatch } from "../../lib/config";
import { BUTTON_SX } from "../../lib/buttons";
import type { AppConfig, UpdateChannel, UpdateView } from "../../lib/types";
import { useUpdateActions, useUpdateState } from "../../lib/updateContext";
import SettingGroup from "./SettingGroup";
import SettingRow from "./SettingRow";
import { SelectRow, SwitchRow } from "./controls";

const CHANNELS: { value: UpdateChannel; label: string }[] = [
  { value: "stable", label: "稳定版" },
  { value: "preview", label: "预览版（含测试版）" },
];

/**
 * 设置 → 更新。
 *
 * 不单开顶部页签：更新是低频动作，占一个一级入口不值得（`TABS.md` 定过
 * 顶部只有三个页签）。放在设置里单独一类，既能被找到，也不用挤导航。
 *
 * 这里只管「看状态 + 手动触发」，真正的流程都在 `UpdateDialog` 里 ——
 * 两处不重复画进度。
 */
export default function UpdateSection(props: {
  config: AppConfig;
  patch: (patch: ConfigPatch) => void;
}) {
  const { config, patch } = props;
  const update = config.update;
  const { view } = useUpdateState();
  const { check, openDialog } = useUpdateActions();

  const phase = view?.phase ?? "idle";
  const current = view?.currentVersion ?? "0.1.0";
  const busy = phase === "checking" || phase === "downloading" || phase === "verifying";

  return (
    <>
      <SettingGroup title="版本" desc="更新只替换 XCL.exe，data/ 里的存档、账号、模组都不动。">
        <SettingRow
          title={`Xin Craft Launcher ${current}`}
          desc={versionSubtitle(view, phase)}
        >
          <Stack direction="row" sx={{ alignItems: "center", gap: 1 }}>
            {phase !== "checking" && <StatusChip view={view} />}
            <Button
              variant="outlined"
              onClick={() => void check(false)}
              disabled={busy}
              sx={BUTTON_SX}
            >
              检查更新
            </Button>
          </Stack>
        </SettingRow>

        {(phase === "available" || phase === "ready") && (
          <SettingRow
            title={
              phase === "ready"
                ? "更新包已下载完成"
                : `可更新到 ${view?.latestVersion ?? ""}`
            }
            desc={
              phase === "ready"
                ? "重启启动器即可完成替换；替换期间会有一个小窗口提示进度。"
                : "打开更新窗口查看这次改了什么。"
            }
          >
            <Button variant="contained" onClick={openDialog} sx={BUTTON_SX}>
              {phase === "ready" ? "查看" : "查看更新"}
            </Button>
          </SettingRow>
        )}
      </SettingGroup>

      {(view?.changelog.length ?? 0) > 0 && phase === "available" && (
        <SettingGroup title={`${view?.latestVersion ?? ""} 更新内容`}>
          <Box sx={{ px: 2, py: 1.5 }}>
            <Stack component="ul" sx={{ m: 0, pl: 0, listStyle: "none", gap: 0.75 }}>
              {view?.changelog.map((line, index) => (
                <Stack
                  component="li"
                  key={index}
                  direction="row"
                  sx={{ gap: 1, alignItems: "flex-start" }}
                >
                  <Box
                    aria-hidden
                    sx={{
                      width: 5,
                      height: 5,
                      mt: "8px",
                      flexShrink: 0,
                      borderRadius: "50%",
                      bgcolor: "primary.main",
                    }}
                  />
                  <Typography sx={{ fontSize: 12, lineHeight: 1.75, color: "text.secondary" }}>
                    {line}
                  </Typography>
                </Stack>
              ))}
            </Stack>
          </Box>
        </SettingGroup>
      )}

      <SettingGroup title="更新设置">
        <SwitchRow
          title="启动时检查更新"
          desc="每次打开向云端取一次版本信息。发现了会提示，但不会自己装。"
          checked={update.autoCheck}
          onChange={(next) => patch({ update: { autoCheck: next } })}
        />

        <SelectRow
          title="更新频道"
          desc="稳定版只在正式版本发布时提示；预览版会提前收到测试版。"
          value={update.channel}
          options={CHANNELS}
          onChange={(next) => patch({ update: { channel: next } })}
          width={190}
        />
      </SettingGroup>
    </>
  );
}

/** 状态胶囊。检查中不显示 —— 规范不允许给加载过程配文字。 */
function StatusChip({ view }: { view: UpdateView | null }) {
  if (!view) {
    return <Chip size="small" label="还没检查" sx={{ height: 22, fontSize: 11 }} />;
  }
  switch (view.phase) {
    case "available":
      return (
        <Chip
          size="small"
          color="primary"
          label={`有新版本 ${view.latestVersion ?? ""}`}
          sx={{ height: 22, fontSize: 11 }}
        />
      );
    case "upToDate":
      return <Chip size="small" label="已是最新" sx={{ height: 22, fontSize: 11 }} />;
    case "downloading":
      return (
        <Chip
          size="small"
          label={`下载中 ${Math.round(view.fraction * 100)}%`}
          sx={{ height: 22, fontSize: 11 }}
        />
      );
    case "ready":
      return (
        <Chip size="small" color="primary" label="等待重启" sx={{ height: 22, fontSize: 11 }} />
      );
    case "failed":
      return (
        <Chip
          size="small"
          label={view.error?.title ?? "更新失败"}
          sx={{
            height: 22,
            fontSize: 11,
            bgcolor: (t) => alpha(t.palette.error.main, 0.1),
            color: "error.main",
          }}
        />
      );
    default:
      return null;
  }
}

function versionSubtitle(view: UpdateView | null, phase: string): string {
  if (phase === "failed") return view?.error?.message ?? "上一次更新没有成功";
  if (view?.forced) return "这个版本必须更新后才能继续使用";
  return "便携版，整个文件夹拷走就能用。";
}
