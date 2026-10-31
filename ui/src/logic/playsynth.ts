// 再生中に、近いトーク（これから読まれる行）の音声を裏で作る。旧#52
// どの行を作るかの選び方だけをここに置く（store.ts から呼ぶ。テストは ui/test/playsynth.test.ts）
import type { Script, VoiceItem } from "../types";
import { ttsKeyText, voicedLines } from "./voice.ts";

/** 再生位置からこの秒数先までに始まる行を作る */
export const AHEAD_SEC = 30;
/** 1 回に選ぶ行の数の上限 */
export const MAX_AHEAD_LINES = 5;

/**
 * 再生位置 t の行（いま読まれている行・次に読まれる行）とその後の、まだ今の調整で作っていない行を、再生順（近い順）に返す
 * busy は作成中の行（選ばない）。今の調整で作った音声があるか（VoiceItem.audio）は Rust の並びで見る
 */
export function pickAheadLines(
  layout: VoiceItem[],
  script: Script,
  t: number,
  busy: ReadonlySet<string>,
): string[] {
  const lines = new Map(voicedLines(script).map((x) => [x.l.id, x.l]));
  const out: string[] = [];
  for (const it of [...layout].sort((a, b) => a.start - b.start)) {
    if (it.start + it.dur <= t) continue; // もう読み終わった
    if (it.start >= t + AHEAD_SEC) break;
    const l = lines.get(it.line_id);
    if (!l || !ttsKeyText(l.text) || busy.has(l.id) || it.audio != null) continue;
    out.push(l.id);
    if (out.length >= MAX_AHEAD_LINES) break;
  }
  return out;
}
