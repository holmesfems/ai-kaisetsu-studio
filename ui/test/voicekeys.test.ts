// 声の鍵（読み上げ・字幕の文と空白の削り方）が Rust・Python と同じ結果になるかを、共通の表で確かめる
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { paramsOf, setParams, setVoice, staleAdj, subText, ttsKeyText, voiceOf, voicedLines } from "../src/logic/voice.ts";

interface Case {
  name: string;
  sp: string;
  text: string;
  tts: string;
  sub: string;
  key: string;
}
const cases: Case[] = JSON.parse(readFileSync(new URL("../../src-tauri/tests/fixtures/voice_keys.json", import.meta.url), "utf8"));

const script = (sp: string, text: string) => {
  const l = { id: "l1", sp, text, memo: "" } as any;
  return { s: { blocks: [{ id: "b", file: "1.yaml", title: "", lines: [l], notes: "" }], memo: "", voice: {}, params: {}, rev: 0 } as any, l };
};

test("表が読めている", () => {
  assert.ok(cases.length >= 20);
});

for (const c of cases) {
  test(`共通の表: ${c.name}`, () => {
    assert.equal(ttsKeyText(c.text), c.tts);
    assert.equal(subText(c.text), c.sub);
    assert.equal(`${c.sp}\u0001${ttsKeyText(c.text)}`, c.key);
    // 手直しを書いたら、同じ行で読める。読み上げる行かどうかも表の tts の空かどうかと一致する
    const { s, l } = script(c.sp, c.text);
    setVoice(s, l, "<xml/>");
    setParams(s, l, { a: 1 });
    assert.equal(s.voice.l1.text, c.tts);
    assert.equal(voiceOf(s, l), "<xml/>");
    assert.deepEqual(paramsOf(s, l), { a: 1 });
    assert.equal(staleAdj(s, l), false);
    assert.equal(voicedLines(s).length, c.tts === "" ? 0 : 1);
  });
}
