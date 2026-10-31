// 推敲案の見出しと中身（logic/proposals.ts）
import test from "node:test";
import assert from "node:assert/strict";
import type { ProposalItem } from "../src/gen/ProposalItem.ts";
import { NO_BLOCK, proposalFocusLine, proposalHead } from "../src/logic/proposals.ts";

const item = (o: Partial<ProposalItem>): ProposalItem => ({
  kind: "edit", line_id: null, after_id: null, block_id: null, old_text: "", new_text: "", sp: "", reason: "", status: "pending",
  new_id: null, after_block: null, notes: "", lines: [], ...o,
});
const nos: Record<string, number> = { a: 3, b: 5 };
const lineNo = (id: string | null) => (id ? nos[id] ?? null : null);
const titles: Record<string, string> = { b1: "導入", b2: "本編" };
const blockTitle = (id: string | null) => (id && titles[id]) || NO_BLOCK;

test("今までの kind の見出し", () => {
  assert.deepEqual(proposalHead(item({ kind: "edit", line_id: "a", old_text: "前", new_text: "後" }), lineNo, blockTitle),
    { head: "行 3", body: { t: "diff", a: "前", b: "後" } });
  assert.deepEqual(proposalHead(item({ kind: "delete", line_id: "b", old_text: "消す" }), lineNo, blockTitle),
    { head: "行 5 を削除", body: { t: "del", lines: ["消す"] } });
  assert.equal(proposalHead(item({ kind: "speaker", line_id: "a", old_text: "文" }), lineNo, blockTitle).head, "行 3 の話者");
  assert.deepEqual(proposalHead(item({ kind: "insert", after_id: "a", new_text: "足す" }), lineNo, blockTitle),
    { head: "行 3 の後に追加", body: { t: "ins", lines: ["足す"] } });
  assert.equal(proposalHead(item({ kind: "insert", block_id: "b2", new_text: "足す" }), lineNo, blockTitle).head, "ブロック「本編」の頭に追加");
  assert.equal(proposalHead(item({ kind: "edit", line_id: "zz" }), lineNo, blockTitle).head, "行 ?");
});

test("memo・notes", () => {
  assert.deepEqual(proposalHead(item({ kind: "memo", line_id: "a", old_text: "旧", new_text: "新" }), lineNo, blockTitle),
    { head: "行 3 のメモ", body: { t: "diff", a: "旧", b: "新" } });
  assert.deepEqual(proposalHead(item({ kind: "memo", line_id: "a", old_text: "旧", new_text: "" }), lineNo, blockTitle).body, { t: "del", lines: ["旧"] });
  assert.deepEqual(proposalHead(item({ kind: "notes", block_id: "b1", old_text: "旧", new_text: "新" }), lineNo, blockTitle),
    { head: "ブロック「導入」のメモ", body: { t: "diff", a: "旧", b: "新" } });
  assert.equal(proposalHead(item({ kind: "notes", block_id: "xx", new_text: "新" }), lineNo, blockTitle).head, `ブロック「${NO_BLOCK}」のメモ`);
});

test("move", () => {
  assert.deepEqual(proposalHead(item({ kind: "move", line_id: "a", after_id: "b", old_text: "文" }), lineNo, blockTitle),
    { head: "行 3 を行 5 の後へ移動", body: { t: "plain", text: "文" } });
  assert.equal(proposalHead(item({ kind: "move", line_id: "a", block_id: "b2", old_text: "文" }), lineNo, blockTitle).head, "行 3 をブロック「本編」の頭へ移動");
});

test("ブロックの追加・削除・改名", () => {
  const lines = [{ sp: "トーク1", text: "あ", memo: "" }, { sp: "トーク2", text: "い", memo: "" }];
  assert.deepEqual(proposalHead(item({ kind: "block_add", new_text: "新", lines }), lineNo, blockTitle),
    { head: "ブロック「新」を追加（2 行）", body: { t: "ins", lines: ["[トーク1] あ", "[トーク2] い"] } });
  assert.equal(proposalHead(item({ kind: "block_add", new_text: "新", after_block: "b1", lines }), lineNo, blockTitle).head,
    "ブロック「導入」の後に、ブロック「新」を追加（2 行）");
  assert.deepEqual(proposalHead(item({ kind: "block_add", new_text: "新", notes: "説明", lines }), lineNo, blockTitle).body,
    { t: "ins", lines: ["メモ: 説明", "[トーク1] あ", "[トーク2] い"] });
  assert.deepEqual(proposalHead(item({ kind: "block_delete", block_id: "b1", old_text: "導入", lines }), lineNo, blockTitle),
    { head: "ブロック「導入」を削除（2 行）", body: { t: "del", lines: ["[トーク1] あ", "[トーク2] い"] } });
  assert.deepEqual(proposalHead(item({ kind: "block_delete", old_text: "導入", notes: "説明", lines: [{ sp: "トーク1", text: "あ", memo: "m" }] }), lineNo, blockTitle).body,
    { t: "del", lines: ["メモ: 説明", "[トーク1] あ（メモ: m）"] });
  assert.deepEqual(proposalHead(item({ kind: "block_rename", block_id: "b1", old_text: "旧題", new_text: "新題" }), lineNo, blockTitle),
    { head: "ブロックの見出し", body: { t: "diff", a: "旧題", b: "新題" } });
});

test("見出しを押したときに飛ぶ行", () => {
  const first = (id: string | null) => (id === "b1" ? "a" : null);
  assert.equal(proposalFocusLine(item({ kind: "edit", line_id: "x" }), first), "x");
  assert.equal(proposalFocusLine(item({ kind: "insert", after_id: "y", block_id: "b1" }), first), "y");
  assert.equal(proposalFocusLine(item({ kind: "notes", block_id: "b1" }), first), "a");
  assert.equal(proposalFocusLine(item({ kind: "block_delete", block_id: "b2" }), first), null);
  assert.equal(proposalFocusLine(item({ kind: "block_add", block_id: "nb", after_block: "b1" }), first), "a");
});
