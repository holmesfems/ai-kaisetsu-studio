// タイムラインのホイールの割り当て（ui/src/util.ts の wheelAct）の確かめ。issue 旧#13
import { test } from "node:test";
import assert from "node:assert/strict";
import { edgeScrollStep, wheelAct } from "../src/logic/util.ts";

const page = { w: 800, h: 300 };
const W = (o: Partial<Pick<WheelEvent, "deltaX" | "deltaY" | "deltaMode" | "altKey" | "ctrlKey" | "shiftKey">>) => ({
  deltaX: 0, deltaY: 0, deltaMode: 0, altKey: false, ctrlKey: false, shiftKey: false, ...o,
});

test("修飾なしは左右スクロール", () => {
  assert.deepEqual(wheelAct(W({ deltaY: 100 }), page), { kind: "h", d: 100 });
  assert.deepEqual(wheelAct(W({ deltaX: -30, deltaY: 5 }), page), { kind: "h", d: -30 }); // 横スワイプ
});

test("Shift は上下スクロール", () => {
  assert.deepEqual(wheelAct(W({ deltaY: 100, shiftKey: true }), page), { kind: "v", d: 100 });
  assert.deepEqual(wheelAct(W({ deltaX: 100, shiftKey: true }), page), { kind: "v", d: 100 }); // deltaX に入る環境
});

test("Alt はズーム（上に回すと拡大）", () => {
  assert.deepEqual(wheelAct(W({ deltaY: -100, altKey: true }), page), { kind: "zoom", zoomIn: true });
  assert.deepEqual(wheelAct(W({ deltaY: 100, altKey: true }), page), { kind: "zoom", zoomIn: false });
  // Alt で deltaX に入ってくる環境でも向きが取れる
  assert.deepEqual(wheelAct(W({ deltaX: -100, altKey: true }), page), { kind: "zoom", zoomIn: true });
  // Alt+Shift もズーム（スクロールにはしない）
  assert.equal(wheelAct(W({ deltaY: 100, altKey: true, shiftKey: true }), page).kind, "zoom");
});

test("Ctrl（タッチパッドのピンチ）もズームのまま", () => {
  assert.deepEqual(wheelAct(W({ deltaY: -3, ctrlKey: true }), page), { kind: "zoom", zoomIn: true });
});

test("行・ページ単位はピクセルに換算する", () => {
  assert.deepEqual(wheelAct(W({ deltaY: 3, deltaMode: 1 }), page), { kind: "h", d: 120 });
  assert.deepEqual(wheelAct(W({ deltaY: 1, deltaMode: 2 }), page), { kind: "h", d: 800 });
  assert.deepEqual(wheelAct(W({ deltaY: 1, deltaMode: 2, shiftKey: true }), page), { kind: "v", d: 300 });
});

test("ドラッグで端を越えたら、はみ出しに応じて少しずつスクロールする", () => {
  assert.equal(edgeScrollStep(500, 100, 900), 0); // 範囲の中は動かさない
  assert.equal(edgeScrollStep(901, 100, 900), 2); // 少しでも越えたら最低 2px
  assert.equal(edgeScrollStep(950, 100, 900), 15);
  assert.equal(edgeScrollStep(5000, 100, 900), 30); // 遠くても上限で止める（一気に最後まで行かない）
  assert.equal(edgeScrollStep(50, 100, 900), -15); // 左も同じ
});
