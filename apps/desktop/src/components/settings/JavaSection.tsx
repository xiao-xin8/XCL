import { useCallback, useEffect, useState } from "react";
import { Box, Button, Chip, Stack, Typography } from "@mui/material";
import { invoke } from "@tauri-apps/api/core";
import type { AppConfig, AppError, SystemJava } from "../../lib/types";
import { toAppError } from "../../lib/types";
import ErrorState from "../ErrorState";
import type { ConfigPatch } from "../../lib/config";
import Loading from "../Loading";
import SettingGroup from "./SettingGroup";
import { ActionRow, SwitchRow, TextRow } from "./controls";

/**
 * Java 设置。
 *
 * 这里只读不写：列出本机扫出来的 Java，确认「启动时能用哪个」。
 * 下载 / 删除属于管理，统一放在「下载 → Java 运行环境」那一页 ——
 * 同一件事只在能干活的地方做，设置页不摆两个半成品入口。
 */
export default function JavaSection(props: {
  config: AppConfig;
  patch: (patch: ConfigPatch) => void;
}) {
  const { config, patch } = props;
  const java = config.java;

  const [list, setList] = useState<SystemJava[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<AppError | null>(null);

  const load = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      setList(await invoke<SystemJava[]>("java_detected"));
    } catch (e: unknown) {
      setError(toAppError(e, "扫描本机 Java 失败"));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  return (
    <>
      <SettingGroup
        title="自动选择"
        desc="启动时按版本要求挑一个；找不到就先看你指定的，再看 data/java/ 里下过的。"
      >
        <SwitchRow
          title="缺 Java 时自动下载"
          desc="找不到合适的 Java 就自己下一个，装在 data/java 里。你的系统环境我们一根手指都不碰。"
          checked={java.autoDownload}
          onChange={(next) => patch({ java: { autoDownload: next } })}
        />

        <TextRow
          title="手动指定 Java"
          desc="填了就只用它，哪怕版本不匹配。这是给你留的后门，不是给新手用的；留空则自动探测。"
          value={java.manualPath ?? ""}
          onChange={(next) =>
            patch({ java: { manualPath: next.trim().length === 0 ? null : next.trim() } })
          }
          placeholder="留空 = 自动探测"
        />
      </SettingGroup>

      <SettingGroup
        title={`本机 Java（${list.length}）`}
        desc="启动时从这几个里挑版本合适的那个。要下载或删除 Java，去「下载 → Java 运行环境」。"
      >
        <Box sx={{ px: 2, py: 1 }}>
          {loading ? (
            <Box sx={{ display: "grid", placeItems: "center", py: 6 }}>
              <Loading />
            </Box>
          ) : error ? (
            <ErrorState variant="inline" error={error} onRetry={() => void load()} />
          ) : list.length === 0 ? (
            <Typography sx={{ fontSize: 12.5, color: "text.secondary", py: 2, lineHeight: 1.7 }}>
              一个都没扫到。去「下载 → Java 运行环境」装一个，或者在上面手动指定路径。
            </Typography>
          ) : (
            list.map((item) => (
              <Stack
                key={item.path}
                direction="row"
                spacing={1}
                sx={{ alignItems: "baseline", py: 0.75 }}
              >
                <Typography
                  sx={{ fontSize: 12.5, fontWeight: 600, minWidth: 58, flexShrink: 0 }}
                >
                  Java {item.major}
                </Typography>
                <Typography
                  sx={{
                    flex: 1,
                    minWidth: 0,
                    fontSize: 11.5,
                    color: "text.secondary",
                    wordBreak: "break-all",
                  }}
                >
                  {item.version} · {item.path}
                </Typography>
                {item.managed && (
                  <Chip
                    size="small"
                    variant="outlined"
                    label="XCL 管理"
                    sx={{ height: 20, fontSize: 10.5, flexShrink: 0 }}
                  />
                )}
              </Stack>
            ))
          )}
        </Box>

        <ActionRow
          title="重新扫描"
          desc="刚装了新 Java，或者手动挪过目录？扫一遍。"
        >
          <Button size="small" variant="outlined" disabled={loading} onClick={() => void load()}>
            重新扫描
          </Button>
        </ActionRow>
      </SettingGroup>
    </>
  );
}
