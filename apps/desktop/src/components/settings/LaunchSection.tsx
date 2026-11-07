import { useEffect, useRef, useState } from "react";
import { Box, Typography, alpha } from "@mui/material";
import { invoke } from "@tauri-apps/api/core";
import type { AppConfig, MemoryMode, MemoryStats } from "../../lib/types";
import type { ConfigPatch } from "../../lib/config";
import { CARD_MOTION, EASE, SETTLE } from "../../motion/tokens";
import SettingGroup from "./SettingGroup";
import SettingRow from "./SettingRow";
import { NumberInput, SelectRow, SliderRow, SwitchRow, TextRow } from "./controls";

const MEMORY_MODE_OPTIONS: { value: MemoryMode; label: string }[] = [
  { value: "auto", label: "自动" },
  { value: "custom", label: "手动" },
];

function formatMemory(mb: number): string {
  return `${(mb / 1024).toFixed(mb % 1024 === 0 ? 0 : 1)} GB`;
}

const ISOLATION_OPTIONS = [
  { value: "all", label: "全部版本（推荐）" },
  { value: "nonReleaseOrModable", label: "非正式版或模组版" },
  { value: "nonRelease", label: "仅非正式版" },
  { value: "modable", label: "仅模组版本" },
  { value: "off", label: "关闭隔离" },
] as const;

/**
 * 语言只影响第一次进游戏时写进 options.txt 的值。
 * 值本身是小写的 MC 语言代码，按版本大小写由后端 `lang_value_for_version` 处理。
 */
const LANGUAGE_OPTIONS = [
  { value: "zh_cn", label: "简体中文" },
  { value: "zh_tw", label: "繁體中文" },
  { value: "en_us", label: "English (US)" },
  { value: "ja_jp", label: "日本語" },
  { value: "ko_kr", label: "한국어" },
] as const;

const PRIORITY_OPTIONS = [
  { value: "low", label: "低" },
  { value: "normal", label: "普通（默认）" },
  { value: "high", label: "高" },
] as const;

export default function LaunchSection(props: {
  config: AppConfig;
  patch: (patch: ConfigPatch) => void;
}) {
  const { config, patch } = props;
  const launch = config.launch;
  const [mem, setMem] = useState<MemoryStats | null>(null);
  const started = useRef(false);

  /**
   * 内存数据每 2 秒刷一次。
   *
   * 它不是"加载数据"，是一块活的仪表：系统占用随时在变，刚在高级里
   * 点完清理、或者关掉一个浏览器，这里应该立刻反映出来。
   */
  useEffect(() => {
    let alive = true;
    let timer = 0;
    const tick = () => {
      void invoke<MemoryStats>("memory_stats", { versionId: null })
        .then((data) => {
          if (alive) setMem(data);
        })
        .catch(() => {
          // 读不到就先不显示条，不影响设置本身
        });
    };
    // 切页后先让入场、配置到达、卡片补播都跑完，到点一次性取数据：
    // 空槽早已占位，条子从 0 直接冲到实际宽度，不会在动画中途改布局。
    // 之后依赖变化（切分配方式 / 换版本）立刻重算。
    const start = () => {
      if (!alive) return;
      tick();
      timer = window.setInterval(tick, 2000);
    };
    let settle = 0;
    if (started.current) {
      start();
    } else {
      settle = window.setTimeout(start, SETTLE.liveDataReveal);
    }
    started.current = true;
    return () => {
      alive = false;
      if (settle) window.clearTimeout(settle);
      window.clearInterval(timer);
    };
    // 换分配方式 / 换版本时立刻重算，别等下一个 2 秒
  }, [launch.memoryMode, launch.memoryMb, config.ui.selectedVersion]);

  return (
    <>
      <SettingGroup
        title="运行环境"
        desc="这些值对所有版本生效。单个版本想不一样，去启动页的版本详情里改。"
      >
        <SelectRow
          title="默认版本隔离"
          desc="开隔离 = 每个版本各用一份存档和 mods。关掉以后，你装的 200 个 mod 会和原版存档住同一间屋子——出事别怪我没提醒。"
          value={launch.isolation}
          options={[...ISOLATION_OPTIONS]}
          onChange={(next) => patch({ launch: { isolation: next } })}
          width={200}
        />

        <SelectRow
          title="内存分配"
          desc={
            launch.memoryMode === "auto"
              ? "按这个版本的 mod 数量和当前可用内存估算：mod 越多给得越多，内存越紧给得越少。"
              : "不是越多越好。你给它 32G，它也只是花更长时间把 32G 用满，然后 OOM。"
          }
          value={launch.memoryMode}
          options={MEMORY_MODE_OPTIONS}
          onChange={(next) => patch({ launch: { memoryMode: next } })}
          width={120}
        />

        {launch.memoryMode === "custom" && (
          <SliderRow
            title="手动分配"
            desc="不是越多越好。按这个版本的实际需要调，不够再改。"
            value={launch.memoryMb}
            min={512}
            max={32768}
            step={512}
            onChange={(next) => patch({ launch: { memoryMb: next } })}
            format={formatMemory}
          />
        )}

        <MemoryMeter
          stats={mem}
          customMb={launch.memoryMode === "custom" ? launch.memoryMb : null}
        />

        <SettingRow
          title="游戏窗口大小"
          desc="854×480 是 2011 年的审美。全屏启动时这里不起作用。"
        >
          <Box sx={{ display: "flex", alignItems: "center", gap: 1 }}>
            <NumberInput
              value={launch.windowWidth}
              min={200}
              max={7680}
              width={96}
              ariaLabel="窗口宽度"
              onChange={(next) => patch({ launch: { windowWidth: next } })}
            />
            <Typography sx={{ fontSize: 12, color: "text.secondary" }}>×</Typography>
            <NumberInput
              value={launch.windowHeight}
              min={200}
              max={4320}
              width={96}
              ariaLabel="窗口高度"
              onChange={(next) => patch({ launch: { windowHeight: next } })}
            />
          </Box>
        </SettingRow>

        <SwitchRow
          title="全屏启动"
          desc="进游戏直接铺满屏幕。笔记本上想切出来会稍微麻烦一点。"
          checked={launch.fullscreen}
          onChange={(next) => patch({ launch: { fullscreen: next } })}
        />

        <SelectRow
          title="游戏语言"
          desc="只在第一次进游戏时写进去。你自己在游戏里改过语言，这里就不插手了。"
          value={launch.language}
          options={[...LANGUAGE_OPTIONS]}
          onChange={(next) => patch({ launch: { language: next } })}
          width={160}
        />

        <SwitchRow
          title="使用高性能显卡"
          desc="笔记本双显卡必开：启动时把 java.exe 和启动器都标成「使用高性能显卡」。改的是 Windows 的显卡首选项，不是 JVM 参数；其它系统上不起作用。"
          checked={launch.highPerfGpu}
          onChange={(next) => patch({ launch: { highPerfGpu: next } })}
        />

        <SwitchRow
          title="启动时进行内存优化"
          desc="启动游戏前清一次内存，把暂时用不上的部分收回来给它用。内存够大的机器可以不开。"
          checked={launch.trimMemory}
          onChange={(next) => patch({ launch: { trimMemory: next } })}
        />
      </SettingGroup>

      <SettingGroup title="命令行" desc="留空就是不加。参数按空格拆分，带空格的值请用引号包起来。">
        <TextRow
          title="JVM 参数"
          desc="空格分隔。写错了启动器拦不住你，Java 会用它自己的方式告诉你哪里不对。"
          value={launch.extraJvmArgs}
          onChange={(next) => patch({ launch: { extraJvmArgs: next } })}
          placeholder="-XX:+UseG1GC"
          multiline
          rows={2}
        />

        <TextRow
          title="游戏参数"
          desc="追加到游戏主类后面。一般用不到，除非你在玩别人的整合包文档。"
          value={launch.extraGameArgs}
          onChange={(next) => patch({ launch: { extraGameArgs: next } })}
          placeholder="--demo"
          multiline
          rows={2}
        />
      </SettingGroup>

      <SettingGroup
        title="启动流程"
        desc="启动前命令跑在游戏之前，工作目录是游戏目录。失败只会记日志，不会挡住启动。"
      >
        <TextRow
          title="启动前执行命令"
          desc="交给系统执行（Windows 走 cmd /C），所以重定向、&& 都能写。"
          value={launch.preLaunchCommand}
          onChange={(next) => patch({ launch: { preLaunchCommand: next } })}
          placeholder="例如：start ./tools/launcher-helper.exe"
        />

        <SwitchRow
          title="等命令执行完再启动"
          desc="关掉就是「甩出去不管」，适合启个后台程序；开着的话命令不结束就不会启动游戏。"
          checked={launch.waitCommand}
          disabled={launch.preLaunchCommand.trim().length === 0}
          onChange={(next) => patch({ launch: { waitCommand: next } })}
        />

        <SelectRow
          title="进程优先级"
          desc="游戏跑起来之后设置。「高」能让游戏多抢一点 CPU，代价是切出去时偶尔卡一下。只有 Windows 生效。"
          value={launch.priority}
          options={[...PRIORITY_OPTIONS]}
          onChange={(next) => patch({ launch: { priority: next } })}
          width={160}
        />
      </SettingGroup>

      <Box sx={{ px: 1, pb: 1 }}>
        <Typography sx={{ fontSize: 11.5, color: "text.secondary", lineHeight: 1.7 }}>
          某个版本想单独分配内存、换个 Java？去启动页选中那个版本，在详情卡里改。
        </Typography>
      </Box>
    </>
  );
}

/**
 * 内存占用条：一根，三段。
 *
 * 深色 = 系统已用，浅色 = 游戏将分配，空着 = 还剩多少。
 * 三段共用「总物理内存」这一个刻度，所以长度就是真实占比 —— 这正是它存在的
 * 理由：光看「4.5 GB」没有尺度感，看到条才知道这台机器还剩多少。
 */
function MemoryMeter(props: { stats: MemoryStats | null; customMb: number | null }) {
  const { stats, customMb } = props;
  // 首帧就按最终结构占位：数据回来只改数字和条宽，卡片高度不变。
  // 早先空态直接 return null，条出现时把整张卡撑高、下面的卡全被推下去，
  // 正好撞在入场动画里 —— 这就是切到设置页会顿一下的原因。
  const s = stats && stats.totalMb > 0 ? stats : null;
  const total = s?.totalMb ?? 0;
  const used = s ? Math.min(s.usedMb, total) : 0;
  const game = s ? (customMb ?? s.gameMb) : 0;
  // 内存紧张时 used + game 可能超过总量：剩余夹到 0，条按 100% 截断
  const rest = s ? Math.max(0, total - used - game) : 0;
  const width = (value: number) =>
    total > 0 ? `${Math.min(100, (value / total) * 100)}%` : "0%";
  const shown = (mb: number) => (s ? formatMemory(mb) : "—");

  const transition = `width ${CARD_MOTION.collapseEnterMax}ms ${EASE.outFluent}`;

  return (
    <Box sx={{ px: 2, py: 1.25 }}>
      <Box
        sx={{
          display: "flex",
          height: 8,
          borderRadius: 999,
          overflow: "hidden",
          // 底色就是「剩余」：什么都不画的地方自然露出来
          bgcolor: (t) => alpha(t.palette.text.primary, 0.08),
        }}
      >
        <Box
          sx={{
            width: width(used),
            flexShrink: 0,
            bgcolor: "primary.dark",
            transition,
          }}
        />
        <Box
          sx={{
            width: width(game),
            flexShrink: 0,
            bgcolor: "primary.main",
            transition,
          }}
        />
      </Box>

      <Box
        sx={{
          mt: 1,
          display: "flex",
          flexWrap: "wrap",
          alignItems: "center",
          columnGap: 2,
          rowGap: 0.5,
        }}
      >
        <Legend color="primary.dark" label="系统已用" value={shown(used)} />
        <Legend
          color="primary.main"
          label="游戏将分配"
          value={
            s && customMb === null && s.modCount > 0
              ? `${shown(game)} · ${s.modCount} 个 mod`
              : shown(game)
          }
        />
        <Legend color="text.disabled" label="剩余" value={shown(rest)} />
      </Box>
    </Box>
  );
}

/** 图例：一个色点 + 名字 + 数值。 */
function Legend(props: { color: string; label: string; value: string }) {
  return (
    <Box sx={{ display: "flex", alignItems: "center", gap: 0.6, minWidth: 0 }}>
      <Box
        sx={{
          width: 8,
          height: 8,
          flexShrink: 0,
          borderRadius: "50%",
          bgcolor: props.color,
        }}
      />
      <Typography sx={{ fontSize: 11.5, color: "text.secondary", flexShrink: 0 }}>
        {props.label}
      </Typography>
      <Typography
        noWrap
        sx={{ fontSize: 11.5, fontVariantNumeric: "tabular-nums", minWidth: 0 }}
      >
        {props.value}
      </Typography>
    </Box>
  );
}
