// 効果音の設定（ui/src/sebank.ts）の下書きの扱いの確かめ（issue 旧#14）。アプリを起動せずに `npm test` で動く。
import { test } from "node:test";
import assert from "node:assert/strict";
import {
  contextSpan, dbToGain, draftOf, draftPatch, fmtDb, isDirty, nearestIndex, pickUse, rebaseDraft, seCounts, seLabel, seTimes,
} from "../src/logic/sebank.ts";
import type { Desc, Group, SeBank, SeKind } from "../src/types.ts";

const K = (kind: string, use: string[], cands: [string, number][], extra: Partial<SeKind> = {}): SeKind => ({
  kind, label: kind, level: -14, prio: 50, bankable: true, lite: false, stem: "", use, explicit: true, times: [],
  cands: cands.map(([src, db]) => ({ src, db, exists: true, extra: false, folder: false, source: "" })), ...extra,
});
const BANK = (kinds: SeKind[]): SeBank => ({ path: "fx/se/bank.json", exists: true, files: [], db_range: [-30, 12], kinds });
const G = (key: string, se: [number, string][]): Group =>
  ({ key, se: se.map(([t, kind]) => ({ t, kind })) } as unknown as Group);
const D = (groups: Group[]): Desc => ({ groups } as unknown as Desc);

test("候補を選ぶ: ふつうは 1 つだけ、順番に使うなら足し外し（最後の 1 つは外さない）", () => {
  assert.deepEqual(pickUse(["synth", "H7.wav"], "H9.wav", false), ["H9.wav"]);
  assert.deepEqual(pickUse(["synth"], "H9.wav", true), ["synth", "H9.wav"]);
  assert.deepEqual(pickUse(["synth", "H9.wav"], "synth", true), ["H9.wav"]);
  assert.deepEqual(pickUse(["H9.wav"], "H9.wav", true), ["H9.wav"]);
});

test("下書きの差分: 変えた種類・項目だけを se_bank_set に渡す（0 dB は null で消す）", () => {
  const bank = BANK([K("slam", ["synth", "H7.wav"], [["synth", 0], ["H7.wav", -3]]), K("wipe", ["synth"], [["synth", 0]])]);
  const draft = Object.fromEntries(bank.kinds.map((k) => [k.kind, draftOf(k)]));
  assert.deepEqual(draftPatch(bank, draft), {});
  assert.equal(isDirty(bank, draft), false);
  draft.slam = { use: ["H7.wav"], db: { synth: 0, "H7.wav": 0 } };
  draft.wipe = { use: ["synth"], db: { synth: 2.345 } };
  assert.deepEqual(draftPatch(bank, draft), {
    slam: { use: ["H7.wav"], db: { "H7.wav": null } },
    wipe: { db: { synth: 2.35 } },
  });
  assert.equal(isDirty(bank, draft), true);
});

test("棚の種類の下書き: 音量は選んでいる音の分だけ送る（選んでいない候補の音量は送らない）", () => {
  const bank = BANK([K("guide_done", ["guide_done/a.wav"], [["synth", 0], ["guide_done/a.wav", -3], ["guide_done/b.wav", 0]], { shelf: true })]);
  const draft = Object.fromEntries(bank.kinds.map((k) => [k.kind, draftOf(k)]));
  assert.deepEqual(draftPatch(bank, draft), {});
  // 選んでいない b の音量を動かしても何も送らない
  draft.guide_done.db["guide_done/b.wav"] = 5;
  assert.deepEqual(draftPatch(bank, draft), {});
  // b を選び直すと、古い a の音量（-3）は送らず、b の音量だけ送る
  draft.guide_done = { use: ["guide_done/b.wav"], db: { synth: 0, "guide_done/a.wav": -3, "guide_done/b.wav": 5 } };
  assert.deepEqual(draftPatch(bank, draft), { guide_done: { use: ["guide_done/b.wav"], db: { "guide_done/b.wav": 5 } } });
});

test("読み直したとき: 保存していない変更は残し、ほかの種類は新しい設定にする", () => {
  const old = BANK([K("slam", ["synth"], [["synth", 0], ["H7.wav", 0]]), K("wipe", ["synth"], [["synth", 0]])]);
  const draft = { slam: { use: ["H7.wav"], db: { synth: 0, "H7.wav": -6 } }, wipe: draftOf(old.kinds[1]) };
  // Claude が wipe を変え、slam の候補に H9 を足した
  const next = BANK([
    K("slam", ["synth"], [["synth", 0], ["H7.wav", 0], ["H9.wav", 0]]),
    K("wipe", ["W9.wav"], [["synth", 0], ["W9.wav", 1]]),
  ]);
  const r = rebaseDraft(old, next, draft);
  assert.deepEqual(r.slam, { use: ["H7.wav"], db: { synth: 0, "H7.wav": -6, "H9.wav": 0 } });
  assert.deepEqual(r.wipe, { use: ["W9.wav"], db: { synth: 0, "W9.wav": 1 } });
  // 選んでいた候補が外されたら、保存してある方に戻す
  const gone = BANK([K("slam", ["synth"], [["synth", 0]]), next.kinds[1]]);
  assert.deepEqual(rebaseDraft(old, gone, draft).slam.use, ["synth"]);
  // 初めて読んだとき
  assert.deepEqual(rebaseDraft(null, next, {}).wipe, draftOf(next.kinds[1]));
});

test("鳴る時刻: 微調整のずらしを入れ、隠したクリップは除く。確定音は数え上げの時刻", () => {
  const desc = D([G("a@c1#1", [[3, "slam"], [1, "wipe"]]), G("b@c2#1", [[2, "slam"]]), G("c@c3#1", [[5, "slam"], [6, "count"]])]);
  const ov = { "b@c2#1": { shift: 0.5 }, "c@c3#1": { hidden: true } };
  assert.deepEqual(seTimes(desc, ov, "slam"), [2.5, 3]);
  assert.deepEqual(seTimes(desc, {}, "confirm"), [6]);
  assert.deepEqual(seTimes(null, {}, "slam"), []);
  assert.deepEqual(seCounts(desc, ov), { slam: 2, wipe: 1, confirm: 0, confirm_neg: 0 });
  assert.equal(seCounts(desc, {}).confirm, 1);
});

test("前後を聞く区間と、近い箇所の選び方", () => {
  assert.deepEqual(contextSpan(10, 100), [8.5, 12.5]);
  assert.deepEqual(contextSpan(0.5, 100), [0, 3]);
  assert.deepEqual(contextSpan(99, 100), [97.5, 100]);
  const ts = [1, 5, 9];
  assert.equal(nearestIndex(ts, 4.2), 1);
  assert.equal(nearestIndex(ts, 5, 1), 2);
  assert.equal(nearestIndex(ts, 9.5, 1), 0); // 最後の次は最初へ
  assert.equal(nearestIndex(ts, 5, -1), 0);
  assert.equal(nearestIndex(ts, 0.5, -1), 2);
  assert.equal(nearestIndex([], 3), -1);
});

test("音量の表示と倍率", () => {
  assert.equal(fmtDb(0), "0 dB");
  assert.equal(fmtDb(0.02), "0 dB");
  assert.equal(fmtDb(-3), "-3.0 dB");
  assert.equal(fmtDb(2.5), "+2.5 dB");
  assert.ok(Math.abs(dbToGain(-6) - 0.501) < 0.001);
  assert.equal(dbToGain(0), 1);
});

test("種類の表示名: 設定（ワーカーが付けた label）を引き、無ければ種類の名前", () => {
  const bank = BANK([K("chapter", ["synth"], [["synth", 0]], { label: "章タイトル" }), K("mine", ["synth"], [["synth", 0]])]);
  assert.equal(seLabel("chapter", bank), "章タイトル");
  assert.equal(seLabel("mine", bank), "mine");
  assert.equal(seLabel("glitch_s", bank), "glitch_s");
  assert.equal(seLabel("chapter", null), "chapter");
  assert.equal(seLabel("chapter"), "chapter");
});
