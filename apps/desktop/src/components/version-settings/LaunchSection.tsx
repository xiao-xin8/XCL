import { Box, MenuItem, Select, Slider, Switch, TextField, Typography } from "@mui/material";
import { useEffect, useRef, useState } from "react";
import type { ReactNode } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { LaunchOverride, MemoryStats, VersionSettingsView } from "../../lib/types";
import SettingGroup from "../settings/SettingGroup";
import Reveal from "../settings/Reveal";
import { NumberInput } from "../settings/controls";

/**
 * 单个版本的启动设置。
 *
 * 每一行是一个「跟随全局 ↔ 只对这个版本」的开关：关着的时候不出现编辑控件，
 * 只在副标题里写清会用什么值 —— 摆一个灰输入框既改不动，又让人以为坏了。
 *
 * 例外是 Java 路径：空就是「不指定、跟随全局」，所以它不需要开关，
 * 反而留一个随时能清空的输入框更直接。
 */
function formatMemory(mb: number): string {
  return `${(mb / 1024).toFixed(mb % 1024 === 0 ? 0 : 1)} GB`;
}

export default function LaunchSection(props: {
  settings: VersionSettingsView;
  patch: (fields: Partial<LaunchOverride>) => Promise<void>;
}) {
  const { settings, patch } = props;
  const over = settings.overrides;
  const global = settings.global;
  const [mem, setMem] = useState<MemoryStats | null>(null);

  // 这一页有版本上下文，直接按这个版本的 mod 数算自动值
  useEffect(() => {
    let alive = true;
    void invoke<MemoryStats>("memory_stats", { versionId: settings.versionId })
      .then((data) => {
        if (alive) setMem(data);
      })
      .catch(() => {
        // 读不到就只显示「自动」，不影响其它设置
      });
    return () => {
      alive = false;
    };
  }, [settings.versionId]);

  /** 打开开关时给它一个初始值：优先沿用全局那份，别先看到空白 */
  const turnOn = <K extends keyof LaunchOverride>(field: K, value: LaunchOverride[K]) => {
    void patch({ [field]: value } as Partial<LaunchOverride>);
  };
  const turnOff = (field: keyof LaunchOverride) => {
    void patch({ [field]: null } as Partial<LaunchOverride>);
  };

  return (
    <>
      <SettingGroup
        title="启动"
        desc="只改这个版本。开关关掉就跟全局设置走，设置页改了它也跟着变。"
      >
        {/* 内存 */}
        <MemoryOverrideRow
          over={over}
          global={global}
          autoMb={mem ? mem.gameMb : null}
          onChange={(fields) => void patch(fields)}
        />

        {/* JVM 参数 */}
        <OverrideRow
          title="JVM 参数"
          note={
            global.jvmArgs.trim() ? `跟随全局：${global.jvmArgs}` : "跟随全局：没有额外参数"
          }
          active={over.jvmArgs !== null}
          onToggle={(on) => (on ? turnOn("jvmArgs", global.jvmArgs) : turnOff("jvmArgs"))}
          block
        >
          <DraftTextField
            label="JVM 参数"
            placeholder="-XX:+UseG1GC"
            value={over.jvmArgs ?? ""}
            onCommit={(text) => void patch({ jvmArgs: text })}
            helperText="整份替换全局那一条，不会拼在一起。留空就是「这个版本不要额外参数」。"
          />
        </OverrideRow>

        {/* 游戏参数 */}
        <OverrideRow
          title="游戏参数"
          note={
            global.gameArgs.trim() ? `跟随全局：${global.gameArgs}` : "跟随全局：没有额外参数"
          }
          active={over.gameArgs !== null}
          onToggle={(on) => (on ? turnOn("gameArgs", global.gameArgs) : turnOff("gameArgs"))}
          block
        >
          <DraftTextField
            label="游戏参数"
            placeholder="--demo"
            value={over.gameArgs ?? ""}
            onCommit={(text) => void patch({ gameArgs: text })}
          />
        </OverrideRow>

        {/* 窗口大小 */}
        <OverrideRow
          title="窗口大小"
          note={
            over.windowWidth === null
              ? `跟随全局：${global.windowWidth} × ${global.windowHeight}`
              : "只对这个版本生效"
          }
          active={over.windowWidth !== null || over.windowHeight !== null}
          onToggle={(on) =>
            on
              ? void patch({
                  windowWidth: global.windowWidth,
                  windowHeight: global.windowHeight,
                })
              : void patch({ windowWidth: null, windowHeight: null })
          }
        >
          <Box sx={{ display: "flex", alignItems: "center", gap: 1.5 }}>
            <NumberInput
              value={over.windowWidth ?? global.windowWidth}
              onChange={(next) => void patch({ windowWidth: next })}
              min={160}
              max={16384}
              width={100}
              ariaLabel="窗口宽度"
            />
            <Typography sx={{ fontSize: 12, color: "text.secondary" }}>×</Typography>
            <NumberInput
              value={over.windowHeight ?? global.windowHeight}
              onChange={(next) => void patch({ windowHeight: next })}
              min={160}
              max={16384}
              width={100}
              ariaLabel="窗口高度"
            />
          </Box>
        </OverrideRow>

        {/* 全屏：三态下拉。布尔值不适合「开关 + 开关」—— 两个并排没人分得清哪个是哪个 */}
        <Box sx={{ px: 2, py: 1.25, display: "flex", alignItems: "center", gap: 2 }}>
          <Box sx={{ flex: 1, minWidth: 0 }}>
            <Typography sx={{ fontSize: 13, lineHeight: 1.6 }}>全屏启动</Typography>
            <Typography sx={{ fontSize: 11.5, color: "text.secondary", lineHeight: 1.65 }}>
              {over.fullscreen === null
                ? `跟随全局：${global.fullscreen ? "全屏" : "窗口"}`
                : "只对这个版本生效"}
            </Typography>
          </Box>
          <Select
            size="small"
            value={over.fullscreen === null ? "follow" : over.fullscreen ? "on" : "off"}
            onChange={(e) =>
              void patch({
                fullscreen:
                  e.target.value === "follow" ? null : e.target.value === "on",
              })
            }
            sx={{ width: 190, fontSize: 13 }}
            slotProps={{ input: { "aria-label": "全屏启动" } }}
          >
            <MenuItem value="follow" sx={{ fontSize: 13 }}>
              跟随全局（{global.fullscreen ? "全屏" : "窗口"}）
            </MenuItem>
            <MenuItem value="on" sx={{ fontSize: 13 }}>
              全屏
            </MenuItem>
            <MenuItem value="off" sx={{ fontSize: 13 }}>
              窗口
            </MenuItem>
          </Select>
        </Box>

        {/* 语言 */}
        <OverrideRow
          title="游戏语言"
          note={
            over.language === null
              ? `跟随全局：${LANG_LABEL[global.language] ?? global.language}`
              : "只对这个版本生效"
          }
          active={over.language !== null}
          onToggle={(on) =>
            on ? turnOn("language", global.language) : turnOff("language")
          }
        >
          <Select
            size="small"
            value={over.language ?? global.language}
            onChange={(e) => void patch({ language: e.target.value })}
            sx={{ width: 190, fontSize: 13 }}
            slotProps={{ input: { "aria-label": "游戏语言" } }}
          >
            {Object.entries(LANG_LABEL).map(([value, label]) => (
              <MenuItem key={value} value={value} sx={{ fontSize: 13 }}>
                {label}
              </MenuItem>
            ))}
          </Select>
        </OverrideRow>

        {/* 启动前命令 */}
        <OverrideRow
          title="启动前命令"
          note={
            global.preLaunchCommand.trim()
              ? `跟随全局：${global.preLaunchCommand}`
              : "跟随全局：不执行"
          }
          active={over.preLaunchCommand !== null}
          onToggle={(on) =>
            on ? turnOn("preLaunchCommand", global.preLaunchCommand) : turnOff("preLaunchCommand")
          }
          block
        >
          <DraftTextField
            label="启动前命令"
            placeholder="cmd /C echo hello"
            value={over.preLaunchCommand ?? ""}
            onCommit={(text) => void patch({ preLaunchCommand: text })}
            helperText="工作目录是游戏目录；失败只记日志，不会拦住启动。"
          />
          <Box sx={{ display: "flex", alignItems: "center", gap: 1, mt: 1 }}>
            <Switch
              size="small"
              checked={over.waitCommand ?? global.waitCommand}
              onChange={(_, next) => void patch({ waitCommand: next })}
              slotProps={{ input: { "aria-label": "等待命令完成" } }}
            />
            <Typography sx={{ fontSize: 12, color: "text.secondary" }}>
              等命令执行完再启动游戏
            </Typography>
          </Box>
        </OverrideRow>
      </SettingGroup>

      <SettingGroup
        title="Java"
        desc="有的版本就认某个 Java。定死之后哪怕全局换了 Java，它也不跟着动。"
      >
        <JavaPathRow
          value={over.javaPath ?? global.javaPath ?? ""}
          inherited={global.javaPath}
          onChange={(text) => void patch({ javaPath: text })}
        />
      </SettingGroup>
    </>
  );
}

const LANG_LABEL: Record<string, string> = {
  zh_cn: "简体中文",
  zh_tw: "繁體中文",
  en_us: "English (US)",
  ja_jp: "日本語",
  ru_ru: "Русский",
};

/**
 * Java 路径。
 *
 * 这里没有开关：空着就等于跟随全局。给了开关反而尴尬 ——
 * 打开却没填路径时，到底算不算「指定了」？
 */
function JavaPathRow(props: {
  value: string;
  inherited: string | null;
  onChange: (text: string) => void;
}) {
  const { value, inherited, onChange } = props;
  const [draft, setDraft] = useState(value);
  const focused = useRef(false);

  // 外部值变了（改完回包、换版本）就同步；正在打字时别打断
  useEffect(() => {
    if (!focused.current) setDraft(value);
  }, [value]);

  const commit = () => {
    if (draft !== value) onChange(draft);
  };

  return (
    <Box sx={{ px: 2, py: 1.25 }}>
      <TextField
        size="small"
        fullWidth
        label="Java 可执行文件"
        placeholder="留空则自动挑选，例如 C:/Program Files/Java/jdk-17/bin/javaw.exe"
        value={draft}
        onChange={(e) => setDraft(e.target.value)}
        onFocus={() => {
          focused.current = true;
        }}
        onBlur={() => {
          focused.current = false;
          commit();
        }}
        onKeyDown={(e) => {
          if (e.key === "Enter") e.currentTarget.blur();
        }}
        helperText={
          inherited
            ? `留空则跟随全局：${inherited}`
            : "留空则按版本要求自动挑选。填了就用它，哪怕版本对不上。"
        }
      />
    </Box>
  );
}

/**
 * 多行参数输入：失焦或 Ctrl+Enter 才提交。
 *
 * 每敲一个字就写一次 `instance.toml` 既没必要，也会让「正在编辑」的半截值落盘。
 */
function DraftTextField(props: {
  label: string;
  placeholder?: string;
  value: string;
  onCommit: (text: string) => void;
  helperText?: string;
}) {
  const { label, placeholder, value, onCommit, helperText } = props;
  const [draft, setDraft] = useState(value);
  const focused = useRef(false);

  useEffect(() => {
    if (!focused.current) setDraft(value);
  }, [value]);

  const commit = () => {
    if (draft !== value) onCommit(draft);
  };

  return (
    <TextField
      size="small"
      fullWidth
      multiline
      rows={2}
      label={label}
      placeholder={placeholder}
      helperText={helperText}
      value={draft}
      onChange={(e) => setDraft(e.target.value)}
      onFocus={() => {
        focused.current = true;
      }}
      onBlur={() => {
        focused.current = false;
        commit();
      }}
      onKeyDown={(e) => {
        // 多行框里回车是换行，Ctrl/Cmd + 回车才是提交
        if (e.key === "Enter" && (e.ctrlKey || e.metaKey)) {
          e.preventDefault();
          e.currentTarget.blur();
        }
      }}
    />
  );
}

/**
 * 内存：滑杆拖动只改本地，松手才落盘；右边数字框负责精确输入。
 * 拖的过程中每一帧都写一次配置文件没有必要，还会让 toml 反复重写。
 */
function MemoryControl(props: { value: number; onChange: (next: number) => void }) {
  const { value, onChange } = props;
  const [local, setLocal] = useState(value);
  const dragging = useRef(false);

  useEffect(() => {
    if (!dragging.current) setLocal(value);
  }, [value]);

  return (
    <Box sx={{ display: "flex", alignItems: "center", gap: 2 }}>
      <Slider
        size="small"
        min={512}
        max={32768}
        step={256}
        value={local}
        onChange={(_, next) => {
          dragging.current = true;
          setLocal(next as number);
        }}
        onChangeCommitted={(_, next) => {
          dragging.current = false;
          setLocal(next as number);
          if (next !== value) onChange(next as number);
        }}
        sx={{ flex: 1, minWidth: 180 }}
        aria-label="内存分配"
      />
      <NumberInput value={value} onChange={onChange} min={512} max={65536} width={96} ariaLabel="内存分配" />
      <Typography sx={{ fontSize: 12, color: "text.secondary" }}>MB</Typography>
    </Box>
  );
}

/**
 * 内存覆盖：三态下拉（跟随全局 / 自动 / 手动）。
 * 不用分段标签，和设置页其它选择项的交互保持一致。
 */
function MemoryOverrideRow(props: {
  over: LaunchOverride;
  global: VersionSettingsView["global"];
  autoMb: number | null;
  onChange: (fields: Partial<LaunchOverride>) => void;
}) {
  const { over, global, autoMb, onChange } = props;
  const value = over.memoryMode ?? "follow";
  const autoText = autoMb === null ? "自动" : `自动（${formatMemory(autoMb)}）`;
  const globalText =
    global.memoryMode === "auto" ? autoText : `手动（${formatMemory(global.memoryMb)}）`;
  const note =
    value === "follow"
      ? `跟随全局：${globalText}`
      : value === "auto"
        ? `只对这个版本生效：${autoText}`
        : "只对这个版本生效";

  const setMode = (next: string) => {
    if (next === "follow") {
      onChange({ memoryMode: null, memoryMb: null });
    } else if (next === "auto") {
      onChange({ memoryMode: "auto", memoryMb: null });
    } else {
      onChange({ memoryMode: "custom", memoryMb: over.memoryMb ?? global.memoryMb });
    }
  };

  return (
    <Box sx={{ px: 2, py: 1.25 }}>
      <Box sx={{ display: "flex", alignItems: "center", gap: 2 }}>
        <Box sx={{ flex: 1, minWidth: 0 }}>
          <Typography sx={{ fontSize: 13, lineHeight: 1.6 }}>内存分配</Typography>
          <Typography
            sx={{
              fontSize: 11.5,
              color: "text.secondary",
              lineHeight: 1.65,
              wordBreak: "break-all",
            }}
          >
            {note}
          </Typography>
        </Box>
        <Select
          size="small"
          value={value}
          onChange={(event) => setMode(event.target.value)}
          sx={{ width: 140, fontSize: 13 }}
          slotProps={{ input: { "aria-label": "内存分配" } }}
        >
          <MenuItem value="follow" sx={{ fontSize: 13 }}>
            跟随全局
          </MenuItem>
          <MenuItem value="auto" sx={{ fontSize: 13 }}>
            自动
          </MenuItem>
          <MenuItem value="custom" sx={{ fontSize: 13 }}>
            手动
          </MenuItem>
        </Select>
      </Box>
      {value === "custom" && (
        <Box sx={{ pt: 1.25 }}>
          <MemoryControl
            value={over.memoryMb ?? global.memoryMb}
            onChange={(next) => onChange({ memoryMode: "custom", memoryMb: next })}
          />
        </Box>
      )}
    </Box>
  );
}

/**
 * 一行覆盖设置：标题 + 说明 + 右侧开关。
 *
 * `block` 表示控件要整行宽度（文本框、滑杆这类），放在标题下面。
 */
function OverrideRow(props: {
  title: string;
  note: ReactNode;
  active: boolean;
  onToggle: (next: boolean) => void;
  children: ReactNode;
  block?: boolean;
}) {
  const { title, note, active, onToggle, children, block = false } = props;

  return (
    <Box sx={{ px: 2, py: 1.25 }}>
      <Box sx={{ display: "flex", alignItems: "center", gap: 2 }}>
        <Box sx={{ flex: 1, minWidth: 0 }}>
          <Typography sx={{ fontSize: 13, lineHeight: 1.6 }}>{title}</Typography>
          <Typography
            sx={{
              fontSize: 11.5,
              color: "text.secondary",
              lineHeight: 1.65,
              wordBreak: "break-all",
            }}
          >
            {note}
          </Typography>
        </Box>
        {!block && (
          <Reveal
            open={active}
            axis="x"
            innerSx={{ display: "flex", alignItems: "center", gap: 1 }}
          >
            {children}
          </Reveal>
        )}
        <Switch
          size="small"
          checked={active}
          onChange={(_, next) => onToggle(next)}
          slotProps={{ input: { "aria-label": `${title}只对这个版本生效` } }}
        />
      </Box>
      {block && (
        <Reveal open={active}>
          <Box sx={{ pt: 1.25 }}>{children}</Box>
        </Reveal>
      )}
    </Box>
  );
}
