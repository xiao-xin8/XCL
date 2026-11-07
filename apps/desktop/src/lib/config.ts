import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { useToast } from "../components/Toast";
import { toAppError, type AppConfig, type AppError } from "./types";

/**
 * 配置的读写。
 *
 * 规则：
 * - 改动走 `config_patch`，只提交变了的字段。后端递归合并 + `normalized()`
 *   再落盘，然后把归一后的完整配置返回，前端以它为准（钳制后的值只听后端）。
 * - 前端乐观更新：先改本地，失败回滚并 toast。成功不提示 —— 每改一个开关
 *   弹一次「已保存」会吵得没法用。
 * - 连点几下时用序号挡旧响应：后发的请求才允许覆盖状态。
 */

type DeepPartial<T> = {
  [K in keyof T]?: T[K] extends readonly unknown[]
    ? T[K]
    : T[K] extends object | null
      ? DeepPartial<NonNullable<T[K]>> | null
      : T[K];
};

export type ConfigPatch = DeepPartial<AppConfig>;

function isPlainObject(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

/** 与后端 `merge_json` 同语义：对象逐键递归，数组/标量整体替换。 */
export function mergePatch<T>(base: T, patch: unknown): T {
  if (!isPlainObject(base) || !isPlainObject(patch)) {
    return (patch === undefined ? base : patch) as T;
  }
  const out: Record<string, unknown> = { ...base };
  for (const [key, value] of Object.entries(patch)) {
    if (value === undefined) continue;
    out[key] = mergePatch(out[key], value);
  }
  return out as T;
}

export type ConfigApi = {
  config: AppConfig | null;
  loading: boolean;
  /** 原始 AppError：交给统一的 `ErrorState` 渲染，不再在这里拼字符串 */
  error: AppError | null;
  patch: (patch: ConfigPatch) => Promise<void>;
  reset: () => Promise<void>;
  reload: () => Promise<void>;
};

export function useConfig(): ConfigApi {
  const toast = useToast();
  const [config, setConfig] = useState<AppConfig | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<AppError | null>(null);
  /** 响应序号：只有最后一次请求的返回值能写进状态 */
  const seq = useRef(0);

  const load = useCallback(async () => {
    setError(null);
    try {
      const next = await invoke<AppConfig>("config_get");
      setConfig(next);
    } catch (e: unknown) {
      setError(toAppError(e, "读取设置失败"));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  const patch = useCallback(
    async (next: ConfigPatch) => {
      const mine = ++seq.current;
      setConfig((prev) => (prev ? mergePatch(prev, next) : prev));
      try {
        const saved = await invoke<AppConfig>("config_patch", { patch: next });
        if (mine === seq.current) setConfig(saved);
      } catch (e: unknown) {
        const err = toAppError(e, "保存设置失败");
        toast(`${err.title}：${err.message}`, "error");
        // 回滚到磁盘上的真值，别让界面停在一个没存下来的状态
        if (mine === seq.current) void load();
      }
    },
    [load, toast],
  );

  const reset = useCallback(async () => {
    const mine = ++seq.current;
    try {
      const next = await invoke<AppConfig>("config_reset");
      if (mine === seq.current) setConfig(next);
      toast("设置已恢复默认", "success");
    } catch (e: unknown) {
      const err = toAppError(e, "恢复默认失败");
      toast(`${err.title}：${err.message}`, "error");
      if (mine === seq.current) void load();
    }
  }, [load, toast]);

  return { config, loading, error, patch, reset, reload: load };
}
