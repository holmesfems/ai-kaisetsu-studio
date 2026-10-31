// 台本の行の欄のキーの判定（ui/src/linekeys.ts）の確かめ。旧#49: Backspace で上の行まで消え、Ctrl+Z で戻らなかった
import { test } from "node:test";
import assert from "node:assert/strict";
import { lineKeyAction, type LineKey } from "../src/logic/linekeys.ts";

const k = (o: Partial<LineKey>): LineKey => ({ key: "a", ctrlKey: false, shiftKey: false, altKey: false, repeat: false, isComposing: false, selStart: 0, selEnd: 0, ...o });

test("行頭の Backspace は前の行と結合する", () => {
  assert.equal(lineKeyAction(k({ key: "Backspace" })), "mergePrev");
  assert.equal(lineKeyAction(k({ key: "Backspace", shiftKey: true })), "mergePrev");
});

test("押しっぱなしの繰り返しでは結合しない（上の行まで次々に消さない）", () => {
  assert.equal(lineKeyAction(k({ key: "Backspace", repeat: true })), null);
});

test("変換中・行頭でない・範囲を選んでいるときは結合しない", () => {
  assert.equal(lineKeyAction(k({ key: "Backspace", isComposing: true })), null);
  assert.equal(lineKeyAction(k({ key: "Backspace", keyCode: 229 })), null);
  assert.equal(lineKeyAction(k({ key: "Backspace", selStart: 3, selEnd: 3 })), null);
  assert.equal(lineKeyAction(k({ key: "Backspace", selStart: 0, selEnd: 4 })), null);
});

test("Ctrl+Z / Ctrl+Shift+Z / Ctrl+Y はアプリの履歴で戻す・やり直す", () => {
  assert.equal(lineKeyAction(k({ key: "z", ctrlKey: true })), "undo");
  assert.equal(lineKeyAction(k({ key: "Z", ctrlKey: true, shiftKey: true })), "redo");
  assert.equal(lineKeyAction(k({ key: "y", ctrlKey: true })), "redo");
  assert.equal(lineKeyAction(k({ key: "z", metaKey: true })), "undo");
  assert.equal(lineKeyAction(k({ key: "z", ctrlKey: true, isComposing: true })), null, "変換中は IME に任せる");
});

test("Ctrl+Alt+Z（外の変更を戻す）や、ふつうの文字は受けない", () => {
  assert.equal(lineKeyAction(k({ key: "z", ctrlKey: true, altKey: true })), null);
  assert.equal(lineKeyAction(k({ key: "z" })), null);
  assert.equal(lineKeyAction(k({ key: "Enter" })), null);
});
