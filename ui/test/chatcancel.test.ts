// 送り間違いの取り消し（ui/src/chatcancel.ts）の確かめ。旧#21: 送った直後に Esc / Ctrl+C で取り消したい
import { test } from "node:test";
import assert from "node:assert/strict";
import { LATE_KEY_MS, cancelTarget, isCancelKey, mergeChatUpdate, noticeNote, restoreDraft, shortcutTarget, stateNote } from "../src/logic/chatcancel.ts";
import type { ChatMsg } from "../src/types.ts";

const msg = (id: number, role: ChatMsg["role"], state: ChatMsg["state"] = "", ctx: any = {}): ChatMsg => ({
  id, ts: "2026-10-17 12:00:00", role, text: `m${id}`, ctx, attachments: [], state,
});
const key = (k: string, mods: Partial<{ ctrlKey: boolean; metaKey: boolean; shiftKey: boolean; altKey: boolean; isComposing: boolean; keyCode: number }> = {}) => ({
  key: k, ctrlKey: false, metaKey: false, shiftKey: false, altKey: false, ...mods,
});

test("取り消しの対象は、最後に送った自分のメッセージ（届く前・届いた後）", () => {
  assert.equal(cancelTarget([msg(1, "user", "delivered"), msg(2, "user", "queued")])?.id, 2);
  assert.equal(cancelTarget([msg(1, "user", "delivered"), msg(2, "claude")])?.id, 1, "Claude の返事の後でも取り消せる");
  // クリップへのメモなど、kind つきでも自分で書いたものは対象
  assert.equal(cancelTarget([msg(1, "user", "queued", { kind: "clip_note" })])?.id, 1);
});

test("取り消し済み・推敲案の結果・取り消しの知らせは対象にしない", () => {
  assert.equal(cancelTarget([msg(1, "user", "cancelled")]), null);
  assert.equal(cancelTarget([msg(1, "user", "delivered"), msg(2, "user", "cancel_late")]), null, "取り消した後で一つ前を取り消さない");
  assert.equal(cancelTarget([msg(1, "user", "delivered"), msg(2, "user", "queued", { kind: "proposal_result" })])?.id, 1);
  assert.equal(cancelTarget([msg(1, "user", "cancel_late"), msg(2, "user", "delivered", { kind: "cancel_notice" })]), null);
  assert.equal(cancelTarget([msg(1, "user", "")]), null, "前に開いたときの古いメッセージ（印なし）");
  assert.equal(cancelTarget([]), null);
});

test("キーで取り消せるのは、この画面から送った直後だけ", () => {
  const chat = [msg(1, "user", "delivered")];
  assert.equal(shortcutTarget(chat, { id: 1, at: 0 }, 1000)?.id, 1);
  assert.equal(shortcutTarget(chat, { id: 1, at: 0 }, LATE_KEY_MS + 1), null, "届いた後はしばらくしたらキーでは取り消さない");
  assert.equal(shortcutTarget([msg(1, "user", "queued")], { id: 1, at: 0 }, LATE_KEY_MS * 10)?.id, 1, "まだ届いていなければいつでも");
  assert.equal(shortcutTarget(chat, { id: 9, at: 0 }, 10), null);
  assert.equal(shortcutTarget(chat, null, 10), null);
});

test("Esc は取り消し、IME の変換中は IME に任せる", () => {
  assert.equal(isCancelKey(key("Escape"), "", ""), true);
  assert.equal(isCancelKey(key("Escape", { isComposing: true }), "", ""), false);
  assert.equal(isCancelKey(key("Escape", { keyCode: 229 }), "", ""), false);
  assert.equal(isCancelKey(key("Escape", { shiftKey: true }), "", ""), false);
});

test("Ctrl+C は文字を選んでいないときだけ取り消し、選んでいればコピー", () => {
  assert.equal(isCancelKey(key("c", { ctrlKey: true }), "", ""), true);
  assert.equal(isCancelKey(key("C", { metaKey: true }), "", ""), true);
  assert.equal(isCancelKey(key("c", { ctrlKey: true }), "選んだ文", ""), false);
  assert.equal(isCancelKey(key("c", { ctrlKey: true, shiftKey: true }), "", ""), false);
  assert.equal(isCancelKey(key("c"), "", ""), false);
  assert.equal(isCancelKey(key("v", { ctrlKey: true }), "", ""), false);
});

test("書きかけの文があるときは、どちらのキーでも取り消さない", () => {
  assert.equal(isCancelKey(key("Escape"), "", "次の文"), false);
  assert.equal(isCancelKey(key("c", { ctrlKey: true }), "", "次の文"), false);
  assert.equal(isCancelKey(key("Escape"), "", "  \n"), true);
});

test("取り消した文は入力欄に戻す（書きかけがあればその前に）", () => {
  assert.equal(restoreDraft("まちがい", ""), "まちがい");
  assert.equal(restoreDraft("まちがい", "続き"), "まちがい\n続き");
});

test("届いたか・取り消したかの印", () => {
  assert.match(stateNote(msg(1, "user", "queued")), /まだ Claude に届いていません/);
  assert.match(stateNote(msg(1, "user", "cancelled")), /届いていません/);
  assert.match(stateNote(msg(1, "user", "cancel_late")), /無視して/);
  assert.match(stateNote(msg(1, "user", "cancel_aborted")), /作業を止めました/);
  assert.equal(stateNote(msg(1, "user", "delivered")), "");
});

test("取り消しの印は、後から来た「届いた」で上書きしない（同時に渡したとき）", () => {
  const chat = [msg(1, "user", "cancel_late")];
  assert.equal(mergeChatUpdate(chat, msg(1, "user", "delivered"))[0].state, "cancel_late");
  assert.equal(mergeChatUpdate(chat, msg(1, "user", "cancel_aborted"))[0].state, "cancel_aborted");
  assert.equal(mergeChatUpdate([msg(1, "user", "queued")], msg(1, "user", "delivered"))[0].state, "delivered");
  assert.equal(mergeChatUpdate([], msg(2, "user", "delivered")).length, 1, "まだ手元に無ければ足す");
});

test("閉じたときに渡していなかったもの・知らせの行", () => {
  assert.match(stateNote(msg(1, "user", "undelivered")), /届いていません/);
  assert.equal(cancelTarget([msg(1, "user", "undelivered")]), null);
  assert.match(noticeNote(msg(2, "user", "queued", { kind: "cancel_notice" })), /伝えます（送信待ち）/);
  assert.match(noticeNote(msg(2, "user", "delivered", { kind: "cancel_notice" })), /伝えました/);
});
