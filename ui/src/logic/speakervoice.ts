// 話者ごとの声（声のエンジンの話者の値とスタイルの重み）。トークトラックの ⚙ で変える。
// 台本フォルダの _voice_speakers.json に、共通のプリセット（エンジンの PRESETS。VoiSona なら voisona_presets.json）から変えた項目だけを残す。
// 声を作るときは、声の中継（voice_server.py）がプリセットにこれを重ねる（セリフごとの抑揚はさらにその上）。
// 項目と範囲はエンジンが決める（logic/voiceengine.ts の paramFields）。VOICE_PARAMS は知っている項目の名前・単位の表

/** _voice_speakers.json の 1 人分（変えた項目だけ） */
export interface SpeakerVoice {
  speed?: number;
  volume?: number;
  pitch?: number;
  intonation?: number;
  alp?: number;
  huskiness?: number;
  style_weights?: number[];
  /** エンジンが決める、知らない項目 */
  [key: string]: number | number[] | undefined;
}

export type VoiceParam = "speed" | "volume" | "pitch" | "intonation" | "alp" | "huskiness";

/** 話者の声の項目の範囲（clamp・既定の値に要るところだけ） */
export interface VoiceField { key: string; def: number; min: number; max: number }

/** voice_server の /api/speakers が返す 1 人分 */
export interface SpeakerInfo {
  voice_name: string;
  /** 共通のプリセットの値 */
  base: SpeakerVoice;
  /** エンジンのスタイルの名前（重みの順） */
  style_names: string[];
  /** 画面に出す名前（プリセットの日本語つきの名前。数が合わなければ style_names） */
  style_labels: string[];
  /** エンジンからスタイルの名前を受け取れなかったときのエラー */
  error?: string | null;
}

/** 知っている項目の名前・単位の表（VoiSona Talk の画面の並び SPD・VOL・PIT・INTO・ALP・HUS。範囲は VoiSona が受け付ける範囲で、エンジンが違えば paramFields が範囲を差し替える） */
export const VOICE_PARAMS: {
  key: VoiceParam; label: string; abbr: string; unit: string; def: number; min: number; max: number; step: number; tip: string;
}[] = [
  { key: "speed", label: "速さ", abbr: "SPD", unit: "倍", def: 1, min: 0.2, max: 5, step: 0.01, tip: "話す速さ（0.2〜5 倍）" },
  { key: "volume", label: "音量", abbr: "VOL", unit: "dB", def: 0, min: -8, max: 8, step: 0.1, tip: "声の大きさ（−8〜+8 dB）" },
  { key: "pitch", label: "高さ", abbr: "PIT", unit: "セント", def: 0, min: -600, max: 600, step: 1, tip: "声の高さ（−600〜+600 セント。100 セントで半音）" },
  { key: "intonation", label: "抑揚", abbr: "INTO", unit: "倍", def: 1, min: 0, max: 2, step: 0.01, tip: "抑揚の大きさ（0〜2 倍）。セリフごとの抑揚を変えた行は、そちらが優先されます" },
  { key: "alp", label: "声の若さ", abbr: "ALP", unit: "", def: 0, min: -1, max: 1, step: 0.01, tip: "声の若さ・太さ（−1〜+1）" },
  { key: "huskiness", label: "ハスキー", abbr: "HUS", unit: "", def: 0, min: -20, max: 20, step: 0.1, tip: "かすれ具合（−20〜+20）" },
];

const EPS = 1e-6;
const near = (a: number, b: number) => Math.abs(a - b) < EPS;
const r3 = (v: number) => Math.round(v * 1000) / 1000;
const clamp = (v: number, lo: number, hi: number) => Math.min(hi, Math.max(lo, v));

/** その話者の今の値（プリセットに、この回の変更を重ねたもの） */
export function effectiveVoice(base: SpeakerVoice | undefined, over: SpeakerVoice | undefined, fields: VoiceField[] = VOICE_PARAMS): SpeakerVoice {
  const out: SpeakerVoice = { ...(base || {}) };
  for (const p of fields) {
    const v = over?.[p.key];
    if (typeof v === "number") out[p.key] = v;
    else if (out[p.key] == null) out[p.key] = p.def;
  }
  const w = over?.style_weights;
  if (w && (!base?.style_weights || w.length === base.style_weights.length)) out.style_weights = [...w];
  return out;
}

/**
 * 1 人分の変更に patch を重ねる（null の項目は消す）。範囲に丸め、プリセットと同じ値になった項目は消す。
 * 何も残らなければ null（その話者の項目ごと消す）
 */
export function patchVoice(
  over: SpeakerVoice | undefined,
  base: SpeakerVoice | undefined,
  patch: { [K in keyof SpeakerVoice]?: SpeakerVoice[K] | null },
  fields: VoiceField[] = VOICE_PARAMS,
): SpeakerVoice | null {
  const out: SpeakerVoice = { ...(over || {}) };
  for (const p of fields) {
    if (!(p.key in patch)) continue;
    const v = patch[p.key];
    if (v != null && typeof v !== "number") continue;
    const b = (base?.[p.key] as number | undefined) ?? p.def;
    if (v == null || !Number.isFinite(v) || near(r3(clamp(v, p.min, p.max)), b)) delete out[p.key];
    else out[p.key] = r3(clamp(v, p.min, p.max));
  }
  if ("style_weights" in patch) {
    const w = patch.style_weights?.map((x) => r3(clamp(Number.isFinite(x) ? x : 0, 0, 1)));
    const bw = base?.style_weights;
    if (!w || !w.length || (bw && bw.length === w.length && w.every((x, i) => near(x, bw[i])))) delete out.style_weights;
    else out.style_weights = w;
  }
  // 項目は名前の順（Rust が保存して読み直したときと同じ順にし、音声の印（Rust の voice/analysis.rs の sig）が変わらないように）
  const keys = Object.keys(out).sort();
  return keys.length ? Object.fromEntries(keys.map((k) => [k, out[k]])) as SpeakerVoice : null;
}

/** スタイルの i 番目の重みを v にした重みの並び（今の値から） */
export function withStyleWeight(cur: number[], i: number, v: number | null, base: number[] | undefined): number[] {
  const w = [...cur];
  w[i] = v ?? base?.[i] ?? 0;
  return w;
}

/** 声が変わった話者（a と b は _voice_speakers.json の中身） */
export function changedSpeakers(a: Record<string, SpeakerVoice> | undefined, b: Record<string, SpeakerVoice> | undefined): string[] {
  const keys = new Set([...Object.keys(a || {}), ...Object.keys(b || {})]);
  return [...keys].filter((sp) => JSON.stringify(a?.[sp] ?? null) !== JSON.stringify(b?.[sp] ?? null));
}

/** 配合の割合（重みの合計で割る。全部 0 なら全部 0） */
export function styleShares(w: number[]): number[] {
  const sum = w.reduce((a, x) => a + Math.max(0, x), 0);
  return w.map((x) => (sum > 0 ? Math.max(0, x) / sum : 0));
}
