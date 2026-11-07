import { invoke } from "@tauri-apps/api/core";

/**
 * 统一的接口缓存层。
 *
 * 所有命令都走这里，解决三类问题：
 * 1. 重复请求：同一命令 + 同一参数，TTL 内直接返回内存里的结果
 * 2. 并发去重：同一个请求还在飞行中时，后续调用复用同一个 Promise
 * 3. 切页面闪烁：组件重新挂载时可以先用 `peek()` 拿到旧数据，避免白屏转圈
 *
 * 注意：真正的"权威缓存"在后端（Rust 侧的 TtlCache），这一层只是让 UI 不闪、不发无用请求。
 * 两者不冲突：前端 TTL 更短，过期后请求后端，后端命中自己的缓存就直接返回。
 */

export type ApiOptions = {
  /** 跳过前端缓存，强制打后端（"刷新"按钮用） */
  force?: boolean;
  /** 前端缓存有效期，默认 5 分钟 */
  ttlMs?: number;
};

type CacheEntry = {
  data: unknown;
  at: number;
};

const DEFAULT_TTL_MS = 5 * 60 * 1000;

const store = new Map<string, CacheEntry>();
const inflight = new Map<string, Promise<unknown>>();

function makeKey(command: string, args: Record<string, unknown>): string {
  // 参数顺序会影响 JSON.stringify 结果，这里按键排序保证稳定
  const sorted = Object.keys(args)
    .sort()
    .reduce<Record<string, unknown>>((acc, key) => {
      acc[key] = args[key];
      return acc;
    }, {});
  return `${command}::${JSON.stringify(sorted)}`;
}

/** 读缓存（不触发请求）。用于组件首次渲染时"先显示旧数据"。 */
export function peek<T>(command: string, args: Record<string, unknown> = {}): T | undefined {
  const entry = store.get(makeKey(command, args));
  return entry?.data as T | undefined;
}

/** 命令式取数据：命中缓存直接返回，否则发请求并缓存。 */
export async function apiGet<T>(
  command: string,
  args: Record<string, unknown> = {},
  options: ApiOptions = {},
): Promise<T> {
  const { force = false, ttlMs = DEFAULT_TTL_MS } = options;
  const key = makeKey(command, args);

  if (!force) {
    const entry = store.get(key);
    if (entry && Date.now() - entry.at < ttlMs) {
      return entry.data as T;
    }
  }

  const pending = inflight.get(key);
  if (pending) {
    return pending as Promise<T>;
  }

  const request = invoke<T>(command, { ...args, force })
    .then((data) => {
      store.set(key, { data, at: Date.now() });
      return data;
    })
    .finally(() => {
      inflight.delete(key);
    });

  inflight.set(key, request);
  return request;
}

/**
 * 让某个命令的缓存失效（例如切换下载源、刚装完一个版本之后）。
 *
 * 按命令名前缀匹配 —— 同一个命令的缓存键会带不同参数（`force` 之类），
 * 只删精确键等于什么都没删。
 */
export function invalidate(command: string): void {
  const prefix = `${command}::`;
  for (const key of [...store.keys()]) {
    if (key.startsWith(prefix)) store.delete(key);
  }
}

/** 清空全部前端缓存。 */
export function clearApiCache(): void {
  store.clear();
  inflight.clear();
}
