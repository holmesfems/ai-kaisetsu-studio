// アクセント欄で、1つの単語（<word>）をモーラの間で2つに分ける。
// phoneme は「|」でモーラごと、hl は1字が1モーラ、pronunciation はカタカナのモーラ。
// 表記（original・文字）はモーラの割合で分ける（かなだけで字数がモーラ数と同じなら、ちょうどその位置）。

export type WordParts = { phoneme: string; hl: string; pronunciation: string; text: string; original: string };

export const moras = (kana: string) => kana.replace(/[^ァ-ヴー]/g, "").match(/[ァ-ヴー][ャュョァィゥェォヮ]?/g) || [];

/** 左に k モーラ（1..n-1）を残して分けられるか */
export function canSplit(w: WordParts, k: number): boolean {
  const n = w.hl.length;
  return n >= 2 && k >= 1 && k < n && w.phoneme.split("|").length === n && moras(w.pronunciation).length === n;
}

function cutText(s: string, k: number, n: number): [string, string] {
  const cs = [...s];
  if (cs.length < 2) return [s, ""];
  const c = cs.length === n ? k : Math.min(cs.length - 1, Math.max(1, Math.round((cs.length * k) / n)));
  return [cs.slice(0, c).join(""), cs.slice(c).join("")];
}

export function splitWord(w: WordParts, k: number): [WordParts, WordParts] | null {
  if (!canSplit(w, k)) return null;
  const n = w.hl.length;
  const ph = w.phoneme.split("|");
  const ms = moras(w.pronunciation);
  const [t1, t2] = cutText(w.text, k, n);
  const [o1, o2] = w.original === w.text ? [t1, t2] : cutText(w.original, k, n);
  const r1 = ms.slice(k).join("");
  return [
    { phoneme: ph.slice(0, k).join("|"), hl: w.hl.slice(0, k), pronunciation: ms.slice(0, k).join(""), text: t1, original: o1 },
    // 表記が1字で分けられないときは、右側を読みのカタカナで書く
    { phoneme: ph.slice(k).join("|"), hl: w.hl.slice(k), pronunciation: r1, text: t2 || r1, original: o2 || r1 },
  ];
}

/** DOM の <word> を分ける。右側は新しいブロック（chain="0"）として、すぐ後ろに入れる */
export function splitWordEl(w: Element, k: number): Element | null {
  const parts = splitWord(
    {
      phoneme: w.getAttribute("phoneme") || "",
      hl: w.getAttribute("hl") || "",
      pronunciation: w.getAttribute("pronunciation") || "",
      text: w.textContent || "",
      original: w.getAttribute("original") ?? (w.textContent || ""),
    },
    k,
  );
  if (!parts) return null;
  const [a, b] = parts;
  const r = w.cloneNode(true) as Element;
  const put = (el: Element, p: WordParts) => {
    el.setAttribute("phoneme", p.phoneme);
    el.setAttribute("hl", p.hl);
    el.setAttribute("pronunciation", p.pronunciation);
    if (el.hasAttribute("original")) el.setAttribute("original", p.original);
    el.textContent = p.text;
  };
  put(w, a);
  put(r, b);
  r.setAttribute("chain", "0");
  // 活用の情報は語の終わり（右側）のもの
  for (const at of ["cform", "ctype"]) w.removeAttribute(at);
  w.parentNode!.insertBefore(r, w.nextSibling);
  return r;
}
