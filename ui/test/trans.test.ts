// 素材クリップのトランジション（ui/src/trans.ts）の確かめ。絵の計算は Rust 側（cargo test の trans::tests）。
import { test } from "node:test";
import assert from "node:assert/strict";
import { TRANS_DEFAULT_DUR, linkSwitch, prevClip, setTrans, transDur } from "../src/logic/trans.ts";
import type { MediaClip } from "../src/types.ts";

const C = (id: string, start: number, dur: number, src = `${id}.png`): MediaClip => ({
  id, name: id, src, start, dur, src_in: 0, loop: false, x: 300, y: 540, scale: 1, opacity: 1,
  fade_in: 0, fade_out: 0, volume_db: 0, speaker: "", flip: false,
});

test("transDur: 無し・既定・クリップより長くしない", () => {
  assert.equal(transDur(undefined, 5), 0);
  assert.equal(transDur({ kind: "", dur: 1 }, 5), 0);
  assert.equal(transDur({ kind: "wipe", dur: 0 }, 5), TRANS_DEFAULT_DUR);
  assert.equal(transDur({ kind: "wipe", dur: 2 }, 0.5), 0.5);
});

test("setTrans: 付ける・変える・外す（外したらキーごと消す）", () => {
  const c = C("a", 0, 5);
  setTrans(c, "in", { kind: "slide" });
  assert.deepEqual(c.trans_in, { kind: "slide", dur: TRANS_DEFAULT_DUR, dir: "" });
  setTrans(c, "in", { dir: "right", dur: 0.5 });
  assert.deepEqual(c.trans_in, { kind: "slide", dur: 0.5, dir: "right" });
  setTrans(c, "in", { kind: "" });
  assert.ok(!("trans_in" in c));
  // JSON にしても null を出さない（Rust 側の読み込みに合わせる）
  assert.ok(!JSON.stringify(c).includes("trans_in"));
});

test("prevClip: 前に始まって一番遅く終わるもの", () => {
  const a = C("a", 0, 4), b = C("b", 1, 2), c = C("c", 4, 3);
  assert.equal(prevClip([a, b, c], c)?.id, "a");
  assert.equal(prevClip([a, b, c], a), null);
});

test("linkSwitch: 接しているクリップを切れ目の中央で重ねる", () => {
  const a = C("a", 0, 4), b = C("b", 4, 3);
  const clips = [a, b];
  assert.equal(linkSwitch(clips, "b", "wipe", 0.4)?.id, "a");
  assert.equal(b.start, 3.8);
  assert.equal(b.dur, 3.2); // 終わりは変えない
  assert.equal(a.start + a.dur, 4.2);
  // ワイプは前のクリップを逆の向きに閉じる（同じ走査線で入れ替わる）
  assert.deepEqual(a.trans_out, { kind: "wipe", dur: 0.4, dir: "right" });
  assert.deepEqual(b.trans_in, { kind: "wipe", dur: 0.4, dir: "" });
});

test("linkSwitch: 向きのある種類は前の抜けを逆向きに、向きの無い種類は前の向きを変えない", () => {
  const a = C("a", 0, 4), b = C("b", 4, 3);
  b.trans_in = { kind: "slide", dur: 0.4, dir: "up" };
  linkSwitch([a, b], "b", "slide", 0.4);
  assert.equal(a.trans_out?.dir, "down");
  const p = C("p", 0, 4), q = C("q", 4, 3);
  p.trans_out = { kind: "wipe", dur: 0.4, dir: "up" };
  linkSwitch([p, q], "q", "fade", 0.4);
  assert.deepEqual(p.trans_out, { kind: "fade", dur: 0.4, dir: "up" });
});

test("linkSwitch: 録画は素材の頭もずらす・描く順を前のクリップの後ろにする", () => {
  const a = C("a", 0, 4, "a.mp4"), b = C("b", 4, 3, "b.mp4");
  b.src_in = 10;
  const clips = [b, a];
  linkSwitch(clips, "b", "fade", 1);
  assert.equal(b.start, 3.5);
  assert.equal(b.src_in, 9.5);
  assert.deepEqual(clips.map((x) => x.id), ["a", "b"]);
});

test("linkSwitch: もう重なっていれば重なりの中央、短いクリップからはみ出さない", () => {
  const a = C("a", 0, 5), b = C("b", 4, 2);
  linkSwitch([a, b], "b", "fade", 0.4);
  // 切れ目は重なり（4〜5）の中央 4.5
  assert.equal(b.start, 4.3);
  assert.equal(a.start + a.dur, 4.7);
  // 短いクリップ: 重なりは残り（0.3 秒）の 2 倍まで
  const p = C("p", 0, 4), q = C("q", 4, 0.3);
  linkSwitch([p, q], "q", "fade", 2);
  assert.equal(q.trans_in?.dur, 0.6);
  assert.equal(q.start, 3.7);
  assert.equal(p.start + p.dur, 4.3);
  assert.equal(linkSwitch([a], "a", "fade"), null);
});
