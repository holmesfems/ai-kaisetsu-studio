// 失敗の文言の共通の部品（純粋な関数）。
// 本体のコマンドの失敗は ApiError の形で届く（invoke の reject）。画面は各自で文を組まず、ここを通す。
import type { ApiError } from "../gen/ApiError.ts";
import type { ErrorCode } from "../gen/ErrorCode.ts";

/** コードごとの次の操作の案内（無いコードは案内なし） */
export const ERROR_HINT: Partial<Record<ErrorCode, string>> = {
  no_project: "回を開いてから操作してください",
  need_ffmpeg: "設定で ffmpeg の場所を指定してください",
  voice_down: "VoiSona Talk が起動しているか確認してください",
  engine_down: "設定で Python の場所を確認し、演出を読み直してください",
};

// 生成された ErrorCode の全部を数える（Rust にコードを足すと、ここが型エラーになって気づける）
const CODE_TABLE: Record<ErrorCode, true> = {
  failed: true, not_found: true, denied: true, bad_format: true, bad_input: true, no_project: true,
  canceled: true, need_ffmpeg: true, voice_down: true, engine_down: true, partly_saved: true,
};

function isCode(c: unknown): c is ErrorCode {
  return typeof c === "string" && Object.prototype.hasOwnProperty.call(CODE_TABLE, c);
}

/** invoke の reject（ApiError の形・message を持つ object・ただの文字列・Error）を ApiError にそろえる */
export function toApiError(e: unknown): ApiError {
  if (typeof e === "object" && e !== null && !(e instanceof Error)) {
    const o = e as Record<string, unknown>;
    if (typeof o.message === "string") {
      const a: ApiError = {
        code: isCode(o.code) ? o.code : "failed",
        message: o.message,
        detail: typeof o.detail === "string" ? o.detail : o.message,
      };
      if (isCode(o.cause)) a.cause = o.cause;
      return a;
    }
  }
  if (e instanceof Error) return { code: "failed", message: e.message, detail: e.stack ?? e.message };
  const s = typeof e === "string" ? e : String(e);
  return { code: "failed", message: s, detail: s };
}

/** 失敗の理由（message だけ。文字列で比べる所・欄に出す所用） */
export function errorMessage(e: unknown): string {
  return toApiError(e).message;
}

/** 「{what}: {理由}」＋コードごとの案内。what は「台本を保存できませんでした」「回を開けませんでした」のような文の頭
 *  （動詞で「できませんでした」「〜めませんでした」が変わるので、呼ぶ側が書く）。PartlySaved は理由の文がそのまま全体 */
export function failText(what: string, e: unknown): string {
  const a = toApiError(e);
  if (a.code === "partly_saved") {
    // あとの失敗の元の種類（ffmpeg なし・演出ワーカー停止など）の案内を付ける
    const h = a.cause ? ERROR_HINT[a.cause] : undefined;
    return h ? `${a.message.replace(/。$/, "")}。${h}` : a.message;
  }
  const hint = ERROR_HINT[a.code];
  return `${what}: ${hint ? `${a.message.replace(/。$/, "")}。${hint}` : a.message}`;
}
