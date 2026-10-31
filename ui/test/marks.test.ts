// モニターの印（ui/src/logic/marks.ts、仕様 docs/spec/feat-chat-marks.md）の確かめ。`npm test` で動く。
import { test } from "node:test";
import assert from "node:assert/strict";
import {
  DONE_ALPHA, MARK_COLORS, MARK_ONLY_TEXT, MAX_PEN, MAX_UNDER, bandLanes, bodyWithMarks, bytesToBase64, circled, circledList, clipsUnder,
  defaultSpan, drawMarks, dropMarkImages, groupByT, hitMark, listedMarks, markBBox, newMark, sendableMarks, shapeFromDrag, simplify,
  visibleMarks, type P,
} from "../src/logic/marks.ts";
import type { Mark } from "../src/gen/Mark.ts";
import type { Group, MediaClip, Track } from "../src/types.ts";

const M = (id: number, over: Partial<Mark> = {}): Mark => ({
  ...newMark("red", { kind: "pin", at: [100, 100] }, "", 5, [0, 10]), id, ...over,
});

const G = (key: string, bbox: [number, number, number, number] | null, t0 = 0, t1 = 10, z = 0): Group =>
  ({ key, label: key, func: null, line: 1, src: "", t0, t1, n: 1, kinds: [], z, fp: "", gui: false, se: [], bbox, rec: [] }) as Group;
const C = (id: string, over: Partial<MediaClip> = {}): MediaClip => ({
  id, name: id, src: `${id}.png`, start: 0, dur: 10, src_in: 0, loop: false,
  x: 960, y: 540, scale: 1, opacity: 1, fade_in: 0, fade_out: 0, volume_db: 0, ...over,
} as MediaClip);
const T = (id: string, kind: Track["kind"], clips: MediaClip[] = [], over: Partial<Track> = {}): Track => ({
  id, kind, name: id, clips, hidden: false, muted: false, locked: false, volume_db: 0, ...over,
});
const size = () => ({ w: 400, h: 800 });
const under = (shape: Mark["shape"], t: number, o: { groups?: Group[]; overrides?: any; tracks?: Track[]; size?: (s: string) => any } = {}) =>
  clipsUnder(shape, t, { groups: o.groups ?? [], overrides: o.overrides ?? {}, tracks: o.tracks ?? [], size: o.size ?? size });

test("色の表: 4 色・固定の色と意味", () => {
  assert.deepEqual(MARK_COLORS.map((c) => [c.key, c.hex, c.meaning]), [
    ["red", "#ff4d5e", "直して"], ["yellow", "#ffd23f", "注目"], ["green", "#3ddc84", "良い（残す）"], ["blue", "#4cc9ff", "参考"],
  ]);
});

test("番号: ①②…と、並べ書き", () => {
  assert.equal(circled(1), "①");
  assert.equal(circled(20), "⑳");
  assert.equal(circled(21), "㉑");
  assert.equal(circled(99), "(99)");
  assert.equal(circledList([1, 2]), "①②");
});

// ---- clipsUnder
test("clipsUnder: 重なる演出・素材だけ入る", () => {
  const groups = [G("a@c1#1", [800, 400, 1100, 500]), G("b@c1#2", [0, 0, 100, 100])];
  const tracks = [T("fx", "fx"), T("img", "image", [C("p1", { x: 960, y: 540, scale: 0.5 }), C("p2", { x: 100, y: 100, scale: 0.1 })])];
  const r = under({ kind: "rect", box: [820, 410, 300, 120] }, 5, { groups, tracks });
  assert.deepEqual(r.clips, ["a@c1#1"]);
  assert.deepEqual(r.media, ["p1"]);
});

test("clipsUnder: 区間の外・hidden の演出・隠した/ロックしたトラックは入らない", () => {
  const shape = { kind: "pin", at: [900, 450] } as const;
  const groups = [G("in", [800, 400, 1000, 500]), G("late", [800, 400, 1000, 500], 6, 9), G("hid", [800, 400, 1000, 500])];
  assert.deepEqual(under(shape, 5, { groups, tracks: [T("fx", "fx")], overrides: { hid: { hidden: true } } }).clips, ["in"]);
  // 微調整のずらし（shift）で区間が動く
  assert.deepEqual(under(shape, 7, { groups, tracks: [T("fx", "fx")], overrides: { in: { shift: 8 }, hid: { hidden: true } } }).clips, ["late"]);
  // FX トラックを隠す・ロックする → 演出は入らない
  assert.deepEqual(under(shape, 5, { groups, tracks: [T("fx", "fx", [], { locked: true })] }).clips, []);
  assert.deepEqual(under(shape, 5, { groups, tracks: [T("fx", "fx", [], { hidden: true })] }).clips, []);
  const img = (over: Partial<Track>) => [T("fx", "fx"), T("img", "image", [C("p")], over)];
  assert.deepEqual(under(shape, 5, { tracks: img({}) }).media, ["p"]);
  assert.deepEqual(under(shape, 5, { tracks: img({ hidden: true }) }).media, []);
  assert.deepEqual(under(shape, 5, { tracks: img({ locked: true }) }).media, []);
  // 素材の区間の外
  assert.deepEqual(under(shape, 12, { tracks: img({}) }).media, []);
  // 大きさがまだ分からない素材は飛ばす
  assert.deepEqual(under(shape, 5, { tracks: img({}), size: () => undefined }).media, []);
});

test("clipsUnder: 微調整の倍率・移動を入れた窓で当たる", () => {
  const groups = [G("w", [0, 0, 100, 100])];
  const tracks = [T("fx", "fx")];
  const shape = { kind: "pin", at: [1050, 50] } as const;
  assert.deepEqual(under(shape, 5, { groups, tracks }).clips, []);
  assert.deepEqual(under(shape, 5, { groups, tracks, overrides: { w: { dx: 1000 } } }).clips, ["w"]);
});

test("clipsUnder: 小さい順・最大 5", () => {
  const groups = Array.from({ length: 8 }, (_, i) => G(`g${i}`, [100 - (7 - i) * 10, 100 - (7 - i) * 10, 200 + (7 - i) * 10, 200 + (7 - i) * 10]));
  const r = under({ kind: "rect", box: [140, 140, 20, 20] }, 5, { groups, tracks: [T("fx", "fx")] });
  assert.equal(r.clips.length, MAX_UNDER);
  assert.deepEqual(r.clips, ["g7", "g6", "g5", "g4", "g3"]); // g7 が一番小さい
  const clips = Array.from({ length: 7 }, (_, i) => C(`m${i}`, { x: 960, y: 540, scale: 0.1 + i * 0.1 }));
  const m = under({ kind: "pin", at: [960, 540] }, 5, { tracks: [T("img", "image", clips)] });
  assert.deepEqual(m.media, ["m0", "m1", "m2", "m3", "m4"]);
});

test("clipsUnder: 矢印・ペンは外接矩形で重なりを見る", () => {
  const groups = [G("a", [1000, 100, 1100, 200])];
  const tracks = [T("fx", "fx")];
  assert.deepEqual(under({ kind: "arrow", from: [900, 150], to: [1050, 150] }, 5, { groups, tracks }).clips, ["a"]);
  assert.deepEqual(under({ kind: "arrow", from: [100, 800], to: [300, 900] }, 5, { groups, tracks }).clips, []);
  assert.deepEqual(under({ kind: "pen", points: [[950, 90], [1010, 120], [1000, 300]] }, 5, { groups, tracks }).clips, ["a"]);
});

// ---- 間引き・形
test("simplify: 直線は 2 点、同じ点は畳む", () => {
  const line: P[] = Array.from({ length: 50 }, (_, i) => [100 + i * 10, 200 + i * 5]);
  assert.deepEqual(simplify(line), [line[0], line[49]]);
  assert.deepEqual(simplify([[5, 5], [5, 5], [5, 5]]), [[5, 5]]);
});

test("simplify: 曲線は最大 32 点、端は残る", () => {
  const circle: P[] = Array.from({ length: 600 }, (_, i) => [960 + 400 * Math.cos(i / 20), 540 + 300 * Math.sin(i / 20)]);
  const s = simplify(circle);
  assert.ok(s.length > 2 && s.length <= MAX_PEN, String(s.length));
  assert.deepEqual(s[0], [Math.round(circle[0][0]), Math.round(circle[0][1])]);
  const noise: P[] = Array.from({ length: 800 }, (_, i) => [(i * 7919) % 1900, (i * 104729) % 1070]);
  assert.ok(simplify(noise).length <= MAX_PEN);
});

test("shapeFromDrag: 小さい（6px 未満の）ドラッグはピン", () => {
  assert.deepEqual(shapeFromDrag("rect", [[100, 100], [103, 104]]), { kind: "pin", at: [100, 100] });
  assert.deepEqual(shapeFromDrag("arrow", [[100, 100], [100, 100]]), { kind: "pin", at: [100, 100] });
  assert.deepEqual(shapeFromDrag("pen", [[100, 100], [102, 101]]), { kind: "pin", at: [100, 100] });
  assert.deepEqual(shapeFromDrag("rect", [[100, 100], [106, 100]]).kind, "rect");
  assert.deepEqual(shapeFromDrag("pin", [[10.4, 20.6], [500, 500]]), { kind: "pin", at: [10, 21] });
});

test("shapeFromDrag: 矩形は逆向きのドラッグでも x, y, 幅, 高さ。矢印は始点→終点。画面の外は端に寄せる", () => {
  assert.deepEqual(shapeFromDrag("rect", [[400, 300], [100, 50]]), { kind: "rect", box: [100, 50, 300, 250] });
  assert.deepEqual(shapeFromDrag("arrow", [[900, 500], [1200, 500]]), { kind: "arrow", from: [900, 500], to: [1200, 500] });
  assert.deepEqual(shapeFromDrag("rect", [[-50, -50], [3000, 2000]]), { kind: "rect", box: [0, 0, 1920, 1080] });
  const pen = shapeFromDrag("pen", [[0, 0], [100, 100], [200, 0]]);
  assert.deepEqual(pen, { kind: "pen", points: [[0, 0], [100, 100], [200, 0]] });
});

// ---- 区間・絞り込み
test("defaultSpan: t を含むシーン。無ければ t±2（0 より前には行かない）", () => {
  const scenes = [{ t0: 0, t1: 12 }, { t0: 12, t1: 30 }];
  assert.deepEqual(defaultSpan(12, scenes), [12, 30]);
  assert.deepEqual(defaultSpan(5, scenes), [0, 12]);
  assert.deepEqual(defaultSpan(50, scenes), [48, 52]);
  assert.deepEqual(defaultSpan(1, []), [0, 3]);
});

test("visibleMarks: 区間 [t0, t1)・済は「済も表示」のとき・表示を切ると出ない", () => {
  const ms = [M(1, { t0: 2, t1: 6 }), M(2, { t0: 2, t1: 6, state: "done" }), M(3, { t0: 8, t1: 9 })];
  const ids = (a: Mark[]) => a.map((m) => m.id);
  assert.deepEqual(ids(visibleMarks(ms, 3, true, false)), [1]);
  assert.deepEqual(ids(visibleMarks(ms, 3, true, true)), [1, 2]);
  assert.deepEqual(ids(visibleMarks(ms, 2, true, false)), [1]); // 頭を含む
  assert.deepEqual(ids(visibleMarks(ms, 6, true, false)), []); // 終わりは含まない
  assert.deepEqual(ids(visibleMarks(ms, 8.5, true, false)), [3]);
  assert.deepEqual(ids(visibleMarks(ms, 3, false, true)), []);
});

test("listedMarks（タイムライン）: 表示を切ると出ない・済は「済も表示」のとき", () => {
  const ms = [M(1), M(2, { state: "done" }), M(3, { state: "sent", by: "claude" })];
  assert.deepEqual(listedMarks(ms, true, false).map((m) => m.id), [1, 3]);
  assert.deepEqual(listedMarks(ms, true, true).map((m) => m.id), [1, 2, 3]);
  assert.deepEqual(listedMarks(ms, false, true), []);
});

test("bandLanes: 重なる帯は別の段、重ならなければ同じ段", () => {
  const { lane, n } = bandLanes([M(1, { t0: 0, t1: 5 }), M(2, { t0: 3, t1: 8 }), M(3, { t0: 5, t1: 9 })]);
  assert.equal(n, 2);
  assert.deepEqual([lane.get(1), lane.get(2), lane.get(3)], [0, 1, 0]);
  assert.equal(bandLanes([]).n, 0);
});

// ---- チャットに添える印
test("sendableMarks: 自分の draft だけ・外したものは除く・番号順", () => {
  const ms = [M(3), M(1), M(2, { state: "sent" }), M(4, { by: "claude", state: "draft" }), M(5, { state: "done" })];
  assert.deepEqual(sendableMarks(ms, new Set()).map((m) => m.id), [1, 3]);
  assert.deepEqual(sendableMarks(ms, new Set([1])).map((m) => m.id), [3]);
});

test("groupByT: 確認画像は印の t ごとに 1 枚", () => {
  const g = groupByT([M(1, { t: 5 }), M(2, { t: 7.5 }), M(3, { t: 5.0004 })]);
  assert.deepEqual(g.map((x) => [x.t, x.marks.map((m) => m.id)]), [[5, [1, 3]], [7.5, [2]]]);
});

test("bodyWithMarks: 印だけなら「（印を送りました）」。文があればそのまま、画像だけなら画像の文", () => {
  assert.equal(bodyWithMarks("  ", 1, 2, "（画像を送りました）"), MARK_ONLY_TEXT);
  assert.equal(bodyWithMarks("小さく", 1, 2, "（画像を送りました）"), "小さく");
  assert.equal(bodyWithMarks("", 1, 0, "（画像を送りました）"), "（画像を送りました）");
  assert.equal(bodyWithMarks("", 0, 0, "（画像を送りました）"), "");
});

test("dropMarkImages: 取り消しで入力欄に戻す画像から、印の確認画像を除く", () => {
  assert.deepEqual(
    dropMarkImages(["E:\\x\\studio\\chat\\mark-20261022-1.jpg", "E:\\x\\studio\\chat\\paste-1.png", "/a/chat/mark-2.jpg"]),
    ["E:\\x\\studio\\chat\\paste-1.png"],
  );
});

test("bytesToBase64: 大きな配列でも btoa と同じ", () => {
  const b = new Uint8Array(100000).map((_, i) => i % 251);
  assert.equal(bytesToBase64(b), Buffer.from(b).toString("base64"));
});

// ---- 当たり判定
test("hitMark: 線・番号の丸の近くで選ぶ。矩形の中は当たりにしない・近いものを優先", () => {
  const rect = M(1, { shape: { kind: "rect", box: [200, 200, 400, 300] } });
  const arrow = M(2, { shape: { kind: "arrow", from: [800, 800], to: [1200, 800] } });
  const pen = M(3, { shape: { kind: "pen", points: [[1500, 100], [1600, 200], [1700, 100]] } });
  const pin = M(4, { shape: { kind: "pin", at: [100, 900] } });
  const all = [rect, arrow, pen, pin];
  assert.equal(hitMark(all, 400, 203, 8), 1); // 上の辺
  assert.equal(hitMark(all, 400, 250, 8), null); // 矩形の中
  assert.equal(hitMark(all, 205, 205, 8), 1); // 左上の番号の丸
  assert.equal(hitMark(all, 1000, 805, 8), 2);
  assert.equal(hitMark(all, 1000, 830, 8), null);
  assert.equal(hitMark(all, 1650, 150, 8), 3);
  assert.equal(hitMark(all, 110, 905, 8), 4);
  assert.equal(hitMark(all, 124, 900, 8), 4); // 丸の半径 17 の外側 8 まで
  assert.equal(hitMark(all, 130, 900, 8), null);
  // 重なっていれば新しい番号
  assert.equal(hitMark([M(7, { shape: { kind: "pin", at: [50, 50] } }), M(8, { shape: { kind: "pin", at: [50, 50] } })], 50, 50, 8), 8);
});

test("markBBox", () => {
  assert.deepEqual(markBBox({ kind: "rect", box: [10, 20, 30, 40] }), [10, 20, 40, 60]);
  assert.deepEqual(markBBox({ kind: "arrow", from: [50, 5], to: [10, 40] }), [10, 5, 50, 40]);
  assert.deepEqual(markBBox({ kind: "pen", points: [[3, 9], [7, 1]] }), [3, 1, 7, 9]);
});

// ---- 描き方（記録する偽の ctx）
function fake() {
  const log: { op: string; args: unknown[] }[] = [];
  const state: Record<string, unknown> = {};
  const ctx: any = new Proxy({}, {
    get(_t, name: string) {
      if (name === "measureText") return (s: string) => ({ width: s.length * 10 });
      if (name in state) return state[name];
      return (...args: unknown[]) => { log.push({ op: name, args }); };
    },
    set(_t, name: string, v) {
      state[name] = v;
      log.push({ op: `set:${name}`, args: [v] });
      return true;
    },
  });
  return { ctx, log };
}
const ops = (log: { op: string }[], name: string) => log.filter((l) => l.op === name);

test("drawMarks: 済は不透明度 0.35 と ✓、Claude の印は破線、ふつうは実線・不透明", () => {
  const rect = { kind: "rect", box: [10, 10, 100, 50] } as const;
  const a = fake();
  drawMarks(a.ctx, [M(1, { shape: rect })], 1, { note: false });
  assert.ok(ops(a.log, "set:globalAlpha").every((l) => l.args[0] === 1));
  assert.ok(ops(a.log, "setLineDash").every((l) => (l.args[0] as number[]).length === 0));
  assert.equal(ops(a.log, "fillText").some((l) => l.args[0] === "✓"), false);
  const d = fake();
  drawMarks(d.ctx, [M(1, { shape: rect, state: "done" })], 1, { note: false });
  assert.equal(ops(d.log, "set:globalAlpha")[0].args[0], DONE_ALPHA);
  assert.equal(ops(d.log, "fillText").some((l) => l.args[0] === "✓"), true);
  const c = fake();
  drawMarks(c.ctx, [M(1, { shape: rect, by: "claude" })], 1, { note: false });
  assert.ok(ops(c.log, "setLineDash").some((l) => (l.args[0] as number[]).length > 0));
});

test("drawMarks: 線は 4px・丸は直径 34px（k 倍）、色は表の色、番号は白", () => {
  const f = fake();
  drawMarks(f.ctx, [M(12, { color: "blue", shape: { kind: "arrow", from: [100, 100], to: [300, 100] } })], 0.5, { note: false });
  const lw = ops(f.log, "set:lineWidth").map((l) => l.args[0]);
  assert.ok(lw.includes(4 * 0.5));
  const arc = ops(f.log, "arc")[0].args;
  assert.deepEqual([arc[0], arc[1], arc[2]], [50, 50, 17 * 0.5]); // 始点に丸
  assert.ok(ops(f.log, "set:strokeStyle").some((l) => l.args[0] === "#4cc9ff"));
  assert.ok(ops(f.log, "fillText").some((l) => l.args[0] === "12"));
});

test("drawMarks: 一言はモニターだけ（note: false の確認画像には描かない）", () => {
  const m = M(1, { note: "もう少し小さく" });
  const mon = fake();
  drawMarks(mon.ctx, [m], 1, { note: true });
  assert.ok(ops(mon.log, "fillText").some((l) => l.args[0] === "もう少し小さく"));
  const img = fake();
  drawMarks(img.ctx, [m], 1, { note: false });
  assert.equal(ops(img.log, "fillText").some((l) => l.args[0] === "もう少し小さく"), false);
});

test("drawMarks: ピンは丸だけ（線は描かない）・番号 0（描き途中）は数字なし・選んだ印は白い破線の枠", () => {
  const f = fake();
  drawMarks(f.ctx, [M(0)], 1, { note: true, selectedId: null });
  assert.equal(ops(f.log, "strokeRect").length, 0);
  assert.equal(ops(f.log, "lineTo").length, 0);
  assert.equal(ops(f.log, "fillText").length, 0);
  assert.equal(ops(f.log, "arc").length, 1);
  const s = fake();
  drawMarks(s.ctx, [M(2)], 1, { note: false, selectedId: 2 });
  assert.equal(ops(s.log, "strokeRect").length, 1);
});
