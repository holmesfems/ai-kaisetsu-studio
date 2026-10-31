// 元に戻すの「3 方向の戻し」（ST-6）。
//
// 元に戻すは、操作の前（before）と後（after）の写しを持っておき、今の状態（cur）から「その操作で変わった所だけ」を
// before に戻す。操作の後に Claude や外のエディタが変えた所はそのまま残る。
// 自分の操作の Ctrl+Z と、外の変更を戻す Ctrl+Alt+Z の両方で使う。ui/test/history.test.ts で確かめる。

type Json = unknown;

const isObj = (v: Json): v is Record<string, Json> => !!v && typeof v === "object" && !Array.isArray(v);
const idOf = (v: Json) => (isObj(v) && typeof v.id === "string" ? v.id : null);
const idArray = (v: Json): v is Record<string, Json>[] => Array.isArray(v) && v.every((x) => idOf(x) !== null);

/** 中身が同じか（オブジェクトの鍵の順番は見ない） */
export function deepEq(a: Json, b: Json): boolean {
  if (a === b) return true;
  if (Array.isArray(a) || Array.isArray(b)) {
    if (!Array.isArray(a) || !Array.isArray(b) || a.length !== b.length) return false;
    return a.every((x, i) => deepEq(x, b[i]));
  }
  if (isObj(a) && isObj(b)) {
    const ka = Object.keys(a).filter((k) => a[k] !== undefined);
    const kb = Object.keys(b).filter((k) => b[k] !== undefined);
    return ka.length === kb.length && ka.every((k) => deepEq(a[k], b[k]));
  }
  // NaN どうしも同じとみなす
  return typeof a === "number" && typeof b === "number" && Number.isNaN(a) && Number.isNaN(b);
}

const clone = <T>(v: T): T => (v === undefined ? v : structuredClone(v));

/**
 * cur から、after → before の変化だけを取り消した値を返す。
 * - その操作で変わっていない所は cur のまま
 * - 変わった所が、その後に変わっていなければ before に戻す
 * - その後にも変わっていたら cur のまま（conflicts に数える）
 * - id を持つものの配列（トラック・クリップ・ブロック・セリフ）は id で突き合わせる
 * ignore の鍵（rev など）は見ない
 */
export function revert3<T>(cur: T, after: T, before: T, ignore: string[] = ["rev"]): { value: T; conflicts: number } {
  let conflicts = 0;
  const skip = new Set(ignore);
  // 操作で消されたものの入れ直しは、全体を戻し終えてから決める（別のトラック・ブロックへ動いたものが、
  // 動いた先から消えたかどうかは最後まで分からないので）。出来上がった値のどこにも無いものだけを入れ直す
  const inserts: { out: Json[]; prev: string | null; x: Json }[] = [];
  const go = (c: Json, a: Json, b: Json): Json => {
    if (deepEq(a, b)) return c;
    // オブジェクトと id の配列は中まで見る（丸ごと before に戻すと、別の所へ動いたものが 2 つになる）
    if (isObj(c) && isObj(a) && isObj(b)) {
      const out: Record<string, Json> = { ...c };
      for (const k of new Set([...Object.keys(a), ...Object.keys(b)])) {
        if (skip.has(k)) continue;
        const inA = a[k] !== undefined;
        const inB = b[k] !== undefined;
        if (inA && !inB) {
          // 操作で足された鍵: その後に変わっていなければ消す
          if (c[k] === undefined) continue;
          if (deepEq(c[k], a[k])) delete out[k];
          else conflicts++;
        } else if (!inA && inB) {
          // 操作で消された鍵: 無ければ戻す
          if (c[k] === undefined) out[k] = clone(b[k]);
          else if (!deepEq(c[k], b[k])) conflicts++;
        } else if (inA && inB) {
          if (c[k] === undefined) {
            // その後に消されている
            if (!deepEq(a[k], b[k])) conflicts++;
            continue;
          }
          out[k] = go(c[k], a[k], b[k]);
        }
      }
      return out;
    }
    if (idArray(c) && idArray(a) && idArray(b)) {
      const am = new Map(a.map((x) => [idOf(x)!, x]));
      const bm = new Map(b.map((x) => [idOf(x)!, x]));
      const out: Json[] = [];
      for (const x of c) {
        const id = idOf(x)!;
        if (am.has(id) && !bm.has(id)) {
          // 操作で足されたもの: その後に変わっていなければ消す
          if (deepEq(x, am.get(id))) continue;
          conflicts++;
          out.push(x);
        } else if (am.has(id) && bm.has(id)) out.push(go(x, am.get(id), bm.get(id)));
        else out.push(x);
      }
      // 操作で消されたもの: 入れ直す候補（before での一つ前の隣の後ろに。最後に決める）
      b.forEach((x, i) => {
        const id = idOf(x)!;
        if (am.has(id) || out.some((y) => idOf(y) === id)) return;
        inserts.push({ out, prev: i > 0 ? idOf(b[i - 1]) : null, x });
      });
      // 操作での並べ替え: 両方にあるものの順番が、今も操作の後のままなら、操作の前の順番に戻す
      const common = (arr: Json[]) => arr.map((x) => idOf(x)!).filter((id) => am.has(id) && bm.has(id));
      const orderA = common(a);
      const orderB = common(b);
      if (orderA.join("\u0000") !== orderB.join("\u0000")) {
        const orderNow = common(out);
        if (orderNow.join("\u0000") === orderA.join("\u0000")) {
          const slots = out.map((x, i) => (am.has(idOf(x)!) && bm.has(idOf(x)!) ? i : -1)).filter((i) => i >= 0);
          const byId = new Map(out.map((x) => [idOf(x)!, x]));
          slots.forEach((slot, k) => {
            out[slot] = byId.get(orderB[k])!;
          });
        } else conflicts++;
      }
      return out;
    }
    if (deepEq(c, a)) return clone(b);
    conflicts++;
    return c;
  };
  const value = go(cur, after, before);
  const present = new Set<string>();
  const collect = (v: Json) => {
    if (Array.isArray(v)) v.forEach(collect);
    else if (isObj(v)) {
      const id = idOf(v);
      if (id) present.add(id);
      Object.values(v).forEach(collect);
    }
  };
  collect(value);
  for (const { out, prev, x } of inserts) {
    const id = idOf(x)!;
    if (present.has(id)) {
      // 戻した後もどこか別の所にある（動いた先で、その後に変わっていた）: 2 つにしない
      conflicts++;
      continue;
    }
    const j = prev ? out.findIndex((y) => idOf(y) === prev) : -1;
    out.splice(j + 1, 0, clone(x));
    present.add(id);
  }
  return { value: value as T, conflicts };
}

/**
 * 外から届いた台本 remote に、画面の未保存の編集（acked → current）を重ねる。
 * 同じ所を両方が変えたら remote が勝つ（もうディスクにあり、保存時の 3 者マージも remote を見る）。rev は remote のもの
 */
export function mergeLocalEdits<T extends { rev?: number }>(remote: T, acked: T | null, current: T | null): { value: T; local: boolean } {
  if (!acked || !current) return { value: remote, local: false };
  if (deepEq({ ...acked, rev: 0 }, { ...current, rev: 0 })) return { value: remote, local: false };
  const r = revert3(remote, acked, current);
  return { value: { ...r.value, rev: remote.rev }, local: true };
}
