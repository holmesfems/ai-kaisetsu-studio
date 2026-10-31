// 攻略動画の画面の表の行（ui/src/logic/guide.ts）の確かめ
import test from "node:test";
import assert from "node:assert/strict";
import type { Guide } from "../src/gen/Guide.ts";
import type { GuideFixed } from "../src/gen/GuideFixed.ts";
import type { GuideOp } from "../src/gen/GuideOp.ts";
import type { GuideState } from "../src/gen/GuideState.ts";
import type { SrcAt } from "../src/gen/SrcAt.ts";
import {
  nowDelayed, nowLeadFromHere, prevOut, nowLeadInputError, nowLeadText, nowStartLabel,
  addCut, analysisSummary, BENCH_ITEMS, benchFromPercent, benchPercentText, anchorHereError, anchorRows, claudeUndoLabel, claudeUndoTitle, cutFromMarks, cutRows, DEFAULT_CUT_FADE, fixedDiffersFromSettings,
  anchorEdit, editWithRetry, opEdit, opGoneMessage, fixedRows, fmtGameTime, fmtSeconds, isRevConflict, manualSpan, opAddError, opMarkKind, opMarkTitle, opRows, parseSeconds, partRows, slotRows, slotToggleError,
  withAllUsed, withAnchorT, withCutFade, withFixedOwn, withOp, withoutCut, withoutOp, withSlotNote, withSlotToggled,
} from "../src/logic/guide.ts";

const FIXED: GuideFixed = {
  formation_end: 8, formation_tail: 0.5, manual_lead: 2, manual_tail: 1.5, auto_lead: 2, auto_tail: 1.5, result_len: 8, now_lead: 1,
};

function guide(over: Partial<Guide> = {}): Guide {
  return {
    version: 1, src: "A.MP4", anchors: {}, fixed: {}, cuts: [],
    layout: { scale: 0.7, manual_scale: 0.75, manual_x: 0, manual_y: 0, crops: [] }, ops: [], formation: { slots: [], used: [], notes: {} }, ...over,
  };
}

function state(over: Partial<GuideState> = {}): GuideState {
  return {
    guide: guide(), fixed_effective: FIXED, fixed_settings: FIXED, plan_parts: [], anchor_out: {}, ops_out: [], cuts_out: [],
    analysis: { exists: false, frames: 0, battles: [], warnings: [], anchors: {} }, error: null, scanning: false, warnings: [], rev: 1, claude_undo: null, audio_rev: null, audio_err: null, ...over,
  };
}

test("アンカーの表: 手で決めた値が使う値で、無ければ候補", () => {
  const g = state({
    guide: guide({
      anchors: { manual_end: { t: 170, auto: 181.043 }, manual_cost0: { auto: 9.208 }, auto_end: { t: 300.945, auto: 300.945 } },
    }),
    anchor_out: { manual_end: 171.5 },
  });
  const rows = anchorRows(g);
  assert.deepEqual(rows.map((r) => r.name), ["formation_dark", "manual_cost0", "manual_end", "auto_cost0", "auto_end"]);
  const by = (n: string) => rows.find((r) => r.name === n)!;
  assert.deepEqual([by("manual_end").value, by("manual_end").auto, by("manual_end").custom, by("manual_end").out], [170, 181.043, true, 171.5]);
  // 候補だけ: 使う値は候補、手では変えていない
  assert.deepEqual([by("manual_cost0").value, by("manual_cost0").t, by("manual_cost0").custom], [9.208, null, false]);
  // t が候補と同じなら「変えた」にしない
  assert.equal(by("auto_end").custom, false);
  // 何も無いアンカー
  assert.deepEqual([by("formation_dark").value, by("formation_dark").out], [null, null]);
});

test("固定値の表: 効いている値・この回だけ・設定の値を分ける", () => {
  const g = state({
    guide: guide({ fixed: { manual_lead: 3 } }),
    fixed_effective: { ...FIXED, manual_lead: 3, result_len: 5 },
    fixed_settings: { ...FIXED, result_len: 5 },
  });
  const rows = fixedRows(g);
  assert.equal(rows.length, 8);
  const lead = rows.find((r) => r.key === "manual_lead")!;
  assert.deepEqual([lead.effective, lead.own, lead.setting], [3, 3, 2]);
  const len = rows.find((r) => r.key === "result_len")!;
  assert.deepEqual([len.effective, len.own, len.setting], [5, null, 5]);
  // 設定と違う項目があれば「設定に保存」の対象
  assert.equal(fixedDiffersFromSettings(g), true);
  assert.equal(fixedDiffersFromSettings(state()), false);
  // guide.json が無い回でも設定の値が出る
  assert.equal(fixedRows(state({ guide: null }))[2].own, null);
});

test("パートの行: 名前は日本語、長さは終わり − 始まり", () => {
  const g = state({
    plan_parts: [{ name: "formation", t0: 0, t1: 8 }, { name: "manual", t0: 8, t1: 183.5 }, { name: "x", t0: 183.5, t1: 184 }],
  });
  const rows = partRows(g);
  assert.deepEqual(rows.map((r) => r.label), ["編成", "手動", "x"]);
  assert.equal(rows[1].len, 175.5);
});

test("アンカーを直す: 元は変えず、null で候補に戻す", () => {
  const g = guide({ anchors: { manual_end: { t: 170, auto: 181 } } });
  const a = withAnchorT(g, "manual_end", 171.25);
  assert.equal(a.anchors.manual_end.t, 171.25);
  assert.equal(a.anchors.manual_end.auto, 181);
  assert.equal(g.anchors.manual_end.t, 170);
  const b = withAnchorT(g, "manual_end", null);
  assert.equal("t" in b.anchors.manual_end, false);
  assert.equal(b.anchors.manual_end.auto, 181);
  // まだ無いアンカーにも書ける
  assert.equal(withAnchorT(g, "auto_end", 300).anchors.auto_end.t, 300);
});

test("この回だけの固定値を直す: null で設定の値に戻す", () => {
  const g = guide({ fixed: { manual_lead: 3 } });
  assert.equal(withFixedOwn(g, "auto_lead", 1.5).fixed.auto_lead, 1.5);
  assert.equal(withFixedOwn(g, "auto_lead", 1.5).fixed.manual_lead, 3);
  const r = withFixedOwn(g, "manual_lead", null);
  assert.deepEqual(r.fixed, {});
  assert.deepEqual(g.fixed, { manual_lead: 3 });
});

test("秒の入力: 小数・分:秒・空・読めない", () => {
  assert.equal(parseSeconds("12.4"), 12.4);
  assert.equal(parseSeconds(" 3 "), 3);
  assert.equal(parseSeconds("1:23.5"), 83.5);
  assert.equal(parseSeconds("0:05"), 5);
  assert.equal(parseSeconds(""), null);
  assert.equal(parseSeconds("abc"), undefined);
  assert.equal(parseSeconds("-1"), undefined);
  assert.equal(parseSeconds("1:2:3"), undefined);
  assert.equal(fmtSeconds(9.2080001), "9.208");
  assert.equal(fmtSeconds(3), "3");
  assert.equal(fmtSeconds(null), "");
});

test("解析の要約", () => {
  assert.equal(analysisSummary(state()), "まだ解析していません");
  const g = state({ analysis: { exists: true, frames: 18291, battles: [{ start: 1, end: 2, cost0: 1, end_band: 2 }], warnings: [], anchors: {} } });
  assert.match(analysisSummary(g), /18,291 コマ・戦闘区間 1 個/);
});

// ---------------------------------------------------------------- 段3

const at = (part: string, t_src: number, frame = 0): SrcAt => ({ frame, t_src, part });

test("ゲーム内の時間: 60 フレームで 1 秒、コスト 1 = 30 フレーム。分:秒:フレーム", () => {
  assert.equal(fmtGameTime(0), "00:00:00");
  // 1 コスト = 30 フレーム = 0.5 秒
  assert.equal(fmtGameTime(30), "00:00:30");
  assert.equal(fmtGameTime(60), "00:01:00");
  assert.equal(fmtGameTime(125), "00:02:05");
  assert.equal(fmtGameTime(3600 + 59), "01:00:59");
  // 解析の値は小数（1.042 = 約 1 フレーム）。フレームは切り捨て
  assert.equal(fmtGameTime(1.042), "00:00:01");
  assert.equal(fmtGameTime(59.9999), "00:00:59");
  assert.equal(fmtGameTime(4985), "01:23:05");
  // 読めていない・戦闘の前（負）は空
  assert.equal(fmtGameTime(null), "");
  assert.equal(fmtGameTime(undefined), "");
  assert.equal(fmtGameTime(-1), "");
  assert.equal(fmtGameTime(Number.NaN), "");
});

function op(id: string, over: Partial<GuideOp> = {}): GuideOp {
  return { id, t: 10, kind: "deploy", op: "", where: "", precise: false, cue_text: "", note: "", ...over };
}

test("操作の行: 出力の時刻・コスト・ゲーム内の時間・出ない理由", () => {
  const g = state({
    guide: guide({
      ops: [
        op("op1", { t: 30, kind: "deploy", op: "ウタゲ", where: "R3C5 ↑", precise: true, cue_text: "Kill15" }),
        op("op2", { t: 45, kind: "skill" }),
        op("op3", { t: 5, kind: "trust" }),
        op("op4", { t: 60, kind: "retreat" }),
        op("op5", { t: 70, kind: "" }),
      ],
    }),
    ops_out: [
      { id: "op1", t_out: 31.5, t_now: null, frame: 1800, cost: 27, game: 125, in_cut: false },
      { id: "op2", t_out: null, t_now: null, frame: 2700, cost: null, game: null, in_cut: true },
      { id: "op3", t_out: null, t_now: null, frame: 300, cost: 10, game: 0, in_cut: false },
      { id: "op4", t_out: 61, t_now: null, frame: 3600, cost: 12, game: 3600, in_cut: false },
    ],
  });
  const rows = opRows(g);
  assert.deepEqual(rows.map((r) => r.id), ["op1", "op2", "op3", "op4", "op5"]);
  const by = (id: string) => rows.find((r) => r.id === id)!;
  assert.deepEqual(
    [by("op1").tOut, by("op1").cost, by("op1").game, by("op1").kindLabel, by("op1").problem],
    [31.5, 27, "00:02:05", "配置", null],
  );
  assert.equal(by("op1").precise, true);
  // カットの中・手動の外は赤字の理由つき
  assert.match(by("op2").problem!, /カットの中/);
  assert.match(by("op3").problem!, /手動パートの外/);
  // 信頼枠は薄く、ゲーム内の時間は分:秒:フレーム
  assert.equal(by("op3").trust, true);
  assert.equal(by("op3").kindLabel, "信頼枠");
  assert.equal(by("op4").game, "01:00:00");
  // 組み立てられていない（ops_out が無い）操作は理由なし・出力の時刻なし
  assert.deepEqual([by("op5").tOut, by("op5").problem, by("op5").kindLabel], [null, null, "（未設定）"]);
  assert.deepEqual(opRows(state({ guide: null })), []);
});

test("操作を直す・消す: 元は変えず、残りの id は振り直さない", () => {
  const g = guide({ ops: [op("op1"), op("op2"), op("op3")] });
  const a = withOp(g, "op2", { op: "ウタゲ", precise: true })!;
  assert.equal(a.ops[1].op, "ウタゲ");
  assert.equal(a.ops[1].precise, true);
  assert.equal(g.ops[1].op, "");
  const b = withoutOp(a, "op2")!;
  assert.deepEqual(b.ops.map((o) => o.id), ["op1", "op3"]);
  assert.equal(a.ops.length, 3);
  // 知らない id（ほかで消された操作）は null。何も直さない
  assert.equal(withOp(g, "zzz", { op: "x" }), null);
  assert.equal(withoutOp(g, "zzz"), null);
  assert.equal(opGoneMessage("op7"), "操作 op7 はほかで消されました");
});

test("opEdit: 食い違いで取り直したら、最新の操作の値から決め直す（相対の動かし方が古い値のままにならない）", async () => {
  // 最初は op1 が t=10。保存が食い違い、取り直すと Claude が t=50 に動かしている
  const mk = (t: number): Guide => guide({ ops: [op("op1", { t })] });
  let local: { guide: Guide | null; rev: number } = { guide: mk(10), rev: 1 };
  const server = { guide: mk(10), rev: 1 };
  let conflict = true;
  const saved: number[] = [];
  const io = {
    current: () => local,
    async save(g: Guide, rev: number) {
      if (conflict) {
        conflict = false;
        server.guide = mk(50);
        server.rev = 2;
        return CONFLICT;
      }
      assert.equal(rev, server.rev);
      saved.push(g.ops[0].t);
      server.guide = g;
      return null;
    },
    async refetch() {
      local = { guide: server.guide, rev: server.rev };
      return true;
    },
  };
  const flags = { gone: false };
  const seen: number[] = [];
  // 「今の t から 1 足す」を非同期で決める（本物は guide_step の呼び出し）
  const r = await editWithRetry(io, opEdit("op1", async (o) => { seen.push(o.t); return { t: o.t + 1 }; }, flags));
  assert.deepEqual(r, { kind: "saved" });
  assert.deepEqual(seen, [10, 50]);
  assert.deepEqual(saved, [51]);
  assert.equal(flags.gone, false);
});

test("opEdit: 取り直した最新に操作が無ければ保存せず gone を立てる", async () => {
  const flags = { gone: false };
  const g = guide({ ops: [op("op1")] });
  let called = 0;
  const r = await opEdit("op9", () => { called++; return {}; }, flags)(g);
  assert.equal(r, null);
  assert.equal(flags.gone, true);
  assert.equal(called, 0);
  // 食い違いのあと、取り直した側で消されていた: skipped（保存しない）
  const io = {
    current: () => ({ guide: guide({ ops: [op("op1")] }), rev: 1 }),
    save: async () => CONFLICT,
    refetch: async () => true,
  };
  let fresh = false;
  const io2 = { ...io, current: () => ({ guide: fresh ? guide({ ops: [] }) : guide({ ops: [op("op1")] }), rev: 1 }), refetch: async () => { fresh = true; return true; } };
  const f2 = { gone: false };
  assert.deepEqual(await editWithRetry(io2, opEdit("op1", () => ({ note: "x" }), f2)), { kind: "skipped" });
  assert.equal(f2.gone, true);
});

test("anchorEdit: 最新のアンカーの値（手の値 → 候補）から決め直す。値が無ければ noValue", async () => {
  const flags = { noValue: false };
  const seen: number[] = [];
  const g = guide({ anchors: { manual_cost0: { t: 12, auto: 10 }, manual_end: { auto: 99 } } });
  const a = (await anchorEdit("manual_cost0", async (v) => { seen.push(v); return v + 1; }, flags)(g))!;
  assert.equal(a.anchors["manual_cost0"].t, 13);
  const b = (await anchorEdit("manual_end", async (v) => { seen.push(v); return v - 1; }, flags)(g))!;
  assert.equal(b.anchors["manual_end"].t, 98);
  assert.deepEqual(seen, [12, 99]);
  assert.equal(await anchorEdit("auto_end", async (v) => v, flags)(g), null);
  assert.equal(flags.noValue, true);
});

test("操作の印: 精密は◆、大まかは●、信頼枠は灰色の点", () => {
  assert.equal(opMarkKind({ trust: true, precise: true }), "trust");
  assert.equal(opMarkKind({ trust: false, precise: true }), "precise");
  assert.equal(opMarkKind({ trust: false, precise: false }), "rough");
});

test("手動パートの範囲（印をドラッグできる所）", () => {
  const g = state({ plan_parts: [{ name: "formation", t0: 0, t1: 8 }, { name: "manual", t0: 8, t1: 183.5 }] });
  const [a, b] = manualSpan(g)!;
  assert.equal(a, 8);
  assert.ok(Math.abs(b - (183.5 - 1 / 60)) < 1e-9);
  assert.equal(manualSpan(state()), null);
});

test("カットの行: 長さ・継ぎ目の出力の時刻（組み立てられていなければ null）", () => {
  const g = state({
    guide: guide({ cuts: [{ src_in: 40, src_out: 52, fade: 0.5 }, { src_in: 60, src_out: 62, fade: 0 }] }),
    cuts_out: [[35, 35.5], [50, 50]],
  });
  const rows = cutRows(g);
  assert.deepEqual(rows.map((r) => [r.srcIn, r.srcOut, r.len, r.fade, r.outIn, r.outOut]), [[40, 52, 12, 0.5, 35, 35.5], [60, 62, 2, 0, 50, 50]]);
  // 組み立てが無ければ継ぎ目は不明
  assert.deepEqual(cutRows(state({ guide: guide({ cuts: [{ src_in: 1, src_out: 2, fade: 0 }] }) })).map((r) => r.outIn), [null]);
});

test("イン・アウトからカットを足す: 素材の時刻にして、両端が手動パートの中でなければ断る", () => {
  const g = guide();
  // 手動パートの中の 2 点 → 素材の時刻で足す（フェードは既定）
  const r = cutFromMarks(g, 20, 30, at("manual", 40.0, 2400), at("manual", 52.0, 3120));
  assert.ok(r.ok);
  if (r.ok) assert.deepEqual(r.guide.cuts, [{ src_in: 40, src_out: 52, fade: DEFAULT_CUT_FADE }]);
  assert.deepEqual(g.cuts, []);
  // イン・アウトが無い・逆
  assert.match((cutFromMarks(g, null, 30, null, null) as { error: string }).error, /イン（I）とアウト（O）/);
  assert.match((cutFromMarks(g, 30, 20, at("manual", 1), at("manual", 2)) as { error: string }).error, /アウトはインより後/);
  // 動画の外
  assert.match((cutFromMarks(g, 20, 30, null, at("manual", 2)) as { error: string }).error, /動画の外/);
  // 手動パートの外（編成・自動）
  const e1 = cutFromMarks(g, 2, 30, at("formation", 1), at("manual", 2)) as { error: string };
  assert.match(e1.error, /インが手動パートの外.*編成/);
  const e2 = cutFromMarks(g, 20, 300, at("manual", 1), at("auto", 2)) as { error: string };
  assert.match(e2.error, /アウトが手動パートの外.*自動/);
  // 同じコマになった（間にコマが無い）ときは断る
  assert.ok(!cutFromMarks(g, 20, 20.001, at("manual", 5, 100), at("manual", 5, 100)).ok);
});

test("カットを足す・消す・フェードを直す: 重なるカットは断り、素材の時刻の順に並べる", () => {
  const g = guide({ cuts: [{ src_in: 60, src_out: 62, fade: 0 }] });
  const r = addCut(g, { src_in: 40, src_out: 52, fade: 0.25 });
  assert.ok(r.ok);
  if (!r.ok) return;
  assert.deepEqual(r.guide.cuts.map((c) => c.src_in), [40, 60]);
  assert.equal(g.cuts.length, 1);
  const bad = addCut(r.guide, { src_in: 51, src_out: 61, fade: 0.25 });
  assert.ok(!bad.ok);
  if (!bad.ok) assert.match(bad.error, /重なっています/);
  assert.ok(!addCut(g, { src_in: 5, src_out: 4, fade: 0 }).ok);
  assert.equal(withCutFade(r.guide, 0, 0.75).cuts[0].fade, 0.75);
  assert.equal(r.guide.cuts[0].fade, 0.25);
  assert.deepEqual(withoutCut(r.guide, 0).cuts.map((c) => c.src_in), [60]);
});

test("編成の枠: 空の used は全部使う。切り替えで used が変わり、全部使うなら空に戻る", () => {
  const g = guide();
  const rows = slotRows(g);
  assert.equal(rows.length, 12);
  assert.ok(rows.every((r) => r.used));
  // 全部使う状態で枠 3 を外す → 3 以外が used
  const a = withSlotToggled(g, 3);
  assert.deepEqual(a.formation.used, [0, 1, 2, 4, 5, 6, 7, 8, 9, 10, 11]);
  assert.equal(slotRows(a)[3].used, false);
  assert.equal(g.formation.used.length, 0);
  // もう一度切り替えると全部使うに戻る（used は空）
  assert.deepEqual(withSlotToggled(a, 3).formation.used, []);
  // 途中の状態
  const b = withSlotToggled(withSlotToggled(a, 7), 5);
  assert.deepEqual(slotRows(b).filter((r) => !r.used).map((r) => r.index), [3, 5, 7]);
  assert.deepEqual(withSlotToggled(b, 5).formation.used, [0, 1, 2, 4, 5, 6, 8, 9, 10, 11]);
  // 全部使うボタン
  assert.deepEqual(withAllUsed(b).formation.used, []);
  // 最後の 1 枠は外せない（used が空 = 全部使う になってしまうため）
  const one = guide({ formation: { slots: [], used: [4], notes: {} } });
  assert.match(slotToggleError(one, 4)!, /1 つも無くなります/);
  assert.equal(withSlotToggled(one, 4), one);
  assert.equal(slotToggleError(one, 2), null);
  assert.deepEqual(withSlotToggled(one, 2).formation.used, [2, 4]);
  // 範囲外は何もしない
  assert.equal(withSlotToggled(g, 12), g);
  // 枠を自分で書いた編成は、その数
  const custom = guide({ formation: { slots: [[0, 0, 1, 1], [1, 1, 2, 2]], used: [], notes: {} } });
  assert.equal(slotRows(custom).length, 2);
});

test("編成の枠の補足: 空にすると消す", () => {
  const g = guide();
  const a = withSlotNote(g, 2, "  Lv90 / 潜在5  ");
  assert.equal(a.formation.notes["2"], "Lv90 / 潜在5");
  assert.equal(slotRows(a)[2].note, "Lv90 / 潜在5");
  assert.deepEqual(g.formation.notes, {});
  const b = withSlotNote(a, 2, " ");
  assert.deepEqual(b.formation.notes, {});
});

test("アンカーを今のコマにする: そのアンカーのパートの中でなければ断る", () => {
  assert.equal(anchorHereError("manual_cost0", at("manual", 9.2083)), null);
  assert.equal(anchorHereError("formation_dark", at("formation", 3.475)), null);
  assert.equal(anchorHereError("auto_end", at("auto", 300.9)), null);
  const e = anchorHereError("manual_end", at("auto", 220))!;
  assert.match(e, /手動の作戦終了/);
  assert.match(e, /「手動」パートの中/);
  assert.match(e, /今の位置は「自動」パート/);
  assert.match(anchorHereError("manual_end", null)!, /動画の外/);
  assert.match(anchorHereError("formation_dark", at("result", 1))!, /リザルト/);
});

// ---------------------------------------------------------------- 保存の食い違い（rev）と Claude の変更

const CONFLICT = { code: "bad_input", message: "ほかで変わりました（guide.json が、画面に出ている内容から変わっています）", detail: "x" };

test("保存の食い違いは、理由の頭の文で見分ける（本体の REV_CONFLICT と同じ）", () => {
  assert.equal(isRevConflict(CONFLICT), true);
  assert.equal(isRevConflict({ code: "failed", message: "攻略動画にしていません", detail: "" }), false);
  assert.equal(isRevConflict("ほかで変わりました"), true);
  assert.equal(isRevConflict(new Error("disk full")), false);
});

/** editWithRetry の道具。server は本体の guide と rev。stale を true にすると、保存の前に Claude が先に変えた状態にする */
function fakeIo(opts: { conflicts: number; refetchOk?: boolean }) {
  const server = { guide: guide({ src: "A.MP4" }), rev: 1 };
  let local = { guide: guide({ src: "A.MP4" }) as Guide | null, rev: 1 };
  let conflictsLeft = opts.conflicts;
  const log: string[] = [];
  return {
    log, server, local: () => local,
    io: {
      current: () => local,
      async save(g: Guide, rev: number) {
        log.push(`save rev=${rev} src=${g.src} ops=${g.ops.length}`);
        if (conflictsLeft > 0) {
          conflictsLeft--;
          // Claude が先に操作を足していた
          server.guide = { ...server.guide, ops: [{ id: "op1", t: 1, kind: "deploy", op: "", where: "", precise: false, cue_text: "", note: "" } as GuideOp] };
          server.rev += 1;
          return CONFLICT;
        }
        if (rev !== server.rev) return CONFLICT;
        server.guide = g;
        server.rev += 1;
        local = { guide: g, rev: server.rev };
        return null;
      },
      async refetch() {
        log.push("refetch");
        if (opts.refetchOk === false) return false;
        local = { guide: server.guide, rev: server.rev };
        return true;
      },
    },
  };
}

test("editWithRetry: 食い違いなら取り直して、最新の中身に fn を当て直して保存する（Claude の変更を消さない）", async () => {
  const f = fakeIo({ conflicts: 1 });
  const r = await editWithRetry(f.io, (g) => ({ ...g, src: "B.MP4" }));
  assert.deepEqual(r, { kind: "saved" });
  assert.deepEqual(f.log, ["save rev=1 src=B.MP4 ops=0", "refetch", "save rev=2 src=B.MP4 ops=1"]);
  // 2 回目は Claude が足した操作を持ったまま、画面の直しも入っている
  assert.equal(f.server.guide.src, "B.MP4");
  assert.equal(f.server.guide.ops.length, 1);
});

test("editWithRetry: やり直すのは 1 回だけ。2 回続けて食い違えば error", async () => {
  const f = fakeIo({ conflicts: 2 });
  const r = await editWithRetry(f.io, (g) => ({ ...g, src: "B.MP4" }));
  assert.equal(r.kind, "error");
  assert.deepEqual(f.log.filter((l) => l === "refetch").length, 1);
  assert.equal(f.log.filter((l) => l.startsWith("save")).length, 2);
});

test("editWithRetry: 食い違い以外の失敗は取り直さない・fn が null なら保存しない・取り直せなければ skipped", async () => {
  const io = fakeIo({ conflicts: 0 }).io;
  const bad = { ...io, save: async () => ({ code: "failed", message: "disk full", detail: "" }) };
  assert.equal((await editWithRetry(bad, (g) => g)).kind, "error");
  const f = fakeIo({ conflicts: 0 });
  assert.deepEqual(await editWithRetry(f.io, () => null), { kind: "skipped" });
  assert.deepEqual(f.log, []);
  const g = fakeIo({ conflicts: 1, refetchOk: false });
  assert.deepEqual(await editWithRetry(g.io, (x) => ({ ...x, src: "C.MP4" })), { kind: "skipped" });
  // 攻略動画でない（guide が null）なら何もしない
  assert.deepEqual(await editWithRetry({ ...io, current: () => ({ guide: null, rev: 0 }) }, (x) => x), { kind: "skipped" });
});

test("Claude の変更を戻すボタンの文: 理由は詰めて、全文と時刻は title に", () => {
  const u = { id: 1, at: "2026-10-21T21:03:00", reason: "Kill15 のタイミングの文を足す" };
  assert.equal(claudeUndoLabel(u), "Claude の変更を戻す（Kill15 のタイミングの文を足す）");
  const long = { id: 1, at: u.at, reason: "とても長い理由".repeat(10) };
  const label = claudeUndoLabel(long, 10);
  assert.ok(label.includes("…）") && [...label].length < 40, label);
  assert.equal(claudeUndoLabel({ id: 1, at: u.at, reason: "  " }), "Claude の変更を戻す");
  const t = claudeUndoTitle(u);
  assert.ok(t.includes("2026-10-21 21:03:00") && t.includes("Kill15"), t);
});

test("待機欄: 0〜1 の値は % の文字で出す（小数 1 桁まで・末尾の 0 は落とす）", () => {
  assert.equal(benchPercentText(0.5), "50");
  assert.equal(benchPercentText(0.333), "33.3");
  assert.equal(benchPercentText(1), "100");
  assert.equal(benchPercentText(0.1), "10");
});

test("待機欄: % の入力を 0〜1 の値にして、ほかの項目は変えない", () => {
  const b = { scale: 0.5, opacity: 0.5 };
  assert.deepEqual(benchFromPercent(b, "scale", "40"), { ok: true, bench: { scale: 0.4, opacity: 0.5 } });
  assert.deepEqual(benchFromPercent(b, "opacity", " 75% "), { ok: true, bench: { scale: 0.5, opacity: 0.75 } });
  assert.deepEqual(benchFromPercent(b, "scale", "33.3"), { ok: true, bench: { scale: 0.333, opacity: 0.5 } });
  // 元は変えない
  assert.deepEqual(b, { scale: 0.5, opacity: 0.5 });
});

test("待機欄: 範囲の端は通り、外・数でない入力は理由つきで断る（本体の範囲と同じ）", () => {
  const b = { scale: 0.5, opacity: 0.5 };
  assert.deepEqual(BENCH_ITEMS.map((i) => [i.key, i.min, i.max]), [["scale", 20, 100], ["opacity", 10, 100]]);
  for (const [key, text] of [["scale", "20"], ["scale", "100"], ["opacity", "10"], ["opacity", "100"]] as const) {
    assert.equal(benchFromPercent(b, key, text).ok, true, `${key} ${text}`);
  }
  for (const [key, text, msg] of [
    ["scale", "19", "20〜100%"], ["scale", "101", "20〜100%"], ["opacity", "9", "10〜100%"], ["opacity", "0", "10〜100%"],
    ["scale", "", "数（%）"], ["opacity", "半分", "数（%）"], ["scale", "-5", "数（%）"], ["scale", "5e1", "数（%）"],
  ] as const) {
    const r = benchFromPercent(b, key, text);
    assert.equal(r.ok, false, `${key} ${text}`);
    if (!r.ok) assert.ok(r.error.includes(msg), r.error);
  }
});

test("操作を足せるか: 手動パートの中だけ。外・動画の外・組み立て無しは理由つき（本体の guide_op_add と同じ判断）", () => {
  const parts = [
    { name: "formation", t0: 0, t1: 8 }, { name: "manual", t0: 8, t1: 183.5 },
    { name: "auto", t0: 183.5, t1: 200 }, { name: "result", t0: 200, t1: 208 },
  ];
  const g = state({ plan_parts: parts, cuts_out: [[60, 60.25]] });
  assert.equal(opAddError(g, 8), null, "手動の頭");
  assert.equal(opAddError(g, 100), null);
  assert.equal(opAddError(g, 60.1), null, "カットの継ぎ目（出力に素材がある）は本体も断らない");
  assert.equal(opAddError(g, 183.5 - 1 / 60), null, "手動の最後のコマ");
  assert.match(opAddError(g, 183.5 - 1e-6)!, /「自動」/, "半コマより近ければ本体と同じく次のパートのコマ");
  assert.match(opAddError(g, 183.5)!, /手動パートの中でだけ.*「自動」/, "手動の終わりは次のパート");
  assert.match(opAddError(g, 3)!, /「編成」/);
  assert.match(opAddError(g, 205)!, /「リザルト」/);
  assert.match(opAddError(g, 300)!, /動画の外/);
  assert.match(opAddError(g, -1)!, /動画の外/);
  assert.match(opAddError(state({ plan_parts: [] }), 10)!, /組み立てられていません/);
  assert.match(opAddError(state({ guide: null, plan_parts: parts }), 10)!, /攻略動画にしていません/);
});

test("補足（info）: 行に出る・古い guide.json（info 無し）は空・印の title に足す", () => {
  const g = state({ guide: guide({ ops: [
    { id: "op1", t: 1, kind: "deploy", op: "ウタゲ", where: "R3C5", info: "S2 は\n 溜めてから", precise: false, cue_text: "", note: "" },
    { id: "op2", t: 2, kind: "skill", op: "A", where: "", precise: false, cue_text: "", note: "" } as unknown as GuideOp,
  ] }) });
  const rows = opRows(g);
  assert.equal(rows[0].info, "S2 は\n 溜めてから");
  assert.equal(rows[1].info, "", "info の無い操作は空");
  assert.equal(opMarkTitle(rows[0]), "配置 ウタゲ R3C5\n補足: S2 は 溜めてから");
  assert.equal(opMarkTitle(rows[1]), "スキル A");
  assert.equal(opMarkTitle({ kindLabel: "", op: "", where: "", info: "" }), "（未設定）");
  assert.equal(withOp(g.guide!, "op1", { info: "x" })!.ops[0].info, "x");
});

test("NOW の先出し: 汎用値の表の行・操作の行の個別値と NOW の始まり", () => {
  const f = fixedRows(state()).find((r) => r.key === "now_lead")!;
  assert.deepEqual([f.label, f.effective, f.own, f.setting], ["NOW の先出し", 1, null, 1]);
  const g = state({
    guide: guide({ ops: [
      op("op1", { t: 30, kind: "deploy", now_lead: 3 }),
      op("op2", { t: 31, kind: "skill" }),
      op("op3", { t: 32, kind: "trust" }),
      op("op4", { t: 33, kind: "deploy" }),
    ] }),
    ops_out: [
      { id: "op1", t_out: 31.5, t_now: 28.5, frame: 1, cost: null, game: null, in_cut: false },
      { id: "op2", t_out: 32.5, t_now: 31.5, frame: 2, cost: null, game: null, in_cut: false },
      { id: "op3", t_out: 33, t_now: 33, frame: 3, cost: null, game: null, in_cut: false },
      { id: "op4", t_out: null, t_now: null, frame: 4, cost: null, game: null, in_cut: true },
    ],
  });
  const rows = opRows(g);
  assert.deepEqual(rows.map((r) => [r.nowLead, r.nowLeadGeneric, r.tNow, r.nowDelayed]), [
    [3, 1, 28.5, null], [null, 1, 31.5, null], [null, 1, 33, null], [null, 1, null, null],
  ]);
  // 前の操作で遅れた（先出し 1 秒のはずが、前の操作の 32.0 まで）
  const late = opRows(state({
    guide: guide({ ops: [op("op0", { t: 0.5, kind: "deploy" }), op("op1", { t: 1, kind: "deploy" })] }),
    ops_out: [
      { id: "op0", t_out: 32.0, t_now: 31.8, frame: 0, cost: null, game: null, in_cut: false },
      { id: "op1", t_out: 32.5, t_now: 32.0, frame: 1, cost: null, game: null, in_cut: false },
    ],
  }));
  assert.equal(late[1].nowDelayed, "prev", "32.5 − 1 = 31.5 より遅い、前の操作の 32.0 から始まる");
  assert.equal(late[0].nowDelayed, "head", "前の操作が無いのに遅れたのは手動パートの頭");
});

test("NOW の先出し: 遅れの判断と表示の文", () => {
  assert.equal(nowDelayed(32.5, 31.5, 1, 30), null);
  assert.equal(nowDelayed(32.5, 32.0, 1, 32.0), "prev");
  assert.equal(nowDelayed(32.5, 32.0, 1, null), "head");
  assert.equal(nowDelayed(32.5, 32.5, 0, 31), null, "先出し 0（信頼枠）は遅れとは言わない");
  assert.equal(nowDelayed(null, null, 1), null);
  assert.equal(prevOut([{ t_out: 30 }, { t_out: 31 }, { t_out: 32 }, { t_out: null }], 32), 31);
  assert.equal(prevOut([{ t_out: 32 }], 32), null, "同じ時刻は前の操作にしない");
  const tcOf = (t: number) => `T${t}`;
  assert.deepEqual(nowStartLabel({ tNow: 31.5, tOut: 32.5, nowDelayed: null }, tcOf)!.text, "NOW T31.5〜");
  assert.equal(nowStartLabel({ tNow: 8, tOut: 8.5, nowDelayed: "head" }, tcOf)!.text, "NOW T8〜（手動パートの頭から）");
  const d = nowStartLabel({ tNow: 32, tOut: 32.5, nowDelayed: "prev" }, tcOf)!;
  assert.equal(d.text, "NOW T32〜（前の操作の後）");
  assert.match(d.title, /前の操作/);
  assert.equal(nowStartLabel({ tNow: null, tOut: null, nowDelayed: null }, tcOf), null);
  // 実際の tc（60fps）で
  assert.equal(nowStartLabel({ tNow: 31.4, tOut: 32, nowDelayed: null }, (t) => `0:${t.toFixed(2)}`)!.text, "NOW 0:31.40〜");
  assert.equal(nowLeadText(1), "1.0");
  assert.equal(nowLeadText(2.5), "2.5");
  assert.equal(nowLeadText(0.333), "0.33");
});

test("NOW の先出し: 再生位置からの計算（1/60 に丸める・後ろ・10 秒超・t_out 無し）", () => {
  assert.deepEqual(nowLeadFromHere(31.5, 30.5), { ok: true, lead: 1 });
  // 1/60 に丸める（31.5 − 30.51 = 0.99 → 59.4 フレーム → 59/60）
  const r = nowLeadFromHere(31.5, 30.51);
  assert.ok(r.ok && Math.abs(r.lead - 59 / 60) < 1e-12);
  // ちょうど操作の位置は 0、10 秒ちょうどは通る
  assert.deepEqual(nowLeadFromHere(31.5, 31.5), { ok: true, lead: 0 });
  assert.deepEqual(nowLeadFromHere(40, 30), { ok: true, lead: 10 });
  // 半コマ以内の後ろは 0 に丸まる。1 コマ以上後ろは断る
  assert.deepEqual(nowLeadFromHere(31.5, 31.5 + 0.2 / 60), { ok: true, lead: 0 });
  const after = nowLeadFromHere(31.5, 31.6);
  assert.ok(!after.ok && /操作より後/.test(after.error));
  const far = nowLeadFromHere(50, 30);
  assert.ok(!far.ok && /10 秒より前/.test(far.error));
  const none = nowLeadFromHere(null, 30);
  assert.ok(!none.ok && /動画に出ていない/.test(none.error));
  // 数の入力の範囲
  assert.equal(nowLeadInputError(0), null);
  assert.equal(nowLeadInputError(10), null);
  assert.match(nowLeadInputError(10.5)!, /0〜10 秒/);
  assert.match(nowLeadInputError(-1)!, /0〜10 秒/);
});

test("NOW の先出し: withOp は undefined の項目を消す（汎用値に戻す）", () => {
  const g = guide({ ops: [op("op1", { now_lead: 2 }), op("op2")] });
  const a = withOp(g, "op1", { now_lead: undefined })!;
  assert.equal("now_lead" in a.ops[0], false);
  assert.equal(JSON.stringify(a.ops[0]).includes("now_lead"), false);
  // 元は変えない
  assert.equal(g.ops[0].now_lead, 2);
  assert.equal(withOp(g, "op1", { now_lead: 4 })!.ops[0].now_lead, 4);
});
