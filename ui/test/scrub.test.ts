// 数値のドラッグ（ui/src/scrub.ts）の確かめ（issue 旧#12）。`npm test` で動く
import { test } from "node:test";
import assert from "node:assert/strict";
import { cleanNum, clampRange, fmtNum, nudge, parseNum, scrubMove, scrubMult, scrubValue, stepDecimals } from "../src/logic/scrub.ts";

test("範囲の広い項目でも、1px ＝ 1 step で動く（位置は 1px ずつ）", () => {
  // 位置 x（-2000〜4000）でも、10px 動かすと 10 だけ変わる
  let cur = 960;
  cur = scrubMove(cur, 10, 1, 1, -2000, 4000);
  assert.equal(scrubValue(cur, 1, 1, -2000, 4000), 970);
  cur = scrubMove(cur, -25, 1, 1, -2000, 4000);
  assert.equal(scrubValue(cur, 1, 1, -2000, 4000), 945);
});

test("Shift で 10 倍・Ctrl で 0.1 倍", () => {
  assert.equal(scrubMult({}), 1);
  assert.equal(scrubMult({ shiftKey: true }), 10);
  assert.equal(scrubMult({ ctrlKey: true }), 0.1);
  assert.equal(scrubMult({ metaKey: true }), 0.1);
  assert.equal(scrubValue(scrubMove(0, 3, 1, 10), 1, 10), 30);
  assert.equal(scrubValue(scrubMove(0, 3, 1, 0.1), 1, 0.1), 0.3);
});

test("端で止まり、戻すとすぐ動く", () => {
  let cur = 0.95;
  cur = scrubMove(cur, 20, 0.01, 1, 0, 1);
  assert.equal(scrubValue(cur, 0.01, 1, 0, 1), 1);
  cur = scrubMove(cur, -1, 0.01, 1, 0, 1);
  assert.equal(scrubValue(cur, 0.01, 1, 0, 1), 0.99);
});

test("浮動小数の端数が出ない", () => {
  let cur = 0;
  for (let i = 0; i < 3; i++) cur = scrubMove(cur, 1, 0.1, 1);
  assert.equal(scrubValue(cur, 0.1, 1), 0.3);
  assert.equal(scrubValue(1.2 + 0.033 * 7, 0.033, 1), 1.431);
  assert.equal(cleanNum(0.1 + 0.2, 0.1), 0.3);
  assert.equal(stepDecimals(0.033), 3);
  assert.equal(stepDecimals(0.0033), 4);
  assert.equal(stepDecimals(0.005), 3);
  assert.equal(stepDecimals(5), 0);
});

test("↑↓ で 1 刻み（範囲で止まる）", () => {
  assert.equal(nudge(1, 1, 0.005, 1, 0.01, 8), 1.005);
  assert.equal(nudge(1, -1, 1, 10), -9);
  assert.equal(nudge(100, 1, 0.5, 1, 0, 100), 100);
});

test("入力の読み取り（全角も受ける・読めなければ null）", () => {
  assert.equal(parseNum(" 12.5 "), 12.5);
  assert.equal(parseNum("－３０"), -30);
  assert.equal(parseNum("１２．５"), 12.5);
  assert.equal(parseNum(""), null);
  assert.equal(parseNum("abc"), null);
  assert.equal(clampRange(5000, -2000, 4000), 4000);
  assert.equal(fmtNum(1.23456), "1.235");
});
