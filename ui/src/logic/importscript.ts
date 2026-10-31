import type { Script } from "../types.ts";
import { paramsOf, voiceOf } from "./voice.ts";

/** 取り込むものの選び方 */
export interface ImportPick {
  /** 取り込むブロックの id と、その中で取り込む行の id（ブロックの並び・行の並びは取り込み元のまま） */
  blocks: { id: string; lines: string[] }[];
  /** 行のメモを写す */
  memo: boolean;
  /** ブロックの notes を写す */
  notes: boolean;
  /** 話者の声の設定を写す（同じ名前は上書き） */
  speakers: boolean;
}

/**
 * 取り込み元の台本 src から、選んだブロック・行を dst の末尾に足す（dst を直接書き換える。editScript の中で呼ぶ）。
 * 行は新しい id になり、アクセント・抑揚の手直しは新しい id に中身ごと写す（sp・text が同じなのでそのまま効く）。
 * 今の文に効いていない古い手直し（文を直す前のもの）は写さない。
 * 開始時刻（start）は null にする。src は書き換えない。
 */
export function importScript(
  dst: Script,
  src: Script,
  pick: ImportPick,
  newId: (p: string) => string,
): { added: number; voice: number; params: number; speakers: string[] } {
  const res = { added: 0, voice: 0, params: 0, speakers: [] as string[] };
  const used = new Set<string>();
  for (const b of dst.blocks) {
    used.add(b.id);
    for (const l of b.lines) used.add(l.id);
  }
  const fresh = (p: string) => {
    let id = newId(p);
    while (used.has(id)) id = newId(p);
    used.add(id);
    return id;
  };
  const sps = new Set<string>();
  for (const pb of pick.blocks) {
    const sb = src.blocks.find((b) => b.id === pb.id);
    if (!sb) continue;
    const want = new Set(pb.lines);
    const lines = sb.lines
      .filter((l) => want.has(l.id))
      .map((l) => {
        const id = fresh("l");
        const nl: Script["blocks"][number]["lines"][number] = { id, sp: l.sp, text: l.text, start: null };
        if (pick.memo && l.memo) nl.memo = l.memo;
        const v = voiceOf(src, l) != null ? src.voice[l.id] : undefined;
        if (v) { dst.voice[id] = structuredClone(v); res.voice++; }
        const p = paramsOf(src, l) != null ? src.params[l.id] : undefined;
        if (p) { dst.params[id] = structuredClone(p); res.params++; }
        sps.add(l.sp);
        return nl;
      });
    if (!lines.length) continue;
    dst.blocks.push({ id: fresh("b"), file: null, title: sb.title, lines, notes: pick.notes ? sb.notes : "" });
    res.added += lines.length;
  }
  if (pick.speakers && src.speakers) {
    for (const sp of sps) {
      const v = src.speakers[sp];
      if (!v) continue;
      (dst.speakers ??= {})[sp] = structuredClone(v);
      res.speakers.push(sp);
    }
  }
  return res;
}
