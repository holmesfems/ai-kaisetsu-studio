// ⚙ 設定のパッチ (ST-12)
import { test } from "node:test";
import assert from "node:assert/strict";
import { settingsPatch } from "../src/logic/settings.ts";
import type { Settings } from "../src/gen/Settings.ts";

const base: Settings = {
  python: "python", ffmpeg: "", an_root: "D:/an", template_dir: "", voisona_user: "u", voisona_pass: "", voice_port: 8765,
  workers: 3, cache_mb: 1500, last_project: "a", recent: [], api_port: 47815, delivery: "mod", cancel_grace: 3,
};

test("変わった項目だけを返す", () => {
  const p = settingsPatch(base, { ...base, cache_mb: 2000, python: "py" });
  assert.deepEqual(p, { cache_mb: 2000, python: "py" });
});

test("何も変わらなければ空", () => {
  assert.deepEqual(settingsPatch(base, { ...base }), {});
});

test("開いている間に本体が変えた項目（last_project）は送らない", () => {
  assert.deepEqual(settingsPatch(base, { ...base, last_project: "古い" , recent: ["x"] }), {});
});

test("パスワードは入力して変えたときだけ、空にするときは空文字", () => {
  assert.deepEqual(settingsPatch({ ...base, voisona_pass: "x" }, { ...base, voisona_pass: "x" }), {});
  assert.deepEqual(settingsPatch(base, { ...base, voisona_pass: "new" }), { voisona_pass: "new" });
  assert.deepEqual(settingsPatch({ ...base, voisona_pass: "x" }, { ...base, voisona_pass: "" }), { voisona_pass: "" });
});

test("数の欄が数でないときは送らない（変えない）", () => {
  const p = settingsPatch(base, { ...base, workers: "abc" as unknown as number });
  assert.ok(!("workers" in p));
});
