// 読み上げ辞書（voice_dict.json）の登録の決まりの確かめ
import { test } from "node:test";
import assert from "node:assert/strict";
import { cleanReading, flatHl, guessReading, moras, setMora, upsertEntry } from "../src/logic/dict.ts";

test("読みはカタカナにして、記号などは落とす", () => {
  assert.equal(cleanReading("しぐれ"), "シグレ");
  assert.equal(cleanReading("じぇ・しか "), "ジェシカ");
  assert.deepEqual(moras("じぇしか"), ["ジェ", "シ", "カ"]);
  assert.equal(flatHl(3), "lhh");
  assert.equal(flatHl(0), "");
});

test("モーラのクリックは「低 → 高 → 低」の 1 つの山を保つ", () => {
  const go = (hl: string, j: number) => setMora([...hl], j).join("");
  assert.equal(go("lhhh", 0), "hlll"); // 頭を押すと頭高
  assert.equal(go("hlll", 2), "hhhl"); // 低を押すと山を広げる
  assert.equal(go("lhhh", 3), "lhhl"); // 山の中を押すとそこから下げる
  assert.equal(go("lhhl", 1), "llhl"); // 山の頭を押すと山を縮める
  assert.equal(go("llll", 2), "llhl");
  assert.equal(go("hlhl", 3), "hhhh"); // 山が 2 つあったら 1 つにしてから
});

test("解析の語から、選んだ表記にちょうど重なる語の読みとアクセントを拾う", () => {
  const words = [
    { orig: "時", pron: "し", hl: "h" },
    { orig: "雨", pron: "ぐれ", hl: "ll" },
    { orig: "は", pron: "わ", hl: "l" },
  ];
  assert.deepEqual(guessReading(words, "時雨"), { reading: "シグレ", accent: "hll" });
  assert.deepEqual(guessReading(words, "雨は"), { reading: "グレワ", accent: "lll" });
  // 語の途中で切れる表記は拾わない
  assert.deepEqual(guessReading(words, "時雨は!"), { reading: "", accent: "" });
  assert.deepEqual(guessReading([{ orig: "時雨", pron: "しぐれ", hl: "" }], "時"), { reading: "", accent: "" });
  // モーラ数と hl が合わなければ、アクセントは空（平板から始める）
  assert.deepEqual(guessReading([{ orig: "AB", pron: "えーびー", hl: "lh" }], "AB"), { reading: "エービー", accent: "" });
});

test("登録は同じ表記を置き換え、表記を変えた編集は元の位置に置く", () => {
  const e = (surface: string, reading = "ア") => ({ surface, reading });
  assert.deepEqual(upsertEntry([e("A"), e("B")], e("C"), "").map((x) => x.surface), ["A", "B", "C"]);
  assert.deepEqual(upsertEntry([e("A"), e("B"), e("C")], e("B", "イ"), "B"), [e("A"), e("B", "イ"), e("C")]);
  assert.deepEqual(upsertEntry([e("A"), e("B"), e("C")], e("X"), "B").map((x) => x.surface), ["A", "X", "C"]);
  // 新しい表記がほかの項目と同じなら、それも置き換えて 1 つにする
  assert.deepEqual(upsertEntry([e("A"), e("B"), e("C")], e("C"), "A").map((x) => x.surface), ["C", "B"]);
});
