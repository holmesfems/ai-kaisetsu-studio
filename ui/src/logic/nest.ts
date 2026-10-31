// RecWindow に差し込む録画＝ネストの、純粋な計算（store を使うものは state/nest.ts）。
import type { Group, MediaClip, Nest, NestFit, Override, Track } from "../types";
import { nid } from "./voice.ts";

export const FIT_LABEL: Record<NestFit, string> = { contain: "収める", stretch: "引き伸ばす", cover: "埋める" };

export const isRecGroup = (g: Group) => g.kinds.includes("RecWindow") || !!g.rec?.length;

/** 数のときだけ数として読む（Rust の as_f64 と同じ。文字列・null・欠けは既定） */
export const numOr = (v: unknown, d: number): number => (typeof v === "number" ? v : d);

/** 演出クリップの、微調整を入れた後の見えている区間（最低 0.05 秒）。見本 ui_rules.json の group_span で縛っている（ui/test/rules.test.ts）。Rust は cache::group_span */
export function groupSpan(g: Group, o?: Override): [number, number] {
  const sh = numOr(o?.shift, 0);
  const a = g.t0 + sh;
  return [a, Math.max(a + 0.05, g.t1 + sh + numOr(o?.end, 0))];
}

/** RecWindow の窓が見えている区間 */
export const windowSpan = groupSpan;

/** キャンバス → 画面の対応（横・縦の倍率と、キャンバスの左上）。Rust の Nest::map。見本 ui_rules.json の nest_map で縛っている（ui/test/rules.test.ts） */
export function nestMap(n: Pick<Nest, "w" | "h" | "fit">, box: number[]) {
  const bw = box[2] - box[0];
  const bh = box[3] - box[1];
  const cw = Math.max(1, n.w);
  const ch = Math.max(1, n.h);
  let sx: number;
  let sy: number;
  if (n.fit === "stretch") {
    sx = bw / cw;
    sy = bh / ch;
  } else {
    sx = sy = n.fit === "cover" ? Math.max(bw / cw, bh / ch) : Math.min(bw / cw, bh / ch);
  }
  return { sx, sy, ox: box[0] + (bw - cw * sx) / 2, oy: box[1] + (bh - ch * sy) / 2 };
}

/** ネストの既定（キャンバス 1920x1080・収める・窓は録画の窓の既定）。見本 ui_rules.json の nest_default で縛っている（Rust は Nest::default） */
export const NEST_DEFAULT = { box: [600, 100, 1880, 820] as [number, number, number, number], w: 1920, h: 1080, fit: "contain" as NestFit };

/** 元の窓 base に微調整（倍率・中心・移動）を入れた窓（1920x1080 の座標）。見本 ui_rules.json の window_box で縛っている（Rust は overrides::window_box） */
export function windowBoxOf(base: readonly number[], o?: Override): [number, number, number, number] {
  const s = numOr(o?.scale, 1);
  const p = o?.pivot as unknown[] | undefined;
  const pv = Array.isArray(p) && typeof p[0] === "number" && typeof p[1] === "number" ? [p[0], p[1]] : [(base[0] + base[2]) / 2, (base[1] + base[3]) / 2];
  const dx = numOr(o?.dx, 0);
  const dy = numOr(o?.dy, 0);
  return [pv[0] + (base[0] - pv[0]) * s + dx, pv[1] + (base[1] - pv[1]) * s + dy, pv[0] + (base[2] - pv[0]) * s + dx, pv[1] + (base[3] - pv[1]) * s + dy];
}

export const newTrack = (n: number): Track => ({
  id: nid("nt"), kind: "video", name: `V${n}`, clips: [], hidden: false, muted: false, locked: false, volume_db: 0,
});

export function findClip(n: Nest | undefined, id: string): { tr: Track; c: MediaClip } | null {
  for (const tr of n?.tracks ?? []) {
    const c = tr.clips.find((x) => x.id === id);
    if (c) return { tr, c };
  }
  return null;
}

/** 区間 [a, b) が空いているか（1e-6 秒の余裕）。見本 ui_rules.json の tracks.free で縛っている（Rust は mediaops::free） */
export const free = (tr: Track, a: number, b: number, except?: string) =>
  !tr.clips.some((c) => c.id !== except && c.start < b - 1e-6 && c.start + c.dur > a + 1e-6);

/** 素材をキャンバスに収める大きさ（縦横比を保つ）。contain は見本 ui_rules.json の tracks.fit_scale で縛っている（Rust は mediaops::fit_scale） */
export function fitScale(n: Pick<Nest, "w" | "h">, w: number, h: number, how: "contain" | "cover" | "width" | "height" = "contain") {
  if (how === "contain" && !(w > 0 && h > 0)) return 1; // 大きさが分からない素材は等倍（Rust の fit_scale と同じ）
  const a = n.w / Math.max(1, w);
  const b = n.h / Math.max(1, h);
  const s = how === "width" ? a : how === "height" ? b : how === "cover" ? Math.max(a, b) : Math.min(a, b);
  return Math.round(s * 100000) / 100000;
}

