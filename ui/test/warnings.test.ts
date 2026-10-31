// 演出エンジンの警告の知らせ方（ui/src/warnings.ts、issue 旧#30）。アプリを起動せずに `npm test` で動く。
import { test } from "node:test";
import assert from "node:assert/strict";
import { freshWarnings, splitWarnings, warningToast } from "../src/logic/warnings.ts";

const W = (key: string, message = key + " の警告") => ({ key, message });

test("初めての警告は知らせ、同じ警告は読み込み直しても知らせない", () => {
  let r = freshWarnings(new Set(), [W("se"), W("gui:a")]);
  assert.deepEqual(r.fresh.map((w) => w.key), ["se", "gui:a"]);
  r = freshWarnings(r.seen, [W("gui:a"), W("se")]);
  assert.deepEqual(r.fresh, []);
});

test("増えた警告・文が変わった警告だけを知らせる", () => {
  let r = freshWarnings(new Set(), [W("se", "古い理由")]);
  r = freshWarnings(r.seen, [W("se", "新しい理由"), W("font:x")]);
  assert.deepEqual(r.fresh.map((w) => w.message), ["新しい理由", "font:x の警告"]);
});

test("消えた警告は忘れる（直した後にまた起きたら知らせる）", () => {
  let r = freshWarnings(new Set(), [W("gui:a")]);
  r = freshWarnings(r.seen, []);
  assert.equal(r.seen.size, 0);
  r = freshWarnings(r.seen, [W("gui:a")]);
  assert.deepEqual(r.fresh.map((w) => w.key), ["gui:a"]);
});

test("警告が無い返事（古いワーカー・未読み込み）でも落ちない", () => {
  assert.deepEqual(freshWarnings(new Set(["x"]), undefined).fresh, []);
  assert.deepEqual(freshWarnings(new Set(), null).seen.size, 0);
});

test("通知の文: 1 件はその文と一覧の場所、多いときは件数と先頭 3 件", () => {
  const one = warningToast([W("se", "効果音の理由")]);
  assert.match(one, /^効果音の理由/);
  assert.match(one, /「演出」から一覧/);
  const t = warningToast([W("a"), W("b"), W("c"), W("d"), W("e")]);
  assert.match(t, /5 件/);
  assert.match(t, /・a の警告/);
  assert.doesNotMatch(t, /d の警告/);
  assert.match(t, /ほか 2 件/);
});

test("info の警告は通知の対象にしない（警告と分けて数える）", () => {
  const info = { key: "kit-missing", message: "演出の kit がありません", level: "info" };
  const r = freshWarnings(new Set(), [info, W("se")]);
  assert.deepEqual(r.fresh.map((w) => w.key), ["se"]);
  assert.equal(r.seen.size, 1);
  const s = splitWarnings([info, W("se"), { ...W("x"), level: "warn" }]);
  assert.deepEqual(s.warns.map((w) => w.key), ["se", "x"]);
  assert.deepEqual(s.infos.map((w) => w.key), ["kit-missing"]);
  assert.deepEqual(splitWarnings(undefined), { warns: [], infos: [] });
});
