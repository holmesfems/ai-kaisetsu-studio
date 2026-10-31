// プレビューのクリックで選べるトラック（ui/src/pick.ts、issue 旧#11）の確かめ。`npm test` で動く。
import { test } from "node:test";
import assert from "node:assert/strict";
import { fxSelectable, pickMedia, selectable } from "../src/logic/pick.ts";
import type { MediaClip, Track } from "../src/types.ts";

const C = (id: string, over: Partial<MediaClip> = {}): MediaClip => ({
  id, name: id, src: `${id}.png`, start: 0, dur: 10, src_in: 0, loop: false,
  x: 960, y: 540, scale: 1, opacity: 1, fade_in: 0, fade_out: 0, volume_db: 0, ...over,
} as MediaClip);
const T = (id: string, kind: Track["kind"], clips: MediaClip[] = [], over: Partial<Track> = {}): Track => ({
  id, kind, name: id, clips, hidden: false, muted: false, locked: false, volume_db: 0, ...over,
});
/** 背景 1920x1080、立ち絵 400x800 */
const size = (src: string) => (src.startsWith("bg") ? { w: 1920, h: 1080 } : { w: 400, h: 800 });

test("selectable: 隠した・ロックしたトラックは選ばない", () => {
  assert.equal(selectable({ hidden: false, locked: false }), true);
  assert.equal(selectable({ hidden: true, locked: false }), false);
  assert.equal(selectable({ hidden: false, locked: true }), false);
});

test("fxSelectable: FX トラックのロック・非表示で演出を選ばない", () => {
  assert.equal(fxSelectable([]), true);
  assert.equal(fxSelectable([T("fx", "fx")]), true);
  assert.equal(fxSelectable([T("fx", "fx", [], { locked: true })]), false);
  assert.equal(fxSelectable([T("fx", "fx", [], { hidden: true })]), false);
});

test("pickMedia: 上のトラックから選び、ロックしたトラックは飛ばして下を選ぶ", () => {
  const bg = T("v1", "video", [C("bg")]);
  const chara = T("v3", "image", [C("chara", { x: 1500 })]);
  // 立ち絵の上をクリック → 立ち絵
  assert.equal(pickMedia([bg, chara], 1, 1500, 540, size)?.clip.id, "chara");
  // 立ち絵をロック → その下の背景
  const locked = { ...chara, locked: true };
  assert.equal(pickMedia([bg, locked], 1, 1500, 540, size)?.clip.id, "bg");
  // 背景もロック → 何も選ばない
  assert.equal(pickMedia([{ ...bg, locked: true }, locked], 1, 1500, 540, size), null);
});

test("pickMedia: 隠したトラック・時間外・範囲外・音声・大きさ不明は選ばない", () => {
  const chara = T("v3", "image", [C("chara", { start: 5, dur: 5 })]);
  assert.equal(pickMedia([chara], 6, 960, 540, size)?.clip.id, "chara");
  assert.equal(pickMedia([{ ...chara, hidden: true }], 6, 960, 540, size), null);
  assert.equal(pickMedia([chara], 4, 960, 540, size), null);
  assert.equal(pickMedia([chara], 10, 960, 540, size), null); // 終わりちょうどは含まない
  assert.equal(pickMedia([chara], 6, 1200, 540, size), null); // 幅 400 の外
  assert.equal(pickMedia([T("a1", "audio", [C("bgm")])], 1, 960, 540, size), null);
  assert.equal(pickMedia([chara], 6, 960, 540, () => undefined), null);
});

test("pickMedia: ロックしていない背景は選べる（以前の v1 の決め打ちは無くなった）", () => {
  assert.equal(pickMedia([T("v1", "video", [C("bg")])], 1, 100, 100, size)?.clip.id, "bg");
});
