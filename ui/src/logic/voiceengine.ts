// 声のエンジン（声の中継の ping の engine）と、エンジンごとの話者の声の項目。約束は docs/spec/issues-8-12.md
import { VOICE_PARAMS } from "./speakervoice.ts";

export type VoiceFeature = "accent" | "durations" | "intonation" | "styles";

export interface VoiceEngine {
  name: string;
  label: string;
  features: Set<string>;
}

/** 古い中継（ping に engine が無い）は VoiSona とみなす。できること（全部）も VoiSona のもの */
const LEGACY: VoiceEngine = {
  name: "voisona",
  label: "VoiSona Talk",
  features: new Set(["accent", "durations", "intonation", "styles"]),
};

/** ping の戻り値（voice.info）から声のエンジンを読む。engine が無い・壊れている古い中継は VoiSona */
export function engineOf(info: unknown): VoiceEngine {
  const e = (info as { engine?: { name?: unknown; label?: unknown; features?: unknown } } | null | undefined)?.engine;
  if (!e || typeof e !== "object" || typeof e.name !== "string") return { ...LEGACY, features: new Set(LEGACY.features) };
  const features = Array.isArray(e.features) ? e.features.filter((f): f is string => typeof f === "string") : [];
  return { name: e.name, label: typeof e.label === "string" && e.label ? e.label : e.name, features: new Set(features) };
}

export function has(engine: VoiceEngine, feature: VoiceFeature): boolean {
  return engine.features.has(feature);
}

/** 話者の声の 1 項目（画面の入力欄 1 つ分） */
export interface ParamField {
  key: string;
  label: string;
  abbr: string;
  unit: string;
  def: number;
  min: number;
  max: number;
  step: number;
  tip: string;
}

const EPS = 1e-9;

/** 範囲に合う刻み幅（幅の 1/100 を 10 のべきに丸める） */
function stepFor(min: number, max: number): number {
  const span = max - min;
  if (!(span > 0)) return 0.01;
  return Math.pow(10, Math.floor(Math.log10(span / 100)));
}

/**
 * /api/speakers の range（項目 → [最小, 最大]）から、話者の声の項目を作る。
 * 知っている項目（VOICE_PARAMS）は名前・略称・単位を使う。範囲が知っているものと違うときは、単位と説明が合わないので外す。
 * 知らない項目は項目名のまま。範囲が数の組でない項目は出さない
 */
export function paramFields(range: Record<string, unknown> | null | undefined): ParamField[] {
  const out: ParamField[] = [];
  for (const [key, r] of Object.entries(range ?? {})) {
    if (!Array.isArray(r) || r.length !== 2) continue;
    const [min, max] = r as unknown[];
    if (typeof min !== "number" || typeof max !== "number" || !Number.isFinite(min) || !Number.isFinite(max) || !(max > min)) continue;
    const known = VOICE_PARAMS.find((p) => p.key === key);
    const same = !!known && Math.abs(known.min - min) < EPS && Math.abs(known.max - max) < EPS;
    const def = Math.min(max, Math.max(min, known?.def ?? 0));
    if (known && same) out.push({ ...known, def });
    else if (known) out.push({ key, label: known.label, abbr: known.abbr, unit: "", def: key === "volume" && min >= 0 ? Math.min(max, 1) : def, min, max, step: stepFor(min, max), tip: `${known.label}（${min}〜${max}）` });
    else out.push({ key, label: key, abbr: "", unit: "", def, min, max, step: stepFor(min, max), tip: `${key}（${min}〜${max}）` });
  }
  return out;
}

/** 設定の選択肢（voice_engine の値 → 選択欄の項目）。`voisona`・`voicevox` 以外は自作のエンジンのファイル */
export type EngineChoice = "voisona" | "voicevox" | "file";

export function engineChoice(value: string | null | undefined): EngineChoice {
  const v = (value ?? "").trim();
  if (v === "" || v === "voisona") return "voisona";
  if (v === "voicevox") return "voicevox";
  return "file";
}
