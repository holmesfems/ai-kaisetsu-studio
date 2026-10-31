// 再生で「準備できていない区間」の手前で止まって待つ・そろったら再開する判断。旧#83
// Rust の play_ready が返す区間を見て決めるだけの純粋な関数（時計や音は io/player.ts が動かす）
import type { PlayReady } from "../gen/PlayReady.ts";

/** この秒数より手前に未準備が来たら止まる */
export const LEAD = 0.25;
/** 待ったあと、この秒数先まで（終わりが近ければ終わりまで）そろったら再開する */
export const RESUME_AHEAD = 3;
/** 再生位置からこの秒数先まで問い合わせる */
export const QUERY_AHEAD = 4;
/** 同じ時刻とみなす幅（1 コマ） */
const SAME_AT = 1 / 30;

export type GapKind = "fx" | "subs" | "voice";
export interface Gap {
  /** 未準備が始まる時刻（秒）。t より前から続いているものは t */
  at: number;
  kinds: GapKind[];
}

/** t 以降にかかる未準備の区間のうち、一番早い始まり。待たない声の行（voiceWaitable が false）は数えない */
export function firstGap(r: PlayReady, t: number, voiceWaitable: (lineId: string) => boolean): Gap | null {
  const starts: [GapKind, number][] = [];
  const add = (kind: GapKind, a: number, b: number) => {
    if (b > t) starts.push([kind, Math.max(a, t)]);
  };
  for (const [a, b] of r.fx) add("fx", a, b);
  for (const [a, b] of r.subs) add("subs", a, b);
  for (const g of r.voice) if (voiceWaitable(g.line_id)) add("voice", g.t0, g.t1);
  if (!starts.length) return null;
  const at = Math.min(...starts.map((x) => x[1]));
  const kinds: GapKind[] = [];
  for (const [k, a] of starts) if (a - at <= SAME_AT && !kinds.includes(k)) kinds.push(k);
  return { at, kinds };
}

/** 止まるか: 未準備が LEAD 秒より手前に来ている（終わりより先の未準備は気にしない） */
export function shouldWait(t: number, end: number, gap: Gap | null): boolean {
  return gap != null && gap.at < Math.min(t + LEAD, end);
}

/** 再開するか: 未準備が無いか、RESUME_AHEAD 秒先（終わりが近ければ終わり）まで来ない */
export function canResume(t: number, end: number, gap: Gap | null): boolean {
  return gap == null || gap.at >= Math.min(t + RESUME_AHEAD, end);
}

/** 待っている間の帯の文。何を待っているか（演出のコマの進み・声・字幕） */
export function waitText(r: PlayReady, t: number, voiceWaitable: (lineId: string) => boolean): string {
  const parts: string[] = [];
  if (r.fx.some(([, b]) => b > t)) parts.push(`演出のコマ ${r.fx_count[0]}/${r.fx_count[1]}`);
  const nv = r.voice.filter((g) => g.t1 > t && voiceWaitable(g.line_id)).length;
  if (nv > 0) parts.push(`声を作成中（${nv} 行）`);
  if (r.subs.some(([, b]) => b > t)) parts.push("字幕");
  return parts.length ? `準備中: ${parts.join("・")}` : "準備中";
}
