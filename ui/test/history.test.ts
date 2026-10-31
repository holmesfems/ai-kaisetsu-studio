// 元に戻すの 3 方向の戻し（ST-6）
import { test } from "node:test";
import assert from "node:assert/strict";
import { deepEq, revert3 } from "../src/logic/history.ts";

const clip = (id: string, x = 0) => ({ id, x });
const doc = (clips: any[], extra: any = {}) => ({ tracks: [{ id: "v3", clips }], gap: 0.3, ...extra });

test("操作で変わった所だけ戻し、その後の外の変更は残す", () => {
  const before = doc([clip("a", 0), clip("b", 0)]);
  const after = doc([clip("a", 100), clip("b", 0)]); // 自分が a を動かした
  const cur = doc([clip("a", 100), clip("b", 50)], { gap: 0.5 }); // その後 Claude が b と間隔を変えた
  const r = revert3(cur, after, before);
  assert.deepEqual(r.value, doc([clip("a", 0), clip("b", 50)], { gap: 0.5 }));
  assert.equal(r.conflicts, 0);
});

test("同じ所がその後にも変わっていたら、戻さずに数える", () => {
  const before = doc([clip("a", 0)]);
  const after = doc([clip("a", 100)]);
  const cur = doc([clip("a", 300)]);
  const r = revert3(cur, after, before);
  assert.deepEqual(r.value, cur);
  assert.equal(r.conflicts, 1);
});

test("足したクリップは消し、消したクリップは元の場所に戻す", () => {
  const before = doc([clip("a"), clip("b"), clip("c")]);
  const after = doc([clip("a"), clip("c"), clip("n")]); // b を消して n を足した
  const cur = doc([clip("z"), clip("a"), clip("c"), clip("n")]); // その後 Claude が z を足した
  const r = revert3(cur, after, before);
  assert.deepEqual(r.value.tracks[0].clips.map((c: any) => c.id), ["z", "a", "b", "c"]);
});

test("外の変更を戻す（Ctrl+Alt+Z）と、自分のその後の操作は残る", () => {
  const before = { voice: { k1: "a" }, rev: 1 };
  const after = { voice: { k1: "a", k2: "b" }, rev: 2 }; // 外で k2 を足した
  const cur = { voice: { k1: "x", k2: "b" }, rev: 5 }; // その後 自分が k1 を直した
  const r = revert3(cur, after, before);
  assert.deepEqual(r.value, { voice: { k1: "x" }, rev: 5 });
});

test("戻したものをもう一度（やり直し）", () => {
  const before = doc([clip("a", 0)]);
  const after = doc([clip("a", 100)]);
  const undone = revert3(after, after, before).value;
  assert.ok(deepEq(undone, before));
  assert.ok(deepEq(revert3(undone, before, after).value, after));
});

test("別のトラックへ動かしたクリップを戻しても、同じ id が 2 つにならない", () => {
  const before = { tracks: [{ id: "t1", clips: [clip("x", 0)] }, { id: "t2", clips: [] as any[] }] };
  const after = { tracks: [{ id: "t1", clips: [] as any[] }, { id: "t2", clips: [clip("x", 0)] }] };
  const cur = { tracks: [{ id: "t1", clips: [] as any[] }, { id: "t2", clips: [clip("x", 9)] }] }; // その後に x が変わった
  const r = revert3(cur, after, before);
  const ids = r.value.tracks.flatMap((t: any) => t.clips.map((c: any) => c.id));
  assert.deepEqual(ids, ["x"]);
  assert.ok(r.conflicts > 0);
});

test("別のトラック・ブロックへ動かしただけなら、戻す・やり直すで元どおり", () => {
  const before = { tracks: [{ id: "t1", clips: [clip("x", 0), clip("y", 0)] }, { id: "t2", clips: [] as any[] }] };
  const after = { tracks: [{ id: "t1", clips: [clip("y", 0)] }, { id: "t2", clips: [clip("x", 0)] }] };
  const undone = revert3(after, after, before);
  assert.deepEqual(undone.value, before);
  assert.equal(undone.conflicts, 0);
  const redone = revert3(undone.value, before, after);
  assert.deepEqual(redone.value, after);
  // 台本の行をブロックの間で動かしたときも同じ
  const sb = { blocks: [{ id: "b1", lines: [{ id: "l1", text: "a" }] }, { id: "b2", lines: [] as any[] }] };
  const sa = { blocks: [{ id: "b1", lines: [] as any[] }, { id: "b2", lines: [{ id: "l1", text: "a" }] }] };
  assert.deepEqual(revert3(sa, sa, sb).value, sb);
});

test("並べ替えを戻す（その後に別の所が変わっていても）", () => {
  const before = { tracks: [{ id: "a", v: 0 }, { id: "b", v: 0 }, { id: "c", v: 0 }] };
  const after = { tracks: [{ id: "c", v: 0 }, { id: "a", v: 0 }, { id: "b", v: 0 }] }; // c を一番下へ
  const cur = { tracks: [{ id: "c", v: 0 }, { id: "a", v: 5 }, { id: "b", v: 0 }] }; // その後 Claude が a を変えた
  const r = revert3(cur, after, before);
  assert.deepEqual(r.value.tracks.map((t: any) => t.id), ["a", "b", "c"]);
  assert.equal(r.value.tracks[0].v, 5);
  assert.equal(r.conflicts, 0);
});

test("鍵の順番が違っても同じとみなす", () => {
  assert.ok(deepEq({ a: 1, b: [1, { c: 2, d: 3 }] }, { b: [1, { d: 3, c: 2 }], a: 1 }));
  assert.ok(!deepEq({ a: 1 }, { a: 1, b: 2 }));
});
