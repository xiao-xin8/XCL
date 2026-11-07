/**
 * 下载进度 store 的测试。
 *
 * 重点不是「能存能读」，而是两条不变量：
 * 1. 只有变化的那条任务换对象，其它任务保持引用 —— 这是「别的组件不重渲染」的前提；
 * 2. 阶段只能往前走，完成计数每个任务只加一次 —— 界面提示与列表刷新靠它。
 *
 * ```bash
 * node --test src/lib/downloadStore.test.ts
 * ```
 */
import assert from "node:assert/strict";
import { test } from "node:test";
import {
  isFilePairDone,
  markCancelled,
  peekDoneSignal,
  peekFileDoneSignal,
  peekTask,
  peekTaskForFile,
  publishTask,
  removeTask,
  syncTasks,
} from "./downloadStore.ts";
import type { TaskView } from "./types.ts";

let seq = 0;

function task(taskId: string, over: Partial<TaskView> = {}): TaskView {
  seq += 1;
  return {
    taskId,
    versionId: `v-${taskId}`,
    parentId: null,
    title: null,
    target: null,
    phase: "running",
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
    startedAtMs: seq,
    ...over,
  };
}

test("同一进度只影响那一条任务，其它任务的引用不变", () => {
  publishTask(task("a"));
  publishTask(task("b"));
  const before = peekTask("b");

  publishTask(task("a", { doneBytes: 500, fraction: 0.5 }));

  assert.equal(peekTask("b"), before, "没变化的任务必须保持同一个对象");
  assert.equal(peekTask("a")?.doneBytes, 500);
});

test("迟到的旧快照（阶段回退）被丢弃", () => {
  publishTask(task("rollback", { phase: "done" }));
  const result = publishTask(task("rollback", { phase: "running" }));
  assert.equal(result.accepted, false);
  assert.equal(peekTask("rollback")?.phase, "done");
});

test("完成计数每个任务只加一次，重复的完成快照不会重复计数", () => {
  publishTask(task("count", { phase: "running" }));
  const before = peekDoneSignal();

  publishTask(task("count", { phase: "done" }));
  publishTask(task("count", { phase: "done" }));
  publishTask(task("count", { phase: "done", doneBytes: 999 }));

  assert.equal(peekDoneSignal(), before + 1);
});

test("后端任务表对齐不会清掉本地进度，也不会重复插入", () => {
  publishTask(task("sync", { doneBytes: 800, fraction: 0.8 }));

  const added = syncTasks([
    task("sync", { doneBytes: 100, fraction: 0.1 }),
    task("sync-new"),
  ]);

  assert.deepEqual(
    added.map((item) => item.taskId),
    ["sync-new"],
    "已在本地的不算新增",
  );
  // 对齐以任务表为准：running 阶段的帧本来就是持续更新的（终态才受单调规则保护）
  assert.equal(peekTask("sync")?.doneBytes, 100);
});

test("取消只作用于还在跑的任务", () => {
  publishTask(task("cancel-a"));
  markCancelled("cancel-a", "已取消");
  assert.equal(peekTask("cancel-a")?.phase, "cancelled");

  publishTask(task("cancel-b", { phase: "done" }));
  markCancelled("cancel-b", "已取消");
  assert.equal(peekTask("cancel-b")?.phase, "done", "已完成的任务不能被改回取消");
});

test("移除任务后读不到，重复移除不报错", () => {
  publishTask(task("gone"));
  removeTask("gone");
  assert.equal(peekTask("gone"), undefined);
  removeTask("gone");
});

test("同一个文件装到两个实例：按落点分开取，互不覆盖", () => {
  publishTask(task("f-a", { versionId: "mod:file-1", target: "1.21.10" }));
  publishTask(task("f-b", { versionId: "mod:file-1", target: "1.20.1" }));

  assert.equal(peekTaskForFile("file-1", "1.21.10")?.taskId, "f-a");
  assert.equal(peekTaskForFile("file-1", "1.20.1")?.taskId, "f-b");
  assert.equal(peekTaskForFile("file-1", "1.19.2"), undefined, "没装过的落点没有任务");
  assert.equal(peekTaskForFile("file-2", "1.21.10"), undefined, "别的文件不串过来");
});

test("任务被清掉后「已完成」还在，重新开始才作废", () => {
  publishTask(task("p1", { versionId: "mod:file-7", target: "1.21.10" }));
  assert.equal(isFilePairDone("file-7", "1.21.10"), false);

  publishTask(
    task("p1", { versionId: "mod:file-7", target: "1.21.10", phase: "done" }),
  );
  removeTask("p1");
  assert.equal(
    isFilePairDone("file-7", "1.21.10"),
    true,
    "成功的任务 1.2s 后会被清掉，行上仍要标已完成",
  );

  publishTask(task("p2", { versionId: "mod:file-7", target: "1.21.10" }));
  assert.equal(isFilePairDone("file-7", "1.21.10"), false, "重下时旧的结论作废");
});

test("单文件完成计数只认 mod 任务，不和版本安装混在一起", () => {
  const before = peekFileDoneSignal();

  publishTask(task("ver-only", { phase: "running" }));
  publishTask(task("ver-only", { phase: "done" }));
  assert.equal(peekFileDoneSignal(), before, "版本安装完成不算单文件完成");

  publishTask(task("mod-only", { versionId: "mod:file-9", target: "1.21.10" }));
  publishTask(
    task("mod-only", { versionId: "mod:file-9", target: "1.21.10", phase: "done" }),
  );
  assert.equal(peekFileDoneSignal(), before + 1);
});
