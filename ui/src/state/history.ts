// 元に戻す・やり直し・見比べ・外の変更（Claude や外のエディタ）の履歴。
import { call } from "../io/api";
import { deepEq, revert3 } from "../logic/history";
import { stripAuto } from "../logic/overrides";
import type { Override, ProjectDoc, Script } from "../types";
import { relayout, scheduleSave, bumpBack } from "./script";
import { dropPendingDocEdits, sendOverrides } from "./project";
import { toast, toastError, useS } from "./store";

const get = useS.getState;
const set = useS.setState;

export interface Snapshot {
  script: Script | null;
  overrides: Record<string, Override>;
  doc: ProjectDoc | null;
}

/** 元に戻すの 1 回ぶん。after は次の操作か外の変更が来たときに埋まる（まだなら今の状態） */
export interface HistEntry {
  before: Snapshot;
  after: Snapshot | null;
  /** 外の変更を戻した 1 回ぶんなら、その外の変更の id（元に戻す・やり直しで「戻しました」の印を切り替える） */
  extId?: number;
}

/** Claude や外のエディタでの変更（Ctrl+Alt+Z で戻す。ST-6） */
export interface ExtChange {
  id: number;
  at: number;
  label: string;
  before: Snapshot;
  after: Snapshot;
  undone: boolean;
}

// 比べるための微調整の形（実体は logic/overrides.ts。テストできるよう logic に置いた）
export { stripAuto };

function snap(): Snapshot {
  const s = get();
  return { script: s.script, overrides: s.overrides, doc: s.project?.doc ?? null };
}

// 続けざまの変更（数値のドラッグなど）は、元に戻す 1 回ぶんにまとめる
let lastHist = 0;
let histHeld = false;
/**
 * 数値をドラッグしている間は、元に戻すを 1 回ぶんにまとめる（ゆっくり動かしても 600ms で分かれないように）。
 * 始めたときに 1 回だけ積み、離したら次の変更は別の 1 回にする
 */
export function holdHist(on: boolean) {
  if (on && !histHeld) {
    lastHist = 0;
    pushHist();
    histHeld = true;
  } else if (!on) {
    histHeld = false;
    lastHist = 0;
  }
}
export function pushHist() {
  if (histHeld) {
    set({ future: [] });
    return;
  }
  const now = Date.now();
  const merge = now - lastHist < 600;
  lastHist = now;
  if (merge) {
    set({ future: [] });
    return;
  }
  const cur = snap();
  // 前の 1 回ぶんは、ここで終わり（後の状態を覚える）
  set((s) => ({ past: [...closed(s.past, cur).slice(-150), { before: cur, after: null }], future: [] }));
}

function closed(past: HistEntry[], cur: Snapshot): HistEntry[] {
  const last = past[past.length - 1];
  if (!last || last.after) return past;
  return [...past.slice(0, -1), { ...last, after: cur }];
}


/**
 * 今の状態から、after → before の変化だけを取り消して書く（3 方向の戻し）。
 * その後に変わった所は残し、残した数を返す
 */
async function revertChange(after: Snapshot, before: Snapshot): Promise<number> {
  let conflicts = 0;
  // 3 つとも先に今の状態から計算して画面に入れ、それから送る（待っている間に来た変更を古い状態で上書きしないように）
  const s = get();
  let script: Script | null = null;
  if (s.script && after.script && before.script) {
    const r = revert3(s.script, after.script, before.script);
    conflicts += r.conflicts;
    if (!deepEq(r.value, s.script)) script = { ...r.value, rev: s.script.rev };
  }
  const ro = revert3(s.overrides, after.overrides, before.overrides);
  conflicts += ro.conflicts;
  const ov = deepEq(ro.value, s.overrides) ? null : ro.value;
  let doc: ProjectDoc | null = null;
  if (s.project && after.doc && before.doc) {
    const rd = revert3(s.project.doc, after.doc, before.doc);
    conflicts += rd.conflicts;
    if (!deepEq(rd.value, s.project.doc)) doc = rd.value;
  }
  if (script) set({ script });
  if (ov) set({ overrides: ov });
  if (doc && s.project) set({ project: { ...s.project, doc } });
  if (script) scheduleSave();
  if (doc) {
    // 送っていない doc の変更は、今の doc ごと送るので捨てる（後で Claude の doc の上にやり直さないように）。
    // doc を先に送る（overrides の返事を待つ間に来た Claude の doc で、戻した分が消えないように）
    dropPendingDocEdits();
    const d = get().project?.doc ?? doc;
    await call("set_doc", { doc: d });
    bumpBack();
  }
  if (ov) await sendOverrides(() => call("set_overrides", { ov }));
  relayout();
  return conflicts;
}

const keptNote = (n: number) => (n ? `（${n} か所はその後に変わっていたので、そのままにしました）` : "");

/** 自分の操作を元に戻す。その後の Claude や外での変更は戻さない（ST-6） */
export async function undo() {
  const s = get();
  const e = s.past[s.past.length - 1];
  if (!e) return;
  const after = e.after ?? snap();
  lastHist = 0;
  set({ past: s.past.slice(0, -1), future: [...s.future, { ...e, after }] });
  if (e.extId != null) markExt(e.extId, false);
  const n = await revertChange(after, e.before);
  if (n) toast("元に戻しました" + keptNote(n));
}

export async function redo() {
  const s = get();
  const e = s.future[s.future.length - 1];
  if (!e || !e.after) return;
  lastHist = 0;
  set({ future: s.future.slice(0, -1), past: [...s.past, { ...e }] });
  if (e.extId != null) markExt(e.extId, true);
  const n = await revertChange(e.before, e.after);
  if (n) toast("やり直しました" + keptNote(n));
}

let comparing = false;

/**
 * 見比べ（B を押している間）: 一番新しい外の変更より前の姿をモニターだけに出す。データも履歴も変えない（ST-6）。
 * その後の自分の操作は残した姿（今の状態から、その外の変更だけを取り消したもの）
 */
export async function startCompare() {
  if (comparing) return;
  const s = get();
  const e = [...s.ext].reverse().find((x) => !x.undone);
  if (!e || !s.project) {
    toast("見比べられる外の変更はありません");
    return;
  }
  comparing = true;
  const doc = e.after.doc && e.before.doc ? revert3(s.project.doc, e.after.doc, e.before.doc).value : s.project.doc;
  const ov = revert3(s.overrides, e.after.overrides, e.before.overrides).value;
  set({ compare: e.label });
  try {
    await call("preview_state", { doc, ov });
  } catch (err) {
    toastError("見比べを開始できませんでした", err);
  }
  if (!comparing) {
    // 始めている間に離された
    await call("preview_state", { doc: null, ov: null }).catch((e) => toastError("見比べを終了できませんでした", e));
  }
  bumpBack();
}

export async function endCompare() {
  if (!comparing) return;
  comparing = false;
  set({ compare: null });
  try {
    await call("preview_state", { doc: null, ov: null });
  } catch (err) {
    toastError("見比べを終了できませんでした", err);
  }
  bumpBack();
}

let extId = 0;

const markExt = (id: number, undone: boolean) =>
  set((s) => ({ ext: s.ext.map((x) => (x.id === id ? { ...x, undone } : x)) }));

/**
 * Claude や外のエディタでの変更を入れる。前後の写しを外の変更の履歴に残す。
 * 自分の操作の 1 回ぶんはその手前で閉じ（外の変更を自分の操作に混ぜない）、数値のドラッグ中なら続きを新しい 1 回ぶんにする
 */
export function applyExternal(label: string, apply: () => void) {
  const before = snap();
  apply();
  const after = snap();
  if (deepEq(before, after)) return;
  lastHist = 0;
  set((s) => {
    const past = closed(s.past, before);
    return {
      past: histHeld ? [...past, { before: after, after: null }] : past,
      ext: [...s.ext.slice(-49), { id: ++extId, at: Date.now(), label, before, after, undone: false }],
    };
  });
}

/**
 * 外での変更を戻す（Ctrl+Alt+Z。id を省くと、まだ戻していない一番新しいもの）。
 * 戻す操作は自分の操作として積むので、Ctrl+Z で外の変更をもう一度入れられる
 */
export async function undoExternal(id?: number) {
  const s = get();
  const e = id != null ? s.ext.find((x) => x.id === id) : [...s.ext].reverse().find((x) => !x.undone);
  if (!e || e.undone) {
    toast("戻せる外の変更はありません");
    return;
  }
  lastHist = 0;
  pushHist();
  lastHist = 0;
  // この 1 回ぶんは「外の変更 e を戻した」もの（Ctrl+Z で戻すと e の印も戻る）
  set((st) => ({ past: st.past.map((x, i) => (i === st.past.length - 1 ? { ...x, extId: e.id } : x)) }));
  const n = await revertChange(e.after, e.before);
  markExt(e.id, true);
  toast(`「${e.label}」を戻しました` + keptNote(n));
}

/** 続けざまの変更をまとめる判定を切る（行の分割・結合など、直前・直後とまとめたくない操作の前後で呼ぶ） */
export function resetHistMerge() {
  lastHist = 0;
}
