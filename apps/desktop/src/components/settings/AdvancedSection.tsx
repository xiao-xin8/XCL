import { useCallback, useEffect, useState } from "react";
import {
  Box,
  Button,
  Dialog,
  DialogActions,
  DialogContent,
  DialogContentText,
  DialogTitle,
} from "@mui/material";
import { invoke } from "@tauri-apps/api/core";
import { formatBytes } from "../../lib/format";
import { setAppearance } from "../../lib/appearance";
import type { ConfigPatch } from "../../lib/config";
import { toAppError, type AppConfig, type SystemJava, type TrimReport, type UnusedReport } from "../../lib/types";
import { useToast } from "../Toast";
import Loading from "../Loading";
import SettingGroup from "./SettingGroup";
import { ActionRow, DangerRow, InfoRow, SelectRow } from "./controls";

type AppInfo = {
  name: string;
  version: string;
  targetOs: string;
  targetArch: string;
  debug: boolean;
};

type CacheEntry = {
  kind: "http" | "temp" | "skin";
  label: string;
  desc: string;
  bytes: number;
};

const LOG_OPTIONS = [
  { value: "error", label: "只记错误" },
  { value: "warn", label: "警告（默认）" },
  { value: "info", label: "常规" },
  { value: "debug", label: "调试" },
  { value: "trace", label: "详细" },
] as const;

/** 复制文本：优先用剪贴板 API，WebView 里失败就退回隐藏 textarea。 */
async function copyText(text: string): Promise<boolean> {
  try {
    await navigator.clipboard.writeText(text);
    return true;
  } catch {
    try {
      const area = document.createElement("textarea");
      area.value = text;
      area.style.position = "fixed";
      area.style.opacity = "0";
      document.body.appendChild(area);
      area.select();
      const ok = document.execCommand("copy");
      document.body.removeChild(area);
      return ok;
    } catch {
      return false;
    }
  }
}

/**
 * 高级：目录、缓存、诊断、配置导入导出、恢复默认。
 *
 * 「关于」单独一页（官网式介绍），这里只放能操作的东西 ——
 * 把它和关于混在一起时，一半是故事一半是按钮，两边都不好用。
 */
function unusedTotal(report: UnusedReport): number {
  return report.librariesBytes + report.indexesBytes + report.objectsBytes;
}

function unusedDesc(report: UnusedReport | null, scanning: boolean): string {
  if (scanning) return "正在统计 libraries 和 assets…";
  if (report === null) {
    return "删过版本之后，可能有库和资源已经没人用了。扫一下看看能腾出多少。";
  }
  const total = unusedTotal(report);
  if (total === 0) return "没有可清理的内容。";
  const files = report.librariesFiles + report.indexesFiles + report.objectsFiles;
  return `可清理 ${files} 个文件，共 ${formatBytes(total)}。存档、mod 和版本设置都不动。`;
}

export default function AdvancedSection(props: {
  config: AppConfig;
  patch: (patch: ConfigPatch) => void;
  onReset: () => void;
  /** 导入后重新拉一次配置（主题 / 动画要立刻换） */
  onReload: () => void;
}) {
  const { config, patch, onReset, onReload } = props;
  const toast = useToast();

  const [info, setInfo] = useState<AppInfo | null>(null);
  const [cache, setCache] = useState<CacheEntry[] | null>(null);
  const [confirmReset, setConfirmReset] = useState(false);
  const [confirmClearAll, setConfirmClearAll] = useState(false);
  const [confirmImport, setConfirmImport] = useState(false);
  const [clearing, setClearing] = useState<string | null>(null);
  const [trimming, setTrimming] = useState(false);
  const [unused, setUnused] = useState<UnusedReport | null>(null);
  const [scanning, setScanning] = useState(false);
  const [cleaning, setCleaning] = useState(false);
  const [confirmUnused, setConfirmUnused] = useState(false);

  /** 扫一遍「没有任何版本引用」的库和资源。只统计，不删。 */
  const scanUnused = useCallback(async () => {
    setScanning(true);
    setConfirmUnused(false);
    try {
      setUnused(await invoke<UnusedReport>("storage_unused_scan"));
    } catch (e: unknown) {
      toast(toAppError(e, "扫描未使用资源失败").message, "error");
    } finally {
      setScanning(false);
    }
  }, [toast]);

  const cleanUnused = useCallback(async () => {
    setCleaning(true);
    try {
      const report = await invoke<UnusedReport>("storage_unused_clean");
      setUnused(report);
      setConfirmUnused(false);
      const freed = unusedTotal(report);
      toast(
        freed > 0 ? `清理完成，释放 ${formatBytes(freed)}` : "没有可清理的内容",
        freed > 0 ? "success" : "info",
      );
    } catch (e: unknown) {
      toast(toAppError(e, "清理未使用资源失败").message, "error");
    } finally {
      setCleaning(false);
    }
  }, [toast]);

  /**
   * 手动清一次内存。
   *
   * 不解释"深度 / 标准"这些内部区别 —— 只关心清出来多少。
   * 没有明显变化也照样说一句，别让人以为按钮没反应。
   */
  const trimMemory = useCallback(async () => {
    setTrimming(true);
    try {
      const report = await invoke<TrimReport>("memory_trim");
      if (report.freedMb > 0) {
        toast(
          `清理完成，可用内存增加 ${formatBytes(report.freedMb * 1024 * 1024)}`,
          "success",
        );
      } else {
        toast("清理完成，可用内存没有明显变化", "info");
      }
    } catch (e: unknown) {
      toast(toAppError(e, "清理内存失败").message, "error");
    } finally {
      setTrimming(false);
    }
  }, [toast]);

  const loadCache = useCallback(async () => {
    try {
      setCache(await invoke<CacheEntry[]>("storage_info"));
    } catch (e: unknown) {
      toast(toAppError(e, "读取缓存信息失败").title, "error");
    }
  }, [toast]);

  useEffect(() => {
    let alive = true;
    void invoke<AppInfo>("app_info")
      .then((data) => {
        if (alive) setInfo(data);
      })
      .catch(() => {
        // 拿不到版本号不影响这一页，静默
      });
    void loadCache();
    return () => {
      alive = false;
    };
  }, [loadCache]);

  const openDir = useCallback(
    async (dir: "data" | "cache") => {
      try {
        await invoke("app_open_dir", { dir });
      } catch (e: unknown) {
        const err = toAppError(e, "打开文件夹失败");
        toast(`${err.title}：${err.message}`, "error");
      }
    },
    [toast],
  );

  const clearCache = useCallback(
    async (kind: string) => {
      setClearing(kind);
      try {
        await invoke("storage_clear", { kind });
        await loadCache();
        toast("缓存已清理", "success");
      } catch (e: unknown) {
        const err = toAppError(e, "清理缓存失败");
        toast(`${err.title}：${err.message}`, "error");
      } finally {
        setClearing(null);
      }
    },
    [loadCache, toast],
  );

  const copyDiagnostics = useCallback(async () => {
    const lines: string[] = [];
    if (info) {
      lines.push(
        `${info.name} ${info.version} · ${info.targetOs}/${info.targetArch}${info.debug ? " · 调试构建" : ""}`,
      );
    }
    lines.push(
      `下载源：${config.mirror.provider.kind === "custom" ? config.mirror.provider.value : config.mirror.provider.kind} · 文件${strategyLabel(config.mirror.downloadStrategy)} · 列表${strategyLabel(config.mirror.versionStrategy)}`,
    );
    lines.push(
      `并发：${config.download.concurrency === 0 ? "自动" : `${config.download.concurrency} 个任务`}`,
    );
    try {
      const javas = await invoke<SystemJava[]>("java_detected");
      lines.push(
        `Java：${javas.length} 个 → ${javas.map((item) => `${item.major}(${item.version})`).join("，") || "无"}`,
      );
    } catch {
      lines.push("Java：扫描失败");
    }
    const ok = await copyText(lines.join("\n"));
    toast(ok ? "环境信息已复制" : "复制失败，请手动截图", ok ? "success" : "error");
  }, [config, info, toast]);

  /** 导出：后端弹保存框并写盘，返回保存到哪（取消 = null）。 */
  const exportConfig = useCallback(async () => {
    try {
      const path = await invoke<string | null>("config_export");
      if (path) toast(`已导出到 ${path}`, "success");
    } catch (e: unknown) {
      const err = toAppError(e, "导出设置失败");
      toast(`${err.title}：${err.message}`, "error");
    }
  }, [toast]);

  /**
   * 导入：整份覆盖当前设置。
   *
   * 主题 / 强调色 / 动画在运行时 store 里另存一份，导入后要立刻刷过去，
   * 不然界面还是旧配色，直到重启才变。
   */
  const importConfig = useCallback(async () => {
    try {
      const next = await invoke<AppConfig | null>("config_import");
      if (!next) return;
      // 背景图是数据不是配置：导入后按新的文件名重新读一次
      const background = await invoke<string | null>("background_image").catch(
        () => null,
      );
      setAppearance({
        themeMode: next.ui.themeMode,
        accent: next.ui.accent,
        accentCustom: next.ui.accentCustom,
        surfaceOpacity: next.ui.surfaceOpacity,
        surfaceBlur: next.ui.surfaceBlur,
        backgroundImage: background,
        backgroundOverlay: next.ui.backgroundOverlay,
        backgroundBlur: next.ui.backgroundBlur,
        animation: next.ui.animation,
      });
      onReload();
      toast("设置已导入", "success");
    } catch (e: unknown) {
      const err = toAppError(e, "导入设置失败");
      toast(`${err.title}：${err.message}`, "error");
    }
  }, [onReload, toast]);

  const totalBytes = (cache ?? []).reduce((sum, item) => sum + item.bytes, 0);

  return (
    <>
      <SettingGroup title="目录">
        <ActionRow title="数据目录" desc="版本、仓库和账号都在这里。整个 data 拷走就能换机器。">
          <Button size="small" variant="outlined" onClick={() => void openDir("data")}>
            打开
          </Button>
        </ActionRow>
        <ActionRow title="缓存目录" desc="下载缓存、皮肤图、半截文件。不进便携包，删了不心疼。">
          <Button size="small" variant="outlined" onClick={() => void openDir("cache")}>
            打开
          </Button>
        </ActionRow>
      </SettingGroup>

      <SettingGroup
        title="缓存"
        desc={`共 ${formatBytes(totalBytes)}。删掉不会丢任何东西，只是下次要重新下。`}
      >
        {cache === null ? (
          <Box sx={{ display: "grid", placeItems: "center", py: 4 }}>
            <Loading />
          </Box>
        ) : (
          cache.map((item) => (
            <InfoRow
              key={item.kind}
              title={item.label}
              desc={item.desc}
              value={formatBytes(item.bytes)}
              action={
                <Button
                  size="small"
                  variant="outlined"
                  disabled={clearing !== null || item.bytes === 0}
                  onClick={() => void clearCache(item.kind)}
                >
                  清理
                </Button>
              }
            />
          ))
        )}
        <ActionRow title="全部清理" desc="三类缓存一起清。">
          <Button
            size="small"
            color="error"
            variant={confirmClearAll ? "contained" : "outlined"}
            disabled={clearing !== null}
            onClick={() => {
              if (!confirmClearAll) {
                setConfirmClearAll(true);
                return;
              }
              setConfirmClearAll(false);
              void clearCache("all");
            }}
          >
            {confirmClearAll ? "确认全部清理" : "全部清理"}
          </Button>
        </ActionRow>

        {/* 删过版本之后才会攒下东西，所以默认不扫，点一下才去看 */}
        <ActionRow title="未使用的资源" desc={unusedDesc(unused, scanning)}>
          {unused === null ? (
            <Button
              size="small"
              variant="outlined"
              disabled={scanning}
              onClick={() => void scanUnused()}
            >
              {scanning ? "扫描中…" : "扫描"}
            </Button>
          ) : unusedTotal(unused) === 0 ? (
            <Button size="small" variant="outlined" disabled={scanning} onClick={() => void scanUnused()}>
              重新扫描
            </Button>
          ) : (
            <Button
              size="small"
              color="error"
              variant={confirmUnused ? "contained" : "outlined"}
              disabled={cleaning}
              onClick={() => {
                if (!confirmUnused) {
                  setConfirmUnused(true);
                  return;
                }
                void cleanUnused();
              }}
            >
              {cleaning
                ? "清理中…"
                : confirmUnused
                  ? "确认清理"
                  : `清理 ${formatBytes(unusedTotal(unused))}`}
            </Button>
          )}
        </ActionRow>
      </SettingGroup>

      <SettingGroup title="诊断" desc="出问题时把这段贴出来，比描述半小时有用。">
        <SelectRow
          title="日志级别"
          desc="调成「调试」能把更多细节写进日志，方便排查；平时保持「警告」就好。改完立刻生效。"
          value={config.log.level}
          options={[...LOG_OPTIONS]}
          onChange={(next) => patch({ log: { level: next } })}
          width={150}
        />
        <ActionRow title="复制环境信息" desc="版本、下载源、并发数、本机 Java 列表。">
          <Button size="small" variant="outlined" onClick={() => void copyDiagnostics()}>
            复制
          </Button>
        </ActionRow>
      </SettingGroup>

      <SettingGroup
        title="配置"
        desc="把整份设置存成文件，换机器或重装时再导回来。存档、账号、版本都不在里面。"
      >
        <ActionRow title="导出设置" desc="保存成 .toml 文件，可以自己留着或发给别人。">
          <Button size="small" variant="outlined" onClick={() => void exportConfig()}>
            导出
          </Button>
        </ActionRow>

        <ActionRow title="导入设置" desc="整份覆盖当前设置，包括主题和下载源。">
          <Button
            size="small"
            variant={confirmImport ? "contained" : "outlined"}
            onClick={() => {
              if (!confirmImport) {
                setConfirmImport(true);
                return;
              }
              setConfirmImport(false);
              void importConfig();
            }}
          >
            {confirmImport ? "确认导入" : "导入"}
          </Button>
        </ActionRow>
      </SettingGroup>

      <SettingGroup title="内存" desc="需要时手动清一次。">
        <ActionRow
          title="立即清理内存"
          desc="把暂时用不上的内存收回来。不会关掉任何程序，只是切回去时要重新加载一下。"
        >
          <Button
            size="small"
            variant="outlined"
            disabled={trimming}
            onClick={() => void trimMemory()}
          >
            {trimming ? "清理中…" : "立即清理"}
          </Button>
        </ActionRow>
      </SettingGroup>

      <SettingGroup
        title="重置"
        desc="只重置设置本身。存档、账号、已安装的版本都还在，只是我们回到陌生人关系。"
      >
        <DangerRow
          title="恢复默认设置"
          desc="内存、源、Java、窗口这些会回到出厂值。"
          label="恢复默认"
          confirmLabel="确认恢复"
          confirming={confirmReset}
          onAction={() => setConfirmReset(true)}
        />
      </SettingGroup>

      <Dialog open={confirmReset} onClose={() => setConfirmReset(false)} maxWidth="xs" fullWidth>
        <DialogTitle sx={{ fontSize: 15 }}>恢复默认设置？</DialogTitle>
        <DialogContent>
          <DialogContentText sx={{ fontSize: 13, lineHeight: 1.7 }}>
            设置会回到出厂值：内存 4G、窗口 854×480、下载源 BMCLAPI、全部版本隔离。
            存档、账号和已安装的版本不受影响。
          </DialogContentText>
        </DialogContent>
        <DialogActions sx={{ px: 2, pb: 1.5 }}>
          <Button size="small" onClick={() => setConfirmReset(false)}>
            取消
          </Button>
          <Button
            size="small"
            color="error"
            variant="contained"
            onClick={() => {
              setConfirmReset(false);
              onReset();
            }}
          >
            确认恢复
          </Button>
        </DialogActions>
      </Dialog>
    </>
  );
}

function strategyLabel(strategy: AppConfig["mirror"]["downloadStrategy"]): string {
  if (strategy === "mirrorFirst") return "镜像优先";
  if (strategy === "officialFirst") return "官方优先";
  return "自动";
}
