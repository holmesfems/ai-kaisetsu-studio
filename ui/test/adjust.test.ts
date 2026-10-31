// 行ごとの声の手直し（_voice.json・_voice_params.json。鍵は行の id）の確かめ
import { test } from "node:test";
import assert from "node:assert/strict";
import { dropStaleAdj, paramsOf, setParams, setVoice, staleAdj, voiceOf } from "../src/logic/voice.ts";
import type { Line, Script } from "../src/types.ts";

const mk = (): { s: Script; l: Line } => {
  const l: Line = { id: "l1", sp: "トーク1", text: "どうも｜A《えー》です", start: null };
  return { s: { blocks: [{ id: "b", file: "1.yaml", title: "", lines: [l], notes: "" }], memo: "", voice: {}, params: {}, rev: 0 }, l };
};

test("手直しは、話者と読み上げの文（ルビは読み）が同じときだけ効く", () => {
  const { s, l } = mk();
  setVoice(s, l, "<x/>");
  setParams(s, l, { intonation: 1.2 });
  assert.deepEqual(s.voice.l1, { sp: "トーク1", text: "どうもえーです", analyzed_text: "<x/>" });
  assert.equal(voiceOf(s, l), "<x/>");
  assert.deepEqual(paramsOf(s, l), { intonation: 1.2 });
  assert.equal(staleAdj(s, l), false);
  // 字幕の表記だけ直す（読みは同じ）なら、そのまま効く
  assert.equal(voiceOf(s, { ...l, text: "どうも｜エー《えー》です" }), "<x/>");
});

test("文を直すと使われなくなり（消えない）、文を戻すとまた効く", () => {
  const { s, l } = mk();
  setVoice(s, l, "<x/>");
  setParams(s, l, { speed: 1.1 });
  const edited = { ...l, text: "別の文です" };
  assert.equal(voiceOf(s, edited), null);
  assert.equal(paramsOf(s, edited), null);
  assert.equal(staleAdj(s, edited), true);
  assert.ok(s.voice.l1 && s.params.l1); // ファイルからは消さない
  assert.equal(voiceOf(s, l), "<x/>");
  assert.equal(staleAdj(s, l), false);
  // 使われていない手直しだけを消す
  dropStaleAdj(s, edited);
  assert.deepEqual([s.voice.l1, s.params.l1], [undefined, undefined]);
});

test("話者を変えても効かなくなる。ほかの行の手直しは触らない", () => {
  const { s, l } = mk();
  setVoice(s, l, "<x/>");
  const other: Line = { id: "l2", sp: "トーク2", text: "ほかの行", start: null };
  setVoice(s, other, "<y/>");
  const moved = { ...l, sp: "トーク2" };
  assert.equal(voiceOf(s, moved), null);
  assert.equal(staleAdj(s, moved), true);
  dropStaleAdj(s, moved);
  assert.equal(s.voice.l1, undefined);
  assert.equal(voiceOf(s, other), "<y/>");
});

test("新しい文で書き直すと、前の文の手直しは置き換わる。空の抑揚は消える", () => {
  const { s, l } = mk();
  setParams(s, l, { speed: 1.1 });
  const edited = { ...l, text: "別の文です" };
  setParams(s, edited, { intonation: 0.5 });
  assert.deepEqual(paramsOf(s, edited), { intonation: 0.5 });
  assert.equal(s.params.l1.text, "別の文です");
  setParams(s, edited, {});
  assert.equal(s.params.l1, undefined);
  setParams(s, edited, null);
  assert.equal(staleAdj(s, edited), false);
});
