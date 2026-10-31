import { create } from "zustand";
import type {
  Analysis, ChatMsg, Desc, EngineStatus, EngineWarning, FontInfo, Override, Proposal, ProjectDoc, Script, SeBank, Selection, Settings,
  SubCheck, SubView, VoiceItem,
} from "../types";
import type { Scene } from "../gen/Scene";
import type { ClaudeInfo } from "../gen/ClaudeInfo";
import type { EngineInfo } from "../gen/EngineInfo";
import type { GuideState } from "../gen/GuideState";
import type { ProjectInfo } from "../gen/ProjectInfo";
import type { MarkFile } from "../gen/MarkFile";
import type { MarkTool } from "../logic/marks";
import type { ExtChange, HistEntry } from "./history";
import type { SpeakerInfo } from "../logic/speakervoice";
import { failText, toApiError } from "../logic/errors";

// 画面の状態の本体。型・唯一の useS（初期値すべて）・toast・total・lastFrameT だけを持つ。
// 動かす処理は slice（history / script / voice / project / chat）に分けてあり、ここからは型だけを import する
// （useS を作る create は読み込まれた時点で走るので、値の import で循環させない）。

export interface Progress {
  task: string;
  done: number;
  total: number;
  msg?: string;
  finished?: boolean;
  /** 先読みで描けなかったコマの数と、その知らせの文（Rust がトーストでも出す） */
  failed?: number;
  error?: string;
}

export interface Toast {
  id: number;
  text: string;
  kind?: "info" | "error" | "ok";
  /** 元の連鎖（ツールチップ用） */
  detail?: string;
}

export interface State {
  settings: Settings | null;
  /** VoiSona のパスワードが設定済みか（中身は画面に来ない） */
  voisonaPassSet: boolean;
  project: ProjectInfo | null;
  script: Script | null;
  analysis: Record<string, Analysis>;
  tracksReady: boolean;
  desc: Desc | null;
  overrides: Record<string, Override>;
  guiClips: any[];
  engine: EngineInfo | null;
  /** 攻略動画（studio/guide.json）の状態。まだ取っていない間は null。guide.guide が null なら攻略動画にしていない回 */
  guide: GuideState | null;
  /** モニターの印（studio/marks.json。Rust が持ち、変わるたびにイベント marks で丸ごと届く） */
  marks: MarkFile;
  /** 選んでいる印の番号 */
  markSel: number | null;
  /** 描いた直後で、一言の入力欄を出している印の番号 */
  markNoteFor: number | null;
  /** 描くモード（モニターのクリック・ドラッグで印を描く） */
  markMode: boolean;
  markTool: MarkTool;
  markColor: string;
  /** 印を表示するか（localStorage "marks"。既定は表示） */
  markShow: boolean;
  /** 済の印も表示するか */
  markShowDone: boolean;
  /** 攻略タブとタイムラインで選んでいる操作の id（guide.json の ops[].id） */
  guideOp: string | null;
  scenes: Scene[];
  chat: ChatMsg[];
  proposals: Proposal[];
  claude: ClaudeInfo;
  voice: { ok: boolean; error?: string; info?: any } | null;
  /** 話者ごとの声の元（voice_server の /api/speakers。読めなかったら error） */
  speakersInfo: { speakers?: Record<string, SpeakerInfo>; /** 話者の声の項目と範囲（エンジンが決める） */ range?: Record<string, unknown>; /** 話者の声を保存するファイル名（エンジンごと。script/ の下） */ file?: string; error?: string } | null;
  layout: VoiceItem[];
  voiceTotal: number;
  // 再生
  t: number;
  playing: boolean;
  scale: number;
  inT: number | null;
  outT: number | null;
  // 表示
  zoom: number;
  selection: Selection;
  selLines: string[];
  progress: Record<string, Progress>;
  toasts: Toast[];
  cacheRanges: [number, number, number][];
  focusLine: string | null;
  synthBusy: Set<string>;
  /** 「音声を一括作成」の進み具合（作っていなければ null）。i 行目を作成中、全部で n 行。stopping は中止を押した後 */
  synthAllRun: { i: number; n: number; stopping: boolean } | null;
  /** 再生しながら裏で作っている行の数（作っていなければ 0） */
  playSynthN: number;
  /** 再生が準備できていない区間の手前で待っているときの帯の文（待っていなければ null）。旧#83 */
  playWait: { text: string } | null;
  /** 録画を差し込む画面（ネスト）を開いている RecWindow のキー */
  nestKey: string | null;
  // 字幕（行の分け方・はみ出し・孤立した設定）と、入っているフォント
  subCheck: SubCheck | null;
  /** 字幕の見た目・区間・鍵（Rust の subs_view の結果。重ね方は画面で計算しない） */
  subView: SubView | null;
  /** subs_view が失敗した理由（失敗している間は subView を null にして、古い見た目・鍵で書かないようにする） */
  subViewErr: string | null;
  fonts: FontInfo[] | null;
  /** 効果音の設定（fx/se/bank.json。効果音の設定の画面を開いたときに読む） */
  seBank: SeBank | null;
  /** Rust 側に回のデータ・セリフの並びが届くたびに増える（字幕はあちらで描くので、届いてから描き直す） */
  backRev: number;
  /** 見比べ中（B を押している間）なら、比べている外の変更の名前 */
  compare: string | null;
  // 元に戻す
  past: HistEntry[];
  future: HistEntry[];
  /** 外での変更の履歴（新しいものが後ろ） */
  ext: ExtChange[];
}

/** 印の表示の入り切り（localStorage "marks"。読めなければ表示） */
function loadMarkShow(): boolean {
  try {
    return localStorage.getItem("marks") !== "0";
  } catch {
    return true;
  }
}

export const useS = create<State>(() => ({
  settings: null,
  voisonaPassSet: false,
  project: null,
  script: null,
  analysis: {},
  tracksReady: false,
  desc: null,
  overrides: {},
  guiClips: [],
  engine: null,
  guide: null,
  guideOp: null,
  marks: { next: 1, marks: [] },
  markSel: null,
  markNoteFor: null,
  markMode: false,
  markTool: "rect",
  markColor: "red",
  markShow: loadMarkShow(),
  markShowDone: false,
  scenes: [],
  chat: [],
  proposals: [],
  claude: { activity: { working: false, text: "", since_ms: 0, updated_ms: 0 }, connected: false, pending: 0, watching: false, delivery: "mod", status: "", port: 0, api_error: null },
  voice: null,
  speakersInfo: null,
  layout: [],
  voiceTotal: 0,
  t: 0,
  playing: false,
  scale: 0.5,
  inT: null,
  outT: null,
  zoom: 40,
  selection: null,
  selLines: [],
  progress: {},
  toasts: [],
  cacheRanges: [],
  focusLine: null,
  synthBusy: new Set(),
  synthAllRun: null,
  playSynthN: 0,
  playWait: null,
  nestKey: null,
  subCheck: null,
  subView: null,
  subViewErr: null,
  fonts: null,
  seBank: null,
  backRev: 0,
  compare: null,
  past: [],
  future: [],
  ext: [],
}));

const get = useS.getState;
const set = useS.setState;

let toastId = 0;
export function toast(text: string, kind: Toast["kind"] = "info", ms = 4200, detail?: string) {
  const id = ++toastId;
  set((s) => ({ toasts: [...s.toasts, { id, text, kind, detail }] }));
  setTimeout(() => set((s) => ({ toasts: s.toasts.filter((x) => x.id !== id) })), ms);
}

/** 失敗のトースト（「{what}: 理由」＋案内。what は「台本を保存できませんでした」のような文の頭）。元の連鎖は console とツールチップに出す */
export function toastError(what: string, e: unknown, ms?: number) {
  const a = toApiError(e);
  console.error(`${what}: ${a.detail}`);
  toast(failText(what, a), "error", ms, a.detail);
}

/** 画面の fps。演出があればその fps、無くて攻略動画のパートがあれば 60、どちらも無ければ 30 */
export const fpsOf = (s: Pick<State, "desc" | "guide">) => s.desc?.fps ?? (s.guide?.plan_parts.length ? 60 : 30);

export const total = () => {
  const s = get();
  let m = Math.max(s.desc?.end ?? 0, s.voiceTotal + 1, s.guide?.plan_parts.at(-1)?.t1 ?? 0);
  for (const tr of s.project?.doc.tracks ?? []) {
    for (const c of tr.clips) if (!c.loop) m = Math.max(m, c.start + c.dur);
  }
  return Math.max(10, m);
};

/**
 * 最後のコマの頭の時刻。total() は「ここで終わる」時刻で、そこではどのクリップも終わっているので、
 * 終わりへのシーク・再生の止まる位置はこちらにする（書き出しの最後のコマと同じ。旧#75）
 */
export const lastFrameT = () => {
  const fps = fpsOf(get());
  return Math.max(0, (Math.ceil(total() * fps - 1e-6) - 1) / fps);
};
