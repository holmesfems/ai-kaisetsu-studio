// モニターの印（Claude への場所つきの指示）の、純粋な計算と描き方（仕様 docs/spec/feat-chat-marks.md）。
// モニターと Claude に送る確認画像の層は、どちらも drawMarks 1 つで描く。state/・io/ は import しない。
import type { Mark } from "../gen/Mark";
import type { MarkShape } from "../gen/MarkShape";
import type { Scene } from "../gen/Scene";
import type { Group, Override, Track } from "../types";
import { fxSelectable, mediaBoxOf, selectable } from "./pick.ts";
import { groupSpan, windowBoxOf } from "./nest.ts";

export type P = [number, number];
export type Box = [number, number, number, number]; // x0, y0, x1, y1
export type MarkTool = "pin" | "rect" | "arrow" | "pen";

/** 印の色（固定の 4 色）。意味は画面の色の上にマウスを置くと出す */
export const MARK_COLORS = [
  { key: "red", hex: "#ff4d5e", name: "赤", meaning: "直して" },
  { key: "yellow", hex: "#ffd23f", name: "黄", meaning: "注目" },
  { key: "green", hex: "#3ddc84", name: "緑", meaning: "良い（残す）" },
  { key: "blue", hex: "#4cc9ff", name: "水色", meaning: "参考" },
] as const;

export const MARK_TOOLS: { key: MarkTool; label: string; help: string }[] = [
  { key: "pin", label: "ピン", help: "クリックした所に印を付けます" },
  { key: "rect", label: "矩形", help: "ドラッグで四角く囲みます" },
  { key: "arrow", label: "矢印", help: "ドラッグで始点から終点へ矢印を引きます" },
  { key: "pen", label: "ペン", help: "ドラッグで自由に線を引きます" },
];

export const markHex = (color: string): string => MARK_COLORS.find((c) => c.key === color)?.hex ?? MARK_COLORS[0].hex;
export const markMeaning = (color: string): string => MARK_COLORS.find((c) => c.key === color)?.meaning ?? "";

/** 線の太さ・番号の丸の直径（どちらも 1920 幅での画素） */
export const LINE_W = 4;
export const BADGE_D = 34;
/** これより小さいドラッグはピンにする（1920 の座標での画素） */
export const DRAG_MIN = 6;
/** ペンの点の最大数 */
export const MAX_PEN = 32;
/** 済の印の不透明度 */
export const DONE_ALPHA = 0.35;
/** clipsUnder が返す演出・素材の最大数 */
export const MAX_UNDER = 5;
export const W = 1920;
export const H = 1080;

/** 印の番号（①〜㉟。それより大きければ (n)） */
export function circled(n: number): string {
  if (n >= 1 && n <= 20) return String.fromCodePoint(0x2460 + n - 1);
  if (n >= 21 && n <= 35) return String.fromCodePoint(0x3251 + n - 21);
  return `(${n})`;
}
export const circledList = (ids: readonly number[]): string => ids.map(circled).join("");

// ---------------------------------------------------------------- 形をつくる

const clampPt = ([x, y]: P): P => [Math.round(Math.min(W, Math.max(0, x))), Math.round(Math.min(H, Math.max(0, y)))];

function segDist(px: number, py: number, a: P, b: P): number {
  const dx = b[0] - a[0];
  const dy = b[1] - a[1];
  const len2 = dx * dx + dy * dy;
  if (len2 === 0) return Math.hypot(px - a[0], py - a[1]);
  const u = Math.max(0, Math.min(1, ((px - a[0]) * dx + (py - a[1]) * dy) / len2));
  return Math.hypot(px - (a[0] + u * dx), py - (a[1] + u * dy));
}

/** 直線（端点 a, b を通る無限の線）までの距離。a == b なら点までの距離 */
function lineDist(p: P, a: P, b: P): number {
  const dx = b[0] - a[0];
  const dy = b[1] - a[1];
  const len2 = dx * dx + dy * dy;
  if (len2 === 0) return Math.hypot(p[0] - a[0], p[1] - a[1]);
  return Math.abs(dx * (a[1] - p[1]) - (a[0] - p[0]) * dy) / Math.sqrt(len2);
}

function rdp(pts: P[], eps: number): P[] {
  const keep = new Array<boolean>(pts.length).fill(false);
  keep[0] = keep[pts.length - 1] = true;
  const stack: [number, number][] = [[0, pts.length - 1]];
  while (stack.length) {
    const [s, e] = stack.pop()!;
    let best = -1;
    let bd = eps;
    for (let i = s + 1; i < e; i++) {
      const d = lineDist(pts[i], pts[s], pts[e]);
      if (d > bd) {
        bd = d;
        best = i;
      }
    }
    if (best >= 0) {
      keep[best] = true;
      stack.push([s, best], [best, e]);
    }
  }
  return pts.filter((_, i) => keep[i]);
}

/** ペンの点の間引き（Douglas-Peucker）。直線は 2 点、最大 max 点。座標は整数にそろえる */
export function simplify(pts: readonly P[], max = MAX_PEN, eps0 = 2): P[] {
  const d: P[] = [];
  for (const p of pts) {
    const q = clampPt(p);
    const last = d[d.length - 1];
    if (!last || last[0] !== q[0] || last[1] !== q[1]) d.push(q);
  }
  if (d.length <= 2) return d;
  let eps = eps0;
  let out = rdp(d, eps);
  while (out.length > max && eps < 4000) {
    eps *= 1.5;
    out = rdp(d, eps);
  }
  if (out.length > max) {
    // 念のため: 等間隔に取る（端は残す）
    out = Array.from({ length: max }, (_, i) => d[Math.round((i * (d.length - 1)) / (max - 1))]);
  }
  return out;
}

/** ドラッグで集めた点（1920 の座標）から印の形を作る。小さすぎるドラッグはピン */
export function shapeFromDrag(tool: MarkTool, pts: readonly P[]): MarkShape {
  const a = clampPt(pts[0]);
  const b = clampPt(pts[pts.length - 1]);
  if (tool === "pin") return { kind: "pin", at: a };
  if (tool === "pen") {
    const s = simplify(pts);
    const xs = s.map((p) => p[0]);
    const ys = s.map((p) => p[1]);
    const diag = Math.hypot(Math.max(...xs) - Math.min(...xs), Math.max(...ys) - Math.min(...ys));
    return diag < DRAG_MIN || s.length < 2 ? { kind: "pin", at: a } : { kind: "pen", points: s };
  }
  if (Math.hypot(b[0] - a[0], b[1] - a[1]) < DRAG_MIN) return { kind: "pin", at: a };
  if (tool === "arrow") return { kind: "arrow", from: a, to: b };
  const x = Math.min(a[0], b[0]);
  const y = Math.min(a[1], b[1]);
  return { kind: "rect", box: [x, y, Math.abs(b[0] - a[0]), Math.abs(b[1] - a[1])] };
}

/** 印の区間の既定: 描いたときの t を含むシーン。シーンが無ければ t の前後 2 秒 */
export function defaultSpan(t: number, scenes: readonly Pick<Scene, "t0" | "t1">[]): [number, number] {
  const sc = scenes.find((s) => s.t0 <= t && t < s.t1);
  return sc ? [sc.t0, sc.t1] : [Math.max(0, t - 2), t + 2];
}

/** 新しい印（id 0。state・by は Rust が user・draft にする） */
export function newMark(color: string, shape: MarkShape, note: string, t: number, span: [number, number]): Mark {
  return {
    id: 0, color, shape, note, t, t0: span[0], t1: span[1], state: "draft", by: "user",
    clips: [], media: [], done_note: "",
  };
}

// ---------------------------------------------------------------- 出す印・タイムラインの印

/** モニターに出す印: 表示が入っていて、t0 <= t < t1。済は「済も表示」のときだけ */
export function visibleMarks(marks: readonly Mark[], t: number, show: boolean, showDone: boolean): Mark[] {
  if (!show) return [];
  return marks.filter((m) => m.t0 <= t && t < m.t1 && (m.state !== "done" || showDone));
}

/** タイムラインに帯を出す印（区間は問わない） */
export function listedMarks(marks: readonly Mark[], show: boolean, showDone: boolean): Mark[] {
  return show ? marks.filter((m) => m.state !== "done" || showDone) : [];
}

/** 帯が重なるときの段（小さい段から詰める） */
export function bandLanes(marks: readonly Mark[]): { lane: Map<number, number>; n: number } {
  const lane = new Map<number, number>();
  const ends: number[] = [];
  for (const m of [...marks].sort((a, b) => a.t0 - b.t0 || a.id - b.id)) {
    let i = ends.findIndex((e) => e <= m.t0 + 1e-6);
    if (i < 0) {
      i = ends.length;
      ends.push(0);
    }
    ends[i] = m.t1;
    lane.set(m.id, i);
  }
  return { lane, n: ends.length };
}

/** チャットで送る印: 自分の draft（外したものは除く）。番号順 */
export function sendableMarks(marks: readonly Mark[], skip: ReadonlySet<number>): Mark[] {
  return marks.filter((m) => m.state === "draft" && m.by === "user" && !skip.has(m.id)).sort((a, b) => a.id - b.id);
}

/** 確認画像は印の t ごとに 1 枚 */
export function groupByT(marks: readonly Mark[]): { t: number; marks: Mark[] }[] {
  const out: { key: number; t: number; marks: Mark[] }[] = [];
  for (const m of marks) {
    const key = Math.round(m.t * 1000);
    const g = out.find((x) => x.key === key);
    if (g) g.marks.push(m);
    else out.push({ key, t: m.t, marks: [m] });
  }
  return out.map(({ t, marks }) => ({ t, marks }));
}

/** 本文が空のとき: 印があれば「（印を送りました）」、画像だけなら imgOnly（chatimg の bodyFor と同じ文） */
export const MARK_ONLY_TEXT = "（印を送りました）";
export function bodyWithMarks(text: string, nImg: number, nMarks: number, imgOnly: string): string {
  const t = text.trim();
  if (t !== "") return t;
  if (nMarks > 0) return MARK_ONLY_TEXT;
  return nImg > 0 ? imgOnly : "";
}

/** 取り消したメッセージの画像のうち、印の確認画像（mark-….jpg）は入力欄に戻さない（送り直すと作り直すので） */
export function dropMarkImages(paths: readonly string[]): string[] {
  return paths.filter((p) => !(p.split(/[\\/]/).pop() ?? "").startsWith("mark-"));
}

export function bytesToBase64(b: Uint8Array): string {
  let s = "";
  for (let i = 0; i < b.length; i += 0x8000) s += String.fromCharCode(...b.subarray(i, i + 0x8000));
  return btoa(s);
}

// ---------------------------------------------------------------- 場所の計算

export function markBBox(s: MarkShape): Box {
  switch (s.kind) {
    case "pin": return [s.at[0], s.at[1], s.at[0], s.at[1]];
    case "rect": return [s.box[0], s.box[1], s.box[0] + s.box[2], s.box[1] + s.box[3]];
    case "arrow": return [Math.min(s.from[0], s.to[0]), Math.min(s.from[1], s.to[1]), Math.max(s.from[0], s.to[0]), Math.max(s.from[1], s.to[1])];
    case "pen": {
      const xs = s.points.map((p) => p[0]);
      const ys = s.points.map((p) => p[1]);
      return [Math.min(...xs), Math.min(...ys), Math.max(...xs), Math.max(...ys)];
    }
  }
}

/** 番号の丸の中心: ピンはその点、矩形・ペンは外接の左上、矢印は始点 */
export function badgePos(s: MarkShape): P {
  if (s.kind === "pin") return s.at;
  if (s.kind === "arrow") return s.from;
  const b = markBBox(s);
  return [b[0], b[1]];
}

/** 点 (x, y) から印の線（と番号の丸）までの距離 */
export function markDistance(m: Pick<Mark, "shape">, x: number, y: number): number {
  const s = m.shape;
  const [bx, by] = badgePos(s);
  let d = Math.max(0, Math.hypot(x - bx, y - by) - BADGE_D / 2);
  if (s.kind === "rect") {
    const [x0, y0, x1, y1] = markBBox(s);
    const c: P[] = [[x0, y0], [x1, y0], [x1, y1], [x0, y1], [x0, y0]];
    for (let i = 0; i < 4; i++) d = Math.min(d, segDist(x, y, c[i], c[i + 1]));
  } else if (s.kind === "arrow") d = Math.min(d, segDist(x, y, s.from, s.to));
  else if (s.kind === "pen") for (let i = 0; i + 1 < s.points.length; i++) d = Math.min(d, segDist(x, y, s.points[i], s.points[i + 1]));
  return d;
}

/** クリックで選ぶ印（tol 画素以内で一番近いもの。同じ近さなら新しい番号）。矩形の中は当たりにしない（中にピンを打てるように） */
export function hitMark(marks: readonly Mark[], x: number, y: number, tol: number): number | null {
  let best: { id: number; d: number } | null = null;
  for (const m of marks) {
    const d = markDistance(m, x, y);
    if (d > tol) continue;
    if (!best || d < best.d - 1e-9 || (Math.abs(d - best.d) <= 1e-9 && m.id > best.id)) best = { id: m.id, d };
  }
  return best ? best.id : null;
}

// ---------------------------------------------------------------- 印の下にあるもの

export interface UnderCtx {
  groups: readonly Group[];
  overrides: Record<string, Override | undefined>;
  tracks: readonly Track[];
  /** 素材の大きさ（まだ分からなければ undefined。その素材は飛ばす） */
  size: (src: string) => { w: number; h: number } | undefined | null;
}

const PIN_TOL = 6;
const overlaps = (a: Box, b: Box) => a[0] <= b[2] && b[0] <= a[2] && a[1] <= b[3] && b[1] <= a[3];
const areaOf = (b: Box) => (b[2] - b[0]) * (b[3] - b[1]);

/**
 * 時刻 t に、印の外接矩形と重なる演出クリップ（キー）・素材クリップ（id）。それぞれ小さい順に最大 5 つ。
 * 演出は Monitor.tsx の pick と同じ（bbox を windowBoxOf と微調整で動かし、見えている区間・hidden でないもの。FX トラックを隠した・ロックしたときは無し）、
 * 素材は pickMedia と同じ箱で、隠した・ロックしたトラックは除く。
 */
export function clipsUnder(shape: MarkShape, t: number, c: UnderCtx): { clips: string[]; media: string[] } {
  const bb = markBBox(shape);
  const m: Box = shape.kind === "pin" ? [bb[0] - PIN_TOL, bb[1] - PIN_TOL, bb[2] + PIN_TOL, bb[3] + PIN_TOL] : bb;
  const fx: { key: string; area: number; z: number }[] = [];
  if (fxSelectable(c.tracks as Track[])) {
    for (const g of c.groups) {
      const o = c.overrides[g.key] ?? undefined;
      if (o?.hidden || !g.bbox) continue;
      const [a, b] = groupSpan(g, o);
      if (t < a || t >= b) continue;
      const box = windowBoxOf(g.bbox, o);
      if (overlaps(m, box)) fx.push({ key: g.key, area: areaOf(box), z: g.z });
    }
  }
  fx.sort((p, q) => p.area - q.area || q.z - p.z);
  const media: { id: string; area: number }[] = [];
  for (const tr of [...c.tracks].reverse()) {
    if ((tr.kind !== "image" && tr.kind !== "video") || !selectable(tr)) continue;
    for (const cl of tr.clips) {
      if (t < cl.start || t >= cl.start + cl.dur) continue;
      const sz = c.size(cl.src);
      if (!sz) continue;
      const box = mediaBoxOf(cl, sz);
      if (overlaps(m, box)) media.push({ id: cl.id, area: areaOf(box) });
    }
  }
  media.sort((p, q) => p.area - q.area);
  return { clips: fx.slice(0, MAX_UNDER).map((x) => x.key), media: media.slice(0, MAX_UNDER).map((x) => x.id) };
}

// ---------------------------------------------------------------- 描く

type Ctx = CanvasRenderingContext2D | OffscreenCanvasRenderingContext2D;
export interface DrawOpts {
  /** 一言を番号の横に描く（モニターだけ。確認画像には描かない） */
  note: boolean;
  /** 選んでいる印（白い破線の枠を付ける） */
  selectedId?: number | null;
}

function strokeShape(ctx: Ctx, s: MarkShape, k: number) {
  if (s.kind === "rect") {
    ctx.strokeRect(s.box[0] * k, s.box[1] * k, s.box[2] * k, s.box[3] * k);
  } else if (s.kind === "arrow") {
    const [x0, y0] = s.from;
    const [x1, y1] = s.to;
    const ang = Math.atan2(y1 - y0, x1 - x0);
    const hl = 26;
    ctx.beginPath();
    ctx.moveTo(x0 * k, y0 * k);
    ctx.lineTo(x1 * k, y1 * k);
    for (const da of [Math.PI * 0.85, -Math.PI * 0.85]) {
      ctx.moveTo(x1 * k, y1 * k);
      ctx.lineTo((x1 + Math.cos(ang + da) * hl) * k, (y1 + Math.sin(ang + da) * hl) * k);
    }
    ctx.stroke();
  } else if (s.kind === "pen") {
    ctx.beginPath();
    s.points.forEach(([x, y], i) => (i ? ctx.lineTo(x * k, y * k) : ctx.moveTo(x * k, y * k)));
    ctx.stroke();
  }
}

/** 印を描く。座標は 1920x1080 で、k 倍して描く（モニターは表示の大きさ ÷ 1920、確認画像の層は 1） */
export function drawMarks(ctx: Ctx, marks: readonly Mark[], k: number, opts: DrawOpts): void {
  for (const m of marks) {
    const col = markHex(m.color);
    const done = m.state === "done";
    ctx.save();
    ctx.globalAlpha = done ? DONE_ALPHA : 1;
    ctx.lineJoin = "round";
    ctx.lineCap = "round";
    // 線: 暗い縁取りの上に色の線（明るい絵の上でも見えるように）。Claude の印は破線
    if (m.shape.kind !== "pin") {
      ctx.setLineDash(m.by === "claude" ? [14 * k, 9 * k] : []);
      ctx.strokeStyle = "rgba(0,0,0,0.55)";
      ctx.lineWidth = (LINE_W + 3) * k;
      strokeShape(ctx, m.shape, k);
      ctx.strokeStyle = col;
      ctx.lineWidth = LINE_W * k;
      strokeShape(ctx, m.shape, k);
      ctx.setLineDash([]);
    }
    // 番号の丸（id 0 は描いている途中のもの。数字は付けない）
    const [bx, by] = badgePos(m.shape);
    const r = (BADGE_D / 2) * k;
    ctx.beginPath();
    ctx.arc(bx * k, by * k, r, 0, Math.PI * 2);
    ctx.fillStyle = col;
    ctx.fill();
    ctx.lineWidth = 2 * k;
    ctx.strokeStyle = "rgba(0,0,0,0.6)";
    ctx.stroke();
    if (m.id > 0) {
      const label = String(m.id);
      ctx.font = `bold ${(label.length > 2 ? 13 : label.length > 1 ? 17 : 21) * k}px sans-serif`;
      ctx.textAlign = "center";
      ctx.textBaseline = "middle";
      ctx.lineWidth = 3 * k;
      ctx.strokeStyle = "rgba(0,0,0,0.5)";
      ctx.strokeText(label, bx * k, by * k + k);
      ctx.fillStyle = "#ffffff";
      ctx.fillText(label, bx * k, by * k + k);
    }
    if (done) {
      const cx = (bx + 14) * k;
      const cy = (by - 14) * k;
      ctx.beginPath();
      ctx.arc(cx, cy, 9 * k, 0, Math.PI * 2);
      ctx.fillStyle = "#ffffff";
      ctx.fill();
      ctx.font = `bold ${13 * k}px sans-serif`;
      ctx.textAlign = "center";
      ctx.textBaseline = "middle";
      ctx.fillStyle = "#118a4a";
      ctx.fillText("✓", cx, cy + k);
    }
    if (opts.note && m.note) {
      ctx.font = `${20 * k}px sans-serif`;
      ctx.textAlign = "left";
      ctx.textBaseline = "middle";
      const w = ctx.measureText(m.note).width + 14 * k;
      const x = bx * k + r + 6 * k;
      ctx.fillStyle = "rgba(0,0,0,0.7)";
      ctx.fillRect(x, by * k - 14 * k, w, 28 * k);
      ctx.fillStyle = "#ffffff";
      ctx.fillText(m.note, x + 7 * k, by * k + k);
    }
    if (opts.selectedId != null && opts.selectedId === m.id) {
      const [x0, y0, x1, y1] = markBBox(m.shape);
      const pad = m.shape.kind === "pin" ? BADGE_D / 2 + 8 : 12;
      ctx.globalAlpha = 1;
      ctx.setLineDash([8 * k, 5 * k]);
      ctx.lineWidth = 2 * k;
      ctx.strokeStyle = "#ffffff";
      ctx.strokeRect((x0 - pad) * k, (y0 - pad) * k, (x1 - x0 + pad * 2) * k, (y1 - y0 + pad * 2) * k);
    }
    ctx.restore();
  }
}
