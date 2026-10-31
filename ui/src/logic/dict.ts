// 読み上げ辞書（voice_dict.json）の決まり。台本エディタの「辞書に登録」と同じ（表記・読み（カタカナ）・アクセント（モーラごとの h / l））

export interface DictEntry {
  surface: string;
  reading: string;
  /** 省くと声のエンジンに任せる */
  accent?: string;
}

const toKata = (s: string) => (s || "").replace(/[ぁ-ゖ]/g, (c) => String.fromCharCode(c.charCodeAt(0) + 0x60));
/** ひらがな・カタカナの読みを、辞書に書くカタカナにする（それ以外の文字は落とす） */
export const cleanReading = (s: string) => toKata(s).replace(/[^ァ-ヴー]/g, "");
export const moras = (kana: string) => cleanReading(kana).match(/[ァ-ヴー][ャュョァィゥェォヮ]?/g) || [];
/** 平板（頭だけ低い） */
export const flatHl = (n: number) => (n ? "l" + "h".repeat(n - 1) : "");

/** 1 語のアクセントは「低 → 高 → 低」だけ。j 番目のモーラをクリックしたときの新しい高低（アクセント欄と同じ決まり） */
export function setMora(arr: string[], j: number) {
  const a = arr.indexOf("h");
  let b = a;
  if (a >= 0) while (b + 1 < arr.length && arr[b + 1] === "h") b++;
  arr = arr.map((_, i) => (a >= 0 && i >= a && i <= b ? "h" : "l"));
  let lo: number, hi: number;
  if (j === 0 && arr[0] !== "h") { lo = 0; hi = 0; }
  else if (arr[j] !== "h") { lo = a < 0 ? j : Math.min(a, j); hi = a < 0 ? j : Math.max(b, j); }
  else if (j === 0 && b === 0) { lo = 1; hi = arr.length - 1; }
  else if (j === a && b > a) { lo = a + 1; hi = b; }
  else { lo = a; hi = j - 1; }
  return arr.map((_, i) => (i >= lo && i <= hi ? "h" : "l"));
}

/** 声のエンジンの解析の語（word の original・pronunciation・hl）から、表記にちょうど重なる語の並びの読みとアクセントを拾う */
export function guessReading(
  words: { orig: string; pron: string; hl: string }[],
  surface: string,
): { reading: string; accent: string } {
  for (let i = 0; i < words.length; i++) {
    let s = "";
    for (let j = i; j < words.length && s.length < surface.length; j++) {
      s += words[j].orig;
      if (s === surface) {
        const ws = words.slice(i, j + 1);
        const reading = ws.map((w) => cleanReading(w.pron)).join("");
        const hl = ws.map((w) => w.hl).join("");
        return { reading, accent: hl.length === moras(reading).length ? hl : "" };
      }
    }
  }
  return { reading: "", accent: "" };
}

/**
 * 保存する項目の accent。辞書は全部のエンジンで共通なので、アクセントを扱えないエンジン（canAccent が false）で
 * 保存しても、既にあるアクセントを落とさない（読みのモーラ数が同じときだけ。変わったら合わなくなるので省く）。
 * 扱えるエンジンでは、アクセントを使う（useAcc）ときだけ acc を入れる
 */
export function savedAccent(
  existing: DictEntry | undefined,
  reading: string,
  o: { canAccent: boolean; useAcc: boolean; acc: string },
): string | undefined {
  if (o.canAccent) return o.useAcc ? o.acc : undefined;
  const old = existing?.accent;
  return old && old.length === moras(reading).length ? old : undefined;
}

/** 登録・編集（表記を変えたときは元の表記の項目と置き換え、元の位置に置く） */
export function upsertEntry(entries: DictEntry[], entry: DictEntry, prevSurface: string): DictEntry[] {
  const at = entries.findIndex((x) => (prevSurface && x.surface === prevSurface) || x.surface === entry.surface);
  const next = entries.filter((x) => x.surface !== entry.surface && !(prevSurface && x.surface === prevSurface));
  next.splice(at < 0 ? next.length : Math.min(at, next.length), 0, entry);
  return next;
}
