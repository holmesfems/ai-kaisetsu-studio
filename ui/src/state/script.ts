// 台本の保存・書き換え（editScript）・セリフの並び（relayout）・台本 → 演出の入力の書き出し。
import { call } from "../io/api";
import { mergeLocalEdits } from "../logic/history";
import type { Script, VoiceItem } from "../types";
import { applyExternal, pushHist, resetHistMerge } from "./history";
import { toast, toastError, useS } from "./store";
import { errorMessage } from "../logic/errors.ts";

const get = useS.getState;
const set = useS.setState;

let saveTimer: number | undefined;
let saving = false;
/** バックエンドが最後に確かめた台本（外の変更が届いたとき、画面の未保存の編集を残して重ねる元） */
let acked: Script | null = null;

/** 外から届いた台本を入れる。画面に未保存の編集があれば、それを重ねて残して保存し直す */
export function receiveScript(label: string, remote: Script) {
  const { value, local } = mergeLocalEdits(remote, acked, get().script);
  acked = remote;
  applyExternal(label, () => set({ script: value }));
  if (local) scheduleSave();
}

export function scheduleSave(delay = 700) {
  clearTimeout(saveTimer);
  saveTimer = window.setTimeout(() => {
    saveTimer = undefined;
    saveNow();
  }, delay);
}

export async function saveNow() {
  const s = get().script;
  if (!s) return;
  if (saving) {
    scheduleSave(300);
    return;
  }
  saving = true;
  try {
    const r = await call("save_script", { script: s });
    // 保存中に打った文字を消さないよう、ファイル名（新しいブロック）と rev だけ受け取る
    const cur = get().script;
    if (r.external) {
      // 外（Claude・台本エディタなど）で変わったファイルは外の中身を残して読み直したので、それを受け取る
      receiveScript("台本（外のエディタ・Claude の直接編集）", r.script);
      toast("外部で変更された台本を読み込みました");
    } else if (cur) {
      const files = new Map<string, string | null>((r.script.blocks as any[]).map((b) => [b.id, b.file]));
      acked = { ...s, rev: r.rev, blocks: s.blocks.map((b) => (b.file ? b : { ...b, file: files.get(b.id) ?? null })) };
      set({
        script: {
          ...cur,
          // 保存中に外の変更（もっと新しい rev）を受け取っていたら、rev を戻さない
          rev: Math.max(cur.rev ?? 0, r.rev),
          blocks: cur.blocks.map((b) => (b.file ? b : { ...b, file: files.get(b.id) ?? null })),
        },
      });
    }
  } catch (e) {
    toastError("台本を保存できませんでした", e);
  } finally {
    saving = false;
  }
}

/**
 * 台本を書き換える（fn の中で直接いじってよい。写しを渡す）。
 * hist が "own" のときは、直前・直後の変更とまとめずに元に戻す 1 回ぶんにする（行の分割・結合など。打った文字と一緒に戻さないように）
 */
export function editScript(fn: (s: Script) => void, hist: boolean | "own" = true) {
  const cur = get().script;
  if (!cur) return;
  if (hist === "own") resetHistMerge();
  if (hist) pushHist();
  const next: Script = structuredClone(cur);
  fn(next);
  set({ script: next });
  if (hist === "own") resetHistMerge();
  scheduleSave();
  relayout();
}

// ------------------------------------------------------------------ セリフの並び
// 並べ方の計算は Rust（voice/layout.rs）。画面は、打った直後・ドラッグ中の仮の並びを Rust に聞いて入れるだけ。
// 保存した台本の並びは Rust が layout イベントで配る（state/index.ts）
let layoutTimer: number | undefined;
/** 仮の並びの問い合わせの世代（古い返事で新しい並びを上書きしない） */
let layoutGen = 0;
/** 仮の並びの失敗を同じ文では 1 回だけ知らせる */
let layoutErr = "";

export function relayout() {
  clearTimeout(layoutTimer);
  layoutTimer = window.setTimeout(doLayout, 60);
}

async function doLayout() {
  const s = get();
  if (!s.script || !s.project) return;
  const gen = ++layoutGen;
  try {
    const r = await call("voice_layout_preview", { script: s.script, gap: s.project.doc.gap, blockGap: s.project.doc.block_gap });
    if (gen !== layoutGen) return;
    layoutErr = "";
    set({ layout: r.items, voiceTotal: r.total, tracksReady: r.ready });
  } catch (e) {
    const msg = errorMessage(e);
    if (msg !== layoutErr) toastError("セリフの並びを計算できませんでした", e);
    layoutErr = msg;
  }
}

/** 画面に、まだ保存していない台本の変更があるか（自動保存の待ち・保存中）。あるとき、本体が配った並びはそのまま入れない */
export function hasUnsavedScript(): boolean {
  return saving || saveTimer !== undefined;
}

/** 本体が配った並びを入れる。画面に保存していない変更があれば、入れずに仮の並びを聞き直す */
export function receiveLayout(l: { items: VoiceItem[]; total: number; ready: boolean }) {
  if (hasUnsavedScript()) {
    relayout();
    return;
  }
  layoutGen++; // 飛んでいる仮の並びの返事を捨てる
  clearTimeout(layoutTimer);
  set({ layout: l.items, voiceTotal: l.total, tracksReady: l.ready });
  bumpBack();
}

export function bumpBack() {
  set((s) => ({ backRev: s.backRev + 1 }));
}

// ------------------------------------------------------------------ 台本 → 演出の入力
let talkBusy = false;

/**
 * files/script.srt と files/talk/ を今の声の並びで書き出す（演出の cue が声に合う）。
 * files/ は見張られているので、演出はそのあと自動で読み直される。
 * 音声の無い行があると Rust 側が断るので、その文をそのまま出す。
 */
export async function exportTalk(): Promise<boolean> {
  if (talkBusy) return false;
  talkBusy = true;
  toast("台本 → 演出の入力を書き出しています…");
  try {
    const r: any = await call("export", { kind: "talk" });
    const lines = ["台本 → 演出の入力を書き出しました"];
    if (r?.srt) lines.push(`${r.srt}（${r.cues} cue）`);
    for (const w of r?.warnings ?? []) lines.push("⚠ " + w);
    toast(lines.join("\n"), r?.warnings?.length ? "info" : "ok", 12000);
    return true;
  } catch (e) {
    toastError("台本 → 演出の入力を書き出せませんでした", e, 12000);
    return false;
  } finally {
    talkBusy = false;
  }
}

/** 保存済みの台本の控えを入れ替える（回を開いたとき。入れた値をそのまま返す） */
export function setAcked(s: Script | null): Script | null {
  return (acked = s);
}
