// チャットに添える画像まわりの小さな部品（ClaudePanel から使い、ui/test/chatimg.test.ts で確かめる）。

/** 画像だけを送るときの本文 */
export const IMG_ONLY_TEXT = "（画像を送りました）";

/** 📎 で選べる拡張子 */
export const IMG_EXTS = ["png", "jpg", "jpeg", "webp", "gif", "bmp"];

/** 貼り付けた画像の MIME から拡張子（対応していなければ null） */
export function mimeExt(mime: string): string | null {
  const m = mime.toLowerCase().replace(/^image\//, "").replace("x-ms-bmp", "bmp");
  const e = m === "jpeg" ? "jpg" : m;
  return IMG_EXTS.includes(e) || e === "jpg" ? e : null;
}

/** 送る本文: 文字が空で画像があるときは決まった文にする */
export function bodyFor(text: string, nImg: number): string {
  const t = text.trim();
  return t === "" && nImg > 0 ? IMG_ONLY_TEXT : t;
}

/** 取り消したメッセージの画像（添付の {image: パス}）のパス */
export function imagePaths(attachments: any[] | undefined): string[] {
  return (attachments ?? []).map((a) => a?.image).filter((p): p is string => typeof p === "string");
}
