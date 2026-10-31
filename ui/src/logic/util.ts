/** 文字を打たない input（スライドバー・チェックボックス・色など）。ここにフォーカスがあってもショートカットは効かせる */
const NON_TEXT_INPUTS = new Set(["range", "checkbox", "radio", "color", "button", "submit", "reset", "file", "image"]);

type FocusLike = { tagName: string; type?: string; isContentEditable?: boolean } | null | undefined;

/** フォーム部品（input・textarea・select・編集できる要素）か */
export function isFormControl(el: FocusLike): boolean {
  return !!el && (el.tagName === "INPUT" || el.tagName === "TEXTAREA" || el.tagName === "SELECT" || !!el.isContentEditable);
}

/** 文字を入力中の欄か（Ctrl+Z などを欄の側に任せるべきか）。スライドバー・チェックボックスなどは含めない */
export function isTextEntry(el: FocusLike): boolean {
  if (!el) return false;
  if (el.tagName === "INPUT") return !NON_TEXT_INPUTS.has((el.type || "text").toLowerCase());
  return el.tagName === "TEXTAREA" || !!el.isContentEditable;
}

/** フォーム部品に残っているフォーカスを外す（モニターでのドラッグなど、preventDefault でフォーカスが移らない操作の前に） */
export function releaseFocus() {
  const a = document.activeElement as HTMLElement | null;
  if (isFormControl(a)) a!.blur();
}

export const clamp = (v: number, a: number, b: number) => Math.max(a, Math.min(b, v));

/** 分:秒.フレーム（30fps） */
export function tc(t: number, fps = 30): string {
  const f = Math.round(t * fps);
  const m = Math.floor(f / (60 * fps));
  const s = Math.floor(f / fps) % 60;
  const ff = f % fps;
  return `${m}:${String(s).padStart(2, "0")}.${String(ff).padStart(2, "0")}`;
}

/** 分:秒.1 桁。先に 0.1 秒に丸めてから分と秒に分ける（59.97 秒が 0:60.0 にならないように） */
export function mmss(t: number): string {
  const d = Math.round(Math.max(0, t) * 10);
  const m = Math.floor(d / 600);
  const s = (d % 600) / 10;
  return `${m}:${s.toFixed(1).padStart(4, "0")}`;
}

/** Premiere のタイムコード（HH:MM:SS:FF） */
export function premiereTc(t: number, fps: number): string {
  const f = Math.round(t * fps);
  const p = (n: number) => String(n).padStart(2, "0");
  return `${p(Math.floor(f / (3600 * fps)))}:${p(Math.floor(f / (60 * fps)) % 60)}:${p(Math.floor(f / fps) % 60)}:${p(f % fps)}`;
}

export function useLocal<T>(key: string, init: T): [() => T, (v: T) => void] {
  const read = () => {
    try {
      const v = localStorage.getItem(key);
      return v == null ? init : (JSON.parse(v) as T);
    } catch {
      return init;
    }
  };
  const write = (v: T) => {
    try {
      localStorage.setItem(key, JSON.stringify(v));
    } catch { /* 無視 */ }
  };
  return [read, write];
}

/** 文字単位の差分（推敲案の表示用。短い文なので素朴な LCS で十分） */
export function charDiff(a: string, b: string): { t: "eq" | "del" | "ins"; s: string }[] {
  const A = [...a];
  const B = [...b];
  const n = A.length;
  const m = B.length;
  const dp: number[][] = Array.from({ length: n + 1 }, () => new Array(m + 1).fill(0));
  for (let i = n - 1; i >= 0; i--) for (let j = m - 1; j >= 0; j--) dp[i][j] = A[i] === B[j] ? dp[i + 1][j + 1] + 1 : Math.max(dp[i + 1][j], dp[i][j + 1]);
  const out: { t: "eq" | "del" | "ins"; s: string }[] = [];
  const push = (t: "eq" | "del" | "ins", ch: string) => {
    const last = out[out.length - 1];
    if (last && last.t === t) last.s += ch;
    else out.push({ t, s: ch });
  };
  let i = 0;
  let j = 0;
  while (i < n && j < m) {
    if (A[i] === B[j]) {
      push("eq", A[i]);
      i++;
      j++;
    } else if (dp[i + 1][j] >= dp[i][j + 1]) push("del", A[i++]);
    else push("ins", B[j++]);
  }
  while (i < n) push("del", A[i++]);
  while (j < m) push("ins", B[j++]);
  return out;
}

export const spClass = (sp: string, speakers: string[]) => "s" + Math.min(2, Math.max(0, speakers.indexOf(sp)));
export const SPEAKERS = ["トーク1", "トーク2"];

/**
 * タイムラインのホイールの割り当て（本体とネスト画面で共通。issue 旧#13）。
 * Alt＝ズーム、Shift＝上下、そのまま＝左右。
 * Ctrl もズームのまま残す（タッチパッドのピンチは ctrlKey つきのホイールとして届くため）。
 * d はスクロール量（行・ページ単位はピクセルに換算済み）。page は deltaMode=2 のときの 1 ページの大きさ。
 */
export type WheelAct = { kind: "zoom"; zoomIn: boolean } | { kind: "h" | "v"; d: number };
export function wheelAct(
  e: Pick<WheelEvent, "deltaX" | "deltaY" | "deltaMode" | "altKey" | "ctrlKey" | "shiftKey">,
  page: { w: number; h: number },
): WheelAct {
  // 大きいほうの向きを使う（トラックパッドの横スワイプや、Shift / Alt で deltaX に入ってくる環境も拾う）
  const d = Math.abs(e.deltaX) > Math.abs(e.deltaY) ? e.deltaX : e.deltaY;
  if (e.altKey || e.ctrlKey) return { kind: "zoom", zoomIn: d < 0 };
  const unit = e.deltaMode === 1 ? 40 : e.deltaMode === 2 ? (e.shiftKey ? page.h : page.w) : 1; // 行・ページ単位をピクセルに
  return { kind: e.shiftKey ? "v" : "h", d: d * unit };
}
