// 話者ごとの声（トークトラックの ⚙ の「VOICE」。ui/src/speakervoice.ts）の確かめ（音声の印は Rust の voice/analysis.rs）
import { test } from "node:test";
import assert from "node:assert/strict";
import { changedSpeakers, effectiveVoice, patchVoice, styleShares, withStyleWeight } from "../src/logic/speakervoice.ts";

const base = { speed: 1.29, volume: 6, pitch: 0, intonation: 1, alp: -0.24, style_weights: [0.53, 0.47, 0, 0] };

test("今の値は、プリセットに変更を重ねたもの（プリセットに無い項目は VoiSona の既定）", () => {
  assert.deepEqual(effectiveVoice(base, { speed: 1, huskiness: 3 }),
    { ...base, speed: 1, huskiness: 3 });
  assert.equal(effectiveVoice(undefined, undefined).speed, 1);
  // スタイルの数が合わない変更（声のライブラリが変わったなど）は使わない
  assert.deepEqual(effectiveVoice(base, { style_weights: [1, 0] }).style_weights, base.style_weights);
});

test("変更はプリセットと違う項目だけを、範囲に丸めて名前の順で残す", () => {
  assert.deepEqual(patchVoice(undefined, base, { speed: 1.5 }), { speed: 1.5 });
  assert.equal(patchVoice({ speed: 1.5 }, base, { speed: 1.29 }), null); // プリセットと同じなら消す
  assert.equal(patchVoice({ speed: 1.5 }, base, { speed: null }), null);
  assert.deepEqual(patchVoice(undefined, base, { pitch: 9999, volume: -20 }), { pitch: 600, volume: -8 });
  assert.deepEqual(Object.keys(patchVoice({ volume: 1 }, base, { alp: 0.5, speed: 2 })!), ["alp", "speed", "volume"]);
  assert.deepEqual(patchVoice(undefined, base, { style_weights: [1, 2, -1, 0] }), { style_weights: [1, 1, 0, 0] });
  assert.equal(patchVoice({ style_weights: [1, 0, 0, 0] }, base, { style_weights: [0.53, 0.47, 0, 0] }), null);
  assert.equal(patchVoice({ speed: 2 }, base, { speed: Number.NaN }), null);
});

test("スタイルの重みを 1 つ変える・リセットする", () => {
  assert.deepEqual(withStyleWeight([0.5, 0.5, 0, 0], 2, 0.3, base.style_weights), [0.5, 0.5, 0.3, 0]);
  assert.deepEqual(withStyleWeight([1, 0, 0, 0], 0, null, base.style_weights), [0.53, 0, 0, 0]);
  assert.deepEqual(styleShares([1, 1, 0, 2]), [0.25, 0.25, 0, 0.5]);
  assert.deepEqual(styleShares([0, 0]), [0, 0]);
});

test("声が変わった話者だけを作り直す", () => {
  assert.deepEqual(changedSpeakers(undefined, undefined), []);
  assert.deepEqual(changedSpeakers({ トーク1: { speed: 1 } }, {}), ["トーク1"]);
  assert.deepEqual(changedSpeakers({ トーク1: { speed: 1 }, トーク2: { alp: 1 } }, { トーク1: { speed: 1 } }), ["トーク2"]);
  assert.deepEqual(changedSpeakers({}, { トーク2: { alp: 1 } }), ["トーク2"]);
});
