import { test } from "node:test";
import assert from "node:assert/strict";
import { addPending, buildLiked, dropPending, likeHead, reattach } from "../src/logic/likes.ts";
import type { ChatMsg } from "../src/types";

const msg = (id: number, role: ChatMsg["role"], text: string, reaction?: "like"): ChatMsg =>
  ({ id, ts: "2026-01-01 00:00", role, text, ctx: {}, attachments: [], reaction });

test("likeHead: 80 字で切る", () => {
  assert.equal(likeHead("はい"), "はい");
  assert.equal(likeHead("あ".repeat(100)), "あ".repeat(80) + "…");
  assert.equal(likeHead("a\n  b"), "a b");
});

test("buildLiked: まだ好き！のものだけ、押した順", () => {
  const chat = [msg(1, "claude", "一つめ", "like"), msg(2, "claude", "二つめ"), msg(3, "claude", "三つめ", "like"), msg(4, "user", "ユーザー", "like")];
  const ids = [3, 1, 2, 4, 99];
  assert.deepEqual(buildLiked(ids, chat), [{ id: 3, head: "三つめ" }, { id: 1, head: "一つめ" }]);
});

test("押して取り消した分は入れない", () => {
  let ids = addPending([], 1);
  ids = dropPending(ids, 1);
  assert.deepEqual(buildLiked(ids, [msg(1, "claude", "x", "like")]), []);
  assert.deepEqual(addPending([1], 1), [1]);
});

test("reattach: 取り消したメッセージの分を付け直す（重複なし）", () => {
  assert.deepEqual(reattach([5], [{ id: 1 }, { id: 5 }]), [5, 1]);
  assert.deepEqual(reattach([5], undefined), [5]);
});
