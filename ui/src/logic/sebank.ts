// 効果音（SE）の設定。演出の動きから自動で付く効果音を、種類（シチュエーション）ごとに「どの音を鳴らすか」「どの大きさか」を決める。
// 中身は回の fx/se/bank.json（読み書きと試聴の音づくりは演出ワーカー。Rust の sebank.rs → akfx_worker.py の se_bank*）。
// ここは画面の下書きの扱い（アプリを起動せずに `npm test` で確かめられるもの）。
import type { Desc, Override, SeBank, SeKind } from "../types";

/** 種類の表示名。名前の表はワーカー（se_bank_info の label）が持つ。設定が読めていない・知らない種類は種類の名前のまま出す */
export const seLabel = (kind: string, bank?: SeBank | null) => bank?.kinds.find((k) => k.kind === kind)?.label ?? kind;

/** 候補の表示名（"synth" は「合成音」） */
export const candLabel = (src: string) => (src === "synth" ? "合成音" : src);

export const dbToGain = (db: number) => Math.pow(10, db / 20);

export function fmtDb(db: number): string {
  const r = Math.round(db * 10) / 10;
  if (Math.abs(r) < 0.05) return "0 dB";
  return `${r > 0 ? "+" : ""}${r.toFixed(1)} dB`;
}

/** 画面の下書き: 種類ごとの「鳴らす候補」と「候補ごとの音量」 */
export type SeDraft = Record<string, { use: string[]; db: Record<string, number> }>;

export function draftOf(k: SeKind): SeDraft[string] {
  return { use: [...k.use], db: Object.fromEntries(k.cands.map((c) => [c.src, c.db])) };
}

/** 候補を選ぶ。multi（順番に使う）なら足し外し（最後の 1 つは外さない）、そうでなければその 1 つだけにする */
export function pickUse(use: string[], src: string, multi: boolean): string[] {
  if (!multi) return [src];
  if (use.includes(src)) return use.length > 1 ? use.filter((s) => s !== src) : use;
  return [...use, src];
}

const sameList = (a: string[], b: string[]) => a.length === b.length && a.every((x, i) => x === b[i]);
const near = (a: number, b: number) => Math.abs(a - b) < 1e-6;

/** 下書きと保存してある設定の違い → se_bank_set に渡す kinds（変わった種類・項目だけ） */
export function draftPatch(bank: SeBank, draft: SeDraft): Record<string, { use?: string[]; db?: Record<string, number | null> }> {
  const out: Record<string, { use?: string[]; db?: Record<string, number | null> }> = {};
  for (const k of bank.kinds) {
    const d = draft[k.kind];
    if (!d) continue;
    const p: { use?: string[]; db?: Record<string, number | null> } = {};
    if (!sameList(d.use, k.use)) p.use = [...d.use];
    for (const c of k.cands) {
      // 棚の種類は選んでいる音だけが音量を持つ（選んでいない候補の音量は送らない。Rust の shelf_patch は受け付けない）
      if (k.shelf && c.src !== (d.use[0] ?? "synth")) continue;
      const v = d.db[c.src] ?? 0;
      if (!near(v, c.db)) (p.db ??= {})[c.src] = near(v, 0) ? null : Math.round(v * 100) / 100;
    }
    if (p.use || p.db) out[k.kind] = p;
  }
  return out;
}

export const isDirty = (bank: SeBank | null, draft: SeDraft) => !!bank && Object.keys(draftPatch(bank, draft)).length > 0;

/** タイムライン上でその種類が鳴る時刻（微調整のずらしを入れ、隠したクリップは除く）。確定音は数え上げの時刻で数える。
 *  間引く前の候補なので、近い所に重なったものは実際には鳴らないことがある */
export function seTimes(desc: Desc | null, overrides: Record<string, Override>, kind: string): number[] {
  if (!desc) return [];
  const want = kind === "confirm" || kind === "confirm_neg" ? "count" : kind;
  const out: number[] = [];
  for (const g of desc.groups) {
    const o = overrides[g.key];
    if (o?.hidden) continue;
    for (const ev of g.se ?? []) if (ev.kind === want) out.push(ev.t + (o?.shift || 0));
  }
  return out.sort((a, b) => a - b);
}

/** 種類ごとの鳴る回数 */
export function seCounts(desc: Desc | null, overrides: Record<string, Override>): Record<string, number> {
  const out: Record<string, number> = {};
  if (!desc) return out;
  for (const g of desc.groups) {
    if (overrides[g.key]?.hidden) continue;
    for (const ev of g.se ?? []) out[ev.kind] = (out[ev.kind] ?? 0) + 1;
  }
  out.confirm = out.confirm_neg = out.count ?? 0;
  return out;
}

/** 文脈試聴の区間（鳴る時刻の少し前から、少し後まで。タイムラインの外には出さない） */
export function contextSpan(t: number, total: number, pre = 1.5, post = 2.5): [number, number] {
  const a = Math.max(0, t - pre);
  return [a, Math.max(a, Math.min(total, t + post))];
}

/** 今の時刻から見て、次（dir=1）・前（dir=-1）に鳴る所の番号 */
export function nearestIndex(times: number[], t: number, dir: 1 | -1 | 0 = 0): number {
  if (!times.length) return -1;
  if (dir === 1) {
    const i = times.findIndex((x) => x > t + 0.05);
    return i < 0 ? 0 : i;
  }
  if (dir === -1) {
    for (let i = times.length - 1; i >= 0; i--) if (times[i] < t - 0.05) return i;
    return times.length - 1;
  }
  let best = 0;
  for (let i = 1; i < times.length; i++) if (Math.abs(times[i] - t) < Math.abs(times[best] - t)) best = i;
  return best;
}

/** 設定を読み直したときの下書き。保存していない変更がある種類はそのまま残す（無くなった候補だけ外す） */
export function rebaseDraft(oldBank: SeBank | null, newBank: SeBank, draft: SeDraft): SeDraft {
  const out: SeDraft = {};
  for (const k of newBank.kinds) {
    const fresh = draftOf(k);
    const d = draft[k.kind];
    const ok = oldBank?.kinds.find((x) => x.kind === k.kind);
    const dirty = !!d && !!ok && Object.keys(draftPatch({ ...oldBank!, kinds: [ok] }, { [k.kind]: d })).length > 0;
    if (!dirty) {
      out[k.kind] = fresh;
      continue;
    }
    const srcs = new Set(k.cands.map((c) => c.src));
    const use = d!.use.filter((s) => srcs.has(s));
    const db = Object.fromEntries(Object.entries(d!.db).filter(([s]) => srcs.has(s)));
    out[k.kind] = { use: use.length ? use : fresh.use, db: { ...fresh.db, ...db } };
  }
  return out;
}
