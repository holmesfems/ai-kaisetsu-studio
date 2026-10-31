import test from "node:test";
import assert from "node:assert/strict";
import { fmtElapsed, HELD_TEXT, statusView, verbAt, VERBS } from "../src/logic/claudestatus.ts";
import type { ChatMsg } from "../src/types.ts";

const T0 = new Date("2026-10-18T12:00:00").getTime();
const msg = (id: number, role: ChatMsg["role"], state: ChatMsg["state"] = "", ctx: any = {}): ChatMsg =>
  ({ id, ts: "2026-10-18 12:00:00", role, text: "x", ctx, attachments: [], state });
const idle = { connected: true, status: "" };
const work = (text = "", since_ms = T0 + 1000) => ({ connected: true, status: "", activity: { working: true, text, since_ms } });

test("何も無ければ出さない", () => {
  assert.equal(statusView([], idle, T0), null);
  assert.equal(statusView([msg(1, "user", "delivered"), msg(2, "claude")], idle, T0 + 5000), null);
});

test("届いて返事が無ければ出す（送信時刻から数える）", () => {
  const v = statusView([msg(1, "user", "delivered")], idle, T0 + 3000)!;
  assert.equal(v.since, T0);
  assert.equal(v.text, "");
});

test("作業中は Mod の知らせを使い、status が優先", () => {
  assert.equal(statusView([], work("台本を読んでいます"), T0)!.text, "台本を読んでいます");
  const c = { ...work("台本を読んでいます"), status: "数字を確認中" };
  assert.equal(statusView([], c, T0)!.text, "数字を確認中");
  assert.equal(statusView([], work(), T0)!.since, T0 + 1000);
});

test("作業中に置いてあるメッセージは、届けますの文", () => {
  const v = statusView([msg(1, "user", "queued")], work("x"), T0)!;
  assert.equal(v.held, true);
  assert.equal(v.text, HELD_TEXT);
});

test("つながっていないときの送信待ちは出さない／古い待ちは打ち切る", () => {
  assert.equal(statusView([msg(1, "user", "queued")], { connected: false, status: "" }, T0), null);
  assert.equal(statusView([msg(1, "user", "delivered")], idle, T0 + 11 * 60 * 1000), null);
});

test("取り消しの知らせは無視、取り消した後は出さない", () => {
  assert.equal(statusView([msg(1, "user", "cancelled")], idle, T0), null);
  assert.ok(statusView([msg(1, "user", "delivered"), msg(2, "user", "", { kind: "cancel_notice" })], idle, T0));
});

test("動詞と経過", () => {
  assert.equal(verbAt(0), "Pondering…");
  assert.equal(verbAt(2500), "Thinking…");
  assert.equal(verbAt(2500 * VERBS.length), "Pondering…");
  assert.equal(fmtElapsed(12999), "12s");
  assert.equal(fmtElapsed(65000), "1m05s");
});
