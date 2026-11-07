/**
 * 任务栏进度汇总的测试。
 *
 * ```bash
 * node --test src/lib/taskbarProgress.test.ts
 * ```
 *
 * 这个文件被 `tsconfig.json` 排除在外（没装 `@types/node`），不参与 `tsc --noEmit`。
 */
import assert from "node:assert/strict";
import { test } from "node:test";
import { summarizeTaskProgress } from "./taskbarProgress.ts";
import type { TaskView } from "./types";

/** 只填这个函数用得到的字段，其余按 `TaskView` 断言掉 */
function task(
  phase: TaskView["phase"],
  doneBytes: number,
  totalBytes: number,
): TaskView {
  return { phase, doneBytes, totalBytes } as TaskView;
}

test("队列空 → 清除进度条", () => {
  assert.equal(summarizeTaskProgress([]), null);
});

test("只有已完成的任务 → 清除", () => {
  assert.equal(summarizeTaskProgress([task("done", 100, 100)]), null);
});

test("在跑的任务按字节算百分比", () => {
  const state = summarizeTaskProgress([task("running", 25, 100)]);
  assert.equal(state?.status, "normal");
  assert.equal(state?.progress, 25);
});

test("多个任务合并计算，而不是按条数", () => {
  // 大任务 10/100 + 小任务 90/100 = 100/200 = 50%，不是两条各半再平均
  const state = summarizeTaskProgress([
    task("running", 10, 100),
    task("running", 90, 100),
  ]);
  assert.equal(state?.progress, 50);
});

test("总字节还是 0（准备中）→ 不确定态，图标转起来", () => {
  const state = summarizeTaskProgress([task("running", 0, 0)]);
  assert.equal(state?.status, "indeterminate");
});

test("done 超过 total 也不会冲过 100%", () => {
  const state = summarizeTaskProgress([task("running", 200, 100)]);
  assert.equal(state?.progress, 100);
});

test("没在跑但有失败 → 红条", () => {
  const state = summarizeTaskProgress([task("failed", 0, 100)]);
  assert.equal(state?.status, "error");
  assert.equal(state?.progress, 100);
});

test("有任务在跑时，失败的不抢戏（先显示进度）", () => {
  const state = summarizeTaskProgress([
    task("failed", 0, 100),
    task("running", 50, 100),
  ]);
  assert.equal(state?.status, "normal");
  assert.equal(state?.progress, 50);
});

test("取消不影响进度条（既不算失败也不算在跑）", () => {
  assert.equal(summarizeTaskProgress([task("cancelled", 30, 100)]), null);
});
