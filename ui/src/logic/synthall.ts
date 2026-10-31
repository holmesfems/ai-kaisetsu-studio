// 「音声を一括作成」の進め方（中止・エラーで止める判定と、終わったときの知らせ）。旧#32
// store.ts から呼ぶ。VoiSona を呼ぶ部分は引数で渡すので、ここだけでテストできる
import type { KeyLike } from "./chatcancel";

/** VoiSona が止まっていると 1 行ごとに長く待つので、この回数続けて失敗したら残りはやめる */
export const MAX_FAILS_IN_ROW = 2;

export type SynthAllResult = {
  /** 作る行の数 */
  total: number;
  /** 作成できた行の数 */
  done: number;
  /** 作成できなかった行の数 */
  failed: number;
  /** 途中で止めた理由（最後まで進めたら null） */
  stopped: "cancel" | "errors" | null;
  /** 止めたときに作成しなかった残りの行の数 */
  left: number;
};

/**
 * 行を 1 行ずつ作る。shouldStop は次の行に進む前に見るので、中止しても今作っている行は最後まで作る（その行の音声は使う）
 */
export async function runSynthAll<T>(
  todo: T[],
  synth: (x: T) => Promise<unknown>,
  opts: {
    shouldStop: () => boolean;
    onProgress?: (i: number, n: number) => void;
    onError?: (x: T, e: unknown) => void;
  },
): Promise<SynthAllResult> {
  const r: SynthAllResult = { total: todo.length, done: 0, failed: 0, stopped: null, left: 0 };
  let fails = 0;
  for (const [i, x] of todo.entries()) {
    if (opts.shouldStop()) {
      r.stopped = "cancel";
      r.left = todo.length - i;
      break;
    }
    opts.onProgress?.(i, todo.length);
    try {
      await synth(x);
      r.done++;
      fails = 0;
    } catch (e) {
      r.failed++;
      opts.onError?.(x, e);
      if (++fails >= MAX_FAILS_IN_ROW && i < todo.length - 1) {
        r.stopped = "errors";
        r.left = todo.length - i - 1;
        break;
      }
    }
  }
  return r;
}

/** 終わったときの知らせ（文と種類） */
export function synthAllNotice(r: SynthAllResult): { text: string; kind: "ok" | "info" | "error" } {
  const failed = r.failed ? `（${r.failed} 行は作成できませんでした）` : "";
  if (r.stopped === "cancel") {
    if (r.done === 0) return { text: `音声を作成する前に中止しました${failed}`, kind: "info" };
    return { text: `${r.done} 行の音声を作成して中止しました${failed}`, kind: "info" };
  }
  if (r.stopped === "errors") {
    return { text: `エラーが続いたため、残りの ${r.left} 行の音声は作成しませんでした`, kind: "error" };
  }
  if (r.failed) return { text: `${r.done} 行の音声を作成しました${failed}`, kind: "error" };
  return { text: "すべての音声を作成しました", kind: "ok" };
}

/**
 * Esc で一括作成を中止するか。文字を打っている欄（Esc は欄の側の操作）・Claude のチャット（送ったメッセージの取り消し）・
 * 録画を差し込む画面（Esc で閉じる）では使わない。IME の変換中は IME に任せる
 */
export function isSynthStopKey(e: KeyLike, where: { typing: boolean; inChat: boolean; nestOpen: boolean }): boolean {
  if (e.key !== "Escape" || e.isComposing || e.keyCode === 229) return false;
  if (e.ctrlKey || e.metaKey || e.shiftKey || e.altKey) return false;
  return !where.typing && !where.inChat && !where.nestOpen;
}
