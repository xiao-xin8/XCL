/**
 * 平台加载器标签的测试。
 *
 * ```bash
 * node --test src/lib/loaders.test.ts
 * ```
 *
 * 这个文件被 `tsconfig.json` 排除在外（没装 `@types/node`），不参与 `tsc --noEmit`。
 */
import assert from "node:assert/strict";
import { test } from "node:test";
import { platformLoaderLabel, platformLoaderLabels } from "./loaders.ts";

test("资源包项目带的 minecraft 不显示", () => {
  // Modrinth 给资源包的 loaders 就是 ["minecraft"]，原样渲染会多一个没信息量的胶囊
  assert.equal(platformLoaderLabel("minecraft"), null);
  assert.deepEqual(platformLoaderLabels(["minecraft"]), []);
});

test("认得的加载器给规范写法，大小写不敏感", () => {
  assert.equal(platformLoaderLabel("fabric"), "Fabric");
  assert.equal(platformLoaderLabel("NeoForge"), "NeoForge");
  assert.equal(platformLoaderLabel("OPTIFINE"), "OptiFine");
});

test("不认识的加载器一律丢掉，不原样透出", () => {
  assert.equal(platformLoaderLabel("rift"), null);
  assert.deepEqual(platformLoaderLabels(["fabric", "rift", "forge"]), ["Fabric", "Forge"]);
});

test("去重且保持传入顺序", () => {
  assert.deepEqual(platformLoaderLabels(["forge", "fabric", "forge"]), ["Forge", "Fabric"]);
  assert.deepEqual(platformLoaderLabels(["Fabric", "fabric"]), ["Fabric"]);
});

test("空列表 / 空串不会崩", () => {
  assert.deepEqual(platformLoaderLabels([]), []);
  assert.equal(platformLoaderLabel(""), null);
});
