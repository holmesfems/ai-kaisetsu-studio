// ⚙ 設定のダイアログ: 開いたときの値と比べて、変えた項目だけを送る (ST-12)
import type { Settings } from "../gen/Settings.ts";
import type { SettingsPatch } from "../gen/SettingsPatch.ts";

/** ダイアログで変えられる項目（数は Rust が範囲に丸めるので、ここでは丸めない） */
const TEXT_KEYS = ["an_root", "template_dir", "python", "ffmpeg", "voisona_user", "voisona_pass"] as const;
const NUM_KEYS = ["workers", "cache_mb", "cancel_grace"] as const;

/** orig（開いたときの設定）と edited（編集中の値）を比べて、変わった項目だけのパッチを返す。変わりが無ければ空 */
export function settingsPatch(orig: Settings, edited: Settings): SettingsPatch {
  const p: SettingsPatch = {};
  for (const k of TEXT_KEYS) {
    if (edited[k] !== orig[k]) p[k] = edited[k];
  }
  if (edited.delivery !== orig.delivery) p.delivery = edited.delivery;
  for (const k of NUM_KEYS) {
    const n = Number(edited[k]);
    // 数でない値は送らない（変えない）。空の欄は 0 になり、Rust が下限に丸める
    if (!Number.isFinite(n)) continue;
    if (n !== orig[k]) p[k] = n;
  }
  return p;
}
