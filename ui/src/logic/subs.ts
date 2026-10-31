// 字幕のフォントの表示名。見た目（共通 → 話者 → セリフの重ね方）・区間・鍵は Rust（render/subs.rs の view）が決める。
import type { FontInfo } from "../types";

export const fontLabel = (f: FontInfo) => `${f.family}${f.weight_axis ? "（可変）" : f.style && !/^regular$/i.test(f.style) ? " " + f.style : ""}`;

/** フォントの指定（パス#番号）を表示名に */
export function fontName(spec: string, fonts: FontInfo[] | null): string {
  const f = fonts?.find((x) => x.spec.toLowerCase() === spec.toLowerCase());
  return f ? fontLabel(f) : spec.split(/[\\/]/).pop() || spec;
}

/** 入力が止まってから字幕の書き換えを確定するまでの待ち（ミリ秒） */
export const SUB_IDLE_MS = 2000;

/** 字幕の書き換えの正規化。空の行を消し、行末の空白を削る（改行は \n に） */
export function normSubText(v: string): string {
  return v.replace(/\r\n/g, "\n").split("\n").map((l) => l.trimEnd()).filter((l) => l.trim()).join("\n");
}

/** 保存された値が変わったとき、欄の値をどうするか。欄を norm したものが保存と同じなら欄のまま（打っている途中の空の行・空白を残す） */
export function syncSubEdit(local: string, saved: string): string {
  return normSubText(local) === saved ? local : saved;
}

/** キー入力から確定のしかたを決める。"now"＝改行せずすぐ確定（Ctrl+Enter）、"after-input"＝改行を入れてから確定（Enter）、null＝何もしない */
export function subEditKey(e: { key: string; ctrlKey: boolean; isComposing: boolean; keyCode: number }): "now" | "after-input" | null {
  if (e.isComposing || e.keyCode === 229 || e.key !== "Enter") return null;
  return e.ctrlKey ? "now" : "after-input";
}
