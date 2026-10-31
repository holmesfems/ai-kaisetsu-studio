// 選択・Claude への文脈・チャットの送受信（好き！・取り消し）と、画面の状態を本体へ送る購読。
import { call } from "../io/api";
import { addPending, buildLiked, dropPending, reattach } from "../logic/likes";
import type { ChatMsg } from "../types";
import { refreshCache } from "./index";
import { useS } from "./store";

const get = useS.getState;
const set = useS.setState;

export function sceneAt(t: number): string {
  const sc = get().scenes.find((x) => x.t0 <= t && t < x.t1);
  return sc ? sc.scene : "-";
}

export function lineNumbers(ids: string[]): number[] {
  const s = get().script;
  if (!s) return [];
  const out: number[] = [];
  let n = 0;
  for (const b of s.blocks) for (const l of b.lines) {
    n++;
    if (ids.includes(l.id)) out.push(n);
  }
  return out;
}

export function chatContext() {
  const s = get();
  const ctx: any = { t: Math.round(s.t * 1000) / 1000, scene: sceneAt(s.t) };
  if (s.selection?.kind === "fx") {
    ctx.clip = s.selection.key;
    const g = s.desc?.groups.find((x) => x.key === (s.selection as any).key);
    if (g) ctx.clip_src = `${s.desc?.path.split(/[\\/]/).pop()}:${g.line} ${g.src.split("\n")[0].slice(0, 120)}`;
  }
  const lines = new Set(s.selLines);
  if (s.selection?.kind === "line") lines.add(s.selection.id);
  if (lines.size) ctx.lines = lineNumbers([...lines]);
  return ctx;
}

/** まだ Claude に伝えていない「好き！」を押した返事の id（次のメッセージの ctx.liked に添える） */
let pendingLikes: number[] = [];

export async function sendChat(text: string, extra: any = {}, attachments: any[] = []) {
  const liked = buildLiked(pendingLikes, get().chat);
  const carried = pendingLikes;
  pendingLikes = [];
  let m: ChatMsg;
  try {
    m = await call("chat_send", { text, ctx: { ...chatContext(), ...extra, ...(liked.length ? { liked } : {}) }, attachments });
  } catch (e) {
    pendingLikes = [...carried, ...pendingLikes.filter((x) => !carried.includes(x))];
    throw e;
  }
  // 猶予 0 秒だと、届いた印（chat-update）の方が先に来ていることがある
  set((s) => (s.chat.some((x) => x.id === m.id) ? {} : { chat: [...s.chat, m] }));
  lastSent = { id: m.id, at: Date.now() };
  return m;
}

/** この画面から最後に送ったメッセージ（Esc / Ctrl+C で取り消せるのは、これの直後だけ） */
let lastSent: { id: number; at: number } | null = null;
export const lastSentChat = () => lastSent;

/**
 * 送ったメッセージの取り消し（issue 旧#21）。まだ Claude に届いていなければ届かないようにし、
 * 届いていれば Claude に「無視して」と伝える。結果の印は chat-update で届く。
 */
export async function cancelChat(id: number): Promise<"retracted" | "late"> {
  const r = await call("chat_cancel", { id });
  set((s) => ({ chat: s.chat.map((x) => (x.id === r.msg.id ? r.msg : x)) }));
  // Claude に届く前に取り消したので、運んでいた「好き！」は次のメッセージに付け直す
  if (r.result === "retracted") pendingLikes = reattach(pendingLikes, r.msg.ctx?.liked);
  return r.result;
}

/** Claude の返事に「好き！」を付ける・外す。Claude には次のメッセージで伝える */
export async function reactChat(id: number, like: boolean) {
  const m = await call("chat_react", { id, reaction: like ? "like" : "" });
  pendingLikes = like ? addPending(pendingLikes, id) : dropPending(pendingLikes, id);
  set((s) => ({ chat: s.chat.map((x) => (x.id === m.id ? m : x)) }));
}

let uiTimer: number | undefined;
useS.subscribe((s, p) => {
  if (s.t !== p.t || s.selection !== p.selection || s.selLines !== p.selLines || s.inT !== p.inT || s.outT !== p.outT) {
    clearTimeout(uiTimer);
    uiTimer = window.setTimeout(() => {
      const st = get();
      // 250ms ごとに走るのでトーストにはしない（Rust 側は失敗しない。IPC が壊れたときだけ）
      call("set_ui", {
        ui: { t: st.t, playing: st.playing, selection: st.selection, lines: lineNumbers(st.selLines), in: st.inT, out: st.outT },
      }).catch((e) => console.warn("画面の状態を本体へ送れませんでした", e));
    }, 250);
  }
  if (s.scale !== p.scale) refreshCache();
});
