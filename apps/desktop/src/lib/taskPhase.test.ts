/**
 * 任务阶段规则的测试。
 *
 * 用 Node 自带的 test runner（`node --test`）跑，不额外装 vitest ——
 * 依赖只有 `node:test` 和 `node:assert`，零安装。
 *
 * ```bash
 * node --test src/lib/taskPhase.test.ts
 * ```
 *
 * 注意：这个文件被 `tsconfig.json` 排除在外（缺 `@types/node` 会报错），
 * 所以它不参与 `tsc --noEmit`；正确性靠 Node 运行时保证。
 */
import assert from "node:assert/strict";
import { test } from "node:test";
import {
  isPhaseAdvance,
  isPhaseRollback,
  isTerminal,
  shouldAccept,
} from "./taskPhase.ts";
import type { TaskPhase, TaskView } from "./types.ts";

function makeView(taskId: string, phase: TaskPhase): TaskView {
  return {
    taskId,
    versionId: "1.0",
    phase,
    totalFiles: 10,
    finishedFiles: 0,
    totalBytes: 1000,
    doneBytes: 0,
    fraction: 0,
    speedBps: 0,
    etaSeconds: null,
    downloaded: 0,
    skipped: 0,
    failed: 0,
    failures: [],
    message: null,
    startedAtMs: 0,
  };
}

test("没有历史记录时一律接受", () => {
  assert.equal(shouldAccept(makeView("a", "running")), true);
  assert.equal(shouldAccept(makeView("a", "done")), true);
});

test("running 的进度帧必须持续更新", () => {
  const existing = makeView("a", "running");
  assert.equal(shouldAccept(makeView("a", "running"), existing), true);
});

test("running 可以推进到任意终态", () => {
  const existing = makeView("a", "running");
  for (const phase of ["done", "failed", "cancelled"] as TaskPhase[]) {
    assert.equal(
      shouldAccept(makeView("a", phase), existing),
      true,
      `running → ${phase} 应该被接受`,
    );
  }
});

test("终态不许回退到 running —— 这是那个卡住 bug 的核心", () => {
  // install_start 的返回值晚一拍才 resolve，带着 phase: running，
  // 如果不拦住，它会把已经写入的 done 覆盖回去
  for (const phase of ["done", "failed", "cancelled"] as TaskPhase[]) {
    const existing = makeView("a", phase);
    assert.equal(
      shouldAccept(makeView("a", "running"), existing),
      false,
      `${phase} → running 必须被拒绝`,
    );
  }
});

test("终态之间可以互相覆盖（权重相同）", () => {
  assert.equal(
    shouldAccept(makeView("a", "cancelled"), makeView("a", "done")),
    true,
  );
  assert.equal(
    shouldAccept(makeView("a", "failed"), makeView("a", "cancelled")),
    true,
  );
});

test("阶段前进与倒退的判定", () => {
  assert.equal(isPhaseAdvance("done", "running"), true);
  assert.equal(isPhaseAdvance("failed", "running"), true);
  assert.equal(isPhaseAdvance("running", "done"), false);
  assert.equal(isPhaseAdvance("done", "done"), false);

  assert.equal(isPhaseRollback("running", "done"), true);
  assert.equal(isPhaseRollback("done", "running"), false);
  assert.equal(isPhaseRollback("done", "done"), false);
});

test("终态判定", () => {
  assert.equal(isTerminal("running"), false);
  assert.equal(isTerminal("done"), true);
  assert.equal(isTerminal("failed"), true);
  assert.equal(isTerminal("cancelled"), true);
});
