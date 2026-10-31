// 声のエンジン（ping の engine）と話者の声の項目（logic/voiceengine.ts）
import { test } from "node:test";
import assert from "node:assert/strict";
import { engineChoice, engineOf, has, paramFields } from "../src/logic/voiceengine.ts";
import { effectiveVoice, patchVoice } from "../src/logic/speakervoice.ts";

test("engine が無い古い中継は VoiSona（全部できる）とみなす", () => {
  for (const info of [undefined, null, {}, { engine: null }, { engine: { label: "x" } }]) {
    const e = engineOf(info);
    assert.equal(e.name, "voisona");
    assert.equal(e.label, "VoiSona Talk");
    for (const f of ["accent", "durations", "intonation", "styles"] as const) assert.ok(has(e, f));
  }
});

test("ping の engine から名前・できることを読む", () => {
  const e = engineOf({ engine: { name: "voicevox", label: "VOICEVOX", features: ["intonation", 3], api: 1 } });
  assert.equal(e.name, "voicevox");
  assert.equal(e.label, "VOICEVOX");
  assert.ok(has(e, "intonation"));
  assert.ok(!has(e, "accent") && !has(e, "durations") && !has(e, "styles"));
  assert.equal(engineOf({ engine: { name: "mine", features: [] } }).label, "mine");
});

const VOISONA = {
  speed: [0.2, 5], volume: [-8, 8], pitch: [-600, 600], intonation: [0, 2], alp: [-1, 1], huskiness: [-20, 20],
};

test("VoiSona の範囲なら、今の項目の並び・名前・単位のまま", () => {
  const f = paramFields(VOISONA);
  assert.deepEqual(f.map((x) => x.key), ["speed", "volume", "pitch", "intonation", "alp", "huskiness"]);
  assert.equal(f[1].unit, "dB");
  assert.equal(f[1].abbr, "VOL");
  assert.equal(f[3].step, 0.01);
});

test("VOICEVOX の範囲なら、その項目だけ。範囲が違う項目は単位を外す", () => {
  const f = paramFields({ speed: [0.5, 2], pitch: [-0.15, 0.15], intonation: [0, 2], volume: [0, 2] });
  assert.deepEqual(f.map((x) => x.key), ["speed", "pitch", "intonation", "volume"]);
  assert.equal(f[0].label, "速さ");
  assert.equal(f[0].min, 0.5);
  assert.equal(f[1].unit, "");
  assert.equal(f[1].step, 0.001);
  assert.equal(f[3].def, 1); // 音量は dB でなく倍率なので既定は 1
  assert.equal(f[2].unit, "倍"); // 範囲が同じ項目は VoiSona の表のまま
});

test("知らない項目は項目名のまま、壊れた範囲は出さない", () => {
  const f = paramFields({ breathiness: [0, 10], bad: [1, 1], worse: "x", nan: [0, null] });
  assert.deepEqual(f.map((x) => x.key), ["breathiness"]);
  assert.equal(f[0].label, "breathiness");
  assert.equal(f[0].def, 0);
  assert.deepEqual(paramFields(undefined), []);
});

test("話者の声の丸め・既定は、エンジンの項目で行う", () => {
  const f = paramFields({ speed: [0.5, 2], breathiness: [0, 10] });
  assert.deepEqual(patchVoice(undefined, undefined, { speed: 9, breathiness: 3 }, f), { breathiness: 3, speed: 2 });
  assert.equal(patchVoice(undefined, undefined, { alp: 0.5 }, f), null); // このエンジンに無い項目は無視
  assert.equal(effectiveVoice(undefined, undefined, f).speed, 1);
  assert.equal(effectiveVoice(undefined, undefined, f).breathiness, 0);
  assert.equal(effectiveVoice(undefined, undefined, f).alp, undefined);
});

test("設定の選択肢: voisona・voicevox 以外はファイル", () => {
  assert.equal(engineChoice("voisona"), "voisona");
  assert.equal(engineChoice(""), "voisona");
  assert.equal(engineChoice("voicevox"), "voicevox");
  assert.equal(engineChoice("C:\eng\mine_tts.py"), "file");
});
