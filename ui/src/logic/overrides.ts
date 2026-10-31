// 微調整（fx/studio_overrides.json）の、純粋な計算。

const isObj = (v: unknown): v is Record<string, unknown> => v !== null && typeof v === "object" && !Array.isArray(v);

/** その値が既定（外してよい）か。Rust の clean_overrides と同じで、数の項目は数のときだけ既定になりうる（文字列や null の shift は残す） */
function isDefault(key: string, v: unknown): boolean {
  switch (key) {
    case "shift":
    case "end":
    case "dx":
    case "dy":
      return typeof v === "number" && Math.abs(v) < 1e-6;
    case "scale":
    case "opacity":
      return typeof v === "number" && Math.abs(v - 1) < 1e-6;
    case "hidden":
      return v === false;
    default:
      return v === null;
  }
}

/**
 * 空の微調整（全部が既定値）を消す。見本 ui_rules.json の clean_overrides で縛っている（ui/test/rules.test.ts。Rust は overrides::clean_overrides）。
 * オブジェクトでない項目は捨てる。pivot だけ残っても意味がないので項目ごと消す（note があれば残す）
 */
export function cleanOverrides(ov: unknown): Record<string, Record<string, unknown>> {
  const out: Record<string, Record<string, unknown>> = {};
  if (!isObj(ov)) return out;
  for (const [k, v] of Object.entries(ov)) {
    if (!isObj(v)) continue;
    const m: Record<string, unknown> = {};
    for (const [kk, vv] of Object.entries(v)) {
      // undefined は JSON に出ない（Rust には無いのと同じ）ので、項目ごと外す
      if (vv !== undefined && !isDefault(kk, vv)) m[kk] = vv;
    }
    if (Object.keys(m).some((x) => x !== "pivot" && x !== "note") || "note" in m) out[k] = m;
  }
  return out;
}

/**
 * 微調整 ov の key に patch を重ねる（項目が null なら外す。patch が null なら key ごと外す）。cleanOverrides の前の形。
 * 見本 ui_rules.json の merge_override で縛っている（ui/test/rules.test.ts。Rust は overrides::merge_override）。
 * オブジェクトでない patch は何もしない。画面だけ、undefined の項目も外す（JSON では項目が無いのと同じなので Rust には届かない）
 */
export function mergeOverride(ov: Record<string, any>, key: string, patch: unknown): Record<string, any> {
  const merged: Record<string, any> = isObj(ov[key]) ? { ...(ov[key] as Record<string, any>) } : {};
  if (patch === null) {
    const { [key]: _drop, ...rest } = ov;
    return rest;
  } else if (isObj(patch)) {
    for (const [k, v] of Object.entries(patch)) {
      if (v === null || v === undefined) delete merged[k];
      else merged[k] = v;
    }
  }
  return { ...ov, [key]: merged };
}

/**
 * 比べるための微調整の形: Rust の clean_overrides と同じく既定の値（ずらし 0・倍率 1・隠さない など）を外し（logic/overrides.ts の cleanOverrides）、
 * 自動で付く印（録画の窓の no_placeholder。Rust の nest_flags が付ける）も外す。画面の変更の折り返しを外の変更と見間違えないように
 */
export function stripAuto(ov: Record<string, any>) {
  const noAuto: Record<string, any> = {};
  for (const [k, v] of Object.entries(ov || {})) {
    if (v && typeof v === "object" && !Array.isArray(v)) {
      const { no_placeholder: _drop, ...rest } = v;
      noAuto[k] = rest;
    } else noAuto[k] = v;
  }
  return cleanOverrides(noAuto);
}
