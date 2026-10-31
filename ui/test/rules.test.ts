// 画面に残してある Rust の決まりの写しが、Rust と同じ結果になるかを、共通の見本 ui_rules.json で確かめる
// （見本は Rust の app/rules_fixture.rs が書く。式を変えたら AK_UPDATE_FIXTURES=1 cargo test -p ak-studio rules_fixture）
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { FIT_LABEL, NEST_DEFAULT, fitScale, free, groupSpan, newTrack, nestMap, windowBoxOf, windowSpan } from "../src/logic/nest.ts";
import { AUDIO_EXTS, IMAGE_EXTS, VIDEO_EXTS, insertPlacement, insertTrack, newMainTrack } from "../src/logic/media.ts";
import { cleanOverrides, mergeOverride, stripAuto } from "../src/logic/overrides.ts";
import { TRANS_DEFAULT_DUR, transDur } from "../src/logic/trans.ts";

const R: any = JSON.parse(readFileSync(new URL("../../src-tauri/tests/fixtures/ui_rules.json", import.meta.url), "utf8"));

const near = (a: number, b: number, m = "") => assert.ok(Math.abs(a - b) < 1e-9, `${m}: ${a} != ${b}`);
const nearArr = (a: number[], b: number[], m = "") => {
  assert.equal(a.length, b.length, m);
  a.forEach((x, i) => near(x, b[i], `${m}[${i}]`));
};

test("表が読めている", () => {
  for (const k of ["group_span", "nest_map", "window_box", "clean_overrides"]) assert.ok(R[k].length >= 10, k);
  assert.ok(R.trans_dur.cases.length >= 10);
  assert.ok(R.tracks.main.length === 9 && R.tracks.insert.length >= 6);
});

test("B1: 演出クリップの見えている区間", () => {
  for (const c of R.group_span) {
    const g: any = { t0: c.t0, t1: c.t1 };
    const m = JSON.stringify(c.ov);
    const [a, b] = groupSpan(g, c.ov);
    near(a, c.a, `a ${m}`);
    near(b, c.b, `b ${m}`);
    assert.deepEqual(windowSpan(g, c.ov), [a, b]);
  }
});

test("B2: ネストのキャンバス → 画面の対応と既定", () => {
  for (const c of R.nest_map) {
    const r = nestMap({ w: c.w, h: c.h, fit: c.fit }, c.box);
    const m = `${c.fit} ${c.w}x${c.h} ${c.box}`;
    near(r.sx, c.sx, `sx ${m}`);
    near(r.sy, c.sy, `sy ${m}`);
    near(r.ox, c.ox, `ox ${m}`);
    near(r.oy, c.oy, `oy ${m}`);
  }
  const { tracks, ...rest } = R.nest_default;
  assert.deepEqual(tracks, []);
  assert.deepEqual(NEST_DEFAULT, rest);
  assert.ok(NEST_DEFAULT.fit in FIT_LABEL);
});

test("B3: RecWindow の窓", () => {
  for (const c of R.window_box) nearArr(windowBoxOf(c.base, c.ov), c.box, JSON.stringify(c.ov));
});

test("B4: トランジションの実際の長さ", () => {
  near(TRANS_DEFAULT_DUR, R.trans_dur.default_dur);
  for (const c of R.trans_dur.cases) near(transDur(c.tr, c.clip_dur), c.dur, JSON.stringify(c));
});

test("B5: 微調整の既定値を外す", () => {
  for (const c of R.clean_overrides) assert.deepEqual(cleanOverrides(c.in), c.out, JSON.stringify(c.in));
});

test("B5: stripAuto は no_placeholder も外す", () => {
  assert.deepEqual(stripAuto({ a: { no_placeholder: true }, b: { shift: 1, no_placeholder: [[1, 2]] }, c: { scale: 1 } }), { b: { shift: 1 } });
  assert.deepEqual(stripAuto({ a: { shift: 1.5, note: "x" } }), { a: { shift: 1.5, note: "x" } });
});

test("B8: 新しいトラックの名前・音量・位置", () => {
  for (const c of R.tracks.main) {
    const t = newMainTrack(c.kind, c.n, "x");
    assert.equal(t.name, c.name);
    assert.equal(t.volume_db, c.volume_db);
    assert.equal(t.kind, c.track_kind);
  }
  for (const c of R.tracks.nest) {
    const t = newTrack(c.n);
    assert.equal(t.name, c.name);
    assert.equal(t.kind, c.kind);
  }
  for (const c of R.tracks.insert) {
    const tracks: any[] = c.kinds.map((k: string, i: number) => ({ id: `t${i}`, kind: k, clips: [] }));
    const at = insertTrack(tracks, { id: "new", kind: c.new, clips: [] } as any, c.top);
    assert.equal(at, c.at, JSON.stringify(c));
    assert.equal(tracks[at].id, "new");
    assert.equal(tracks.length, c.kinds.length + 1);
  }
  for (const c of R.tracks.free) {
    const tr: any = { clips: c.clips.map(([start, dur]: number[], i: number) => ({ id: `c${i}`, start, dur })) };
    assert.equal(free(tr, c.a, c.b), c.free, JSON.stringify(c));
  }
  for (const c of R.tracks.fit_scale) near(fitScale({ w: c.cw, h: c.ch }, c.w, c.h, "contain"), c.scale, JSON.stringify(c));
});

test("B8: insertPlacement は新しいトラックを insertTrack の位置に入れる（代わりのトラックも Rust と同じ名前）", () => {
  const clip: any = { id: "m1", start: 0, dur: 1 };
  const mk = (): any[] => [{ id: "v", kind: "video", clips: [] }, { id: "fx", kind: "fx", clips: [] }];
  const a = mk();
  insertPlacement(a, { clip, kind: "image", nest: null, track_id: null, new_track: null });
  assert.deepEqual(a.map((t) => t.kind), ["video", "image", "fx"]);
  assert.equal(a[1].name, "立ち絵");
  const b = mk();
  insertPlacement(b, { clip, kind: "audio", nest: null, track_id: null, new_track: null });
  assert.equal(b[2].name, "BGM");
  assert.equal(b[2].volume_db, -18);
  const c = mk();
  insertPlacement(c, { clip, kind: "video", nest: "k", track_id: null, new_track: newTrack(3) });
  assert.deepEqual(c.map((t) => t.kind), ["video", "fx", "video"]);
});

test("stripAuto: undefined の項目は無いのと同じ（pivot だけ残る項目は消える）", () => {
  assert.deepEqual(stripAuto({ k: { pivot: [1, 2], shift: undefined, no_placeholder: true } }), {});
});

test("DU-11: 素材の拡張子の表は Rust と同じ", () => {
  assert.deepEqual([...IMAGE_EXTS], R.media_exts.image);
  assert.deepEqual([...VIDEO_EXTS], R.media_exts.video);
  assert.deepEqual([...AUDIO_EXTS], R.media_exts.audio);
});

test("ST-9: 微調整のマージ（項目が null で外れる・patch が null でキーごと・ほかの鍵に触らない）", () => {
  assert.ok(R.merge_override.length >= 10);
  for (const c of R.merge_override) {
    const before = JSON.stringify(c.ov);
    assert.deepEqual(cleanOverrides(mergeOverride(c.ov, c.key, c.patch)), c.out, JSON.stringify(c));
    assert.equal(JSON.stringify(c.ov), before, "元の微調整を書き換えない");
  }
});
