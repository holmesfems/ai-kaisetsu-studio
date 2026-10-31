/** /frame のレスポンスヘッダ X-Info（JSON）の中身。sub は止まっているときだけ、fx_error は演出のコマが失敗したときだけ届く */
export type FrameInfo<S> = { sub: S | null; fxError: string };

/** X-Info を読む。無い・壊れているときは「字幕の情報なし・演出のエラーなし」 */
export function parseFrameInfo<S = unknown>(header: string | null): FrameInfo<S> {
  try {
    const v = JSON.parse(header || "{}");
    return { sub: v?.sub ?? null, fxError: typeof v?.fx_error === "string" ? v.fx_error : "" };
  } catch {
    return { sub: null, fxError: "" };
  }
}

/** 画面の隅に出す短い文（最初の行だけ、長ければ切る） */
export function shortError(s: string, max = 80): string {
  const line = s.split("\n").find((l) => l.trim()) ?? "";
  return line.length > max ? `${line.slice(0, max)}…` : line;
}
