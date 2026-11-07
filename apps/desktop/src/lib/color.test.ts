/**
 * 自定义强调色的推导测试。
 *
 * 重点是两条不变量：
 * 1. 推导出来的 `ink` / 按钮文字，对比度都不低于 4.5（WCAG AA）；
 * 2. 灰色输入不会被染上色相（否则 `#808080` 会变成偏红）。
 *
 * ```bash
 * node --test src/lib/color.test.ts
 * ```
 */
import assert from "node:assert/strict";
import { test } from "node:test";
import {
  contrastRatio,
  deriveAccent,
  hexToHsv,
  hsvToHex,
  normalizeHex,
  readableTextOn,
} from "./color.ts";

test("颜色归一：认 # 和裸值，其余拒绝", () => {
  assert.equal(normalizeHex("#A8CDF0"), "#a8cdf0");
  assert.equal(normalizeHex("a8cdf0"), "#a8cdf0");
  assert.equal(normalizeHex(" #a8cdf0 "), "#a8cdf0");
  assert.equal(normalizeHex("#abc"), null);
  assert.equal(normalizeHex("红色"), null);
  assert.equal(normalizeHex("#a8cdf0ff"), null);
});

test("对比度公式对齐 WCAG", () => {
  assert.ok(Math.abs(contrastRatio("#000000", "#ffffff") - 21) < 0.01);
  assert.ok(Math.abs(contrastRatio("#ffffff", "#ffffff") - 1) < 0.01);
});

test("任何自定义色推导出的文字都够清楚", () => {
  const samples = [
    "#f6de8d",
    "#a8cdf0",
    "#a9dfc4",
    "#f3c0d1",
    "#cfcdc4",
    "#00ff00",
    "#101010",
    "#ffffff",
    "#7f00ff",
  ];

  for (const hex of samples) {
    const { light, dark, bg } = deriveAccent(hex);
    assert.ok(
      contrastRatio(light.ink, bg.light.bgPaper) >= 4.4,
      `${hex}：浅色 ink 对比度不足`,
    );
    assert.ok(
      contrastRatio(dark.ink, bg.dark.bgPaper) >= 4.4,
      `${hex}：深色 ink 对比度不足`,
    );
    assert.ok(
      contrastRatio(light.contrast, light.main) >= 4.4,
      `${hex}：浅色按钮文字对比度不足`,
    );
    assert.ok(
      contrastRatio(dark.contrast, dark.main) >= 4.4,
      `${hex}：深色按钮文字对比度不足`,
    );
  }
});

test("HEX 与 HSV 往返不丢色", () => {
  for (const hex of ["#ff0000", "#00ff00", "#0000ff", "#808080", "#f6de8d", "#123456"]) {
    const { h, s, v } = hexToHsv(hex);
    assert.equal(hsvToHex(h, s, v), hex, `${hex} 往返后变了`);
  }
  assert.equal(hsvToHex(360, 1, 1), "#ff0000", "色相 360 要绕回 0");
});

test("胶囊上的文字取对比度高的一边", () => {
  assert.equal(readableTextOn("#f6de8d"), "#161616");
  assert.equal(readableTextOn("#1b2b3c"), "#ffffff");
});

test("灰色不会被染上色相", () => {
  const { light, dark } = deriveAccent("#808080");
  for (const ink of [light.ink, dark.ink]) {
    assert.equal(ink.slice(1, 3), ink.slice(3, 5), `${ink} 的 R/G 不等`);
    assert.equal(ink.slice(3, 5), ink.slice(5, 7), `${ink} 的 G/B 不等`);
  }
});
