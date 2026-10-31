// /frame の X-Info の読み取り（ui/src/frameinfo.ts、issue 旧#29）の確かめ。`npm test` で動く。
import { test } from "node:test";
import assert from "node:assert/strict";
import { parseFrameInfo, shortError } from "../src/logic/frameinfo.ts";

test("parseFrameInfo: 字幕の情報と演出のエラーを読む", () => {
  const h = JSON.stringify({ sub: { line: "a1", n: 1 }, fx_error: "演出を描けませんでした: boom" });
  assert.deepEqual(parseFrameInfo(h), { sub: { line: "a1", n: 1 }, fxError: "演出を描けませんでした: boom" });
});

test("parseFrameInfo: Rust が \\uXXXX にした日本語も読める", () => {
  const h = '{"fx_error":"\\u6f14\\u51fa"}';
  assert.equal(parseFrameInfo(h).fxError, "演出");
});

test("parseFrameInfo: 無い・空・壊れているときはエラーなし", () => {
  assert.deepEqual(parseFrameInfo(null), { sub: null, fxError: "" });
  assert.deepEqual(parseFrameInfo("{}"), { sub: null, fxError: "" });
  assert.deepEqual(parseFrameInfo("{broken"), { sub: null, fxError: "" });
  assert.deepEqual(parseFrameInfo("null"), { sub: null, fxError: "" });
  assert.equal(parseFrameInfo('{"fx_error": 3}').fxError, "");
});

test("shortError: 最初の行だけ、長ければ切る", () => {
  assert.equal(shortError("演出を描けませんでした: x\nTraceback ..."), "演出を描けませんでした: x");
  assert.equal(shortError("\n\nabc"), "abc");
  assert.equal(shortError("あ".repeat(100), 10), `${"あ".repeat(10)}…`);
  assert.equal(shortError(""), "");
});
