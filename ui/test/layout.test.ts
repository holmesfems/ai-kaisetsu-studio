// 「詰め直す」（relayoutFrom）と、cue とのずれ（cueDrift）の確かめ。アプリを起動せずに `npm test` で動く（Node の型の取り除きを使う）。
// セリフの並べ方・間隔の見積もり・実際の回の確かめは Rust に移した（src-tauri/src/voice/layout.rs の #[cfg(test)]）。
import { test } from "node:test";
import assert from "node:assert/strict";
import { cueDrift, relayoutFrom, voicedLines } from "../src/logic/voice.ts";
import type { Block, Line, Script } from "../src/types.ts";

let uid = 0;
const L = (text: string, start: number | null, sp = "トーク1"): Line => ({ id: `l${++uid}`, sp, text, start });
const B = (title: string, lines: Line[]): Block => ({ id: `b${++uid}`, file: null, title, lines, notes: "" });
const S = (blocks: Block[]): Script => ({ blocks, memo: "", voice: {}, params: {}, rev: 0 });
/** 行の長さ（Rust の並びの dur）。文で引く */
const durs = (s: Script, d: Record<string, number>): Map<string, number> => {
  const a = new Map<string, number>();
  for (const x of voicedLines(s)) if (d[x.l.text] != null) a.set(x.l.id, d[x.l.text]);
  return a;
};

test("詰め直す（relayoutFrom）は間隔どおりに並べる", () => {
  const s = S([B("A", [L("a1", 0), L("a2", 5)]), B("B", [L("b1", 50)])]);
  const an = durs(s, { a1: 2, a2: 1, b1: 1 });
  relayoutFrom(s, an, 0.3, 1.0);
  assert.deepEqual(voicedLines(s).map((x) => x.l.start), [0, 2.3, 4.3]);
});

test("cue とのずれ", () => {
  const items = [0, 2.3, 4.6].map((start, i) => ({ line_id: `x${i}`, n: i + 1, sp: "", text: "", start, dur: 2, audio: null, block: "", key: "" }));
  assert.ok(cueDrift(items, [{ start: 0 }, { start: 2.3 }, { start: 4.62 }]).ok);
  const d = cueDrift(items, [{ start: 0 }, { start: 1.6 }, { start: 3.9 }]);
  assert.equal(d.ok, false);
  assert.equal(d.offs.length, 2);
  assert.ok(Math.abs(d.max - 0.7) < 1e-9);
  assert.equal(cueDrift(items, [{ start: 0 }, { start: 2.3 }]).ok, false); // 数が違う
});
