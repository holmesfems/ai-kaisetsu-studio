// 選んだ字を一時的な読み（ルビ）にする。｜選んだ字《》の形にして、よみを打つ位置（《 と 》の間）を返す。
/** 選びの中に ｜ 《 》 や改行があるときは付けられない */
export function canRuby(text: string): boolean {
  return text.length > 0 && !/[｜《》\r\n]/.test(text);
}

/** value の [start, end) を ｜{選び}《》 にする。付けられなければ null */
export function wrapRuby(value: string, start: number, end: number): { value: string; caret: number } | null {
  const sel = value.slice(start, end);
  if (!canRuby(sel)) return null;
  const head = value.slice(0, start) + "｜" + sel + "《";
  return { value: head + "》" + value.slice(end), caret: head.length };
}
