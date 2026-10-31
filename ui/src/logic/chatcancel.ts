// 送り間違いの取り消し（issue 旧#21）の判定。画面（ClaudePanel）から使い、ui/test/chatcancel.test.ts で確かめる。
//
// 送ったメッセージは、設定の「取り消しの猶予」のあいだ AK Studio で待たせてから Claude に渡す（state: queued）。
// その間に取り消せば Claude には届かない。届いた後（delivered）の取り消しは、Claude に「無視して」と伝えるだけ。
import type { ChatMsg } from "../types";

/** 届いた後のメッセージを、キー（Esc / Ctrl+C）で取り消せるのは送ってからこの時間まで（ボタンならいつでも） */
export const LATE_KEY_MS = 60_000;

export const CANCEL_HINT = "取り消し（Esc / Ctrl+C）";

/** アプリが代わりに送るもの（推敲案の結果・取り消しの知らせ）。clip_note などは自分で書いたメッセージ */
const AUTO_KINDS = ["proposal_result", "cancel_notice"];

/** 自分で書いたメッセージか */
export function isOwnMessage(m: ChatMsg): boolean {
  return m.role === "user" && !AUTO_KINDS.includes(m.ctx?.kind);
}

export function isCancellable(m: ChatMsg): boolean {
  return isOwnMessage(m) && (m.state === "queued" || m.state === "delivered");
}

/** 取り消しの対象: 最後に送った自分のメッセージ（もう取り消してあれば無し） */
export function cancelTarget(chat: ChatMsg[]): ChatMsg | null {
  for (let i = chat.length - 1; i >= 0; i--) {
    const m = chat[i];
    if (isOwnMessage(m)) return isCancellable(m) ? m : null;
  }
  return null;
}

export type LastSent = { id: number; at: number } | null;

/**
 * キーで取り消す対象。この画面から送った直後のメッセージだけ。
 * まだ届いていなければ（queued）時間は問わず、届いた後は LATE_KEY_MS まで（うっかり Esc で Claude に知らせが飛ばないように）。
 */
export function shortcutTarget(chat: ChatMsg[], last: LastSent, now: number): ChatMsg | null {
  const m = cancelTarget(chat);
  if (!m || !last || last.id !== m.id) return null;
  if (m.state === "queued") return m;
  return now - last.at <= LATE_KEY_MS ? m : null;
}

export type KeyLike = {
  key: string;
  ctrlKey: boolean;
  metaKey: boolean;
  shiftKey: boolean;
  altKey: boolean;
  isComposing?: boolean;
  keyCode?: number;
};

/**
 * このキーで取り消すか。
 * - Esc: 修飾キーなし。IME の変換中は IME を閉じるだけ（取り消さない）
 * - Ctrl+C: 文字を選んでいない（入力欄にもチャットの履歴にも）ときだけ。選んでいればふつうにコピー
 * - どちらも、入力欄が空のとき（送った直後）だけ。書きかけの文があるときは何もしない
 */
export function isCancelKey(e: KeyLike, selected: string, draft: string): boolean {
  if (e.isComposing || e.keyCode === 229) return false;
  if (draft.trim() !== "") return false;
  if (e.key === "Escape") return !e.ctrlKey && !e.metaKey && !e.shiftKey && !e.altKey;
  const ctrl = e.ctrlKey || e.metaKey;
  if (ctrl && !e.shiftKey && !e.altKey && (e.key === "c" || e.key === "C")) return selected === "";
  return false;
}

/** 取り消した文を入力欄に戻す（書きかけがあれば、その前に置く） */
export function restoreDraft(cancelled: string, draft: string): string {
  return draft.trim() ? `${cancelled}\n${draft}` : cancelled;
}

/** メッセージの下に出す、届いたか・取り消したかの印 */
export function stateNote(m: ChatMsg): string {
  switch (m.state) {
    case "queued":
      return "送信待ち（まだ Claude に届いていません）";
    case "cancelled":
      return "取り消し（Claude には届いていません）";
    case "cancel_late":
      return "取り消し（届いていたため、Claude に「無視して」と伝えました）";
    case "cancel_aborted":
      return "取り消し（届いていたため、Claude の作業を止めました）";
    case "undelivered":
      return "届いていません（Claude に渡す前に AK Studio を閉じました）";
    default:
      return "";
  }
}

/** 届いた後に取り消したとき、Claude に送る知らせの行（まだ渡していなければそう書く） */
export function noticeNote(m: ChatMsg): string {
  return m.state === "queued"
    ? "↩ Claude に「直前のメッセージは無視して」と伝えます（送信待ち）"
    : "↩ Claude に「直前のメッセージは無視して」と伝えました";
}

/**
 * chat-update（届いた・取り消したの印）を履歴に反映する。送ったばかりでまだ手元に無ければ足す。
 * 取り消しの印は、後から来た「送信待ち」「届いた」で上書きしない（渡すのと取り消しが同時だったとき、知らせの順が前後する）
 */
export function mergeChatUpdate(chat: ChatMsg[], m: ChatMsg): ChatMsg[] {
  if (!chat.some((x) => x.id === m.id)) return [...chat, m];
  return chat.map((x) => {
    if (x.id !== m.id) return x;
    if (x.state?.startsWith("cancel") && !m.state?.startsWith("cancel")) return x;
    return m;
  });
}

/** 取り消しボタンの説明 */
export function cancelTitle(m: ChatMsg): string {
  return m.state === "queued"
    ? `${CANCEL_HINT}: まだ Claude に届いていないため、送信を取り消します`
    : `${CANCEL_HINT}: Claude に届いているため、「直前のメッセージは無視して」と伝えます`;
}
