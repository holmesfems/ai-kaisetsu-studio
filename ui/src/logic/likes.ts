// Claude の返事への「好き！」（issue 旧#71）のうち、まだ Claude に伝えていないものの帳簿。
// 押した瞬間には送らず、次に送るメッセージの ctx.liked にまとめて添える。画面（store.ts）から使い、ui/test/likes.test.ts で確かめる。
import type { ChatMsg } from "../types";

/** 返事の先頭として添える長さ */
export const HEAD_LEN = 80;

export type LikedRef = { id: number; head: string };

export function likeHead(text: string): string {
  const cs = [...text.replace(/\s+/g, " ").trim()];
  return cs.slice(0, HEAD_LEN).join("") + (cs.length > HEAD_LEN ? "…" : "");
}

/** 押した（まだ伝えていない印を足す） */
export function addPending(ids: number[], id: number): number[] {
  return ids.includes(id) ? ids : [...ids, id];
}

/** 取り消した（印を外す） */
export function dropPending(ids: number[], id: number): number[] {
  return ids.filter((x) => x !== id);
}

/** 次のメッセージに添える分: まだ「好き！」が付いている返事だけ（押して取り消した分は入れない） */
export function buildLiked(ids: number[], chat: ChatMsg[]): LikedRef[] {
  const out: LikedRef[] = [];
  for (const id of ids) {
    const m = chat.find((x) => x.id === id);
    if (m && m.role === "claude" && m.reaction === "like") out.push({ id, head: likeHead(m.text) });
  }
  return out;
}

/** 送信待ちのうちに取り消したメッセージが運んでいた分を、次のメッセージに付け直す */
export function reattach(ids: number[], carried: { id: number }[] | undefined): number[] {
  return (carried ?? []).reduce((a, x) => addPending(a, x.id), ids);
}
