// 再生で未準備の手前で止まる・再開する判断 (旧#83)
import { test } from "node:test";
import assert from "node:assert/strict";
import { canResume, firstGap, LEAD, RESUME_AHEAD, shouldWait, waitText } from "../src/logic/playwait.ts";
import type { PlayReady } from "../src/gen/PlayReady.ts";

const ready = (p: Partial<PlayReady> = {}): PlayReady => ({ fx: [], fx_count: [0, 0], subs: [], voice: [], ...p });
const all = () => true;

test("firstGap: 何も無ければ null", () => {
  assert.equal(firstGap(ready(), 1, all), null);
});

test("firstGap: t より前から続く区間は t から", () => {
  const g = firstGap(ready({ fx: [[0.5, 2]] }), 1, all);
  assert.deepEqual(g, { at: 1, kinds: ["fx"] });
});

test("firstGap: t より前に終わった区間は無視", () => {
  assert.equal(firstGap(ready({ fx: [[0, 0.9]] }), 1, all), null);
});

test("firstGap: 一番早い始まりを選ぶ", () => {
  const g = firstGap(ready({ fx: [[3, 4]], subs: [[2, 2.5]] }), 1, all);
  assert.deepEqual(g, { at: 2, kinds: ["subs"] });
});

test("firstGap: 同じ時刻（1 コマ以内）に始まる種類をまとめる", () => {
  const g = firstGap(
    ready({ fx: [[2, 3]], subs: [[2.02, 3]], voice: [{ line_id: "a", t0: 5, t1: 6 }] }),
    1,
    all,
  );
  assert.deepEqual(g, { at: 2, kinds: ["fx", "subs"] });
});

test("firstGap: voiceWaitable が false の行は飛ばす", () => {
  const r = ready({ voice: [{ line_id: "a", t0: 2, t1: 3 }, { line_id: "b", t0: 4, t1: 5 }] });
  assert.deepEqual(firstGap(r, 1, (id) => id === "b"), { at: 4, kinds: ["voice"] });
  assert.equal(firstGap(r, 1, () => false), null);
});

test("shouldWait: LEAD の手前に未準備が来たら止まる", () => {
  assert.equal(shouldWait(1, 100, { at: 1 + LEAD - 0.01, kinds: ["fx"] }), true);
  assert.equal(shouldWait(1, 100, { at: 1 + LEAD, kinds: ["fx"] }), false);
  assert.equal(shouldWait(1, 100, null), false);
});

test("shouldWait: 終わりより先の未準備では止まらない", () => {
  assert.equal(shouldWait(9.9, 10, { at: 10.1, kinds: ["fx"] }), false);
});

test("canResume: RESUME_AHEAD 秒先までそろったら再開", () => {
  assert.equal(canResume(1, 100, null), true);
  assert.equal(canResume(1, 100, { at: 1 + RESUME_AHEAD - 0.1, kinds: ["fx"] }), false);
  assert.equal(canResume(1, 100, { at: 1 + RESUME_AHEAD, kinds: ["fx"] }), true);
});

test("canResume: 終わりまでが 3 秒未満なら終わりまでそろえばよい", () => {
  assert.equal(canResume(8, 10, { at: 10, kinds: ["fx"] }), true);
  assert.equal(canResume(8, 10, { at: 9, kinds: ["fx"] }), false);
});

test("waitText: 演出・声・字幕を並べる", () => {
  const r = ready({
    fx: [[2, 3]],
    fx_count: [12, 90],
    subs: [[2, 3]],
    voice: [{ line_id: "a", t0: 2, t1: 3 }, { line_id: "b", t0: 3, t1: 4 }],
  });
  assert.equal(waitText(r, 1, all), "準備中: 演出のコマ 12/90・声を作成中（2 行）・字幕");
});

test("waitText: 待たない声は数えない・無ければ「準備中」だけ", () => {
  const r = ready({ voice: [{ line_id: "a", t0: 2, t1: 3 }] });
  assert.equal(waitText(r, 1, () => false), "準備中");
  assert.equal(waitText(r, 1, all), "準備中: 声を作成中（1 行）");
});
