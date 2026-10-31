import { test } from "node:test";
import assert from "node:assert/strict";
import { IMG_ONLY_TEXT, bodyFor, imagePaths, mimeExt } from "../src/logic/chatimg.ts";

test("mimeExt: 対応する形式だけ", () => {
  assert.equal(mimeExt("image/png"), "png");
  assert.equal(mimeExt("image/jpeg"), "jpg");
  assert.equal(mimeExt("image/webp"), "webp");
  assert.equal(mimeExt("image/svg+xml"), null);
});

test("bodyFor: 画像だけなら決まった文", () => {
  assert.equal(bodyFor("  ", 1), IMG_ONLY_TEXT);
  assert.equal(bodyFor("  ", 0), "");
  assert.equal(bodyFor(" これ ", 2), "これ");
});

test("imagePaths: image の添付だけ取る", () => {
  assert.deepEqual(imagePaths([{ image: "a.png" }, { t: 1 }, { image: "b.jpg" }]), ["a.png", "b.jpg"]);
  assert.deepEqual(imagePaths(undefined), []);
});
