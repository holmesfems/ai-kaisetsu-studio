// 外の台本の変更に、画面の未保存の編集を重ねる (旧#57)
import { test } from "node:test";
import assert from "node:assert/strict";
import { mergeLocalEdits } from "../src/logic/history.ts";

const line = (id: string, text: string) => ({ id, sp: "a", text, memo: "" });
const mk = (text: string, memo: string, rev: number) => ({
  rev,
  memo,
  blocks: [{ id: "b1", file: "1.yaml", title: "t", lines: [line("l1", text)] }],
});
const acked = mk("あ", "古い", 1);

test("1 行目に打った + 外がメモを変えた: 両方残り、rev は外のもの", () => {
  const m = mergeLocalEdits(mk("あ", "新しい", 2), acked, mk("あ！", "古い", 1));
  assert.equal(m.local, true);
  assert.equal(m.value.memo, "新しい");
  assert.equal(m.value.blocks[0].lines[0].text, "あ！");
  assert.equal(m.value.rev, 2);
});

test("同じ行の文を両方が変えた: 外が勝つ", () => {
  const m = mergeLocalEdits(mk("外", "古い", 2), acked, mk("画面", "古い", 1));
  assert.equal(m.value.blocks[0].lines[0].text, "外");
});

test("未保存の編集が無ければ外の台本そのまま", () => {
  const m = mergeLocalEdits(mk("外", "新", 2), acked, mk("あ", "古い", 1));
  assert.equal(m.local, false);
  assert.equal(m.value.memo, "新");
});
