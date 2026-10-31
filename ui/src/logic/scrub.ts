// 数値を左右ドラッグで変える（Premiere の数値スクラブ）ための計算。画面の部品は components/Inspector.tsx の Num
// ドラッグ量は step で決め、min / max は値を丸めるのにだけ使う（範囲の広い項目でも 1px ＝ 1 step で動く）

/** ドラッグを始めたとみなす動き（px）。これより小さいとクリック（直接入力） */
export const SCRUB_START_PX = 3;

/** 修飾キーの倍率。Shift で粗く（×10）、Ctrl で細かく（×0.1）。Alt はタイムラインのスナップ切りに使うので使わない */
export function scrubMult(mods: { shiftKey?: boolean; ctrlKey?: boolean; metaKey?: boolean }): number {
  if (mods.shiftKey) return 10;
  if (mods.ctrlKey || mods.metaKey) return 0.1;
  return 1;
}

/** 刻みの小数の桁数（0.033 → 3、0.0033 → 4、5 → 0） */
export function stepDecimals(q: number): number {
  if (!(q > 0) || !Number.isFinite(q)) return 0;
  for (let d = 0; d < 8; d++) if (Math.abs(Math.round(q * 10 ** d) - q * 10 ** d) < 1e-6) return d;
  return 8;
}

export function clampRange(v: number, min?: number, max?: number): number {
  if (min !== undefined) v = Math.max(min, v);
  if (max !== undefined) v = Math.min(max, v);
  return v;
}

/** 浮動小数の端数を落とす（0.1+0.2 → 0.3）。桁は刻みに合わせ、最低 3 桁は残す */
export function cleanNum(v: number, q: number): number {
  const d = Math.max(3, stepDecimals(q));
  return Number(v.toFixed(d));
}

/**
 * 1 回のマウスの動き（dx px）ぶん値を進める。cur はドラッグ中の値（丸める前の続き）で、返り値を次の cur に使う。
 * 端で止まったら、そこから戻せばすぐ動くように cur も端に揃える
 */
export function scrubMove(cur: number, dx: number, step: number, mult: number, min?: number, max?: number): number {
  return clampRange(cur + dx * step * mult, min, max);
}

/** 表示・保存する値（刻みの桁で丸めて範囲に収める） */
export function scrubValue(cur: number, step: number, mult: number, min?: number, max?: number): number {
  return clampRange(cleanNum(cur, step * mult), min, max);
}

/** ↑↓ キーで 1 刻み動かす（dir は +1 / -1） */
export function nudge(v: number, dir: number, step: number, mult: number, min?: number, max?: number): number {
  return scrubValue(v + dir * step * mult, step, mult, min, max);
}

/** 入力欄の文字を数値に（全角数字・全角マイナスも受ける）。読めなければ null */
export function parseNum(s: string): number | null {
  const t = s
    .trim()
    .replace(/[０-９．]/g, (c) => String.fromCharCode(c.charCodeAt(0) - 0xfee0))
    .replace(/[−－]/g, "-");
  if (!t) return null;
  const n = Number(t);
  return Number.isFinite(n) ? n : null;
}

/** 表示用の文字（小数 3 桁まで） */
export function fmtNum(v: number): string {
  return String(Math.round(v * 1000) / 1000);
}
