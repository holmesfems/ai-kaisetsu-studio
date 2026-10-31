// 攻略動画の画面の表の行を作る純粋な関数（効く値は本体が決めた GuideState から。ここでは決め直さない）。
import type { Cut } from "../gen/Cut";
import type { Guide } from "../gen/Guide";
import type { GuideBench } from "../gen/GuideBench";
import type { GuideFixed } from "../gen/GuideFixed";
import type { GuideOp } from "../gen/GuideOp";
import type { GuideState } from "../gen/GuideState";
import type { SrcAt } from "../gen/SrcAt";
import type { ClaudeUndo } from "../gen/ClaudeUndo";
import { errorMessage } from "./errors.ts";

/** アンカーの名前と表示名（順番どおり。本体の ANCHOR_NAMES と同じ） */
export const ANCHORS: { name: string; label: string }[] = [
  { name: "formation_dark", label: "編成の暗転" },
  { name: "manual_cost0", label: "手動の最初のコスト" },
  { name: "manual_end", label: "手動の作戦終了" },
  { name: "auto_cost0", label: "自動の最初のコスト" },
  { name: "auto_end", label: "自動の作戦終了" },
];

/** パートの表示名（本体のシーンの帯と同じ） */
export const PART_LABELS: Record<string, string> = { formation: "編成", manual: "手動", auto: "自動", result: "リザルト" };

/** 固定値の項目（秒） */
export const FIXED_ITEMS: { key: keyof GuideFixed; label: string; hint: string }[] = [
  { key: "formation_end", label: "編成の長さ", hint: "出力の 0 秒から編成パートが終わるまで。暗転と、その後に見せる長さがここに収まるよう素材を置きます" },
  { key: "formation_tail", label: "暗転の後", hint: "暗転の後に見せる長さ" },
  { key: "manual_lead", label: "手動の前", hint: "最初のコストの前に見せる長さ" },
  { key: "manual_tail", label: "手動の後", hint: "作戦終了の帯の後に見せる長さ" },
  { key: "auto_lead", label: "自動の前", hint: "最初のコストの前に見せる長さ" },
  { key: "auto_tail", label: "自動の後", hint: "作戦終了の帯の後に見せる長さ" },
  { key: "result_len", label: "リザルトの長さ", hint: "自動パートの素材の続きをそのまま流す長さ" },
  { key: "now_lead", label: "NOW の先出し", hint: "信頼枠以外の操作を、操作の瞬間より前に NOW に出す長さ。操作ごとに変えられます" },
];

export interface AnchorRow {
  name: string;
  label: string;
  /** 解析の候補（素材の秒） */
  auto: number | null;
  /** 手で決めた値（無ければ null） */
  t: number | null;
  /** 使う値（t、無ければ auto） */
  value: number | null;
  /** 出力の何秒に来るか（組み立てられていなければ null） */
  out: number | null;
  /** 手で決めた値が候補と違う */
  custom: boolean;
}

export function anchorRows(g: GuideState): AnchorRow[] {
  const anchors = g.guide?.anchors ?? {};
  return ANCHORS.map(({ name, label }) => {
    const a = anchors[name];
    const t = a?.t ?? null;
    const auto = a?.auto ?? null;
    return { name, label, auto, t, value: t ?? auto, out: g.anchor_out[name] ?? null, custom: t !== null && t !== auto };
  });
}

export interface FixedRow {
  key: keyof GuideFixed;
  label: string;
  hint: string;
  /** 効いている値（回 → 設定 → 既定） */
  effective: number;
  /** この回だけ変えた値（無ければ null） */
  own: number | null;
  /** 設定の値（全部の回で使う） */
  setting: number;
}

export function fixedRows(g: GuideState): FixedRow[] {
  return FIXED_ITEMS.map(({ key, label, hint }) => ({
    key,
    label,
    hint,
    effective: g.fixed_effective[key],
    own: g.guide?.fixed[key] ?? null,
    setting: g.fixed_settings[key],
  }));
}

/** 今の効いている値が、設定の値と違う項目があるか（「設定に保存」の目安） */
export function fixedDiffersFromSettings(g: GuideState): boolean {
  return FIXED_ITEMS.some(({ key }) => g.fixed_effective[key] !== g.fixed_settings[key]);
}

/** 待機欄の項目（画面は % で入れ、本体の GuideBench は 0〜1 の倍率）。範囲は本体の BENCH_*_RANGE と同じ（外れたら本体も断る） */
export const BENCH_ITEMS: { key: keyof GuideBench; label: string; min: number; max: number; hint: string }[] = [
  { key: "scale", label: "大きさ", min: 20, max: 100, hint: "素材の画素に対する倍率（%）。レイヤーの左下に置きます（手動パートはレイヤーごと縮みます）" },
  { key: "opacity", label: "不透明度", min: 10, max: 100, hint: "100 で不透明、小さいほど透けます" },
];

/** 待機欄の値（0〜1）→ 画面に出す % の文字（小数 1 桁まで。末尾の 0 は落とす） */
export function benchPercentText(v: number): string {
  return String(Math.round(v * 1000) / 10);
}

export type BenchParse = { ok: true; bench: GuideBench } | { ok: false; error: string };

/** 待機欄の項目 key を % の入力 text で変えた GuideBench を返す（元は変えない）。数でない・範囲の外は理由つきの失敗 */
export function benchFromPercent(b: GuideBench, key: keyof GuideBench, text: string): BenchParse {
  const item = BENCH_ITEMS.find((i) => i.key === key)!;
  const s = text.trim().replace(/%$/, "").trim();
  const n = /^\d+(?:\.\d+)?$/.test(s) ? Number(s) : NaN;
  if (!Number.isFinite(n)) return { ok: false, error: `${item.label}は数（%）で入力してください（例: 50）` };
  if (n < item.min || n > item.max) return { ok: false, error: `${item.label}は ${item.min}〜${item.max}% にしてください` };
  return { ok: true, bench: { ...b, [key]: Math.round(n * 10) / 1000 } };
}

export interface PartRow {
  name: string;
  label: string;
  t0: number;
  len: number;
}

export function partRows(g: GuideState): PartRow[] {
  return g.plan_parts.map((p) => ({ name: p.name, label: PART_LABELS[p.name] ?? p.name, t0: p.t0, len: p.t1 - p.t0 }));
}

/** アンカーの使う値 t を変えた guide を返す（null で候補に戻す。元は変えない） */
export function withAnchorT(g: Guide, name: string, t: number | null): Guide {
  const next = { ...(g.anchors[name] ?? {}) };
  if (t === null) delete next.t;
  else next.t = t;
  return { ...g, anchors: { ...g.anchors, [name]: next } };
}

/** この回だけの固定値を変えた guide を返す（null で設定の値に戻す。元は変えない） */
export function withFixedOwn(g: Guide, key: keyof GuideFixed, v: number | null): Guide {
  const fixed = { ...g.fixed };
  if (v === null) delete fixed[key];
  else fixed[key] = v;
  return { ...g, fixed };
}

/**
 * 秒の入力を読む。"12.4"・"1:23.5"（分:秒）。空は null（値を消す）、読めなければ undefined
 */
export function parseSeconds(text: string): number | null | undefined {
  const s = text.trim();
  if (s === "") return null;
  const m = /^(?:(\d+):)?(\d+(?:\.\d+)?)$/.exec(s);
  if (!m) return undefined;
  return (m[1] ? Number(m[1]) * 60 : 0) + Number(m[2]);
}

/** 秒の表示（小数 3 桁まで。末尾の 0 は落とす） */
export function fmtSeconds(v: number | null): string {
  if (v === null || !Number.isFinite(v)) return "";
  return String(Math.round(v * 1000) / 1000);
}

/** 解析の要約（画面の一行） */
export function analysisSummary(g: GuideState): string {
  const a = g.analysis;
  if (!a.exists) return "まだ解析していません";
  return `${a.frames.toLocaleString("ja-JP")} コマ・戦闘区間 ${a.battles.length} 個`;
}

// ---------------------------------------------------------------- 段3: アンカーの 1 コマ送り・カット・編成・操作

/** アンカーが属するパート（モニターの今の位置を「今のコマにする」ときに、このパートの中でなければ断る） */
export const ANCHOR_PART: Record<string, string> = {
  formation_dark: "formation",
  manual_cost0: "manual",
  manual_end: "manual",
  auto_cost0: "auto",
  auto_end: "auto",
};

/** 「今のコマにする」を断る理由（今の位置がそのアンカーのパートの中なら null）。at は guide_src_at の結果 */
export function anchorHereError(name: string, at: SrcAt | null): string | null {
  const label = ANCHORS.find((a) => a.name === name)?.label ?? name;
  if (!at) return "今の位置は動画の外か、パートを組み立てられていません";
  const want = ANCHOR_PART[name];
  if (want && at.part !== want) {
    return `「${label}」は「${PART_LABELS[want]}」パートの中のコマにしてください（今の位置は「${PART_LABELS[at.part] ?? at.part}」パートです）`;
  }
  return null;
}

/**
 * ゲーム内の時間の書き方 分:秒:フレーム（フレームは 00〜59。60 フレームで 1 秒、コスト 1 = 30 フレーム = 0.5 秒）。
 * game は解析のゲームのフレーム数（戦闘の最初のコストが 0）。読めていなければ空
 */
export function fmtGameTime(game: number | null | undefined): string {
  if (game === null || game === undefined || !Number.isFinite(game) || game < 0) return "";
  const f = Math.floor(game + 1e-6);
  const two = (n: number) => String(n).padStart(2, "0");
  return `${two(Math.floor(f / 3600))}:${two(Math.floor(f / 60) % 60)}:${two(f % 60)}`;
}

// ---- 操作

export const OP_KINDS: { kind: string; label: string }[] = [
  { kind: "deploy", label: "配置" },
  { kind: "skill", label: "スキル" },
  { kind: "retreat", label: "撤退" },
  { kind: "trust", label: "信頼枠" },
];

export interface OpRow {
  id: string;
  /** guide.ops の中の位置 */
  index: number;
  /** 素材の時刻（秒） */
  t: number;
  kind: string;
  kindLabel: string;
  op: string;
  where: string;
  /** 補足（自由文。動画の NOW/NEXT・操作の帯に出る） */
  info: string;
  precise: boolean;
  cue_text: string;
  note: string;
  /** 信頼枠の配置（行を薄くする。タイムラインでは灰色の点） */
  trust: boolean;
  /** 手動パートでの出力の時刻（出ない・組み立てられていなければ null） */
  tOut: number | null;
  /** そのコマのコスト */
  cost: number | null;
  /** そのコマのゲーム内の時間 分:秒:フレーム（読めていなければ空） */
  game: string;
  /** 出力に出ない理由（無ければ null）。赤字で見せる */
  problem: string | null;
  /** NOW の先出しの個別値（秒。無ければ null = 汎用値） */
  nowLead: number | null;
  /** 汎用値（固定値の now_lead。効いている値） */
  nowLeadGeneric: number;
  /** NOW が始まる出力の時刻（出ない・組み立てられていなければ null） */
  tNow: number | null;
  /** 先出しどおりに始まらない理由: 前の操作の瞬間まで遅れた（"prev"）・手動パートの頭で止まった（"head"）。遅れていなければ null */
  nowDelayed: NowDelay;
}

export function opRows(g: GuideState): OpRow[] {
  const outs = new Map(g.ops_out.map((o) => [o.id, o]));
  return (g.guide?.ops ?? []).map((o, index) => {
    const out = outs.get(o.id);
    let problem: string | null = null;
    if (out?.in_cut) problem = "カットの中にあるので、動画に出ません";
    else if (out && out.t_out === null) problem = "手動パートの外にあるので、動画に出ません";
    return {
      id: o.id,
      index,
      t: o.t,
      kind: o.kind,
      kindLabel: OP_KINDS.find((k) => k.kind === o.kind)?.label ?? (o.kind || "（未設定）"),
      op: o.op,
      where: o.where,
      info: o.info ?? "",
      precise: o.precise,
      cue_text: o.cue_text,
      note: o.note,
      trust: o.kind === "trust",
      tOut: out?.t_out ?? null,
      cost: out?.cost ?? null,
      game: fmtGameTime(out?.game),
      problem,
      nowLead: o.now_lead ?? null,
      nowLeadGeneric: g.fixed_effective.now_lead,
      tNow: out?.t_now ?? null,
      nowDelayed: nowDelayed(out?.t_out ?? null, out?.t_now ?? null, o.kind === "trust" ? 0 : (o.now_lead ?? g.fixed_effective.now_lead),
        prevOut(g.ops_out, out?.t_out ?? null)),
    };
  });
}

/** 操作の項目を直した guide を返す（元は変えない）。patch の値が undefined の項目は消す（本体では None。now_lead を汎用値に戻すとき）。
 *  その id の操作が無ければ null（ほかで消されている） */
export function withOp(g: Guide, id: string, patch: Partial<GuideOp>): Guide | null {
  if (!g.ops.some((o) => o.id === id)) return null;
  return {
    ...g,
    ops: g.ops.map((o) => {
      if (o.id !== id) return o;
      const n: GuideOp = { ...o, ...patch };
      for (const k of Object.keys(patch) as (keyof GuideOp)[]) if (patch[k] === undefined) delete n[k];
      return n;
    }),
  };
}

/** 操作を消した guide を返す。残りの id は振り直さない。その id の操作が無ければ null */
export function withoutOp(g: Guide, id: string): Guide | null {
  if (!g.ops.some((o) => o.id === id)) return null;
  return { ...g, ops: g.ops.filter((o) => o.id !== id) };
}

/** 直す操作が、ほかで消されていたときの通知 */
export function opGoneMessage(id: string): string {
  return `操作 ${id} はほかで消されました`;
}

/** 直す対象が（取り直した最新に）無かったかを、直す関数から呼ぶ側へ伝える旗 */
export type EditFlags = { gone: boolean };

/**
 * 操作 id を直す関数（editWithRetry に渡す）。patchOf はそのときの最新の操作から直す項目を決める。
 * 食い違いで取り直して当て直すときも、最新の操作の値から決め直す（相対の動かし方が古い値のままにならない）。
 * 操作が無ければ null を返して flags.gone を立てる（呼ぶ側が opGoneMessage を出す）
 */
export function opEdit(
  id: string,
  patchOf: (o: GuideOp) => Partial<GuideOp> | Promise<Partial<GuideOp>>,
  flags: EditFlags,
): (g: Guide) => Promise<Guide | null> {
  return async (g) => {
    const o = g.ops.find((x) => x.id === id);
    if (!o) {
      flags.gone = true;
      return null;
    }
    flags.gone = false;
    return withOp(g, id, await patchOf(o));
  };
}

/** アンカー name の値を、そのときの最新の値から tOf で決め直す関数（editWithRetry に渡す）。値がまだ無ければ null を返して flags.noValue を立てる */
export function anchorEdit(
  name: string,
  tOf: (value: number) => Promise<number>,
  flags: { noValue: boolean },
): (g: Guide) => Promise<Guide | null> {
  return async (g) => {
    const v = g.anchors[name]?.t ?? g.anchors[name]?.auto ?? null;
    if (typeof v !== "number") {
      flags.noValue = true;
      return null;
    }
    flags.noValue = false;
    return withAnchorT(g, name, await tOf(v));
  };
}

/** 操作のタイムライン上の印の種類（精密は ◆、大まかは ●、信頼枠は小さな灰色の点） */
export function opMarkKind(o: { trust: boolean; precise: boolean }): "trust" | "precise" | "rough" {
  return o.trust ? "trust" : o.precise ? "precise" : "rough";
}

/** 操作の印をドラッグできる範囲（手動パートの出力の区間。末尾は最後のコマまで）。パートが無ければ null */
export function manualSpan(g: GuideState, fps = 60): [number, number] | null {
  const p = g.plan_parts.find((x) => x.name === "manual");
  return p ? [p.t0, Math.max(p.t0, p.t1 - 1 / fps)] : null;
}

// ---- カット

export const DEFAULT_CUT_FADE = 0.25;

export interface CutRow {
  /** guide.cuts の中の位置（本体は素材の時刻の順に保存する） */
  index: number;
  srcIn: number;
  srcOut: number;
  /** 抜く長さ（秒） */
  len: number;
  fade: number;
  /** 継ぎ目の出力の時刻（組み立てられていなければ null） */
  outIn: number | null;
  outOut: number | null;
}

export function cutRows(g: GuideState): CutRow[] {
  const cuts = g.guide?.cuts ?? [];
  const seams = g.cuts_out.length === cuts.length ? g.cuts_out : [];
  return cuts.map((c, index) => ({
    index,
    srcIn: c.src_in,
    srcOut: c.src_out,
    len: c.src_out - c.src_in,
    fade: c.fade,
    outIn: seams[index]?.[0] ?? null,
    outOut: seams[index]?.[1] ?? null,
  }));
}

export type CutResult = { ok: true; guide: Guide } | { ok: false; error: string };

/**
 * タイムラインのイン・アウト（出力の時刻）を素材の時刻にしてカットを足す。
 * a・b はイン・アウトの時刻の guide_src_at の結果。両端が手動パートの中でなければ断る
 */
export function cutFromMarks(g: Guide, inT: number | null, outT: number | null, a: SrcAt | null, b: SrcAt | null,
                             fade = DEFAULT_CUT_FADE): CutResult {
  if (inT === null || outT === null) return { ok: false, error: "タイムラインにイン（I）とアウト（O）を打ってください" };
  if (outT <= inT) return { ok: false, error: "アウトはインより後にしてください" };
  if (!a || !b) return { ok: false, error: "イン・アウトが動画の外か、パートを組み立てられていません" };
  for (const [name, x] of [["イン", a], ["アウト", b]] as const) {
    if (x.part !== "manual") {
      return { ok: false, error: `${name}が手動パートの外です（今は「${PART_LABELS[x.part] ?? x.part}」パート）。両端を手動パートの中に打ってください` };
    }
  }
  return addCut(g, { src_in: a.t_src, src_out: b.t_src, fade });
}

/** カットを足した guide を返す（素材の時刻の順）。向きが逆・既存のカットと重なるときは断る */
export function addCut(g: Guide, cut: Cut): CutResult {
  if (cut.src_out <= cut.src_in) {
    return { ok: false, error: "カットの終わりが始まりより前になっています（イン・アウトの間に動画のコマがありません）" };
  }
  const hit = g.cuts.find((c) => cut.src_in <= c.src_out && c.src_in <= cut.src_out);
  if (hit) {
    return { ok: false, error: `すでにあるカット（${fmtSeconds(hit.src_in)}〜${fmtSeconds(hit.src_out)} 秒）と重なっています。先にそちらを消してください` };
  }
  const cuts = [...g.cuts, cut].sort((x, y) => x.src_in - y.src_in);
  return { ok: true, guide: { ...g, cuts } };
}

/** カットのフェードの秒を直した guide を返す */
export function withCutFade(g: Guide, index: number, fade: number): Guide {
  return { ...g, cuts: g.cuts.map((c, i) => (i === index ? { ...c, fade } : c)) };
}

export function withoutCut(g: Guide, index: number): Guide {
  return { ...g, cuts: g.cuts.filter((_, i) => i !== index) };
}

// ---- 編成

/** 枠の数の既定（本体の default_slots と同じ。枠の矩形を guide.json に書いていないとき） */
export const DEFAULT_SLOT_COUNT = 12;

export function slotCount(g: Guide): number {
  return g.formation.slots.length || DEFAULT_SLOT_COUNT;
}

export interface SlotRow {
  index: number;
  /** 使う枠か（used が空なら全部使う） */
  used: boolean;
  note: string;
}

export function slotRows(g: Guide): SlotRow[] {
  const used = g.formation.used;
  return Array.from({ length: slotCount(g) }, (_, index) => ({
    index,
    used: used.length === 0 || used.includes(index),
    note: g.formation.notes[String(index)] ?? "",
  }));
}

/** 枠の「使う / 使わない」を切り替えられない理由（使う枠が 1 つも無くなるとき）。切り替えられるなら null */
export function slotToggleError(g: Guide, i: number): string | null {
  const rows = slotRows(g);
  const onlyOne = rows.filter((r) => r.used).length === 1 && rows[i]?.used;
  return onlyOne ? "使う枠が 1 つも無くなります。全部使うときは「全部使う」を押してください" : null;
}

/** 枠 i の「使う / 使わない」を切り替えた guide を返す。全部が使う枠になったら used は空（全部使う）にする */
export function withSlotToggled(g: Guide, i: number): Guide {
  if (i < 0 || i >= slotCount(g) || slotToggleError(g, i)) return g;
  const rows = slotRows(g);
  const used = rows.filter((r) => (r.index === i ? !r.used : r.used)).map((r) => r.index);
  return { ...g, formation: { ...g.formation, used: used.length === rows.length ? [] : used } };
}

export function withAllUsed(g: Guide): Guide {
  return { ...g, formation: { ...g.formation, used: [] } };
}

/** 枠 i の補足を直した guide を返す（空にすると消す） */
export function withSlotNote(g: Guide, i: number, text: string): Guide {
  const notes = { ...g.formation.notes };
  const t = text.trim();
  if (t === "") delete notes[String(i)];
  else notes[String(i)] = t;
  return { ...g, formation: { ...g.formation, notes } };
}

// ---------------------------------------------------------------- 保存の食い違い（rev）と Claude の変更

/** 本体が「その間にほかで guide.json が変わった」と断ったときの理由の頭（本体の `guide::REV_CONFLICT` と同じ文） */
export const REV_CONFLICT = "ほかで変わりました";

/** guide_set の失敗が、保存の食い違い（画面が持っている rev が古い）か */
export function isRevConflict(e: unknown): boolean {
  return errorMessage(e).startsWith(REV_CONFLICT);
}

export type EditOutcome =
  | { kind: "saved" }
  /** 直す中身が無かった（fn が null）・取り直せなかった（理由は取り直した側が出した） */
  | { kind: "skipped" }
  | { kind: "error"; error: unknown };

/**
 * 今の guide を fn で直して保存する。保存が食い違い（ほかで guide.json が変わっていた）で断られたら、
 * 最新を取り直して fn をもう 1 回だけ当てて保存する（それでもだめなら error）。fn は非同期でもよい（当て直すときは最初から呼び直す）。
 * io.save は保存できたら null、できなければ理由を返す（通知は呼ぶ側）。io.refetch は取り直せたら true
 */
export async function editWithRetry(
  io: {
    current(): { guide: Guide | null; rev: number } | null;
    save(g: Guide, rev: number): Promise<unknown | null>;
    refetch(): Promise<boolean>;
  },
  fn: (g: Guide) => Guide | null | Promise<Guide | null>,
): Promise<EditOutcome> {
  for (let attempt = 0; ; attempt++) {
    const cur = io.current();
    const next = cur?.guide ? await fn(cur.guide) : null;
    if (!cur || !next) return { kind: "skipped" };
    const err = await io.save(next, cur.rev);
    if (err === null) return { kind: "saved" };
    if (!isRevConflict(err) || attempt >= 1) return { kind: "error", error: err };
    if (!(await io.refetch())) return { kind: "skipped" };
  }
}

/** 「Claude の変更を戻す（理由）」のボタンの文（理由は長ければ詰める。全文は title に） */
export function claudeUndoLabel(u: ClaudeUndo, max = 24): string {
  const r = u.reason.trim().replace(/\s+/g, " ");
  const short = [...r].length > max ? [...r].slice(0, max).join("") + "…" : r;
  return short ? `Claude の変更を戻す（${short}）` : "Claude の変更を戻す";
}

/** ボタンの title（時刻と全文の理由） */
export function claudeUndoTitle(u: ClaudeUndo): string {
  return `${u.at.replace("T", " ")} に Claude が直した変更を、直す前に戻します。理由: ${u.reason}
その後に画面で直していると戻せません`;
}

/** 操作の印の title の 1〜2 行目。補足があれば最後の行に足す（空白・改行は 1 つの空白にまとめる） */
export function opMarkTitle(o: { kindLabel: string; op: string; where: string; info: string }): string {
  const label = [o.kindLabel, o.op, o.where].filter(Boolean).join(" ") || "（未設定）";
  const info = o.info.split(/\s+/).filter(Boolean).join(" ");
  return info ? `${label}\n補足: ${info}` : label;
}

/**
 * 出力の時刻 t に操作を足せない理由（足せるなら null）。本体の `guide_op_add`（手動パートの中だけ）と同じ判断を、本体に聞く前に出す。
 * パートは出力の時刻で [t0, t1)。カットは出力から抜けているので、手動パートの中ならカットの継ぎ目の上でも本体は断らない
 */
/** 攻略動画の出力の fps（本体の組み立て `GuidePlan::fps` と同じ） */
const OUT_FPS = 60;

export function opAddError(g: GuideState, t: number): string | null {
  if (!g.guide) return "攻略動画にしていません";
  if (g.plan_parts.length === 0) return "パートを組み立てられていません。素材を解析して、アンカーを確かめてください";
  // 本体は出力のコマ（60fps、round(t × 60)）で引くので、同じコマに丸めてから比べる（パートの境目の半コマで食い違わない）
  const tf = Math.round(t * OUT_FPS) / OUT_FPS;
  const p = g.plan_parts.find((x) => tf >= x.t0 - 1e-9 && tf < x.t1 - 1e-9);
  if (!p) return "動画の外です。手動パートの中でだけ足せます";
  if (p.name !== "manual") return `手動パートの中でだけ足せます（ここは「${PART_LABELS[p.name] ?? p.name}」パートです）`;
  return null;
}

// ---- NOW の先出し

/** 操作ごとの NOW の先出しの範囲（秒。本体の `NOW_LEAD_MAX` と同じ。外れたら本体も断る） */
export const NOW_LEAD_MAX = 10;

/** 先出しの秒の表示（整数は小数 1 桁、ほかは 2 桁まで。「汎用 1.0 秒」の 1.0） */
export function nowLeadText(v: number): string {
  return Number.isInteger(v) ? v.toFixed(1) : String(Math.round(v * 100) / 100);
}

/** 先出しの入力（秒）を保存してよいか。ok でなければ理由（通知に出す） */
export function nowLeadInputError(v: number): string | null {
  return Number.isFinite(v) && v >= 0 && v <= NOW_LEAD_MAX ? null : `NOW の先出しは 0〜${NOW_LEAD_MAX} 秒で入れてください`;
}

export type NowLeadFromHere = { ok: true; lead: number } | { ok: false; error: string };

/**
 * 「再生位置から」の先出し = 操作の出力の時刻 tOut − モニターの今の時刻 t を 1/60 秒に丸めたもの。
 * 操作が動画に出ていない・再生位置が操作より後・操作の 10 秒より前のときは理由つきの失敗（保存しない）
 */
export function nowLeadFromHere(tOut: number | null, t: number): NowLeadFromHere {
  if (tOut === null) return { ok: false, error: "この操作は動画に出ていないので、再生位置から決められません" };
  const lead = Math.round((tOut - t) * 60) / 60 || 0; // -0 は 0 に
  if (lead < 0) return { ok: false, error: "再生位置が操作より後です。操作より前に動かしてください" };
  if (lead > NOW_LEAD_MAX) return { ok: false, error: `再生位置が操作の ${NOW_LEAD_MAX} 秒より前です。先出しは 0〜${NOW_LEAD_MAX} 秒です` };
  return { ok: true, lead };
}

export type NowDelay = "prev" | "head" | null;

/** t_out より前（小さい t_out）の操作のうち最も遅い t_out（本体の now_starts の「前の操作」）。無ければ null */
export function prevOut(outs: { t_out: number | null }[], tOut: number | null): number | null {
  if (tOut === null) return null;
  let m: number | null = null;
  for (const o of outs) if (o.t_out !== null && o.t_out < tOut && (m === null || o.t_out > m)) m = o.t_out;
  return m;
}

/** NOW の始まりが、先出しどおり（tOut − lead）より遅れているか。遅れていれば、前の操作の瞬間 prev と同じなら "prev"、ほかは手動パートの頭で止まった "head" */
export function nowDelayed(tOut: number | null, tNow: number | null, lead: number, prev: number | null = null): NowDelay {
  if (tOut === null || tNow === null) return null;
  if (tNow <= tOut - Math.max(0, lead) + 1e-6) return null;
  return prev !== null && Math.abs(tNow - prev) < 1e-6 ? "prev" : "head";
}

/** 操作の一覧に出す NOW の始まり（「NOW 0:31.40〜」。前の操作で遅れていれば「（前の操作の後）」）と、その title。出ない操作は null */
export function nowStartLabel(r: Pick<OpRow, "tNow" | "tOut" | "nowDelayed">, tcOf: (t: number) => string): { text: string; title: string } | null {
  if (r.tNow === null) return null;
  const at = tcOf(r.tNow);
  if (!r.nowDelayed) return { text: `NOW ${at}〜`, title: "動画の NOW がこの操作に切り替わる時刻です" };
  if (r.nowDelayed === "head") {
    return { text: `NOW ${at}〜（手動パートの頭から）`, title: "先出しの長さだけ前に出すと手動パートより前になるので、手動パートの頭から出します" };
  }
  return {
    text: `NOW ${at}〜（前の操作の後）`,
    title: "先出しの長さだけ前に出すと前の操作がまだ終わっていないので、前の操作の瞬間まで遅らせています",
  };
}
