// 再生中に裏で作る行の選び方（ui/src/playsynth.ts）の確かめ。旧#52
import { test } from "node:test";
import assert from "node:assert/strict";
import { AHEAD_SEC, MAX_AHEAD_LINES, pickAheadLines } from "../src/logic/playsynth.ts";
import { voicedLines } from "../src/logic/voice.ts";
import type { Block, Line, Script, VoiceItem } from "../src/types.ts";

let uid = 0;
const L = (text: string, sp = "トーク1"): Line => ({ id: `l${++uid}`, sp, text, start: null });
const B = (lines: Line[]): Block => ({ id: `b${++uid}`, file: null, title: "A", lines, notes: "" });
const S = (lines: Line[]): Script => ({ blocks: [B(lines)], memo: "", voice: {}, params: {}, rev: 0 });
/**
 * Rust の並びの代わり: 読む行を 1 行 2 秒＋間隔 0.3 秒で並べる。made に入れた文の行は「今の調整で作成済み」（audio あり）
 */
const layoutOf = (s: Script, made: string[] = []): VoiceItem[] =>
  voicedLines(s).map((x, i) => ({
    line_id: x.l.id, n: x.n, sp: x.l.sp, text: x.l.text, start: i * 2.3, dur: 2,
    audio: made.includes(x.l.text) ? x.l.text : null, block: "A", key: `${x.l.sp}${x.l.text}`,
  }));
const textOf = (s: Script, id: string) => voicedLines(s).find((x) => x.l.id === id)!.l.text;
const pick = (s: Script, made: string[], t: number, busy: string[] = []) =>
  pickAheadLines(layoutOf(s, made), s, t, new Set(busy)).map((id) => textOf(s, id));

test("作成済みの行は選ばず、未作成の行を近い順に選ぶ", () => {
  const s = S([L("a"), L("b"), L("c"), L("d")]);
  assert.deepEqual(pick(s, ["a", "c"], 0), ["b", "d"]);
});

test("読み終わった行は選ばない（再生位置の行と、その後だけ）", () => {
  const s = S([L("a"), L("b"), L("c")]);
  const made = ["b", "c"];
  // 1 行 2 秒＋間隔 0.3 秒。a は 0〜2 秒
  assert.deepEqual(pick(s, made, 0), ["a"]);
  assert.deepEqual(pick(s, made, 0.5), ["a"], "a を読んでいる途中でも a を含む");
  assert.deepEqual(pick(s, made, 5), [], "a は読み終わった");
});

test("30 秒より先に始まる行は選ばない", () => {
  const s = S(Array.from({ length: 80 }, (_, i) => L(`x${i}`)));
  const items = layoutOf(s);
  const got = pickAheadLines(items, s, 0, new Set());
  assert.ok(got.length > 0);
  for (const id of got) assert.ok(items.find((x) => x.line_id === id)!.start < AHEAD_SEC);
  const far = pickAheadLines(items, s, 100, new Set());
  for (const id of far) assert.ok(items.find((x) => x.line_id === id)!.start < 100 + AHEAD_SEC);
});

test(`上限は ${MAX_AHEAD_LINES} 行で、再生順に並ぶ`, () => {
  const s = S(Array.from({ length: 12 }, (_, i) => L(`x${i}`)));
  assert.deepEqual(pick(s, [], 0), Array.from({ length: MAX_AHEAD_LINES }, (_, i) => `x${i}`));
});

test("作成中の行は選ばない", () => {
  const s = S([L("a"), L("b")]);
  const items = layoutOf(s);
  const [a, b] = s.blocks[0].lines;
  assert.deepEqual(pickAheadLines(items, s, 0, new Set([a.id])), [b.id]);
});

test("読まない行（空白だけ）は選ばない", () => {
  const s = S([L("a"), L("   "), L("b")]);
  assert.deepEqual(pick(s, [], 0), ["a", "b"]);
});

test("調整を変えて作成済みでなくなった行（audio が無い行）は選ぶ", () => {
  // 話者の声を変えた後は、Rust の並びで audio が外れる
  const s = S([L("a"), L("b")]);
  assert.deepEqual(pick(s, [], 0), ["a", "b"]);
});

test("作る行が無ければ空", () => {
  const s = S([L("a")]);
  assert.deepEqual(pick(s, ["a"], 0), []);
  assert.deepEqual(pickAheadLines([], s, 0, new Set()), []);
});
