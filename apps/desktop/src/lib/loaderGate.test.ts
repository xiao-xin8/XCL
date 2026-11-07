/**
 * 加载环闸门的测试。
 *
 * ```bash
 * node --test src/lib/loaderGate.test.ts
 * ```
 *
 * 这个文件被 `tsconfig.json` 排除在外（没装 `@types/node`），不参与 `tsc --noEmit`。
 */
import assert from "node:assert/strict";
import { test } from "node:test";
import {
  gateNextTickIn,
  gateShowsLoader,
  idleGate,
  nextGate,
  type GateLimits,
  type GateState,
} from "./loaderGate.ts";

const LIMITS: GateLimits = { showDelay: 400, minStay: 400 };

/** 从 idle 起跑，按 (时刻, 是否还在加载) 往下走。 */
function run(steps: [number, boolean][]): GateState {
  return steps.reduce<GateState>(
    (state, [now, loading]) => nextGate(state, loading, now, LIMITS),
    idleGate(),
  );
}

test("请求 400ms 内就完成 → 转圈从不出现", () => {
  assert.equal(gateShowsLoader(run([[0, true]])), false);
  assert.equal(gateShowsLoader(run([[0, true], [399, true]])), false);
  const done = run([[0, true], [300, false]]);
  assert.equal(done.phase, "idle");
  assert.equal(gateShowsLoader(done), false);
});

test("超过 400ms → 显示环，且从 400ms 那一刻算起", () => {
  const showing = run([[0, true], [400, true]]);
  assert.equal(showing.phase, "showing");
  assert.equal(showing.shownSince, 400);
  assert.equal(gateShowsLoader(showing), true);
});

test("数据好了也要补满最短停留时间", () => {
  const at500 = run([[0, true], [400, true], [500, false]]);
  assert.equal(at500.phase, "showing", "只显示了 100ms，不能立刻收");

  const at799 = run([[0, true], [400, true], [799, false]]);
  assert.equal(at799.phase, "showing");

  const at800 = run([[0, true], [400, true], [800, false]]);
  assert.equal(at800.phase, "idle");
});

test("数据一直没好就一直显示", () => {
  const state = run([[0, true], [400, true], [5000, true]]);
  assert.equal(state.phase, "showing");
});

test("同一时刻重复调用不会重复迁移", () => {
  const once = run([[0, true]]);
  const twice = nextGate(once, true, 0, LIMITS);
  assert.deepEqual(twice, once);
});

test("从 showing 回到 idle 之后能重新来一轮", () => {
  const back = run([[0, true], [400, true], [900, false]]);
  assert.equal(back.phase, "idle");
  const again = nextGate(back, true, 1000, LIMITS);
  assert.equal(again.phase, "waiting");
  assert.equal(again.waitingSince, 1000);
});

test("gateNextTickIn 只在两个等待窗口给值", () => {
  assert.equal(gateNextTickIn(idleGate(), false, 0, LIMITS), null);
  // 等 showDelay
  assert.equal(gateNextTickIn(run([[0, true]]), true, 150, LIMITS), 250);
  // 等 minStay
  const showing = run([[0, true], [400, true]]);
  assert.equal(gateNextTickIn(showing, false, 550, LIMITS), 250);
  // 已经显示完了，不会再自己动
  assert.equal(gateNextTickIn(showing, false, 900, LIMITS), null);
  // 数据还没好、环已在显示 → 等外部事件，不排计时器
  assert.equal(gateNextTickIn(showing, true, 900, LIMITS), null);
});
