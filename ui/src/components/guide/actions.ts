// 攻略タブとタイムラインの操作（本体に聞く → 保存 → モニターを動かす）。判断は logic/guide.ts の純粋な関数に寄せてある。
import { call } from "../../io/api";
import { player } from "../../io/player";
import { toast, toastError, useS } from "../../state";
import { addGuideOp, editGuide, editGuideInTask, guideTask } from "../../state/guide";
import type { GuideOp } from "../../gen/GuideOp";
import {
  anchorEdit, anchorHereError, anchorRows, cutFromMarks, manualSpan, opEdit, opGoneMessage, opRows, withAnchorT, withoutOp,
} from "../../logic/guide";

const get = useS.getState;

export { editGuide };

/** モニターをそのアンカーの出力の時刻へ（値を変えた結果がすぐ見える） */
export function seekAnchor(name: string) {
  const out = get().guide?.anchor_out[name];
  if (typeof out === "number") player.seek(out);
}

/** アンカーを素材の 1 コマ（Shift で 10 コマ）動かす。delta は ±1・±10 */
export async function stepAnchor(name: string, delta: number) {
  // 続けて押したら 1 回ずつ進むよう、前の変更が済んでから今の値を読んで動かす
  const flags = { noValue: false };
  const ok = await guideTask(async () => {
    const g = get().guide;
    if (!g?.guide) return false;
    try {
      // 動かす元の値は、保存の食い違いで取り直したときも最新の値から決め直す（古い値からの相対にしない）
      const done = await editGuideInTask(anchorEdit(name, (v) => call("guide_step", { t_src: v, delta }), flags));
      if (flags.noValue) toast("このアンカーはまだ値がありません。素材を解析するか、秒を入力してください", "error");
      return done;
    } catch (e) {
      toastError("コマを動かせませんでした", e);
      return false;
    }
  });
  if (ok) seekAnchor(name);
}

/** モニターの今の位置の素材のコマをアンカーにする。そのアンカーのパートの中でなければ断る */
export async function setAnchorHere(name: string) {
  const g = get().guide?.guide;
  if (!g) return;
  try {
    const at = await call("guide_src_at", { t: get().t });
    const err = anchorHereError(name, at);
    if (err || !at) {
      toast(err ?? "今の位置の素材のコマが分かりません", "error");
      return;
    }
    if (await editGuide((cur) => withAnchorT(cur, name, at.t_src))) seekAnchor(name);
  } catch (e) {
    toastError("今のコマを調べられませんでした", e);
  }
}

/** 操作を選んで、そのコマへモニターを動かす（出力に出ない操作は理由を出す） */
export function seekOp(id: string) {
  const g = get().guide;
  if (!g) return;
  useS.setState({ guideOp: id });
  const row = opRows(g).find((r) => r.id === id);
  if (!row) return;
  if (row.tOut !== null) player.seek(row.tOut);
  else if (row.problem) toast(row.problem, "error");
}

/** 今のモニターの位置に操作を足す（手動パートの中だけ） */
export async function addOpHere() {
  const id = await addGuideOp(get().t);
  if (id) toast("操作を足しました。キャラと置き場所を入れてください");
}

/** 出力の時刻 t に操作を足し、その操作を選んでそこへ移る（タイムラインの右クリックから）。断られたら理由は通知に出る */
export async function addOpAt(t: number) {
  const id = await addGuideOp(t);
  if (!id) return;
  seekOp(id);
  toast("操作を足しました。キャラと置き場所を入れてください");
}

/** 操作 id の項目を直して保存する。ほかで消されていれば何も保存せず通知を出す（false）。patch は最新の操作から決められる */
export async function editOp(id: string, patchOf: Partial<GuideOp> | ((o: GuideOp) => Partial<GuideOp> | Promise<Partial<GuideOp>>)): Promise<boolean> {
  const flags = { gone: false };
  const ok = await editGuide(opEdit(id, typeof patchOf === "function" ? patchOf : () => patchOf, flags));
  if (flags.gone) toast(opGoneMessage(id), "error");
  return ok;
}

/** 操作を消す。ほかで先に消されていれば通知を出す */
export async function removeOp(id: string): Promise<boolean> {
  let gone = false;
  const ok = await editGuide((g) => {
    const r = withoutOp(g, id);
    gone = r === null;
    return r;
  });
  if (gone) toast(opGoneMessage(id), "error");
  return ok;
}

/** 操作を素材の 1 コマ（Shift で 10 コマ）動かす */
export async function stepOp(id: string, delta: number) {
  const flags = { gone: false };
  const ok = await guideTask(async () => {
    try {
      // 動かす元の時刻は、保存の食い違いで取り直したときも最新の操作から決め直す（古い値からの相対にしない）
      const done = await editGuideInTask(opEdit(id, async (o) => ({ t: await call("guide_step", { t_src: o.t, delta }) }), flags));
      if (flags.gone) toast(opGoneMessage(id), "error");
      return done;
    } catch (e) {
      toastError("コマを動かせませんでした", e);
      return false;
    }
  });
  if (ok) seekOp(id);
}

/** 操作の印を出力の時刻 tOut までドラッグして離した: その時刻の素材のコマに合わせて保存。手動パートの外へは動かせない */
export async function dragOpTo(id: string, tOut: number): Promise<boolean> {
  const st = get().guide;
  const g = st?.guide;
  if (!st || !g) return false;
  const span = manualSpan(st);
  if (!span) return false;
  try {
    const at = await call("guide_src_at", { t: Math.min(Math.max(tOut, span[0]), span[1]) });
    if (!at || at.part !== "manual") {
      toast("操作は手動パートの外へは動かせません", "error");
      return false;
    }
    return await editOp(id, { t: at.t_src });
  } catch (e) {
    toastError("操作を動かせませんでした", e);
    return false;
  }
}

/** タイムラインのイン・アウトの範囲をカットにする（素材の時刻にして、両端が手動パートの中でなければ断る） */
export async function addCutFromMarks() {
  const s = get();
  try {
    const a = s.inT === null ? null : await call("guide_src_at", { t: s.inT });
    const b = s.outT === null ? null : await call("guide_src_at", { t: s.outT });
    let error: string | null = null;
    // 足すのは順番待ちの中で、その時点の最新の中身に
    const ok = await editGuide((g) => {
      const r = cutFromMarks(g, s.inT, s.outT, a, b);
      if (!r.ok) {
        error = r.error;
        return null;
      }
      return r.guide;
    });
    if (error) toast(error, "error");
    else if (ok) toast("カットを足しました");
  } catch (e) {
    toastError("カットを足せませんでした", e);
  }
}
