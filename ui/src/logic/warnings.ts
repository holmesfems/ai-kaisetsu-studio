// 演出エンジン・字幕のワーカーの警告（issue 旧#30）。読み込みのたびに同じ警告を何度も知らせないよう、前回から増えた・変わったものだけを選ぶ。
import type { EngineWarning } from "../types";

/** 情報（level が "info"）。公開版で kit が無いときなど、ふつうの状態の知らせ。赤くせず、通知もしない */
export const isInfo = (w: EngineWarning) => w.level === "info";

/** 警告（赤く出す）と情報（目立たせない）に分ける */
export function splitWarnings(ws: EngineWarning[] | null | undefined): { warns: EngineWarning[]; infos: EngineWarning[] } {
  const all = ws ?? [];
  return { warns: all.filter((w) => !isInfo(w)), infos: all.filter(isInfo) };
}

const sig = (w: EngineWarning) => `${w.key}\u0000${w.message}`;

/** 前回の警告（seen）から増えた・文が変わったものと、次に覚えておく集合。消えた警告は忘れる（直してまた起きたら、もう一度知らせる） */
export function freshWarnings(seen: Set<string>, cur: EngineWarning[] | null | undefined): { fresh: EngineWarning[]; seen: Set<string> } {
  const next = new Set<string>();
  const fresh: EngineWarning[] = [];
  for (const w of cur ?? []) {
    if (isInfo(w)) continue;
    const s = sig(w);
    if (next.has(s)) continue;
    next.add(s);
    if (!seen.has(s)) fresh.push(w);
  }
  return { fresh, seen: next };
}

const WHERE = "（上の「演出」から一覧を確認できます）";

/** 通知（トースト）の文。多いときは先頭の 3 件だけ */
export function warningToast(fresh: EngineWarning[]): string {
  if (fresh.length === 1) return fresh[0].message + "\n" + WHERE;
  const head = fresh.slice(0, 3).map((w) => "・" + w.message);
  if (fresh.length > 3) head.push(`ほか ${fresh.length - 3} 件`);
  return `警告が ${fresh.length} 件あります${WHERE}\n` + head.join("\n");
}
