// プレビュー（モニター・ネスト画面）のクリックで選べるトラックの判定（issue 旧#11）。
// 純粋な関数だけにして、`npm test` で確かめられるようにしている。
import type { MediaClip, Track } from "../types";

/** トラックの鍵ボタンの説明 */
export const LOCK_HELP = "ロック（プレビューのクリックでは選択しません。タイムラインからは選択できます）";

/** プレビューのクリックで、このトラックのクリップを選んでよいか（隠した・ロックしたトラックは選ばない） */
export const selectable = (tr: Pick<Track, "hidden" | "locked">): boolean => !tr.hidden && !tr.locked;

/** 演出（FX トラック）をプレビューのクリックで選んでよいか。FX トラックが無いときは選べる */
export function fxSelectable(tracks: Track[]): boolean {
  const fx = tracks.find((x) => x.kind === "fx");
  return !fx || selectable(fx);
}

/** 素材クリップが画面に占める箱（x0, y0, x1, y1。1920x1080 の座標）。sz は素材の大きさ。pickMedia と印の clipsUnder で共有 */
export function mediaBoxOf(c: Pick<MediaClip, "x" | "y" | "scale">, sz: { w: number; h: number }): [number, number, number, number] {
  const w = sz.w * c.scale;
  const h = sz.h * c.scale;
  return [c.x - w / 2, c.y - h / 2, c.x + w / 2, c.y + h / 2];
}

/**
 * 画面の (x, y)（1920x1080 の座標）・時刻 t にある素材クリップを、上のトラックから探す。
 * size は素材の大きさ（まだ分からなければ undefined を返す。そのクリップは飛ばす）。
 */
export function pickMedia(
  tracks: Track[],
  t: number,
  x: number,
  y: number,
  size: (src: string) => { w: number; h: number } | undefined | null,
): { track: Track; clip: MediaClip } | null {
  for (const tr of [...tracks].reverse()) {
    if (tr.kind !== "image" && tr.kind !== "video") continue;
    if (!selectable(tr)) continue;
    for (const c of tr.clips) {
      if (t < c.start || t >= c.start + c.dur) continue;
      const sz = size(c.src);
      if (!sz) continue;
      const [x0, y0, x1, y1] = mediaBoxOf(c, sz);
      if (x >= x0 && x <= x1 && y >= y0 && y <= y1) return { track: tr, clip: c };
    }
  }
  return null;
}
