// 演出の微調整（overrides）・回のデータ（editDoc）・回の読み込み・効果音の設定。
import type { Snapshot } from "../gen/Snapshot";
import { call } from "../io/api";
import { deepEq } from "../logic/history";
import { mergeOverride } from "../logic/overrides";
import type { Override, ProjectDoc, SeBank } from "../types";
import { applyExternal, pushHist, stripAuto } from "./history";
import { bumpBack, receiveLayout, relayout, setAcked } from "./script";
import { refreshGuide } from "./guide";
import { clearRegen } from "./voice";
import { toastError, useS } from "./store";

const get = useS.getState;
const set = useS.setState;

export async function patchOverride(key: string, patch: Partial<Override> | null, hist = true) {
  if (hist) pushHist();
  set({ overrides: mergeOverride(get().overrides, key, patch) });
  try {
    await sendOverrides(() => call("patch_override", { key, patch }));
  } catch (e) {
    toastError("調整を保存できませんでした", e);
  }
}

/**
 * 微調整を Rust に送る。返事を待っている間に届く "overrides" は、自分が送ったものの折り返しなので外の変更に数えず、
 * 全部の返事が来たところで最後に届いたものを確かめる（その時点で画面と違えば、Claude などの外の変更）
 */
let ovInFlight = 0;
let ovHeld: Record<string, Override> | null = null;
/** 最近送った微調整（比べる形）。返事と折り返しの届く順番は決まっていないので、古い折り返しを見分けるのに使う */
let ovSent: Record<string, any>[] = [];
export async function sendOverrides<T>(send: () => Promise<T>): Promise<T> {
  ovSent = [...ovSent.slice(-19), stripAuto(get().overrides)];
  ovInFlight++;
  try {
    return await send();
  } finally {
    ovInFlight--;
    if (ovInFlight === 0 && ovHeld) {
      const o = ovHeld;
      ovHeld = null;
      onOverrides(o);
    }
  }
}

export function onOverrides(o: Record<string, Override>) {
  if (ovInFlight > 0) {
    ovHeld = o;
    return;
  }
  // 画面の変更の折り返し（と、自動で付く録画の窓の印 no_placeholder だけの違い）は外の変更として数えない
  const so = stripAuto(o);
  if (deepEq(so, stripAuto(get().overrides))) set({ overrides: o });
  // 自分が前に送ったものの、遅れて届いた折り返し: 画面はもう先にあるので何もしない
  else if (ovSent.some((x) => deepEq(x, so))) return;
  else applyExternal("演出の調整（Claude・外での変更）", () => set({ overrides: o }));
}

// ------------------------------------------------------------------ 回のデータ（トラック）
let docTimer: number | undefined;
/** まだ Rust に送っていない変更。送る前に Claude から新しい doc が来たら、その上にやり直す */
let pendingDocEdits: ((d: ProjectDoc) => void)[] = [];

export function editDoc(fn: (d: ProjectDoc) => void, hist = true) {
  const p = get().project;
  if (!p) return;
  if (hist) pushHist();
  const doc = structuredClone(p.doc);
  fn(doc);
  set({ project: { ...p, doc } });
  pendingDocEdits.push(fn);
  clearTimeout(docTimer);
  // 送るのは発火した時点の doc（作った時点の doc を送ると、その間に来た Claude の変更を消してしまう）
  docTimer = window.setTimeout(() => {
    pendingDocEdits = [];
    const d = get().project?.doc;
    if (d) call("set_doc", { doc: d }).then(bumpBack, (e) => toastError("保存できませんでした", e));
  }, 80);
  relayout();
}

// ------------------------------------------------------------------ 読み込み
export function applySnapshot(v: Snapshot) {
  clearRegen();
  set({
    settings: v.settings ?? null,
    voisonaPassSet: v.voisona_pass_set ?? false,
    project: v.project ?? null,
    script: setAcked(v.script ?? null),
    desc: v.desc ?? null,
    overrides: v.overrides ?? {},
    guiClips: v.gui_clips ?? [],
    engine: v.engine ?? null,
    guide: null,
    guideOp: null,
    chat: v.chat ?? [],
    proposals: v.proposals ?? [],
    claude: v.claude ?? get().claude,
    // セリフの並びと声の解析は本体が持つ（以降の変わった分は layout・analysis イベント）
    analysis: v.voice?.analysis ?? {},
    layout: v.voice?.layout.items ?? [],
    voiceTotal: v.voice?.layout.total ?? 0,
    tracksReady: v.voice?.layout.ready ?? false,
    selection: null,
    selLines: [],
    nestKey: null,
    subCheck: null,
    subView: null,
    subViewErr: null,
    // 効果音の設定（表示名を含む）は回ごと。前の回のものを使わないよう外し、タイムラインが読み直す
    seBank: null,
    past: [],
    future: [],
    ext: [],
  });
  refreshScenes();
  relayout();
  refreshVoiceState(v.project?.root ?? null);
  refreshGuide(v.project?.root ?? null);
}

/**
 * 本体の今の並びと解析を取り直す。回を開く間に届いた layout・analysis は、あとから入れるスナップショットに上書きされるので、
 * 入れた後にもう一度聞く。聞いている間に別の回を開いたら捨てる
 */
async function refreshVoiceState(root: string | null) {
  if (!root) return;
  try {
    const v = await call("voice_state");
    if (get().project?.root !== root) return;
    set((s) => ({ analysis: { ...s.analysis, ...v.analysis } }));
    receiveLayout(v.layout);
  } catch {
    /* 無視（次の layout イベントで追いつく） */
  }
}

export async function refreshScenes() {
  try {
    set({ scenes: await call("scenes") });
  } catch {
    /* 無視 */
  }
}

export async function openProject(path: string) {
  try {
    const v = await call("open_project", { path });
    applySnapshot(v);
    set({ t: 0 });
  } catch (e) {
    toastError("回を開けませんでした", e, 8000);
  }
}

/** 効果音の設定を読み直す */
export async function refreshSeBank(): Promise<SeBank | null> {
  try {
    const b = await call("se_bank");
    set({ seBank: b });
    return b;
  } catch (e) {
    toastError("効果音の設定を読み込めませんでした", e, 8000);
    return null;
  }
}

/** 送っていない画面の変更を捨てる（今の doc ごと送るので、後で Claude の doc の上にやり直さないように） */
export function dropPendingDocEdits() {
  clearTimeout(docTimer);
  pendingDocEdits = [];
}

/** 送っていない画面の変更を、新しい doc の上にやり直す（直す相手が消えていたら、その変更は捨てる） */
export function replayPendingDocEdits(d: ProjectDoc) {
  for (const fn of pendingDocEdits) {
    try {
      fn(d);
    } catch {
      // 直す相手のクリップが Claude の変更で消えていたら、その変更は捨てる
    }
  }
}
