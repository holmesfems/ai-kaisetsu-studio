import type { MediaClip, Track } from "../types";
import { nid } from "./voice.ts";

/** 素材の拡張子。見本 ui_rules.json の media_exts で縛っている（Rust は render::media の IMAGE_EXTS など） */
export const IMAGE_EXTS = ["png", "jpg", "jpeg", "webp", "bmp"] as const;
export const VIDEO_EXTS = ["mp4", "mov", "mkv", "webm", "avi", "m4v"] as const;
export const AUDIO_EXTS = ["wav", "mp3", "m4a", "aac", "flac", "ogg"] as const;

const extOf = (p: string) => {
  const m = /\.([^.\/]+)$/.exec(p);
  return m ? m[1].toLowerCase() : "";
};
export const isImg = (p: string) => (IMAGE_EXTS as readonly string[]).includes(extOf(p));
export const isAud = (p: string) => (AUDIO_EXTS as readonly string[]).includes(extOf(p));
/** タイムラインの色分け（画像・音声・それ以外は動画。Rust の MediaPool::is_image などと同じ） */
export const mediaClass = (p: string): "img" | "aud" | "vid" => (isAud(p) ? "aud" : isImg(p) ? "img" : "vid");


/** Rust（mediaops::plan）が決めた置き方。Claude の media_add と同じ決まり（DU-1） */
export interface Placement {
  clip: MediaClip;
  kind: Track["kind"];
  nest: string | null;
  track_id: string | null;
  new_track: Track | null;
}

/** 本体の新しいトラック（n は同じ種類の何本目か）。名前と音量は見本 ui_rules.json の tracks.main で縛っている（Rust は mediaops::new_track） */
export function newMainTrack(kind: Track["kind"], n: number, id: string): Track {
  const base = kind === "audio" ? "BGM" : kind === "image" ? "立ち絵" : "録画";
  return { id, kind, name: n > 1 ? `${base} ${n}` : base, clips: [], hidden: false, muted: false, locked: false, volume_db: kind === "audio" ? -18 : 0 };
}

/** 新しいトラックを入れる（top は「ネストか音声」で一番上、ほかは演出のトラックの下）。入れた位置を返す。見本 ui_rules.json の tracks.insert で縛っている（Rust は mediaops::insert_track） */
export function insertTrack(tracks: Track[], tr: Track, top: boolean): number {
  if (top) {
    tracks.push(tr);
    return tracks.length - 1;
  }
  const i = tracks.findIndex((x) => x.kind === "fx");
  const at = i < 0 ? tracks.length : i;
  tracks.splice(at, 0, tr);
  return at;
}

/** 置き方を doc に入れる（Rust の apply_plan と同じ: 新しいトラックは、音声とネストは一番上、ほかは演出の下） */
export function insertPlacement(tracks: Track[], p: Placement): string {
  let tr = p.track_id ? tracks.find((x) => x.id === p.track_id) : undefined;
  if (!tr) {
    tr = structuredClone(p.new_track ?? newMainTrack(p.kind, 1, nid("t")));
    insertTrack(tracks, tr, !!p.nest || tr.kind === "audio");
  }
  tr.clips.push(structuredClone(p.clip));
  return tr.id;
}
