// RecWindow（録画の窓）に差し込む録画＝ネスト。Premiere のネストと同じく、自分のキャンバス（既定 1920x1080）の上に
// トラックで素材を並べ、キャンバスの外は捨てて、窓（RecWindow の box）にはめる。合成は Rust の Core::draw_nests。
import { call } from "../io/api";
import { onceKeys, urlFileName } from "../io/notify";
import { splitSrcIn } from "../logic/nestcalc";
import { findClip, free, groupSpan, NEST_DEFAULT, newTrack, windowBoxOf, windowSpan } from "../logic/nest";
import { insertPlacement, isAud, type Placement } from "../logic/media";
import { nid } from "../logic/voice";
import type { Group, MediaClip, Nest, Override } from "../types";
import { editDoc } from "./project";
import { toast, toastError, useS } from "./store";

/** 素材の大きさ・長さ（probe_media の結果を覚える） */
const infoCache = new Map<string, Promise<{ w: number; h: number; dur: number; image: boolean } | null>>();
const probeNotes = onceKeys();
/** 調べられなかった素材を、もう一度調べるまでの間 */
const PROBE_RETRY_MS = 10_000;
export function mediaInfo(src: string) {
  let p = infoCache.get(src);
  if (!p) {
    p = call("probe_media", { path: src })
      .then((r) => ({ w: r.w || 1920, h: r.h || 1080, dur: r.dur || 0, image: !!r.image }))
      .then((v) => {
        probeNotes.forget(src);
        return v;
      })
      .catch((e): null => {
        // 失敗を覚えたままにしない。ただし描画のたびに調べ直さないよう、少し置いてから読み直す
        setTimeout(() => { if (infoCache.get(src) === p) infoCache.delete(src); }, PROBE_RETRY_MS);
        if (probeNotes.first(src)) toastError(`素材（${urlFileName(src)}）を確認できませんでした`, e);
        return null;
      });
    infoCache.set(src, p);
  }
  return p;
}
/** 読み終わっている分だけ（描画中に待てないとき） */
const infoNow = new Map<string, { w: number; h: number; dur: number; image: boolean }>();
export function mediaInfoNow(src: string) {
  const v = infoNow.get(src);
  if (!v) mediaInfo(src).then((x) => { if (x) infoNow.set(src, x); });
  return v;
}

/** 元の窓（演出が読み込まれていれば RecWindow の box、無ければネストに覚えた値） */
export function baseBox(g: Group | undefined, n?: Nest | null): [number, number, number, number] {
  return (g?.rec?.[0]?.box ?? n?.box ?? (useS.getState().desc?.rec_box as any) ?? [...NEST_DEFAULT.box]) as any;
}

/** 微調整（移動・拡大）を入れた後の窓（1920x1080 の座標）。計算は logic/nest.ts の windowBoxOf（Rust の rec_window と見本で縛っている） */
export function windowBox(g: Group | undefined, n: Nest | null | undefined, o?: Override): [number, number, number, number] {
  return windowBoxOf(baseBox(g, n), o);
}

/** ネストを用意する（無ければ作る。窓の位置は演出から覚え直す） */
export function ensureNest(key: string) {
  const st = useS.getState();
  const g = st.desc?.groups.find((x) => x.key === key);
  const cur = st.project?.doc.nests?.[key];
  const box = baseBox(g, cur);
  if (cur && cur.box.every((v, i) => Math.abs(v - box[i]) < 1e-6)) return;
  editDoc((d) => {
    d.nests = d.nests ?? {};
    const n = d.nests[key];
    if (n) n.box = [...box] as any;
    else d.nests[key] = { box: [...box] as any, w: NEST_DEFAULT.w, h: NEST_DEFAULT.h, fit: NEST_DEFAULT.fit, tracks: [newTrack(1), newTrack(2)] };
  }, false);
}

/** ネストを書き換える（元に戻すの履歴つき） */
export function editNest(key: string, fn: (n: Nest) => void, hist = true) {
  editDoc((d) => {
    const n = d.nests?.[key];
    if (n) fn(n);
  }, hist);
}

/** 素材をネストに置く。at を省くと再生位置（窓の区間の中に寄せる）、trackId を省くと空いているトラック。
 * 置き方は Rust（mediaops::plan）が決める（Claude の media_add の nest と同じ） */
export async function addNestMedia(key: string, paths: string[], at?: number, trackId?: string) {
  const st = useS.getState();
  const g = st.desc?.groups.find((x) => x.key === key);
  if (!g) return;
  ensureNest(key);
  const [a, b] = windowSpan(g, st.overrides[key]);
  let t = Math.min(Math.max(at ?? st.t, a), b - 0.1);
  let lastId: string | null = null;
  for (const p of paths) {
    if (isAud(p)) {
      toast(`音声はネストに置けません。本体のタイムラインに置いてください: ${p.split(/[\\/]/).pop()}`, "error");
      continue;
    }
    let pl: Placement;
    try {
      // ensureNest で作ったばかりのネストが見えるよう、画面の今の doc で決める
      const doc = useS.getState().project?.doc;
      if (!doc) return lastId; // 置いている途中に回を閉じた
      pl = await call("media_place", { doc, path: p.replace(/\\/g, "/"), start: t, track: trackId ?? null, nest: key });
    } catch (e) {
      toastError(`素材（${p}）を読み込めませんでした`, e);
      continue;
    }
    editNest(key, (nn) => {
      insertPlacement(nn.tracks, pl);
    });
    lastId = pl.clip.id;
    t = Math.min(b - 0.1, pl.clip.start + pl.clip.dur);
  }
  return lastId;
}

/** 複製（同じトラックの直後。空いていなければ上のトラック） */
export function duplicateClip(key: string, id: string): string | null {
  const n = useS.getState().project?.doc.nests?.[key];
  const f = findClip(n, id);
  if (!f) return null;
  const nc: MediaClip = { ...f.c, id: nid("n"), start: Math.round((f.c.start + f.c.dur) * 1000) / 1000 };
  editNest(key, (nn) => {
    const ti = nn.tracks.findIndex((x) => x.id === f.tr.id);
    let tr = [nn.tracks[ti], ...nn.tracks.slice(ti + 1)].find((x) => x && free(x, nc.start, nc.start + nc.dur));
    if (!tr) {
      tr = newTrack(nn.tracks.length + 1);
      nn.tracks.push(tr);
    }
    tr.clips.push(nc);
  });
  return nc.id;
}

/** 再生位置で 2 つに分ける（後ろ半分の id を返す） */
export function splitClip(key: string, id: string, t: number): string | null {
  const n = useS.getState().project?.doc.nests?.[key];
  const f = findClip(n, id);
  if (!f || t <= f.c.start + 0.02 || t >= f.c.start + f.c.dur - 0.02) return null;
  const cut = Math.round(t * 1000) / 1000;
  const right: MediaClip = { ...f.c, id: nid("n"), start: cut, dur: Math.round((f.c.start + f.c.dur - cut) * 1000) / 1000,
    src_in: splitSrcIn(f.c, cut), fade_in: 0, trans_in: undefined };
  editNest(key, (nn) => {
    const tr = nn.tracks.find((x) => x.id === f.tr.id)!;
    const c = tr.clips.find((x) => x.id === id)!;
    c.dur = Math.round((cut - c.start) * 1000) / 1000;
    c.fade_out = 0;
    delete c.trans_out;
    tr.clips.push(right);
  });
  return right.id;
}

export function deleteClip(key: string, id: string) {
  editNest(key, (nn) => {
    for (const tr of nn.tracks) tr.clips = tr.clips.filter((x) => x.id !== id);
  });
}
