// 読み上げ・字幕の文（台本エディタと同じ決まり）と、行ごとの声の手直し（行の id が鍵）、台本の編集。
// セリフの並べ方・声の解析・音声の署名は Rust が持つ（src-tauri/src/voice/）。
import type { Block, Line, Script, VoiceItem } from "../types";

// ルビ：｜時雨《しぐれ》 → 読み上げには「しぐれ」、字幕には「時雨」
const RUBY = /｜([^｜《》]+)《([^《》]+)》/g;
export const ttsText = (t: string) => t.replace(RUBY, "$2");
export const subText = (t: string) => t.replace(RUBY, "$1");
// 前後の空白の削り方は Rust の str::trim（char::is_whitespace）に合わせる。JS の trim は U+FEFF を削り U+0085 を残すので使わない
const RS_WS = "[\t-\r \u0085\u00a0\u1680\u2000-\u200a\u2028\u2029\u202f\u205f\u3000]";
const RS_TRIM = new RegExp(`^${RS_WS}+|${RS_WS}+$`, "g");
export const trimRs = (t: string) => t.replace(RS_TRIM, "");
/** 読み上げる文を前後の空白で削ったもの。手直しの text・「読み上げる行か」の判定に使う（Rust の tts_key_text と同じ） */
export const ttsKeyText = (t: string) => trimRs(ttsText(t));
// 行ごとの手直し（_voice.json・_voice_params.json）は行の id が鍵で、sp と読み上げの文が今の行と同じときだけ効く。
// 文を直すと使われなくなり（消さない）、文を戻せばまた効く
const adjOk = (e: { sp: string; text: string } | undefined, l: Line) => !!e && e.sp === l.sp && e.text === ttsKeyText(l.text);
/** アクセントの手直し（声のエンジンの解析結果。VoiSona は XML）。効いていなければ null */
export const voiceOf = (s: Script, l: Line): string | null => {
  const e = s.voice[l.id];
  return adjOk(e, l) ? e.analyzed_text || null : null;
};
/** 抑揚・音素の長さなど。効いていなければ null */
export const paramsOf = (s: Script, l: Line): Record<string, any> | null => {
  const e = s.params[l.id];
  return adjOk(e, l) ? e.params || null : null;
};
/** アクセントの手直しを書く（null で消す）。editScript の中で呼ぶ */
export const setVoice = (s: Script, l: Line, xml: string | null) => {
  if (xml == null) delete s.voice[l.id];
  else s.voice[l.id] = { sp: l.sp, text: ttsKeyText(l.text), analyzed_text: xml };
};
/** 抑揚・音素の長さなどを書く（null か空で消す）。editScript の中で呼ぶ */
export const setParams = (s: Script, l: Line, p: Record<string, any> | null) => {
  if (!p || !Object.keys(p).length) delete s.params[l.id];
  else s.params[l.id] = { sp: l.sp, text: ttsKeyText(l.text), params: p };
};
/** この行の id の手直しがあるが、今の文・話者と合わなくて使われていない */
export const staleAdj = (s: Script, l: Line) =>
  (!!s.voice[l.id] && !adjOk(s.voice[l.id], l)) || (!!s.params[l.id] && !adjOk(s.params[l.id], l));
/** 使われていない手直しを消す。editScript の中で呼ぶ */
export const dropStaleAdj = (s: Script, l: Line) => {
  if (s.voice[l.id] && !adjOk(s.voice[l.id], l)) delete s.voice[l.id];
  if (s.params[l.id] && !adjOk(s.params[l.id], l)) delete s.params[l.id];
};
/** その話者の声の変更（_voice_speakers.json。無ければ null） */
export const spVoice = (s: Script, sp: string) => {
  const v = s.speakers?.[sp];
  return v && Object.keys(v).length ? v : null;
};
const r3 = (v: number) => Math.round(v * 1000) / 1000;

export interface Placed {
  l: Line;
  b: Block;
  first: boolean;
  n: number;
}

/** 読み上げる行（空でない行）。n は台本の通し番号 */
export function voicedLines(s: Script): Placed[] {
  const out: Placed[] = [];
  let n = 0;
  for (const b of s.blocks) {
    let first = true;
    for (const l of b.lines) {
      n++;
      if (!ttsKeyText(l.text)) continue;
      out.push({ l, b, first, n });
      first = false;
    }
  }
  return out;
}

/**
 * 台本の声の並び（layout）と、演出が読んでいる files/script.srt の cue の時刻の食い違い。
 * 書き出し（export_talk）と同じく、声を開始の順に並べて i 番目の cue と比べる。
 */
export interface CueDrift {
  /** 一番大きいずれ（秒、絶対値） */
  max: number;
  /** tol を超えてずれている cue（i は 0 始まりの番号、d = 声の開始 − cue の開始） */
  offs: { i: number; cue: number; voice: number; d: number; n: number }[];
  nLines: number;
  nCues: number;
  /** ずれが無く、数も合っている */
  ok: boolean;
}

export function cueDrift(layout: VoiceItem[], cues: { start: number }[], tol = 0.05): CueDrift {
  const items = [...layout].sort((a, b) => a.start - b.start || a.n - b.n);
  const offs: CueDrift["offs"] = [];
  let max = 0;
  const m = Math.min(items.length, cues.length);
  for (let i = 0; i < m; i++) {
    const d = items[i].start - cues[i].start;
    max = Math.max(max, Math.abs(d));
    if (Math.abs(d) > tol) offs.push({ i, cue: cues[i].start, voice: items[i].start, d, n: items[i].n });
  }
  return { max, offs, nLines: items.length, nCues: cues.length, ok: offs.length === 0 && items.length === cues.length };
}

/** 指定した行から後ろを、間隔どおりに並べ直す（id を省くと全体）。durs は行の id → 長さ（秒） */
export function relayoutFrom(s: Script, durs: ReadonlyMap<string, number>, gap: number, blockGap: number, id?: string) {
  const arr = voicedLines(s);
  // 長さは Rust の並び（VoiceItem.dur）から。並びに無い行は見積もり（Rust の est_dur が元。同じ式）
  const estDur = (l: Line) => Math.max(0.8, ttsText(l.text).length * 0.13);
  const durOf = (l: Line) => durs.get(l.id) ?? estDur(l);
  const gapBefore = (x: Placed, idx: number) => (idx === 0 ? 0 : x.first ? blockGap : gap);
  const idx = id ? arr.findIndex((x) => x.l.id === id) : 0;
  if (idx < 0) return;
  let t = idx === 0 ? 0 : (arr[idx - 1].l.start ?? 0) + durOf(arr[idx - 1].l) + gapBefore(arr[idx], idx);
  for (let n = idx; n < arr.length; n++) {
    if (n > idx) t += gapBefore(arr[n], n);
    arr[n].l.start = r3(t);
    t += durOf(arr[n].l);
  }
}

/** トラックでドラッグした行を、その時刻に置き、台本の順番もそこへ移す */
export function moveLineTo(s: Script, lineId: string, t: number) {
  const find = (id: string) => {
    for (const b of s.blocks) {
      const i = b.lines.findIndex((l) => l.id === id);
      if (i >= 0) return { b, i };
    }
    return null;
  };
  const f = find(lineId);
  if (!f) return;
  const [l] = f.b.lines.splice(f.i, 1);
  l.start = r3(Math.max(0, t));
  let after: Placed | null = null;
  const all = voicedLines(s);
  for (const x of all) if (x.l.start != null && x.l.start <= l.start!) after = x;
  const anchor = after || all[0];
  if (!anchor) {
    (s.blocks[0] || f.b).lines.push(l);
    return;
  }
  const g = find(anchor.l.id)!;
  g.b.lines.splice(after ? g.i + 1 : g.i, 0, l);
}

export function findLine(s: Script, id: string): { b: Block; i: number; l: Line } | null {
  for (const b of s.blocks) {
    const i = b.lines.findIndex((l) => l.id === id);
    if (i >= 0) return { b, i, l: b.lines[i] };
  }
  return null;
}

let uid = 0;
export const nid = (p = "n") => `${p}${Date.now().toString(36)}${(++uid).toString(36)}`;
