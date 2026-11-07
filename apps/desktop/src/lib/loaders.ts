/**
 * 平台加载器 → UI 上的规范写法。
 *
 * 只认真正能加载资源的那些，不在表里的一律不展示。 原因是 Modrinth 给
 * 资源包项目的 `loaders` 是 `["minecraft"]`，原样渲染就会在卡片上多出一个
 * `minecraft` 小胶囊 —— 没有任何信息量，还全是小写英文。
 *
 * 注意和「本地版本类型」区分：详情页左栏那套标签（含 `vanilla` → 原版）
 * 说的是本地实例自己的类型，不走这张表。
 */
const PLATFORM_LOADER_LABEL: Record<string, string> = {
  fabric: "Fabric",
  forge: "Forge",
  neoforge: "NeoForge",
  quilt: "Quilt",
  // 光影侧的加载器：接光影时列表和详情直接用得上
  iris: "Iris",
  optifine: "OptiFine",
  canvas: "Canvas",
};

/** 认得的加载器给规范写法，认不得的返回 `null`（调用方据此丢弃）。 */
export function platformLoaderLabel(name: string): string | null {
  return PLATFORM_LOADER_LABEL[name.toLowerCase()] ?? null;
}

/** 一批加载器 → 去重后的规范标签，顺序按传入顺序。 */
export function platformLoaderLabels(list: string[]): string[] {
  const out: string[] = [];
  for (const name of list) {
    const label = platformLoaderLabel(name);
    if (label && !out.includes(label)) {
      out.push(label);
    }
  }
  return out;
}
