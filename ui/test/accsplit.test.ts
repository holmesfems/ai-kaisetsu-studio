// アクセント欄で単語をモーラの間で分ける処理の確かめ
import { test } from "node:test";
import assert from "node:assert/strict";
import { canSplit, splitWord } from "../src/logic/accsplit.ts";

const jin = { phoneme: "j,u|u|o|o|m,u|j,i|N", hl: "lhhllll", pronunciation: "ジューオームジン", text: "縦横無尽", original: "縦横無尽" };

test("縦横無尽を「ジューオー｜ムジン」で分ける", () => {
  const [a, b] = splitWord(jin, 4)!;
  assert.deepEqual(a, { phoneme: "j,u|u|o|o", hl: "lhhl", pronunciation: "ジューオー", text: "縦横", original: "縦横" });
  assert.deepEqual(b, { phoneme: "m,u|j,i|N", hl: "lll", pronunciation: "ムジン", text: "無尽", original: "無尽" });
});

test("かなだけの語は、字の位置ちょうどで分ける", () => {
  const w = { phoneme: "k,a|k,e|n,u|k,e", hl: "lhhh", pronunciation: "カケヌケ", text: "かけぬけ", original: "かけぬけ" };
  const [a, b] = splitWord(w, 1)!;
  assert.equal(a.text, "か");
  assert.equal(b.text, "けぬけ");
});

test("1字の語は、右側を読みのカタカナで書く", () => {
  const w = { phoneme: "t,e|N", hl: "hl", pronunciation: "テン", text: "天", original: "天" };
  const [a, b] = splitWord(w, 1)!;
  assert.equal(a.text, "天");
  assert.equal(b.text, "ン");
});

test("端・モーラ数が合わない語は分けない", () => {
  assert.equal(canSplit(jin, 0), false);
  assert.equal(canSplit(jin, 7), false);
  assert.equal(splitWord({ ...jin, phoneme: "j,u|u" }, 2), null);
});
