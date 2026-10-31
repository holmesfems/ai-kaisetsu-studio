// 台本の行の欄（textarea）のキーの判定。ScriptPanel から切り出して、node --test で確かめる（旧#49）

export type LineKey = {
  key: string;
  ctrlKey: boolean;
  metaKey?: boolean;
  shiftKey: boolean;
  altKey: boolean;
  /** キーを押しっぱなしにして自動で繰り返された押下 */
  repeat: boolean;
  /** IME で変換中（keyCode 229 も変換中として扱う） */
  isComposing: boolean;
  keyCode?: number;
  selStart: number;
  selEnd: number;
};

export type LineKeyAction = "undo" | "redo" | "mergePrev" | null;

/**
 * 行の欄で、アプリが受けるキーかどうか。
 * - Ctrl+Z / Ctrl+Shift+Z / Ctrl+Y は欄の中の元に戻すではなくアプリの履歴で戻す。
 *   行の結合・分割は欄をまたぐので、欄の元に戻すでは戻らなかった（旧#49）。打った文字もアプリの履歴に積んである
 * - 行頭の Backspace は前の行と結合。ただし押しっぱなしの繰り返しでは結合しない
 *   （文字を消し続けて行頭に着いたあと、上の行まで次々に結合して消していた。旧#49）。変換中も結合しない
 */
export function lineKeyAction(e: LineKey): LineKeyAction {
  const ctrl = e.ctrlKey || !!e.metaKey;
  const composing = e.isComposing || e.keyCode === 229;
  if (ctrl && !e.altKey && !composing) {
    const k = e.key.toLowerCase();
    if (k === "z") return e.shiftKey ? "redo" : "undo";
    if (k === "y") return "redo";
  }
  if (e.key === "Backspace" && !e.repeat && !composing && e.selStart === 0 && e.selEnd === 0) {
    return "mergePrev";
  }
  return null;
}
