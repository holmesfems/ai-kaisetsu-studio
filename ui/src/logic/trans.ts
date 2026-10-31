// 素材クリップ（立ち絵・画像・録画）の入り・抜けのトランジション（issue 旧#10）。
// 絵の計算は Rust（src-tauri/src/trans.rs）がプレビューと mp4 の両方でする。ここは画面で設定するための小さな道具だけ。
import type { MediaClip, Transition } from "../types";
import { clipSpeed } from "./nestcalc.ts";
import { isImg } from "./media.ts";

export const TRANS_DEFAULT_DUR = 0.35;

export const TRANS_KINDS: { k: string; label: string; help: string }[] = [
  { k: "fade", label: "フェード", help: "なめらかに現れ、消えます" },
  { k: "slide", label: "スライド", help: "指定した向きから入り、向きへ去ります（160px）" },
  { k: "pop", label: "ポップ", help: "小さく弾んで現れます（演出の enter=pop と同じ）" },
  { k: "slam", label: "スラム", help: "大きく叩きつけて現れます（演出の enter=slam と同じ）" },
  { k: "wipe", label: "ワイプ（走査線）", help: "指定した向きの辺から走査線で開き、閉じます（TitleCard と同じ）" },
  { k: "glitch", label: "グリッチ", help: "横ずれと色ずれ（演出の glitch と同じ）" },
];

export const TRANS_DIRS: { k: string; label: string }[] = [
  { k: "left", label: "左" },
  { k: "right", label: "右" },
  { k: "up", label: "上" },
  { k: "down", label: "下" },
];

/** 逆の向き */
const TRANS_OPPOSITE: Record<string, string> = { left: "right", right: "left", up: "down", down: "up" };

/** 向きを使う種類か */
export const transHasDir = (kind?: string) => kind === "slide" || kind === "wipe";

const r3 = (v: number) => Math.round(v * 1000) / 1000;

/** 実際に掛かる長さ（無しなら 0、クリップより長くはしない）。見本 ui_rules.json の trans_dur で縛っている（ui/test/rules.test.ts。Rust は trans::trans_dur） */
export function transDur(tr: Transition | undefined | null, clipDur: number): number {
  if (!tr || !tr.kind || tr.kind === "none") return 0;
  const d = tr.dur > 0 ? tr.dur : TRANS_DEFAULT_DUR;
  return Math.min(d, Math.max(0, clipDur));
}

/** 入り（in）か抜け（out）のトランジションを変える。kind を "" にすると外す */
export function setTrans(c: MediaClip, side: "in" | "out", patch: Partial<Transition>) {
  const key = side === "in" ? "trans_in" : "trans_out";
  const cur: Transition = c[key] ?? { kind: "", dur: TRANS_DEFAULT_DUR, dir: "" };
  const next: Transition = { ...cur, ...patch };
  if (!next.kind || next.kind === "none") delete c[key];
  else c[key] = { kind: next.kind, dur: r3(Math.max(0, next.dur || 0)), dir: next.dir || "" };
}

/** 同じトラックで c の前にあるクリップ（c より前に始まり、終わりが一番遅いもの） */
export function prevClip(clips: MediaClip[], c: MediaClip): MediaClip | null {
  let best: MediaClip | null = null;
  for (const x of clips) {
    if (x.id === c.id || x.start >= c.start) continue;
    if (!best || x.start + x.dur > best.start + best.dur) best = x;
  }
  return best;
}

/**
 * 前のクリップからの切り替え: 前のクリップと id のクリップを、切れ目を中心に dur 秒だけ重ね、
 * 前の抜けと自分の入りに同じ種類・長さのトランジションを付ける（Premiere のクロスディゾルブを切れ目の中央に置くのと同じ）。
 * slide / wipe では、前の抜けの向きを自分の入りの逆にする。
 * 切れ目は、離れているか接していれば自分の開始、もう重なっていれば重なりの中央。
 * 自分が前のクリップより手前に描かれるように、トラックの中の順番も前のクリップの後ろにする。
 * clips を書き換え、前のクリップを返す（無ければ null で何もしない）。
 */
export function linkSwitch(clips: MediaClip[], id: string, kind: string, dur = TRANS_DEFAULT_DUR): MediaClip | null {
  const c = clips.find((x) => x.id === id);
  if (!c) return null;
  const prev = prevClip(clips, c);
  if (!prev) return null;
  const end = prev.start + prev.dur;
  // 前のクリップが自分の終わりより先まで続いていても、切れ目は自分の中ほどまで
  const cut = end > c.start ? Math.min((c.start + end) / 2, c.start + c.dur / 2) : c.start;
  // 重なりは切れ目の前後に半分ずつ（どちらのクリップからもはみ出さない長さに）
  const d = Math.max(0.05, Math.min(dur > 0 ? dur : TRANS_DEFAULT_DUR, 2 * (c.start + c.dur - cut), 2 * (cut - prev.start)));
  const ns = r3(Math.max(0, cut - d / 2));
  const shift = c.start - ns;
  if (!isImg(c.src)) c.src_in = r3(Math.max(0, c.src_in - shift * clipSpeed(c)));
  c.dur = r3(c.dur + shift);
  c.start = ns;
  prev.dur = r3(ns + d - prev.start);
  setTrans(c, "in", { kind, dur: r3(d) });
  // 向きのある種類は、前のクリップを逆の向きに抜く（ワイプなら同じ走査線で入れ替わり、スライドなら押し出す）。
  // 同じ向きのままだと、ワイプでは前のクリップが先に消えて背景が見えてしまう
  const out: Partial<Transition> = { kind, dur: r3(d) };
  if (transHasDir(kind)) out.dir = TRANS_OPPOSITE[c.trans_in?.dir || "left"] ?? "right";
  setTrans(prev, "out", out);
  // 描く順（後ろほど手前）
  const ip = clips.indexOf(prev);
  const ic = clips.indexOf(c);
  if (ic < ip) {
    clips.splice(ic, 1);
    clips.splice(clips.indexOf(prev) + 1, 0, c);
  }
  return prev;
}
