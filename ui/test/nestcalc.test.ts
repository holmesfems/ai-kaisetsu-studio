// ネストの素材クリップの計算（ui/src/nestcalc.ts・trans.ts の linkSwitch）の確かめ
import { test } from "node:test";
import assert from "node:assert/strict";
import { clipSpeed, commitDrag, maxEnd, minStart, setSpeed, slipSrcIn, sourceSpan, splitSrcIn, trimSrcIn } from "../src/logic/nestcalc.ts";
import { linkSwitch } from "../src/logic/trans.ts";
import type { MediaClip, Nest } from "../src/types.ts";

const C = (id: string, start: number, dur: number, extra: Partial<MediaClip> = {}): MediaClip => ({
  id, name: id, src: `${id}.mp4`, start, dur, src_in: 0, loop: false, x: 960, y: 540, scale: 1, opacity: 1,
  fade_in: 0, fade_out: 0, volume_db: 0, speaker: "", flip: false, ...extra,
});
const N = (): Nest => ({
  box: [0, 0, 1920, 1080], w: 1920, h: 1080, fit: "contain",
  tracks: ["V1", "V2", "V3", "V4"].map((id) => ({ id, kind: "video", name: id, clips: [], hidden: false, muted: false, locked: false, volume_db: 0 })),
} as Nest);

test("commitDrag: 別のトラックへ動かした結果を確定し、そのあと端を動かしてもそのトラックのまま", () => {
  const n = N();
  n.tracks[1].clips.push(C("a", 1, 4));
  assert.ok(commitDrag(n, { id: "a", start: 2, dur: 4, src_in: 0, track: "V4" }));
  assert.equal(n.tracks[1].clips.length, 0);
  assert.deepEqual(n.tracks[3].clips.map((c) => [c.id, c.start]), [["a", 2]]);
  // 端のドラッグ（track は今いるトラック）
  assert.ok(commitDrag(n, { id: "a", start: 2.5, dur: 3.5, src_in: 0.5, track: "V4" }));
  assert.deepEqual(n.tracks[3].clips.map((c) => [c.start, c.dur, c.src_in]), [[2.5, 3.5, 0.5]]);
  assert.equal(n.tracks[1].clips.length, 0);
});

test("commitDrag: 無いクリップ・無いトラックは何もしない", () => {
  const n = N();
  n.tracks[0].clips.push(C("a", 0, 1));
  assert.equal(commitDrag(n, { id: "zz", start: 1, dur: 1, src_in: 0, track: "V1" }), false);
  commitDrag(n, { id: "a", start: 1.00049, dur: 1, src_in: 0, track: "none" });
  assert.equal(n.tracks[0].clips[0].start, 1);
});

test("速度つきの src_in・長さの上限・左端", () => {
  const c = C("a", 10, 4, { src_in: 2, speed: 2 });
  assert.equal(clipSpeed(c), 2);
  assert.equal(clipSpeed(C("b", 0, 1)), 1);
  assert.equal(clipSpeed({ speed: 99 }), 10);
  assert.equal(trimSrcIn(c, 11), 4); // 1 秒進めると素材は 2 秒進む
  assert.equal(minStart(c, false), 9); // 素材の頭（src_in 2 秒 ÷ 速度 2）まで
  assert.equal(minStart(c, true), 0);
  assert.equal(maxEnd(c, 12), 15); // 素材の残り 10 秒 ÷ 2
  assert.equal(maxEnd(c, 0), Infinity);
  assert.equal(splitSrcIn(c, 12), 6);
});

test("setSpeed: 素材の同じ範囲を使うよう長さを変え、等速は speed を持たない", () => {
  const c = C("a", 0, 6);
  setSpeed(c, 2);
  assert.deepEqual([c.speed, c.dur], [2, 3]);
  setSpeed(c, 0.5);
  assert.deepEqual([c.speed, c.dur], [0.5, 12]);
  setSpeed(c, 1);
  assert.equal(c.speed, undefined);
  assert.equal(c.dur, 6);
  setSpeed(c, 100); // 10 倍まで
  assert.equal(c.speed, 10);
});

test("linkSwitch: 速度つきのクリップは src_in の変化も × 速度", () => {
  const p = C("p", 0, 4);
  const c = C("c", 4, 4, { src_in: 5, speed: 2 });
  linkSwitch([p, c], "c", "fade", 0.4);
  // 開始が 0.2 秒前へ → 素材は 0.4 秒ぶん手前から
  assert.equal(c.start, 3.8);
  assert.equal(c.src_in, 4.6);
});

test("sourceSpan: 素材全体の位置（速度つき・頭は負にもなる）", () => {
  assert.deepEqual(sourceSpan(C("a", 30, 5, { src_in: 300 }), 600), [-270, 330]);
  assert.deepEqual(sourceSpan(C("a", 10, 5, { src_in: 4, speed: 2 }), 20), [8, 18]);
});

test("slipSrcIn: 位置と長さはそのままで中身をずらし、素材の頭と終わりで止める", () => {
  const c = C("a", 30, 10, { src_in: 100 });
  assert.equal(slipSrcIn(c, 20, 600), 80, "右へ送ると前の場面が見える");
  assert.equal(slipSrcIn(c, -50, 600), 150);
  assert.equal(slipSrcIn(c, 500, 600), 0, "素材の頭で止まる");
  assert.equal(slipSrcIn(c, -1000, 600), 590, "終わりはクリップの長さぶん残す");
  assert.equal(slipSrcIn(C("b", 0, 10, { src_in: 10, speed: 2 }), 2, 600), 6, "速度ぶん素材が進む");
});

