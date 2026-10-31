// ショートカットを効かせるかどうかの判定（ui/src/util.ts）の確かめ。旧#9: スライドバーを動かした直後に Ctrl+Z が効かなかった
import { test } from "node:test";
import assert from "node:assert/strict";
import { isFormControl, isTextEntry } from "../src/logic/util.ts";

const input = (type?: string) => ({ tagName: "INPUT", type });

test("文字を打つ欄では、元に戻すを欄に任せる", () => {
  for (const t of [undefined, "", "text", "number", "search", "email", "password", "url"]) assert.equal(isTextEntry(input(t)), true, String(t));
  assert.equal(isTextEntry({ tagName: "TEXTAREA" }), true);
  assert.equal(isTextEntry({ tagName: "DIV", isContentEditable: true }), true);
});

test("スライドバー・チェックボックス・色などでは、元に戻すをアプリで受ける", () => {
  for (const t of ["range", "checkbox", "radio", "color", "button", "RANGE"]) assert.equal(isTextEntry(input(t)), false, t);
  assert.equal(isTextEntry({ tagName: "SELECT" }), false);
  assert.equal(isTextEntry({ tagName: "BUTTON" }), false);
  assert.equal(isTextEntry({ tagName: "BODY", isContentEditable: false }), false);
  assert.equal(isTextEntry(null), false);
});

test("再生などのショートカットは、フォーム部品を触っている間は効かせない", () => {
  assert.equal(isFormControl(input("range")), true);
  assert.equal(isFormControl(input("text")), true);
  assert.equal(isFormControl({ tagName: "SELECT" }), true);
  assert.equal(isFormControl({ tagName: "BODY" }), false);
  assert.equal(isFormControl(undefined), false);
});
