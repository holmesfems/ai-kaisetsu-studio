// ほかの回の台本から取り込む (feat-script-import)
import { test } from "node:test";
import assert from "node:assert/strict";
import { importScript } from "../src/logic/importscript.ts";

const mkSrc = (): any => ({
  rev: 1,
  memo: "",
  blocks: [
    {
      id: "sb1", file: "a.yaml", title: "序論", notes: "ノート",
      lines: [
        { id: "s1", sp: "トーク1", text: "こんにちは", start: 1, memo: "メモ1" },
        { id: "s2", sp: "トーク2", text: "どうも", start: 2 },
        { id: "s3", sp: "トーク1", text: "では", start: 3, memo: "メモ3" },
      ],
    },
    { id: "sb2", file: "b.yaml", title: "締め", notes: "", lines: [{ id: "s4", sp: "トーク2", text: "さよなら", start: null }] },
  ],
  voice: { s1: { sp: "トーク1", text: "こんにちは", analyzed_text: "<x/>" } },
  params: { s1: { sp: "トーク1", text: "こんにちは", params: { a: 1 } }, s2: { sp: "トーク2", text: "どうも", params: { b: 2 } } },
  speakers: { トーク1: { pitch: 1 }, トーク2: { pitch: 2 } },
});
const mkDst = (): any => ({
  rev: 0, memo: "",
  blocks: [{ id: "d1", file: "x.yaml", title: "既存", notes: "", lines: [{ id: "dl1", sp: "トーク1", text: "既存", start: 0 }] }],
  voice: {}, params: {},
  speakers: { トーク1: { pitch: 9 }, トーク3: { pitch: 3 } },
});
let c = 0;
const newId = (p: string) => `${p}new${++c}`;
const pick = (o: any = {}) => ({ blocks: [{ id: "sb1", lines: ["s1", "s3"] }], memo: true, notes: true, speakers: true, ...o });

test("選んだ行だけが末尾に足され、並びは元のまま・start と file は null", () => {
  const dst = mkDst();
  const r = importScript(dst, mkSrc(), pick(), newId);
  assert.equal(dst.blocks.length, 2);
  const b = dst.blocks[1];
  assert.equal(b.file, null);
  assert.equal(b.title, "序論");
  assert.deepEqual(b.lines.map((l: any) => l.text), ["こんにちは", "では"]);
  assert.ok(b.lines.every((l: any) => l.start === null));
  assert.equal(r.added, 2);
});

test("id は新しく重ならず、手直しは新しい id に中身ごと写る", () => {
  const dst = mkDst();
  const r = importScript(dst, mkSrc(), pick(), newId);
  const ids = dst.blocks.flatMap((b: any) => [b.id, ...b.lines.map((l: any) => l.id)]);
  assert.equal(new Set(ids).size, ids.length);
  const l1 = dst.blocks[1].lines[0];
  assert.notEqual(l1.id, "s1");
  assert.deepEqual(dst.voice[l1.id], { sp: "トーク1", text: "こんにちは", analyzed_text: "<x/>" });
  assert.deepEqual(dst.params[l1.id], { sp: "トーク1", text: "こんにちは", params: { a: 1 } });
  assert.equal(r.voice, 1);
  assert.equal(r.params, 1);
});

test("手直しの無い行は voice・params に何も足さない（選ばなかった行の手直しも写らない）", () => {
  const dst = mkDst();
  importScript(dst, mkSrc(), pick(), newId);
  assert.equal(Object.keys(dst.voice).length, 1);
  assert.equal(Object.keys(dst.params).length, 1);
});

test("memo・notes は選んだときだけ写る", () => {
  const off = mkDst();
  importScript(off, mkSrc(), pick({ memo: false, notes: false }), newId);
  assert.equal(off.blocks[1].notes, "");
  assert.ok(off.blocks[1].lines.every((l: any) => l.memo === undefined));
  const on = mkDst();
  importScript(on, mkSrc(), pick(), newId);
  assert.equal(on.blocks[1].notes, "ノート");
  assert.deepEqual(on.blocks[1].lines.map((l: any) => l.memo), ["メモ1", "メモ3"]);
});

test("speakers: 取り込んだ行の話者だけ上書き、false なら触らない、undefined でも動く", () => {
  const dst = mkDst();
  const r = importScript(dst, mkSrc(), pick(), newId);
  assert.deepEqual(dst.speakers, { トーク1: { pitch: 1 }, トーク3: { pitch: 3 } });
  assert.deepEqual(r.speakers, ["トーク1"]);
  const off = mkDst();
  importScript(off, mkSrc(), pick({ speakers: false }), newId);
  assert.deepEqual(off.speakers, mkDst().speakers);
  const none = mkDst();
  delete none.speakers;
  importScript(none, mkSrc(), pick(), newId);
  assert.deepEqual(none.speakers, { トーク1: { pitch: 1 } });
});

test("行が 0 になったブロックは足さない", () => {
  const dst = mkDst();
  const r = importScript(dst, mkSrc(), { ...pick(), blocks: [{ id: "sb1", lines: [] }, { id: "sb2", lines: ["s4"] }] }, newId);
  assert.equal(dst.blocks.length, 2);
  assert.equal(dst.blocks[1].title, "締め");
  assert.equal(r.added, 1);
});

test("src を書き換えない", () => {
  const src = mkSrc();
  importScript(mkDst(), src, pick(), newId);
  assert.deepEqual(src, mkSrc());
});

test("今の文に効いていない古い手直しは写さない", () => {
  const src = mkSrc();
  src.voice.s3 = { sp: "トーク1", text: "直す前の文", analyzed_text: "<old/>" };
  src.params.s3 = { sp: "トーク2", text: "では", params: { c: 3 } };
  const dst = mkDst();
  const r = importScript(dst, src, pick(), newId);
  const id3 = dst.blocks[1].lines[1].id;
  assert.equal(dst.voice[id3], undefined);
  assert.equal(dst.params[id3], undefined);
  assert.equal(r.voice, 1);
  assert.equal(r.params, 1);
});
