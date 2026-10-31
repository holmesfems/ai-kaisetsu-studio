// 状態の入口。全 slice と store を再エクスポートし、起動時の init（本体からの知らせの購読）を持つ。
import { call, on } from "../io/api";
import { mergeChatUpdate } from "../logic/chatcancel";
import { freshWarnings, warningToast } from "../logic/warnings";
import type { ChatMsg, Proposal, SeBank } from "../types";
import { applyExternal } from "./history";
import { receiveLayout, receiveScript, relayout } from "./script";
import { loadSpeakersInfo } from "./voice";
import { applySnapshot, onOverrides, openProject, refreshScenes, replayPendingDocEdits } from "./project";
import { loadMarks, receiveMarks } from "./marks";
import { refreshSubCheck } from "./subs";
import { toast, toastError, useS, type Progress } from "./store";

export * from "./store";
export * from "./history";
export * from "./script";
export * from "./voice";
export * from "./project";
export * from "./guide";
export * from "./chat";
export * from "./marks";
// 字幕: 見た目・区間の取り直しの購読はこのファイルを読み込んだときに張る（Inspector を開かなくても Timeline・Monitor に出るように）
export * from "./subs";

const get = useS.getState;
const set = useS.setState;

let inited = false;

export async function init() {
  if (inited) return;
  inited = true;
  // 演出エンジン・字幕の警告は、増えた・変わったときだけ知らせる（読み込みのたびに同じものを出さない）
  let seenWarn = new Set<string>();
  useS.subscribe((s, prev) => {
    if (s.engine === prev.engine) return;
    const r = freshWarnings(seenWarn, s.engine?.warnings);
    seenWarn = r.seen;
    if (r.fresh.length) toast(warningToast(r.fresh), "error", 12000);
  });
  on("desc", (d) => {
    set({ desc: d });
    refreshScenes();
    refreshCache();
  });
  on("engine", (e) => set({ engine: e }));
  // 攻略動画の状態（読み直し・保存・解析の始まりと終わり）。モニターは組み立てが変わったので描き直す
  on("guide", (g) => {
    set((s) => ({ guide: g, backRev: s.backRev + 1 }));
    refreshScenes();
  });
  on("overrides", onOverrides);
  // Claude などが素材トラック・字幕を変えたとき
  on("doc", (d) => {
    const p = get().project;
    if (p) {
      // 送っていない画面の変更があれば、新しい doc の上にやり直す（タイマーはそのまま。発火で合わせたものを送る）
      replayPendingDocEdits(d);
      applyExternal("Claude の変更", () => set({ project: { ...p, doc: d } }));
      relayout();
      // 字幕の改行・大きさが変わると行の分け方・はみ出しも変わる（台本の変更と違い、並びの知らせでは取り直さない）
      refreshSubCheck(150);
    }
  });
  // 回を開いた後に作られた fx/ のタイムラインを見つけたとき（取り消しの履歴には入れない）
  on("fx_timeline", (v) => {
    const p = get().project;
    if (p) set({ project: { ...p, doc: { ...p.doc, fx_timeline: v.fx_timeline }, timelines: v.timelines } });
  });
  on("gui_clips", (o) => set({ guiClips: o }));
  on("script", (v) => {
    if (v.external) toast("台本が外部で変更されたため、再読み込みしました", "info");
    receiveScript(v.external ? "台本（外のエディタ・Claude の直接編集）" : "台本（推敲案の採用・Claude）", v.script);
    relayout();
  });
  // 本体が作ったセリフの並びと声の解析（保存した台本・間隔・解析が変わるたびに届く）
  on("analysis", (u) => set((s) => ({ analysis: { ...s.analysis, ...u.entries } })));
  on("layout", receiveLayout);
  on("chat", (m) => set((s) => ({ chat: [...s.chat, m] })));
  // 届いた・取り消したの印が変わった（送ったばかりでまだ手元に無ければ足す）
  on("chat-update", (m) => set((s) => ({ chat: mergeChatUpdate(s.chat, m) })));
  on("proposals", (p) => set({ proposals: p }));
  // モニターの印（Claude が済にしたときのトーストは本体が出す）
  on("marks", receiveMarks);
  on("claude", (c) => set({ claude: c }));
  on("voice", (v) => {
    set({ voice: v });
    if (v.ok) loadSpeakersInfo();
    else toastError("VoiSona の中継を起動できませんでした", { code: v.code ?? "failed", message: v.error, detail: v.detail ?? v.error }, 9000);
  });
  on("toast", (v) => toast(v.text, v.kind === "error" ? "error" : "info", v.ms ?? (v.kind === "error" ? 9000 : 4200), v.detail));
  on("progress", (p) => {
    set((s) => ({ progress: { ...s.progress, [p.task]: p } }));
    if (p.finished) setTimeout(() => set((s) => {
      const n = { ...s.progress };
      delete n[p.task];
      return { progress: n };
    }), 2500);
  });
  on("cache", () => refreshCache());
  // 効果音の設定が変わった（画面か Claude）。作り直した効果音の読み直しは player.ts
  on("se_bank", (v) => {
    set({ seBank: v.bank });
    if (v.warnings?.length) toast(v.warnings.map((w) => "⚠ " + w).join("\n"), "error", 12000);
  });
  on("ui-focus", (v) => {
    if (typeof v.t === "number") set({ t: v.t });
    if (v.clip) set({ selection: { kind: "fx", key: v.clip } });
    if (v.line !== undefined) {
      // 行の指し方は Rust が引いて line_id にしてある。引けなければ黙らず知らせる
      const id = v.line_id;
      if (id) set({ focusLine: id, selection: { kind: "line", id } });
      else toast(`Claude が指した行が見つかりませんでした: ${v.line}`, "error");
    }
  });
  const v = await call("get_state");
  applySnapshot(v);
  if (!v.project && v.settings?.last_project) {
    await openProject(v.settings.last_project);
  }
  // 画面を開き直したとき（回はもう開いている）は marks イベントが来ないので、取りに行く
  loadMarks();
  setInterval(async () => {
    try {
      set({ claude: await call("claude_info"), engine: await call("engine_info") });
    } catch { /* 無視 */ }
  }, 5000);
}

let cacheTimer: number | undefined;
export function refreshCache() {
  clearTimeout(cacheTimer);
  cacheTimer = window.setTimeout(async () => {
    try {
      set({ cacheRanges: await call("cached_ranges", { scale: get().scale }) });
    } catch { /* 無視 */ }
  }, 300);
}
