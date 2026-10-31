import test from "node:test";
import assert from "node:assert/strict";
import { DEFAULT_BRANDING, mergeBranding } from "../src/logic/branding.ts";

test("上書きが無ければ公開の既定", () => {
  assert.deepEqual(mergeBranding({}), DEFAULT_BRANDING);
  assert.deepEqual(mergeBranding(null), DEFAULT_BRANDING);
  assert.deepEqual(mergeBranding(undefined), DEFAULT_BRANDING);
  assert.deepEqual(mergeBranding("x"), DEFAULT_BRANDING);
});

test("上書きの title・sub が既定に勝つ", () => {
  assert.deepEqual(mergeBranding({ title: "AK STUDIO", sub: "// SAMPLE VIDEO EDITOR" }), { title: "AK STUDIO", sub: "// SAMPLE VIDEO EDITOR" });
});

test("片方だけの上書きは、もう片方が既定のまま", () => {
  assert.deepEqual(mergeBranding({ title: "My Studio" }), { title: "My Studio", sub: DEFAULT_BRANDING.sub });
  assert.deepEqual(mergeBranding({ sub: "memo" }), { title: DEFAULT_BRANDING.title, sub: "memo" });
});

test("sub は空文字で消せる。title の空・文字列でない値は無視する", () => {
  assert.equal(mergeBranding({ sub: "" }).sub, "");
  assert.equal(mergeBranding({ title: "  " }).title, DEFAULT_BRANDING.title);
  assert.deepEqual(mergeBranding({ title: 3, sub: 4 }), DEFAULT_BRANDING);
});

test("前後の空白は取る", () => {
  assert.deepEqual(mergeBranding({ title: " A ", sub: " b " }), { title: "A", sub: "b" });
});
