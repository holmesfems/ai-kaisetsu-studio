// Claude の「考え中」の表示（issue 旧#51）。画面（ClaudePanel）から使い、ui/test/claudestatus.test.ts で確かめる。
//
// 出すとき: Mod が知らせた作業中（activity.working）か、最後に送ったメッセージが届いたのに Claude の返事がまだ無いとき。
// 文の優先: Claude 自身の reply の status > Mod が知らせた何をしているか > 回る動詞
import type { ChatMsg } from "../types";

export type Activity = { working: boolean; text: string; since_ms: number; updated_ms?: number };
export type ClaudeLike = { connected: boolean; status: string; activity?: Activity };

/** 文が無いときに回す動詞（Claude Code のまね） */
export const VERBS = ["Pondering", "Thinking", "Mulling", "Noodling", "Brewing", "Cogitating", "Musing"];
export const VERB_MS = 2500;
export const HELD_TEXT = "Claude の作業が終わったら届けます";

/** 返事が無いまま「待っている」とみなす上限（Mod の知らせが無いときの打ち切り） */
export const AWAIT_MAX_MS = 10 * 60 * 1000;

export type StatusView = {
  /** 本文。空なら回る動詞を出す */
  text: string;
  /** 作業を始めた時刻（ms）。経過秒の元 */
  since: number | null;
  /** 作業中で、まだ Claude に届いていないメッセージがある */
  held: boolean;
};

/** "2026-10-18 12:34:56"（ローカル時刻）→ ms */
export function tsMs(ts: string): number | null {
  const t = new Date(ts.replace(" ", "T")).getTime();
  return Number.isNaN(t) ? null : t;
}

/** 最後のユーザーのメッセージ（取り消しの知らせは除く）と、それより後のメッセージ */
function lastUser(chat: ChatMsg[]): { m: ChatMsg; after: ChatMsg[] } | null {
  for (let i = chat.length - 1; i >= 0; i--) {
    const m = chat[i];
    if (m.role === "user" && m.ctx?.kind !== "cancel_notice") return { m, after: chat.slice(i + 1) };
  }
  return null;
}

/** 出すか。出さないなら null */
export function statusView(chat: ChatMsg[], claude: ClaudeLike, now: number): StatusView | null {
  const a = claude.activity;
  const working = !!a?.working;
  const lu = lastUser(chat);
  const sentAt = lu ? tsMs(lu.m.ts) : null;
  const awaiting =
    !!lu &&
    (lu.m.state === "queued" || lu.m.state === "delivered") &&
    !lu.after.some((m) => m.role === "claude") &&
    (sentAt === null || now - sentAt <= AWAIT_MAX_MS) &&
    // まだ渡していないメッセージは、Claude がつながっているか作業中のときだけ（つながっていなければ待つだけ）
    (lu.m.state === "delivered" || working || claude.connected);
  if (!working && !awaiting) return null;
  const held = working && !!lu && lu.m.state === "queued";
  const text = held ? HELD_TEXT : claude.status || (working ? a!.text : "");
  const since = working && a!.since_ms > 0 ? a!.since_ms : sentAt;
  return { text, since, held };
}

/** 回る動詞。経過時間で切り替わる */
export function verbAt(elapsedMs: number): string {
  return VERBS[Math.floor(Math.max(0, elapsedMs) / VERB_MS) % VERBS.length] + "…";
}

/** 経過の表示（12s / 1m05s） */
export function fmtElapsed(ms: number): string {
  const s = Math.max(0, Math.floor(ms / 1000));
  return s < 60 ? `${s}s` : `${Math.floor(s / 60)}m${String(s % 60).padStart(2, "0")}s`;
}
