// 素材の拡張子の判定（ui/src/logic/media.ts）の確かめ。表は見本 ui_rules.json で Rust と縛っている（rules.test.ts）
import { test } from "node:test";
import assert from "node:assert/strict";
import { isAud, isImg, mediaClass } from "../src/logic/media.ts";

test("拡張子の大文字小文字は区別しない", () => {
  assert.ok(isImg("a.BMP"));
  assert.ok(isAud("x.Ogg"));
  assert.ok(!isImg("a.mp4"));
  assert.ok(!isAud("a.png"));
});

test("パスの途中の点・拡張子なしは画像でも音声でもない", () => {
  assert.ok(!isImg("C:/a.png/file"));
  assert.ok(!isImg("png"));
  assert.ok(isImg("C:/x.d/a.jpeg"));
});

test("mediaClass: 画像・音声・それ以外は動画", () => {
  assert.equal(mediaClass("rec.mkv"), "vid");
  assert.equal(mediaClass("rec.avi"), "vid");
  assert.equal(mediaClass("a.webp"), "img");
  assert.equal(mediaClass("a.flac"), "aud");
  assert.equal(mediaClass("a.xyz"), "vid");
});
