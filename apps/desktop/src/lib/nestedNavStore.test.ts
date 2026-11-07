/**
 * 子页导航 store 的测试（注册 / 覆盖 / 注销只认自己的 token）。
 *
 * ```bash
 * node --test src/lib/nestedNavStore.test.ts
 * ```
 */
import assert from "node:assert/strict";
import { test } from "node:test";
import {
  getNestedPage,
  registerNestedPage,
  resetNestedPage,
  subscribeNestedPage,
  unregisterNestedPage,
} from "./nestedNavStore.ts";

test("注册之后能读到标题，注销之后回到主页面", () => {
  resetNestedPage();
  const token = Symbol("a");
  registerNestedPage(token, "Sodium", () => undefined);
  assert.equal(getNestedPage()?.title, "Sodium");

  unregisterNestedPage(token);
  assert.equal(getNestedPage(), null);
});

test("别人的 token 注销不掉当前注册", () => {
  resetNestedPage();
  const mine = Symbol("mine");
  registerNestedPage(mine, "Sodium", () => undefined);
  unregisterNestedPage(Symbol("other"));
  assert.equal(getNestedPage()?.title, "Sodium");
});

test("同一个子页只改标题时保持引用不变（不白重渲染）", () => {
  resetNestedPage();
  const token = Symbol("a");
  registerNestedPage(token, "Sodium", () => undefined);
  const first = getNestedPage();

  registerNestedPage(token, "Sodium", () => undefined);
  assert.equal(getNestedPage(), first, "同 token 同标题应当原样返回");

  registerNestedPage(token, "Lithium", () => undefined);
  assert.notEqual(getNestedPage(), first, "标题变了要换引用");
  assert.equal(getNestedPage()?.title, "Lithium");
});

test("返回回调跟着最新注册走", () => {
  resetNestedPage();
  const token = Symbol("a");
  let calls = 0;
  registerNestedPage(token, "Sodium", () => {
    calls += 1;
  });
  getNestedPage()?.onBack();
  assert.equal(calls, 1);
});

test("订阅者能收到注册与注销", () => {
  resetNestedPage();
  const seen: (string | null)[] = [];
  const off = subscribeNestedPage(() => {
    seen.push(getNestedPage()?.title ?? null);
  });

  const token = Symbol("a");
  registerNestedPage(token, "Sodium", () => undefined);
  unregisterNestedPage(token);
  off();

  registerNestedPage(Symbol("b"), "Lithium", () => undefined);
  assert.deepEqual(seen, ["Sodium", null], "退订之后不该再收到");
});

test("依赖页压栈之后，返回键跟着栈顶走", () => {
  resetNestedPage();
  const detail = Symbol("detail");
  const dep = Symbol("dep");
  let backDetail = 0;
  let backDep = 0;

  registerNestedPage(detail, "Sodium", () => {
    backDetail += 1;
  });
  registerNestedPage(dep, "Fabric API", () => {
    backDep += 1;
  });

  assert.equal(getNestedPage()?.title, "Fabric API");
  getNestedPage()?.onBack();
  assert.equal(backDep, 1, "栈顶是依赖页");
  assert.equal(backDetail, 0);

  // 依赖页卸载 = 出栈，下面的详情页连同返回键一起恢复
  unregisterNestedPage(dep);
  assert.equal(getNestedPage()?.title, "Sodium");
  getNestedPage()?.onBack();
  assert.equal(backDetail, 1);
});

test("下层页面卸载不会把栈顶的依赖页一起带走", () => {
  resetNestedPage();
  const detail = Symbol("detail");
  const dep = Symbol("dep");
  registerNestedPage(detail, "Sodium", () => undefined);
  registerNestedPage(dep, "Fabric API", () => undefined);

  unregisterNestedPage(detail);
  assert.equal(getNestedPage()?.title, "Fabric API", "栈顶不该被下层注销影响");

  unregisterNestedPage(dep);
  assert.equal(getNestedPage(), null);
});
