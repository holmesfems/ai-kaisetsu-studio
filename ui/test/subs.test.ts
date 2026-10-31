// 字幕の書き換えの確定まわり（ui/src/logic/subs.ts）の確かめ。旧#74: 打っている途中の欄を上書きしない・キーで確定のしかたを決める
import { test } from "node:test";
import assert from "node:assert/strict";
import { normSubText, subEditKey, syncSubEdit } from "../src/logic/subs.ts";

test("normSubText: 空の行・行末の空白・\r\n を消し、中の空白は残す", () => {
  assert.equal(normSubText("あい\n\nうえ\n"), "あい\nうえ");
  assert.equal(normSubText("あい  \nうえ\t"), "あい\nうえ");
  assert.equal(normSubText("あい\r\nうえ"), "あい\nうえ");
  assert.equal(normSubText("あ い\n   \nう"), "あ い\nう");
});

test("syncSubEdit: 打っている途中の空の行・行末の空白は残し、外からの変更は反映する", () => {
  assert.equal(syncSubEdit("あい\n", "あい"), "あい\n");
  assert.equal(syncSubEdit("あい  ", "あい"), "あい  ");
  assert.equal(syncSubEdit("あい", "かき"), "かき");
});

const k = (o: Partial<Parameters<typeof subEditKey>[0]>) => ({ key: "a", ctrlKey: false, isComposing: false, keyCode: 0, ...o });

test("subEditKey: 変換中の Enter は何もしない", () => {
  assert.equal(subEditKey(k({ key: "Enter", isComposing: true })), null);
  assert.equal(subEditKey(k({ key: "Enter", keyCode: 229 })), null);
});

test("subEditKey: Enter は改行してから確定、Ctrl+Enter はすぐ確定、ほかは何もしない", () => {
  assert.equal(subEditKey(k({ key: "Enter" })), "after-input");
  assert.equal(subEditKey(k({ key: "Enter", shiftKey: true } as any)), "after-input");
  assert.equal(subEditKey(k({ key: "Enter", ctrlKey: true })), "now");
  assert.equal(subEditKey(k({ key: "a" })), null);
});
