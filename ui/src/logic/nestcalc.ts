// ネストの素材クリップの計算（再生速度・ドラッグの確定・分割）。画面や Tauri に触れない純粋な関数だけ（ui/test で確かめる）。
import type { MediaClip, Nest } from "../types";

const r3 = (v: number) => Math.round(v * 1000) / 1000;
const clamp = (v: number, a: number, b: number) => Math.max(a, Math.min(b, v));

export const SPEED_MIN = 0.1;
export const SPEED_MAX = 10;

/** 再生速度（無ければ等速。Rust の MediaClip.speed と同じ 0.1〜10） */
export const clipSpeed = (c: { speed?: number }) => clamp(c.speed || 1, SPEED_MIN, SPEED_MAX);

/** ドラッグ中のプレビュー（確定前の値） */
export type DragPrev = { id: string; start: number; dur: number; src_in: number; track: string };

/** ドラッグの結果をネストに書く（トラックが変わっていれば移す）。クリップが無ければ false */
export function commitDrag(n: Nest, p: DragPrev): boolean {
  let from: Nest["tracks"][number] | undefined;
  let c: MediaClip | undefined;
  for (const tr of n.tracks) {
    c = tr.clips.find((x) => x.id === p.id);
    if (c) { from = tr; break; }
  }
  if (!from || !c) return false;
  c.start = r3(p.start);
  c.dur = r3(p.dur);
  c.src_in = r3(p.src_in);
  if (p.track !== from.id) {
    const to = n.tracks.find((x) => x.id === p.track);
    if (to) {
      from.clips = from.clips.filter((x) => x.id !== p.id);
      to.clips.push(c);
    }
  }
  return true;
}

/** 右端を伸ばせる終わりの時刻（素材の長さが分かっていて、画像でもループでもないとき。それ以外は無限） */
export function maxEnd(c: MediaClip, srcDur: number): number {
  return srcDur > 0 ? c.start + Math.max(0, srcDur - c.src_in) / clipSpeed(c) : Infinity;
}

/** 左端を動かしたときの src_in（素材の進みは Δstart × 速度） */
export function trimSrcIn(c: MediaClip, ns: number): number {
  return c.src_in + (ns - c.start) * clipSpeed(c);
}

/** 左端を左へ動かせる限り（素材の頭まで。画像は制限なし） */
export function minStart(c: MediaClip, image: boolean): number {
  return image ? 0 : Math.max(0, c.start - c.src_in / clipSpeed(c));
}

/** 時刻 cut で分けたときの後ろ半分の src_in */
export function splitSrcIn(c: MediaClip, cut: number): number {
  return r3(c.src_in + (cut - c.start) * clipSpeed(c));
}

/** 速度を変える。素材の同じ範囲を使うよう、長さは 旧速度 / 新速度 倍にする（等速は speed を持たない） */
export function setSpeed(c: MediaClip, v: number) {
  const old = clipSpeed(c);
  const ns = clamp(v, SPEED_MIN, SPEED_MAX);
  c.dur = r3(Math.max(0.05, (c.dur * old) / ns));
  if (Math.abs(ns - 1) < 1e-9) delete c.speed;
  else c.speed = r3(ns);
}

/** 素材全体（0 秒〜素材の長さ）がタイムラインのどこに来るか [始まり, 終わり]（秒）。始まりは負にもなる */
export function sourceSpan(c: MediaClip, srcDur: number): [number, number] {
  const sp = clipSpeed(c);
  const a = c.start - c.src_in / sp;
  return [a, a + srcDur / sp];
}

/** 中身だけずらす（スリップ）: クリップの位置と長さはそのままで、dt 秒ぶん素材を右へ送ったときの src_in。
 *  素材の頭（0）と、終わり（クリップの長さぶん残る所）で止める */
export function slipSrcIn(c: MediaClip, dt: number, srcDur: number): number {
  const sp = clipSpeed(c);
  const hi = srcDur > 0 ? Math.max(0, srcDur - c.dur * sp) : Infinity;
  return r3(clamp(c.src_in - dt * sp, 0, hi));
}

