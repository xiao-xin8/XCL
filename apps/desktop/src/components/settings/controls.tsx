import { useEffect, useId, useRef, useState } from "react";
import type { ReactNode } from "react";
import {
  Box,
  Button,
  MenuItem,
  Select,
  Slider,
  Switch,
  TextField,
  Typography,
} from "@mui/material";
import SettingRow from "./SettingRow";

/**
 * 设置行的控件封装。
 *
 * 统一的提交时机（避免「边打字边存」把配置写成半截值）：
 * - Switch / Select：立刻提交
 * - 数字 / 文本：失焦或回车提交，Esc 放弃本次编辑
 * - Slider：拖动过程只动本地，松手才提交
 *
 * 每个控件都是受控的：值来自后端配置，提交后以后端返回的归一值为准。
 */

type RowBase = { title: string; desc?: ReactNode };

export function SwitchRow(
  props: RowBase & { checked: boolean; onChange: (next: boolean) => void; disabled?: boolean },
) {
  const { title, desc, checked, onChange, disabled } = props;
  const id = useId();
  return (
    <SettingRow title={title} desc={desc}>
      <Switch
        id={id}
        size="small"
        checked={checked}
        disabled={disabled}
        onChange={(_, next) => onChange(next)}
        slotProps={{ input: { "aria-label": title } }}
      />
    </SettingRow>
  );
}

export type Option<T extends string | number> = { value: T; label: string };

export function SelectRow<T extends string | number>(
  props: RowBase & {
    value: T;
    options: Option<T>[];
    onChange: (next: T) => void;
    width?: number;
    disabled?: boolean;
  },
) {
  const { title, desc, value, options, onChange, width = 190, disabled } = props;
  const id = useId();
  return (
    <SettingRow title={title} desc={desc}>
      <Select
        id={id}
        size="small"
        value={value}
        disabled={disabled}
        onChange={(e) => onChange(e.target.value as T)}
        sx={{ width, fontSize: 13 }}
        slotProps={{ input: { "aria-label": title } }}
      >
        {options.map((option) => (
          <MenuItem key={String(option.value)} value={option.value} sx={{ fontSize: 13 }}>
            {option.label}
          </MenuItem>
        ))}
      </Select>
    </SettingRow>
  );
}

/** 数字输入（不带行布局）：失焦 / 回车提交，越界值夹到区间里。 */
export function NumberInput(props: {
  value: number;
  onChange: (next: number) => void;
  min?: number;
  max?: number;
  width?: number;
  disabled?: boolean;
  ariaLabel?: string;
}) {
  const { value, onChange, min, max, width = 120, disabled, ariaLabel } = props;
  const [draft, setDraft] = useState(String(value));

  // 外部值变了（别的窗口改了、后端归一过）就同步回来；正在编辑时不打断
  const focused = useRef(false);
  useEffect(() => {
    if (!focused.current) setDraft(String(value));
  }, [value]);

  const commit = () => {
    const parsed = Number(draft.trim());
    if (!Number.isFinite(parsed)) {
      setDraft(String(value));
      return;
    }
    let next = Math.round(parsed);
    if (min != null) next = Math.max(min, next);
    if (max != null) next = Math.min(max, next);
    setDraft(String(next));
    if (next !== value) onChange(next);
  };

  return (
    <TextField
      size="small"
      type="number"
      value={draft}
      disabled={disabled}
      sx={{ width }}
      onChange={(e) => setDraft(e.target.value)}
      onFocus={() => {
        focused.current = true;
      }}
      onBlur={() => {
        focused.current = false;
        commit();
      }}
      onKeyDown={(e) => {
        if (e.key === "Enter") {
          e.currentTarget.blur();
        } else if (e.key === "Escape") {
          setDraft(String(value));
          e.currentTarget.blur();
        }
      }}
      slotProps={{ htmlInput: { min, max, "aria-label": ariaLabel } }}
    />
  );
}

/** 数字行：标题 + 数字输入 + 单位。 */
export function NumberRow(
  props: RowBase & {
    value: number;
    onChange: (next: number) => void;
    min?: number;
    max?: number;
    unit?: string;
    width?: number;
    disabled?: boolean;
  },
) {
  const { title, desc, value, onChange, min, max, unit, width = 120, disabled } = props;

  return (
    <SettingRow title={title} desc={desc}>
      <NumberInput
        value={value}
        onChange={onChange}
        min={min}
        max={max}
        width={width}
        disabled={disabled}
        ariaLabel={title}
      />
      {unit && (
        <Typography sx={{ fontSize: 12, color: "text.secondary", minWidth: 22 }}>
          {unit}
        </Typography>
      )}
    </SettingRow>
  );
}

/** 滑杆 + 读数：拖动只改本地，松手提交。 */
export function SliderRow(
  props: RowBase & {
    value: number;
    onChange: (next: number) => void;
    min: number;
    max: number;
    step?: number;
    format?: (value: number) => string;
    width?: number;
  },
) {
  const { title, desc, value, onChange, min, max, step = 1, format, width = 220 } = props;
  const [local, setLocal] = useState(value);
  const dragging = useRef(false);

  useEffect(() => {
    if (!dragging.current) setLocal(value);
  }, [value]);

  return (
    <SettingRow title={title} desc={desc}>
      <Box sx={{ display: "flex", alignItems: "center", gap: 1.5, width: width + 74 }}>
        <Slider
          size="small"
          value={local}
          min={min}
          max={max}
          step={step}
          onChange={(_, next) => {
            dragging.current = true;
            setLocal(next as number);
          }}
          onChangeCommitted={(_, next) => {
            dragging.current = false;
            const committed = next as number;
            setLocal(committed);
            if (committed !== value) onChange(committed);
          }}
          sx={{ flex: 1 }}
          aria-label={title}
        />
        <Typography
          sx={{
            fontSize: 12,
            color: "text.secondary",
            minWidth: 60,
            textAlign: "right",
            fontVariantNumeric: "tabular-nums",
          }}
        >
          {format ? format(local) : local}
        </Typography>
      </Box>
    </SettingRow>
  );
}

/**
 * 单行 / 多行文本。
 *
 * 用 MUI 自己的 `label`（浮动标签）+ `helperText`，不要自己在上方画一行标题 ——
 * 那是两套视觉，凑在一起就不像 MUI 了。失焦或回车提交。
 */
export function TextRow(
  props: RowBase & {
    value: string;
    onChange: (next: string) => void;
    placeholder?: string;
    multiline?: boolean;
    rows?: number;
    disabled?: boolean;
  },
) {
  const {
    title,
    desc,
    value,
    onChange,
    placeholder,
    multiline = false,
    rows = 3,
    disabled,
  } = props;
  const [draft, setDraft] = useState(value);
  const focused = useRef(false);

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
        label={title}
        helperText={desc}
        multiline={multiline}
        rows={multiline ? rows : undefined}
        value={draft}
        placeholder={placeholder}
        disabled={disabled}
        onChange={(e) => setDraft(e.target.value)}
        onFocus={() => {
          focused.current = true;
        }}
        onBlur={() => {
          focused.current = false;
          commit();
        }}
        onKeyDown={(e) => {
          // 多行框里回车是换行，只有单行才当提交
          if (e.key === "Enter" && !multiline) e.currentTarget.blur();
          if (e.key === "Escape") {
            setDraft(value);
            e.currentTarget.blur();
          }
        }}
      />
    </Box>
  );
}

/** 只读信息行：值靠右，长路径自动换行。 */
export function InfoRow(props: { title: string; desc?: ReactNode; value: ReactNode; action?: ReactNode }) {
  const { title, desc, value, action } = props;
  return (
    <SettingRow title={title} desc={desc}>
      <Box sx={{ display: "flex", alignItems: "center", gap: 1, maxWidth: 460 }}>
        <Typography
          sx={{
            fontSize: 12,
            color: "text.secondary",
            wordBreak: "break-all",
            textAlign: "right",
            lineHeight: 1.6,
          }}
        >
          {value}
        </Typography>
        {action}
      </Box>
    </SettingRow>
  );
}

/** 一行里的按钮组（刷新、打开目录、清除之类）。 */
export function ActionRow(props: { title: string; desc?: ReactNode; children: ReactNode }) {
  return (
    <SettingRow title={props.title} desc={props.desc}>
      {props.children}
    </SettingRow>
  );
}

/** 危险操作：文字按钮 + 二次确认（确认态由调用方控制）。 */
export function DangerRow(props: {
  title: string;
  desc?: ReactNode;
  label: string;
  confirmLabel?: string;
  confirming?: boolean;
  onAction: () => void;
}) {
  const { title, desc, label, confirmLabel = "确认", confirming = false, onAction } = props;
  return (
    <SettingRow title={title} desc={desc}>
      <Button
        size="small"
        color="error"
        variant={confirming ? "contained" : "text"}
        onClick={onAction}
      >
        {confirming ? confirmLabel : label}
      </Button>
    </SettingRow>
  );
}
