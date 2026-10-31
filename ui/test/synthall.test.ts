// 「音声を一括作成」の中止（ui/src/synthall.ts）の確かめ。旧#32: 中止ボタンと Esc で止めたい
import { test } from "node:test";
import assert from "node:assert/strict";
import { MAX_FAILS_IN_ROW, isSynthStopKey, runSynthAll, synthAllNotice } from "../src/logic/synthall.ts";

const key = (k: string, mods: Partial<{ ctrlKey: boolean; metaKey: boolean; shiftKey: boolean; altKey: boolean; isComposing: boolean; keyCode: number }> = {}) => ({
  key: k, ctrlKey: false, metaKey: false, shiftKey: false, altKey: false, ...mods,
});
const free = { typing: false, inChat: false, nestOpen: false };

test("最後まで作れば、全部作成したと知らせる", async () => {
  const made: number[] = [];
  const prog: [number, number][] = [];
  const r = await runSynthAll([1, 2, 3], async (x) => { made.push(x); }, { shouldStop: () => false, onProgress: (i, n) => prog.push([i, n]) });
  assert.deepEqual(made, [1, 2, 3]);
  assert.deepEqual(prog, [[0, 3], [1, 3], [2, 3]]);
  assert.deepEqual(r, { total: 3, done: 3, failed: 0, stopped: null, left: 0 });
  assert.deepEqual(synthAllNotice(r), { text: "すべての音声を作成しました", kind: "ok" });
});

test("中止したら、作成中の行は最後まで作り、次の行に進まない", async () => {
  let stop = false;
  const made: number[] = [];
  const r = await runSynthAll([1, 2, 3, 4], async (x) => {
    // 2 行目を作っている途中で中止が押された
    if (x === 2) stop = true;
    await Promise.resolve();
    made.push(x);
  }, { shouldStop: () => stop });
  assert.deepEqual(made, [1, 2], "作成中だった 2 行目の音声は使う");
  assert.deepEqual(r, { total: 4, done: 2, failed: 0, stopped: "cancel", left: 2 });
  assert.deepEqual(synthAllNotice(r), { text: "2 行の音声を作成して中止しました", kind: "info" });
});

test("最初の行の前に中止されていれば、何も作らない", async () => {
  const r = await runSynthAll([1, 2], async () => assert.fail("作らない"), { shouldStop: () => true });
  assert.deepEqual(r, { total: 2, done: 0, failed: 0, stopped: "cancel", left: 2 });
  assert.equal(synthAllNotice(r).text, "音声を作成する前に中止しました");
});

test("1 行も作れずに中止したら、作成する前に中止したと知らせる", () => {
  assert.equal(synthAllNotice({ total: 4, done: 0, failed: 1, stopped: "cancel", left: 3 }).text, "音声を作成する前に中止しました（1 行は作成できませんでした）");
});

test("失敗した行の後で中止したら、失敗した数も数えて止める", async () => {
  let stop = false;
  const r = await runSynthAll([1, 2, 3, 4], async (x) => {
    if (x === 1) throw new Error("x");
    stop = true;
  }, { shouldStop: () => stop });
  assert.deepEqual(r, { total: 4, done: 1, failed: 1, stopped: "cancel", left: 2 });
  assert.equal(synthAllNotice(r).text, "1 行の音声を作成して中止しました（1 行は作成できませんでした）");
});

test("最後の行を作っている間に中止しても、全部作れたことになる", async () => {
  let stop = false;
  const r = await runSynthAll([1, 2], async (x) => { if (x === 2) stop = true; }, { shouldStop: () => stop });
  assert.equal(r.stopped, null);
  assert.equal(r.done, 2);
});

test("中止のときに失敗した行があれば、その数も知らせる", () => {
  assert.equal(synthAllNotice({ total: 5, done: 2, failed: 1, stopped: "cancel", left: 2 }).text, "2 行の音声を作成して中止しました（1 行は作成できませんでした）");
});

test(`${MAX_FAILS_IN_ROW} 行続けて失敗したら、残りはやめる`, async () => {
  const errs: number[] = [];
  const r = await runSynthAll([1, 2, 3, 4, 5], async (x) => { if (x >= 2) throw new Error("timeout"); }, {
    shouldStop: () => false, onError: (x) => errs.push(x),
  });
  assert.deepEqual(errs, [2, 3]);
  assert.deepEqual(r, { total: 5, done: 1, failed: 2, stopped: "errors", left: 2 });
  assert.deepEqual(synthAllNotice(r), { text: "エラーが続いたため、残りの 2 行の音声は作成しませんでした", kind: "error" });
});

test("失敗が続かなければ最後まで作り、失敗した行の数を知らせる", async () => {
  const r = await runSynthAll([1, 2, 3, 4], async (x) => { if (x % 2 === 0) throw new Error("x"); }, { shouldStop: () => false });
  assert.deepEqual(r, { total: 4, done: 2, failed: 2, stopped: null, left: 0 });
  assert.deepEqual(synthAllNotice(r), { text: "2 行の音声を作成しました（2 行は作成できませんでした）", kind: "error" });
});

test("最後の 2 行で続けて失敗しても、残りは無いのでエラーで止めたことにしない", async () => {
  const r = await runSynthAll([1, 2, 3], async (x) => { if (x >= 2) throw new Error("x"); }, { shouldStop: () => false });
  assert.equal(r.stopped, null);
  assert.equal(r.failed, 2);
});

test("作る行が無ければ、すぐ終わる", async () => {
  const r = await runSynthAll([], async () => {}, { shouldStop: () => false });
  assert.deepEqual(synthAllNotice(r).kind, "ok");
});

test("Esc（修飾キーなし）で中止する", () => {
  assert.equal(isSynthStopKey(key("Escape"), free), true);
  for (const m of ["ctrlKey", "metaKey", "shiftKey", "altKey"] as const) assert.equal(isSynthStopKey(key("Escape", { [m]: true }), free), false, m);
  assert.equal(isSynthStopKey(key("Enter"), free), false);
});

test("入力欄・Claude のチャット・録画を差し込む画面・IME の変換中は、Esc で中止しない", () => {
  assert.equal(isSynthStopKey(key("Escape"), { ...free, typing: true }), false, "入力欄の Esc は欄の側に任せる");
  assert.equal(isSynthStopKey(key("Escape"), { ...free, inChat: true }), false, "チャットの Esc は送ったメッセージの取り消し");
  assert.equal(isSynthStopKey(key("Escape"), { ...free, nestOpen: true }), false, "録画を差し込む画面は Esc で閉じる");
  assert.equal(isSynthStopKey(key("Escape", { isComposing: true }), free), false);
  assert.equal(isSynthStopKey(key("Escape", { keyCode: 229 }), free), false);
});
