/**
 * 启动页的两个小格式化函数。
 *
 * 和 `lib/format` 里那套不是一回事：这两个只服务启动页的「最近启动时间」和
 * 「运行时长」，格式（`MM-DD HH:mm` / `x 分 y 秒`）是页面级的展示决定，
 * 没必要让全站共享。
 */

/** 最近启动时间：`08-21 14:32`。0 当「没有记录」。 */
export function formatDate(ms: number): string {
  if (!ms) return "—";
  const d = new Date(ms);
  const pad = (n: number) => String(n).padStart(2, "0");
  return `${pad(d.getMonth() + 1)}-${pad(d.getDate())} ${pad(d.getHours())}:${pad(d.getMinutes())}`;
}

/** 运行时长：不足 1 分钟只报秒，否则 `x 分 y 秒`。 */
export function formatDuration(ms: number): string {
  const total = Math.floor(ms / 1000);
  const minutes = Math.floor(total / 60);
  const seconds = total % 60;
  if (minutes === 0) return `${seconds} 秒`;
  return `${minutes} 分 ${seconds} 秒`;
}
