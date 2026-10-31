// 用語集（docs/audit/ui-strings-changes.md）の「使わない語」が画面の文言に無いこと、
// 文言の表（logic/terms.ts）が型の全部の値を持つことを見る
import test from "node:test";
import assert from "node:assert/strict";
import { readdirSync, readFileSync, statSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { CLAUDE_ACTIONS, DELIVERY, DELIVERY_ORDER, PROPOSAL_STATUS, TRACK_KIND } from "../src/logic/terms.ts";

const root = fileURLToPath(new URL("../src/", import.meta.url));

function walk(dir: string): string[] {
  const out: string[] = [];
  for (const n of readdirSync(dir)) {
    const p = dir + n;
    if (statSync(p).isDirectory()) {
      if (n !== "gen") out.push(...walk(p + "/"));
    } else if (/\.tsx?$/.test(n)) out.push(p);
  }
  return out;
}

/** コメントを空白に置き換える（文字列・テンプレートの中の // や /* は残す） */
function stripComments(src: string): string {
  let out = "";
  let i = 0;
  while (i < src.length) {
    const c = src[i];
    if (c === "/" && src[i + 1] === "/") {
      while (i < src.length && src[i] !== "\n") i++;
    } else if (c === "/" && src[i + 1] === "*") {
      const e = src.indexOf("*/", i + 2);
      i = e < 0 ? src.length : e + 2;
    } else if (c === '"' || c === "'" || c === "`") {
      let j = i + 1;
      while (j < src.length && src[j] !== c) {
        if (src[j] === "\\") j++;
        else if (c !== "`" && src[j] === "\n") break;
        j++;
      }
      out += src.slice(i, j + 1);
      i = j + 1;
    } else {
      out += c;
      i++;
    }
  }
  return out;
}

const JA = /[぀-ヿ一-鿿]/;

/** 和文を含むひと続きの文（コメントを除き、引用符・{}・<> で区切る）。
 *  文字列リテラル・入れ子のテンプレート・{式} をはさんだ JSX の地の文の切れ端をすべて拾う */
function japaneseTexts(src: string): string[] {
  return [...stripComments(src).matchAll(/[^"'`{}<>\n]+/g)].map((m) => m[0].trim()).filter((t) => JA.test(t));
}

/** 使わない語。SE は和文の中の単語としてのもの（se_list などの名前・SEBank・"SE" の札は対象外） */
const BANNED: [RegExp, string][] = [
  [/エクスポート/, "書き出し"],
  [/メディア/, "素材"],
  [/見送り/, "不採用"],
  [/微調整/, "調整"],
  [/環境設定/, "設定"],
  [/エピソード/, "回"],
  [/(?<![A-Za-z_])SE(?![A-Za-z_])/, "効果音"],
];

/** 除外する文字列（Claude に送る依頼文は用語集の「変えなかったもの」。今は禁止語を含むものが無いので空）。中身の一部で指す */
const ALLOW: string[] = [];

test("画面の文言に用語集の使わない語が無い", () => {
  const bad: string[] = [];
  for (const f of walk(root)) {
    for (const t of japaneseTexts(readFileSync(f, "utf8"))) {
      if (ALLOW.some((a) => t.includes(a))) continue;
      for (const [re, use] of BANNED) if (re.test(t)) bad.push(`${f.slice(root.length)}: ${t.slice(0, 60)} → 「${use}」に`);
    }
  }
  assert.deepEqual(bad, []);
});

/** 失敗の文を生の e で組む書き方（"〜ませんでした: " + e、`〜ませんでした: ${e}`）。logic/errors.ts の failText を通す */
const RAW_FAIL = /ませんでした: ?("\s*\+|\$\{\s*(e|err|error|msg|reason|String)\b)/;

test("失敗の文を生のエラーで組んでいない（failText・toastError を通す）", () => {
  const bad: string[] = [];
  for (const f of walk(root)) {
    if (f.endsWith("/logic/errors.ts")) continue;
    const src = stripComments(readFileSync(f, "utf8"));
    src.split(/\r?\n/).forEach((l, i) => {
      if (RAW_FAIL.test(l)) bad.push(`${f.slice(root.length)}:${i + 1}: ${l.trim().slice(0, 80)}`);
    });
  }
  assert.deepEqual(bad, []);
  // 検査の仕組み
  assert.ok(RAW_FAIL.test('toast("保存できませんでした: " + e)'));
  assert.ok(RAW_FAIL.test("toast(`保存できませんでした: ${e}`)"));
  assert.ok(RAW_FAIL.test("toast(`保存できませんでした: ${String(e)}`)"));
  assert.ok(RAW_FAIL.test('toast("開けませんでした: " + e)'));
  assert.ok(RAW_FAIL.test("toast(`読めませんでした: ${err}`)"));
  assert.ok(!RAW_FAIL.test('toast(failText("保存", e))'));
  assert.ok(!RAW_FAIL.test("toast(`保存できませんでした: ${errorMessage(e)}`)"));
});

test("DELIVERY が送信方法の全部の値を持ち、並びと一致する", () => {
  assert.deepEqual(Object.keys(DELIVERY).sort(), ["channel", "hook", "mod"]);
  assert.deepEqual([...DELIVERY_ORDER].sort(), Object.keys(DELIVERY).sort());
  for (const d of Object.values(DELIVERY)) assert.ok(d.name && d.label && d.hint);
});

test("PROPOSAL_STATUS が結果の状態を持つ", () => {
  assert.deepEqual(Object.keys(PROPOSAL_STATUS).sort(), ["accepted", "rejected", "stale"]);
  assert.deepEqual(Object.values(PROPOSAL_STATUS).sort(), ["不採用", "採用", "適用できず"].sort());
});

test("TRACK_KIND と CLAUDE_ACTIONS の項目が埋まっている", () => {
  assert.deepEqual(Object.keys(TRACK_KIND).sort(), ["audio", "cue", "fx", "guide_audio", "ops", "se", "sub", "video", "voice"]);
  for (const k of Object.values(TRACK_KIND)) assert.ok(k.badge && k.name);
  for (const a of Object.values(CLAUDE_ACTIONS)) assert.ok(a.label && a.title);
});

test("検査の仕組み: コメントは見ない・文字列と JSX の地の文は見る", () => {
  assert.deepEqual(japaneseTexts("// エクスポート\n/* メディア */ const a = 1;"), []);
  assert.deepEqual(japaneseTexts('const a = "url//素材";'), ["url//素材"]);
  assert.deepEqual(japaneseTexts("<b>見送り</b>"), ["見送り"]);
  assert.deepEqual(japaneseTexts("<div>エクスポート {x} です</div>"), ["エクスポート", "です"]);
  assert.deepEqual(japaneseTexts("<div>{n} 件をエクスポート</div>"), ["件をエクスポート"]);
  assert.deepEqual(japaneseTexts("const t = `${b ? `メディア` : \"\"}`;"), ["メディア"]);
  // 札の "SE" は和文を含まないので拾わない。和文の中の SE は拾う
  assert.deepEqual(japaneseTexts('const k = "SE"; const t = "SE の音量";'), ["SE の音量"]);
});
