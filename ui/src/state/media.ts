import { call } from "../io/api";
import { insertPlacement, insertTrack, newMainTrack, type Placement } from "../logic/media";
import { nid } from "../logic/voice";
import type { Track } from "../types";
import { editDoc } from "./project";
import { toastError, useS } from "./store";

/** 素材をタイムラインに置く。track を省くと種類に合ったトラック */
export async function addMedia(paths: string[], at?: number, trackId?: string) {
  const st = useS.getState();
  if (!st.project) return;
  let t = at ?? st.t;
  let last: { track: string; clip: string } | null = null;
  for (const p of paths) {
    let pl: Placement;
    try {
      // 置き方は画面の今の doc で決める（Rust の doc は 80ms 遅れるので、作ったばかりのトラックが見えない）
      const doc = useS.getState().project?.doc;
      if (!doc) return; // 置いている途中に回を閉じた
      pl = await call("media_place", { doc, path: p, start: t, track: trackId ?? null, nest: null });
    } catch (e) {
      toastError(`素材（${p}）を読み込めませんでした`, e);
      continue;
    }
    let track = "";
    editDoc((d) => {
      track = insertPlacement(d.tracks, pl);
    });
    last = { track, clip: pl.clip.id };
    if (pl.kind !== "image") t += pl.clip.dur;
  }
  if (last) useS.setState({ selection: { kind: "media", track: last.track, clip: last.clip } });
}

export function addTrack(kind: Track["kind"]) {
  editDoc((d) => {
    const n = d.tracks.filter((x) => x.kind === kind).length + 1;
    insertTrack(d.tracks, newMainTrack(kind, n, nid("t")), kind === "audio");
  });
}
