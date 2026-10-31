// モニターの印（Claude への場所つきの指示）。持ち主は Rust（core/marks.rs）で、ここは受け取った全部と画面の操作の状態を持つ。
import { call } from "../io/api";
import type { Mark } from "../gen/Mark";
import type { MarkFile } from "../gen/MarkFile";
import { toastError, useS } from "./store";

const set = useS.setState;
const get = useS.getState;

/** 印の全部を受け取る（イベント marks）。選んでいた印が無くなっていたら選びを外す */
export function receiveMarks(f: MarkFile) {
  set((s) => (s.markSel !== null && !f.marks.some((m) => m.id === s.markSel) ? { marks: f, markSel: null } : { marks: f }));
}

/** 開き直した画面用: いまの印を取る（回が開いていれば本体から来る。失敗は黙らない） */
export async function loadMarks() {
  try {
    receiveMarks(await call("marks_get"));
  } catch (e) {
    toastError("印を読み込めませんでした", e);
  }
}

/** 印の表示の入り切り（localStorage "marks"） */
export function setMarkShow(v: boolean) {
  set({ markShow: v });
  try {
    localStorage.setItem("marks", v ? "1" : "0");
  } catch { /* 保存できなくても、この起動のあいだは効く */ }
}

/** 描くモードの入り切り。入るときは、描いたものが見えるように表示も入れる */
export function setMarkMode(v: boolean) {
  if (v && !get().markShow) setMarkShow(true);
  set(v ? { markMode: true } : { markMode: false, markSel: null, markNoteFor: null });
}

/** 印を選ぶ（タイムライン・チャットから来たときは、直せるように描くモードに入る）。t へ移るのは呼ぶ側 */
export function selectMark(id: number | null, enterMode = false) {
  if (enterMode && id !== null) setMarkMode(true);
  set({ markSel: id });
}

/** 印を足す（id 0）か直す。戻りは保存された印。失敗はトーストを出して null */
export async function upsertMark(mark: Mark): Promise<Mark | null> {
  try {
    return await call("mark_upsert", { mark });
  } catch (e) {
    toastError("印を保存できませんでした", e);
    return null;
  }
}

export async function removeMark(id: number) {
  try {
    await call("mark_remove", { id });
    if (get().markSel === id) set({ markSel: null });
  } catch (e) {
    toastError("印を削除できませんでした", e);
  }
}

export async function setMarkState(id: number, state: "draft" | "sent" | "done") {
  try {
    await call("mark_set_state", { id, state });
  } catch (e) {
    toastError("印の状態を変えられませんでした", e);
  }
}
