import { useState } from "react";
import { Button, Stack, Typography } from "@mui/material";
import { invoke } from "@tauri-apps/api/core";
import { toAppError, type AppConfig, type MirrorProvider } from "../../lib/types";
import type { ConfigPatch } from "../../lib/config";
import SettingGroup from "./SettingGroup";
import SettingRow from "./SettingRow";
import { NumberRow, SelectRow, SwitchRow, TextRow } from "./controls";

const STRATEGY_OPTIONS = [
  { value: "auto", label: "自动（推荐）" },
  { value: "mirrorFirst", label: "镜像优先" },
  { value: "officialFirst", label: "官方优先" },
] as const;

const PROVIDER_OPTIONS = [
  { value: "bmclapi", label: "BMCLAPI 镜像" },
  { value: "official", label: "Mojang 官方" },
  { value: "custom", label: "自定义镜像" },
] as const;

export default function DownloadSection(props: {
  config: AppConfig;
  patch: (patch: ConfigPatch) => void;
}) {
  const { config, patch } = props;
  const { mirror, download } = config;

  const providerKind: (typeof PROVIDER_OPTIONS)[number]["value"] = mirror.provider.kind;
  const customUrl = mirror.provider.kind === "custom" ? mirror.provider.value : "";
  const autoConcurrency = download.concurrency === 0;
  /** 选了自定义源但地址是空的：测了也只会走官方，先补上 */
  const customMissing = providerKind === "custom" && customUrl.trim().length === 0;

  const [testing, setTesting] = useState(false);
  const [testResult, setTestResult] = useState<{ ok: boolean; text: string } | null>(
    null,
  );

  const setProvider = (kind: (typeof PROVIDER_OPTIONS)[number]["value"]) => {
    const provider: MirrorProvider =
      kind === "custom" ? { kind: "custom", value: customUrl } : { kind };
    // 换了源，上一次的测试结果就不再代表现在了
    setTestResult(null);
    patch({ mirror: { provider } });
  };

  /**
   * 测试连接：后端真的拉一次版本清单并解析。
   *
   * 设置是即时保存的，所以这里测的就是「当前选中并已落盘的源」。
   * 最长要等网络超时，按钮在等待期间禁用。
   */
  const testSource = async () => {
    setTesting(true);
    setTestResult(null);
    try {
      const result = await invoke<{ source: string; ms: number }>("download_source_test");
      setTestResult({ ok: true, text: `${result.source} · ${result.ms} ms` });
    } catch (error: unknown) {
      const err = toAppError(error, "测试连接失败");
      setTestResult({ ok: false, text: `${err.title}：${err.message}` });
    } finally {
      setTesting(false);
    }
  };

  return (
    <>
      <SettingGroup
        title="下载源"
        desc="官方源最快的时候能跑满带宽；BMCLAPI 慢一点，但在某些网络下官方源根本连不上。"
      >
        <SelectRow
          title="下载源"
          desc="文件和版本清单都从这里取。换源后缓存会自动失效。"
          value={providerKind}
          options={[...PROVIDER_OPTIONS]}
          onChange={setProvider}
          width={180}
        />

        {providerKind === "custom" && (
          <TextRow
            title="自定义镜像地址"
            desc="填镜像根地址，不带结尾斜杠。必须是完整 http(s) 地址。"
            value={customUrl}
            onChange={(next) => {
              // 地址改过，上一次的测试结论就不算数了
              setTestResult(null);
              patch({ mirror: { provider: { kind: "custom", value: next.trim() } } });
            }}
            placeholder="https://mirror.example.com"
          />
        )}

        <SelectRow
          title="文件下载优先"
          desc="下载具体文件时先试谁。镜像挂了会自动回落到另一个，不用手动切。"
          value={mirror.downloadStrategy}
          options={[...STRATEGY_OPTIONS]}
          onChange={(next) => {
            setTestResult(null);
            patch({ mirror: { downloadStrategy: next } });
          }}
          width={180}
        />

        <SelectRow
          title="版本列表优先"
          desc="镜像的版本清单可能慢半拍。急着玩新版本的话选「官方优先」。"
          value={mirror.versionStrategy}
          options={[...STRATEGY_OPTIONS]}
          onChange={(next) => {
            setTestResult(null);
            patch({ mirror: { versionStrategy: next } });
          }}
          width={180}
        />

        <SettingRow
          title="测试连接"
          desc="真的拉一次版本清单并解析。返回 200 却给了错误页的镜像同样算失败。"
          block
        >
          <Stack
            direction="row"
            spacing={1}
            sx={{ alignItems: "center", minWidth: 0, width: "100%" }}
          >
            <Button
              size="small"
              variant="outlined"
              disabled={testing || customMissing}
              onClick={() => void testSource()}
              sx={{ flexShrink: 0 }}
            >
              {testing ? "测试中…" : "测试"}
            </Button>
            {testResult ? (
              <Typography
                sx={{
                  fontSize: 11.5,
                  lineHeight: 1.6,
                  color: testResult.ok ? "success.main" : "error.main",
                  minWidth: 0,
                  wordBreak: "break-all",
                }}
              >
                {testResult.text}
              </Typography>
            ) : (
              customMissing && (
                <Typography sx={{ fontSize: 11.5, color: "text.secondary" }}>
                  先填上面的自定义镜像地址
                </Typography>
              )
            )}
          </Stack>
        </SettingRow>
      </SettingGroup>

      <SettingGroup title="性能" desc="默认值都是实测调出来的，不确定就别动。">
        <SwitchRow
          title="自动决定并发数"
          desc="官方源扛得住，用 64；镜像高并发会 403，用 16。手动调到 128 不会更快，只会让服务器觉得你在攻击它。"
          checked={autoConcurrency}
          onChange={(next) =>
            patch({ download: { concurrency: next ? 0 : Math.max(1, download.concurrency) } })
          }
        />

        <NumberRow
          title="并发下载任务数"
          desc="同时下几个文件。注意这是任务数，不是线程数。"
          value={autoConcurrency ? 0 : download.concurrency}
          min={1}
          max={128}
          disabled={autoConcurrency}
          onChange={(next) => patch({ download: { concurrency: next } })}
          unit="个"
        />

        <NumberRow
          title="速度限制"
          desc="填 0 就是不限速。限速对「分片下载」同样生效，是按总带宽算的。"
          value={download.speedLimitKbps}
          min={0}
          max={1_000_000}
          onChange={(next) => patch({ download: { speedLimitKbps: next } })}
          unit="KB/s"
          width={140}
        />

        <NumberRow
          title="失败重试次数"
          desc="同一个地址失败后再试几次。多试能盖住偶发的连接重置；坏地址只会让你多等一会儿。"
          value={download.retries}
          min={0}
          max={10}
          onChange={(next) => patch({ download: { retries: next } })}
          unit="次"
          width={120}
        />
      </SettingGroup>

      <SettingGroup
        title="连接（高级）"
        desc="一个文件内部开几条连接抢着下。BMCLAPI 不支持分片，选它的时候这里会被强制关掉。"
      >
        <SwitchRow
          title="单文件多连接"
          desc="大文件下载提速主要靠它。16 条是实测最优：32 条会被服务器拒掉一部分请求。"
          checked={download.splitEnabled}
          onChange={(next) => patch({ download: { splitEnabled: next } })}
        />

        <NumberRow
          title="单文件连接数"
          value={download.splitConnections}
          min={2}
          max={64}
          disabled={!download.splitEnabled}
          onChange={(next) => patch({ download: { splitConnections: next } })}
          unit="条"
        />

        <NumberRow
          title="全局连接上限"
          desc="所有文件加起来的天花板。默认 32 = 两个大文件同时全速。"
          value={download.splitTotalConnections}
          min={2}
          max={128}
          disabled={!download.splitEnabled}
          onChange={(next) => patch({ download: { splitTotalConnections: next } })}
          unit="条"
        />
      </SettingGroup>
    </>
  );
}
