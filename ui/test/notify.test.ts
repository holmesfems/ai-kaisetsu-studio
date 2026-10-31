import { test } from "node:test";
import assert from "node:assert/strict";
import { onceKeys, urlFileName } from "../src/io/notify.ts";

test("onceKeys: 同じ鍵は 1 回だけ、forget のあとはまた知らせる", () => {
  const o = onceKeys();
  assert.equal(o.first("a"), true);
  assert.equal(o.first("a"), false);
  assert.equal(o.first("b"), true);
  o.forget("a");
  assert.equal(o.first("a"), true);
  assert.equal(o.first("b"), false);
});

test("urlFileName: p= を decode した最後の部分", () => {
  assert.equal(urlFileName("http://akf.localhost/file?p=E%3A%2Fa%2Fb%2Fbgm.mp3&v=2"), "bgm.mp3");
  assert.equal(urlFileName("http://akf.localhost/audio/abc.wav"), "abc.wav");
  // そのままのパス（mediaInfo の src）は ? や # で切らない
  assert.equal(urlFileName("E:/rec/#1.mp4"), "#1.mp4");
  assert.equal(urlFileName("E:\\rec\\a?b.wav"), "a?b.wav");
});
