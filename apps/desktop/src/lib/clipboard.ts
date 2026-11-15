/**
 * 复制文本：优先用剪贴板 API，WebView 里失败就退回隐藏 textarea。
 *
 * 返回 `false` 表示两条路都失败了，调用方自己决定要不要提示。
 * 设置页的「复制日志路径」和存档详情页的「复制种子」共用这一个。
 */
export async function copyText(text: string): Promise<boolean> {
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
