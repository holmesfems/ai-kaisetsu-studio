// 層の決まり: ui/src/logic/ の純粋な関数は、state/（store）・io/（Tauri）を import しない
import test from "node:test";
import assert from "node:assert/strict";
import { readdirSync, readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

const dir = fileURLToPath(new URL("../src/logic/", import.meta.url));
const files = readdirSync(dir).filter((f) => f.endsWith(".ts"));

/** import / export ... from "x" と import("x") の x を全部拾う */
function specifiers(src: string): string[] {
  const out: string[] = [];
  for (const m of src.matchAll(/(?:from|import)\s*\(?\s*["']([^"']+)["']/g)) out.push(m[1]);
  return out;
}

test("logic/ にファイルがある", () => {
  assert.ok(files.length > 0);
});

for (const f of files) {
  test(`logic/${f} は state/・io/ を import しない`, () => {
    const bad = specifiers(readFileSync(dir + f, "utf8")).filter((s) => s.startsWith("../state") || s.startsWith("../io"));
    assert.deepEqual(bad, []);
  });
}
