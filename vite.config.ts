import { existsSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

const here = dirname(fileURLToPath(import.meta.url));

/**
 * 非公開の見た目の上書き（presets/ui/）のフォルダを探す。順は src-tauri/build.rs の preset_dir と同じ。
 *   1. 環境変数 AK_FX_PRIVATE の親の presets/ui
 *   2. リポジトリの隣の fx-private/presets/ui
 *   3. リポジトリの presets/ui
 */
export function privateUiDir(root: string = here, env: NodeJS.ProcessEnv = process.env): string | null {
  const c: string[] = [];
  const v = env.AK_FX_PRIVATE;
  if (v) c.push(resolve(dirname(resolve(v)), "presets", "ui"));
  c.push(resolve(root, "fx-private", "presets", "ui"));
  c.push(resolve(root, "presets", "ui"));
  return c.find((d) => existsSync(d)) ?? null;
}

// 見つかったフォルダに theme.css・branding.json が無ければ、その分は空の上書き（ui/src/theme/none.*）にする
const dir = privateUiDir();
const pick = (name: string, none: string) => {
  const f = dir ? resolve(dir, name) : "";
  return f && existsSync(f) ? f : resolve(here, "ui", "src", "theme", none);
};
const css = pick("theme.css", "none.css");
const branding = pick("branding.json", "none.json");
console.log(`[見た目] 上書き: ${dir ?? "なし（公開の既定）"} / theme.css: ${css.endsWith("none.css") ? "なし" : css} / branding.json: ${branding.endsWith("none.json") ? "なし" : branding}`);

export default defineConfig({
  root: "ui",
  plugins: [react()],
  clearScreen: false,
  resolve: { alias: { "@private-ui-css": css, "@private-ui-branding": branding } },
  // 上書きは リポジトリの外（fx-private）にもあるので、読み込みを許す
  server: { port: 5174, strictPort: true, fs: { allow: [here, ...(dir ? [dir] : [])] } },
  build: { outDir: "../dist", emptyOutDir: true, target: "es2022" },
});
