// 攻略動画の素材の音のプレビュー（ui/src/logic/guideAudio.ts）の確かめ
import test from "node:test";
import assert from "node:assert/strict";
import { GUIDE_AUDIO_FILE, NO_AUDIO_WARNING, dbGain, guideAudioItem, guideAudioRow } from "../src/logic/guideAudio.ts";

const url = (rev: number) => `file?p=${GUIDE_AUDIO_FILE}&v=${rev}`;

test("素材の音: 作れていれば 0 秒から全部、音量は dB どおり", () => {
  const it = guideAudioItem(123, { guide_audio_db: 0, guide_audio_muted: false }, url)!;
  assert.deepEqual({ at: it.at, from: it.from, dur: it.dur, fi: it.fi, fo: it.fo, loop: it.loop }, { at: 0, from: 0, dur: null, fi: 0, fo: 0, loop: false });
  assert.equal(it.url, "file?p=studio/guide_audio.wav&v=123");
  assert.equal(it.gain, 1);
  assert.ok(Math.abs(guideAudioItem(1, { guide_audio_db: -6 }, url)!.gain - 0.5012) < 1e-3);
  assert.ok(Math.abs(dbGain(20) - 10) < 1e-9);
});

test("素材の音: 作れていない・消音のときは入れない", () => {
  assert.equal(guideAudioItem(null, {}, url), null);
  assert.equal(guideAudioItem(undefined, {}, url), null);
  assert.equal(guideAudioItem(5, { guide_audio_muted: true }, url), null);
  // 古い project.json（項目が無い）は 0 dB・消音なし
  assert.equal(guideAudioItem(5, {}, url)!.gain, 1);
});

test("素材の音の行: 音が無い・作成中・できた", () => {
  assert.deepEqual(guideAudioRow(null, [NO_AUDIO_WARNING], false), { empty: true, title: "素材に音がありません" });
  assert.equal(guideAudioRow(null, [], false).empty, true);
  assert.match(guideAudioRow(null, [], false).title, /作っています/);
  assert.equal(guideAudioRow(9, [], false).empty, false);
  assert.match(guideAudioRow(9, [], true).title, /消音/);
  // 音が無い警告のほうが先
  assert.equal(guideAudioRow(9, [NO_AUDIO_WARNING], false).empty, true);
  // 作れなかったときは理由を出す（できていれば出さない）
  assert.deepEqual(guideAudioRow(null, [], false, "ffmpeg が見つかりません"), { empty: true, title: "素材の音を作れませんでした: ffmpeg が見つかりません" });
  assert.equal(guideAudioRow(9, [], false, "古い理由").empty, false);
  assert.match(guideAudioRow(null, [], false, null).title, /作っています/);
});
