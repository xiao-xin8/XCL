/** 通用展示格式化。后端已经把数字算好，这里只负责"变成人话"。 */

export function formatBytes(bytes: number): string {
  // 0 一律显示 "0 B"，不要横杠 —— 进度里出现横杠会让整行看着在跳
  if (!Number.isFinite(bytes) || bytes <= 0) return "0 B";
  const units = ["B", "KB", "MB", "GB"];
  let value = bytes;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit += 1;
  }
  return `${value.toFixed(unit === 0 || value >= 100 ? 0 : 1)} ${units[unit]}`;
}

/** ISO 时间 → 本地时区的 "YYYY-MM-DD HH:mm"；解析不了就退回日期部分。 */
export function formatDateTime(iso: string | null): string {
  if (!iso) return "";
  const date = new Date(iso);
  if (Number.isNaN(date.getTime())) return iso.slice(0, 10);
  const pad = (n: number) => String(n).padStart(2, "0");
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())} ${pad(date.getHours())}:${pad(date.getMinutes())}`;
}

export function formatSpeed(bytesPerSecond: number): string {
  // 同上：速度 0 就是 "0 B/s"，不用 "—"。真正断流时后端会把速度平滑衰减到 0，
  // 显示 0 反而比忽隐忽现的横杠更让人安心。
  if (!Number.isFinite(bytesPerSecond) || bytesPerSecond <= 0) return "0 B/s";
  return `${formatBytes(bytesPerSecond)}/s`;
}

/** 秒 → "1 分 20 秒" / "12 秒" / "约 2 小时"。 */
export function formatDuration(seconds: number): string {
  if (seconds <= 0) return "即将完成";
  if (seconds < 60) return `${seconds} 秒`;
  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) {
    const rest = seconds % 60;
    return rest ? `${minutes} 分 ${rest} 秒` : `${minutes} 分`;
  }
  const hours = Math.floor(minutes / 60);
  return `约 ${hours} 小时`;
}

/** 毫秒时间戳 → "2 小时前" / "还没启动过"。 */
export function formatRelativeTime(ms: number | null): string {
  if (!ms) return "还没启动过";
  const diff = Date.now() - ms;
  if (diff <= 0) return "刚刚";
  const minutes = Math.floor(diff / 60_000);
  if (minutes < 1) return "刚刚";
  if (minutes < 60) return `${minutes} 分钟前`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `${hours} 小时前`;
  const days = Math.floor(hours / 24);
  if (days < 30) return `${days} 天前`;
  return new Date(ms).toLocaleDateString();
}

/** 下载量之类的计数：一万以上用「万 / 亿」，不显示一长串数字。 */
export function formatCount(count: number): string {
  if (!Number.isFinite(count) || count <= 0) return "0";
  if (count >= 100_000_000) return `${(count / 100_000_000).toFixed(1)} 亿`;
  if (count >= 10_000) return `${(count / 10_000).toFixed(1)} 万`;
  return String(Math.round(count));
}

/** ISO 时间 → "YYYY-MM-DD"（列表里空间紧张时用）。 */
export function formatDate(iso: string | null): string {
  if (!iso) return "";
  const date = new Date(iso);
  if (Number.isNaN(date.getTime())) return iso.slice(0, 10);
  const pad = (n: number) => String(n).padStart(2, "0");
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}`;
}

/**
 * 延迟（毫秒）→ 人话。
 *
 * 局域网点对点常在 0.1~0.5ms，直接 `Math.round` 会显示成「0 ms」，
 * 看着像坏了、也看不出两台机器谁快谁慢。所以小于 10ms 保留一位小数，
 * 真正的 0 写成「<1 ms」——「0 ms」在物理上不存在，写出来就是假数据。
 */
export function formatLatency(ms: number | null | undefined): string {
  if (ms == null || !Number.isFinite(ms) || ms < 0) return "—";
  if (ms < 0.95) return "<1 ms";
  if (ms < 10) return `${ms.toFixed(1)} ms`;
  return `${Math.round(ms)} ms`;
}
