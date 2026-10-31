// 声（声のエンジン）まわり: 解析・音声の作成・一括作成・再生しながら作る・話者ごとの声と作り直し。
import { call } from "../io/api";
import { voicedLines } from "../logic/voice";
import { changedSpeakers, patchVoice, type SpeakerInfo, type SpeakerVoice } from "../logic/speakervoice";
import { paramFields } from "../logic/voiceengine";
import { MAX_FAILS_IN_ROW, runSynthAll, synthAllNotice, type SynthAllResult } from "../logic/synthall";
import { pickAheadLines } from "../logic/playsynth";
import type { Analysis } from "../types";
import { editScript } from "./script";
import { toast, toastError, useS } from "./store";
import { errorMessage, toApiError } from "../logic/errors.ts";

const get = useS.getState;
const set = useS.setState;

export async function voiceApi<T = any>(path: string, body?: any): Promise<T> {
  return call("voice_api", { path, body: body ?? null });
}

/** 1 行の音声を作る（Rust が解析に入れて並びを配る）。送るのは画面の台本なので、保存を待たなくてよい。台本に無い行は null */
export async function synthLine(lineId: string): Promise<Analysis | null> {
  const s = get();
  if (!s.script) return null;
  set((st) => ({ synthBusy: new Set(st.synthBusy).add(lineId) }));
  try {
    return await call("voice_synth", { script: s.script, lineId });
  } finally {
    set((st) => {
      const b = new Set(st.synthBusy);
      b.delete(lineId);
      return { synthBusy: b };
    });
  }
}

/**
 * まだ今の調整で作っていない行を全部作る（sp を渡すとその話者の行だけ）。終わったら（中止しても）結果を知らせる。
 * 作成中に呼んでも何もしない
 */
export async function synthAll(sp?: string): Promise<SynthAllResult | null> {
  const s = get();
  if (!s.script || s.synthAllRun) return null;
  // 今の調整で作った音声が無い行（並びの audio が付いていない行）
  const fresh = new Set(get().layout.filter((it) => it.audio != null).map((it) => it.line_id));
  const todo = voicedLines(s.script).map((x) => x.l)
    .filter((l) => (sp == null || l.sp === sp) && !fresh.has(l.id));
  const regen = sp != null; // 話者の声を変えたための作り直し（知らせは短く、作る行が無ければ黙る）
  if (regen && !todo.length) return { total: 0, done: 0, failed: 0, stopped: null, left: 0 };
  set({ synthAllRun: { i: 0, n: todo.length, stopping: false } });
  try {
    const r = await runSynthAll(todo, (l) => synthLine(l.id), {
      shouldStop: () => !!get().synthAllRun?.stopping,
      onProgress: (i, n) => set((st) => ({ synthAllRun: { i, n, stopping: !!st.synthAllRun?.stopping } })),
      onError: (l, e) => toastError(`行「${l.text.slice(0, 16)}…」の音声を作成できませんでした`, e),
    });
    const m = synthAllNotice(r);
    if (!regen) toast(m.text, m.kind, r.stopped ? 9000 : undefined);
    // 作り直しの中止は、続けて声を変えたため（すぐ作り直す）なので知らせない
    else if (m.kind === "error") toast(m.text, m.kind, 9000);
    else if (!r.stopped) toast(`${sp} の音声を作り直しました（${r.done} 行）`, "ok");
    return r;
  } finally {
    set({ synthAllRun: null });
  }
}

/** 「音声を一括作成」を中止する。今作っている行が終わったところで止まる */
export function stopSynthAll() {
  const r = get().synthAllRun;
  if (r && !r.stopping) set({ synthAllRun: { ...r, stopping: true } });
}

// ------------------------------------------------------------------ 再生しながら音声を作る（旧#52）
let playSynthRun = false;
/** この再生で作れなかった行（同じ行を繰り返し作らない） */
const playSynthFailed = new Set<string>();
let playSynthFails = 0;
let playSynthGaveUp = false;

/** 再生で声を待つかの判断に使う（読むだけ）: この再生で作れなかった行・再生中の作成をあきらめたか */
export function playSynthSkips(): { failed: ReadonlySet<string>; gaveUp: boolean } {
  return { failed: playSynthFailed, gaveUp: playSynthGaveUp };
}

/** 再生中、再生位置の行とこの先の未作成の行を、近い順に 1 行ずつ作る。止まったら次の行には進まない */
async function runPlaySynth() {
  if (playSynthRun) return;
  playSynthRun = true;
  try {
    while (true) {
      const s = get();
      if (!s.playing || s.synthAllRun || !s.voice?.ok || !s.script || playSynthGaveUp) break;
      const busy = new Set([...s.synthBusy, ...playSynthFailed]);
      const ids = pickAheadLines(s.layout, s.script, s.t, busy);
      if (!ids.length) break;
      const id = ids[0];
      set({ playSynthN: ids.length });
      try {
        if (!(await synthLine(id))) playSynthFailed.add(id);
        playSynthFails = 0;
      } catch (e) {
        playSynthFailed.add(id);
        if (++playSynthFails >= MAX_FAILS_IN_ROW) {
          playSynthGaveUp = true;
          toast(`再生中の音声作成をやめました（声のエンジンのエラーが続いたため）: ${errorMessage(e)}`, "error", 9000);
        }
      }
    }
  } finally {
    playSynthRun = false;
    set({ playSynthN: 0 });
  }
}

// 再生が始まったら裏で作り始める。再生位置が進んだとき・並びが変わったときにも見直す（作成中は何もしない）
useS.subscribe((st, prev) => {
  if (st.playing && !prev.playing) {
    playSynthFailed.clear();
    playSynthFails = 0;
    playSynthGaveUp = false;
  }
  if (!st.playing || playSynthRun) return;
  if (st.playing !== prev.playing || Math.floor(st.t) !== Math.floor(prev.t) || st.layout !== prev.layout || st.voice !== prev.voice || st.synthAllRun !== prev.synthAllRun) {
    runPlaySynth();
  }
});

// ------------------------------------------------------------------ 話者ごとの声（トークトラックの ⚙）
/** 古い中継の知らせ（Rust の analysis::OLD_SERVER と同じ文。話者の一覧の取得が 404 のときに使う） */
export const OLD_SERVER =
  "声の中継（voice_server.py）が古いため、話者の声の設定を使えません。古い中継（台本エディタのフォルダから起動したもの）を閉じてから、AK Studio で中継を再起動してください。";

/** voice_server から、話者ごとのプリセットの声とスタイルの名前を読む */
export async function loadSpeakersInfo() {
  try {
    const r = await voiceApi<{ speakers: Record<string, SpeakerInfo>; range?: Record<string, unknown> }>("/api/speakers");
    set({ speakersInfo: { speakers: r.speakers, range: r.range } });
  } catch (e) {
    const a = toApiError(e);
    set({ speakersInfo: { error: a.code === "not_found" ? OLD_SERVER : a.message } });
  }
}

/** 声を変えてから、その話者の行を作り直し始めるまでの待ち（スライダーを動かしている間は作らない） */
export const REGEN_DELAY = 1500;
let regenTimer: number | undefined;
const regenSps = new Set<string>();
/** 今の「一括作成」が、声を変えたための作り直しか（そうなら、また変えたときに止めてやり直す） */
let regenRunning = false;
/** 作り直しを止めたのが、続けて声を変えたためか（そうなら残りの話者も次の作り直しで作る。Esc の中止なら残さない） */
let regenSuperseded = false;
/** 回を開くたびに増やす（前の回の作り直しを続けない） */
let regenGen = 0;

function scheduleRegen(sp: string) {
  regenSps.add(sp);
  clearTimeout(regenTimer);
  regenTimer = window.setTimeout(runRegen, REGEN_DELAY);
  if (regenRunning) {
    regenSuperseded = true;
    stopSynthAll();
  }
}

async function runRegen() {
  if (get().synthAllRun) {
    // 自分で始めた一括作成は終わるのを待つ（作り直しの作成は scheduleRegen で止めてある）
    regenTimer = window.setTimeout(runRegen, 500);
    return;
  }
  if (!get().voice?.ok) return; // 声のエンジンにつながったら、一括作成で作れる（行は「調整後の音声が未作成」になる）
  const sps = [...regenSps];
  regenSps.clear();
  regenRunning = true;
  regenSuperseded = false;
  const gen = regenGen;
  try {
    for (const [i, sp] of sps.entries()) {
      if (gen !== regenGen) break; // 回が変わった
      const r = await synthAll(sp);
      if (r?.stopped) {
        // 続けて声を変えたために止めたなら、まだ作り直していない話者を残す（すぐ次の作り直しで作る）
        if (regenSuperseded && gen === regenGen) for (const x of sps.slice(i)) regenSps.add(x);
        break;
      }
    }
  } finally {
    regenRunning = false;
    regenSuperseded = false;
  }
}

// 話者の声が変わったら（画面・元に戻す・外のエディタや Claude の書き換え）、少し待ってからその話者の行を作り直す。
// 回を開いた・切り替えたときは作り直さない（作っていない行は「音声を一括作成」で作る）
useS.subscribe((st, prev) => {
  const a = st.script?.speakers, b = prev.script?.speakers;
  if (a === b || !st.script || !prev.script || st.project?.root !== prev.project?.root) return;
  for (const sp of changedSpeakers(a, b)) scheduleRegen(sp);
});

/** 回を開いた・切り替えたときは、前の回の作り直しの予定を捨て、作り直しの途中なら止める */
export function clearRegen() {
  clearTimeout(regenTimer);
  regenTimer = undefined;
  regenSps.clear();
  regenGen++;
  if (regenRunning) stopSynthAll();
}

/**
 * 話者の声を変える（null の項目はプリセットに戻す）。少し待ってから、その話者の行の音声を作り直す。
 * patch を null にすると、その話者の変更を全部消す
 */

export function setSpeakerVoice(sp: string, patch: { [K in keyof SpeakerVoice]?: SpeakerVoice[K] | null } | null) {
  const base = get().speakersInfo?.speakers?.[sp]?.base;
  const range = get().speakersInfo?.range;
  const fields = range ? paramFields(range) : undefined; // 古い中継（range が無い）は VoiSona の項目
  editScript((s) => {
    const all = { ...(s.speakers || {}) };
    const v = patch ? patchVoice(all[sp], base, patch, fields) : null;
    if (v) all[sp] = v;
    else delete all[sp];
    s.speakers = all;
  });
}
