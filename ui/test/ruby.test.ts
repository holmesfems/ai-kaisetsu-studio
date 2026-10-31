// 選んだ字を一時的な読み（ルビ）にする変換の確かめ
import { test } from "node:test";
import assert from "node:assert/strict";
import { canRuby, wrapRuby } from "../src/logic/ruby.ts";

test("選んだ字を ｜字《》 にして、カーソルは 《 と 》 の間", () => {
  const r = wrapRuby("今日は時雨です", 3, 5)!;
  assert.equal(r.value, "今日は｜時雨《》です");
  assert.equal(r.value[r.caret - 1], "《");
  assert.equal(r.value[r.caret], "》");
});

test("選びの前後の空白もそのまま含める", () => {
  assert.equal(wrapRuby("a b c", 1, 4)!.value, "a｜ b 《》c");
});

test("すでにルビの記号や改行があれば付けない", () => {
  assert.equal(wrapRuby("｜時雨《しぐれ》", 0, 8), null);
  assert.equal(wrapRuby("ab《", 0, 3), null);
  assert.equal(wrapRuby("a\nb", 0, 3), null);
  assert.equal(wrapRuby("abc", 1, 1), null);
  assert.ok(canRuby("時雨"));
});
