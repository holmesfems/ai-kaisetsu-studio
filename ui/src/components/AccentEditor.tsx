// アクセント（高低・ブロック）・音素の長さ・抑揚の調整。台本エディタの「アクセント欄」と同じ決まり。
import React, { useMemo, useRef, useState } from "react";
import { editScript, synthLine, useS } from "../state";
import { player } from "../io/player";
import { akf } from "../io/api";
import type { Line } from "../types";
import { findLine, paramsOf, setParams, setVoice, voiceOf } from "../logic/voice";
import { moras, splitWordEl } from "../logic/accsplit";

function accentBlocks(doc: Document): Element[][] {
  const blocks: Element[][] = [];
  for (const ph of Array.from(doc.getElementsByTagName("acoustic_phrase"))) {
    let cur: Element[] | null = null;
    for (const w of Array.from(ph.getElementsByTagName("word"))) {
      if (!(w.getAttribute("phoneme") && w.getAttribute("hl"))) {
        cur = null;
        continue;
      }
      if (!cur || w.getAttribute("chain") !== "1") blocks.push((cur = []));
      cur.push(w);
    }
  }
  return blocks;
}
const blockHl = (blk: Element[]) => blk.flatMap((w) => [...(w.getAttribute("hl") || "")]);
function putHl(blk: Element[], arr: string[]) {
  let k = 0;
  for (const w of blk) {
    const n = (w.getAttribute("hl") || "").length;
    w.setAttribute("hl", arr.slice(k, k + n).join(""));
    k += n;
  }
}
// 1ブロックは「低 → 高 → 低」だけ
function normHl(arr: string[]) {
  const a = arr.indexOf("h");
  if (a < 0) return arr;
  let b = a;
  while (b + 1 < arr.length && arr[b + 1] === "h") b++;
  return arr.map((_, i) => (i >= a && i <= b ? "h" : "l"));
}
function setMora(arr: string[], j: number) {
  arr = normHl(arr);
  const a = arr.indexOf("h");
  let b = a;
  if (a >= 0) while (b + 1 < arr.length && arr[b + 1] === "h") b++;
  let lo: number, hi: number;
  if (j === 0 && arr[0] !== "h") { lo = 0; hi = 0; }
  else if (arr[j] !== "h") { lo = a < 0 ? j : Math.min(a, j); hi = a < 0 ? j : Math.max(b, j); }
  else if (j === 0 && b === 0) { lo = 1; hi = arr.length - 1; }
  else if (j === a && b > a) { lo = a + 1; hi = b; }
  else { lo = a; hi = j - 1; }
  return arr.map((_, i) => (i >= lo && i <= hi ? "h" : "l"));
}

const MW = 26, GAP = 4, WPAD = 12, SEP = 24, YH = 22, YL = 58;
const LEN_PX = 600;
const phKind = (ph: string) => (ph === "sil" || ph === "pau" ? "pau" : /^(a|i|u|e|o|A|I|U|E|O|N|cl)$/.test(ph) ? "v" : "c");

export default function AccentEditor({ line }: { line: Line }) {
  const script = useS((s) => s.script)!;
  const item = useS((s) => s.layout.find((x) => x.line_id === line.id));
  const a = useS((s) => (item ? s.analysis[item.key] : undefined));
  // 抑揚の標準は、その話者の声（トークトラックの ⚙ で変えた値。変えていなければ共通のプリセット）
  const intoDefault = useS((s) =>
    s.script?.speakers?.[line.sp]?.intonation
      ?? s.speakersInfo?.speakers?.[line.sp]?.base.intonation
      ?? s.voice?.info?.intonation?.[line.sp] ?? 1);
  const [tab, setTab] = useState<"acc" | "len">(() => (localStorage.getItem("accTab") as any) || "acc");
  const edited = voiceOf(script, line);
  const xml = edited || a?.analyzed_text;
  const fresh = item?.audio != null;
  const params = paramsOf(script, line) || {};

  const play = async () => {
    const r = await synthLine(line.id);
    if (r) player.playOne(akf(`/audio/${r.id}.wav`));
  };
  const setParam = (name: string, value: any) =>
    editScript((s) => {
      const l = findLine(s, line.id)?.l;
      if (!l) return;
      const pr = { ...(paramsOf(s, l) || {}) };
      if (value == null) delete pr[name];
      else pr[name] = value;
      setParams(s, l, pr);
    });

  const intoCur = params.intonation ?? intoDefault;
  const head = (
    <div className="acc-tools" style={{ marginBottom: 4 }}>
      <button className={tab === "acc" ? "on" : ""} onClick={() => { setTab("acc"); localStorage.setItem("accTab", "acc"); }}>アクセント</button>
      <button className={tab === "len" ? "on" : ""} onClick={() => { setTab("len"); localStorage.setItem("accTab", "len"); }}>長さ</button>
      <span style={{ marginLeft: 10 }}>抑揚</span>
      <input
        type="range" min={0} max={2} step={0.05} defaultValue={intoCur} key={intoCur}
        onChange={(e) => { (e.target.nextSibling as HTMLElement).textContent = Number(e.target.value).toFixed(2); }}
        onPointerUp={(e) => {
          const v = Number((e.target as HTMLInputElement).value);
          setParam("intonation", Math.abs(v - intoDefault) < 1e-6 ? null : v);
        }}
      />
      <span className="ui">{Number(intoCur).toFixed(2)}</span>
      {intoCur !== intoDefault && <button className="ghost" onClick={() => setParam("intonation", null)}>標準に戻す</button>}
      <span className="grow" />
      <button className="primary" onClick={play}>▶ 試聴</button>
    </div>
  );

  const doc = useMemo(() => (xml ? new DOMParser().parseFromString(xml, "application/xml") : null), [xml]);
  if (!xml || !doc) {
    return (
      <div className="accent">
        <div className="dim">解析中…（▶ で一度音声を作ると、アクセントを調整できます）</div>
      </div>
    );
  }

  const commit = () => {
    accentBlocks(doc).forEach((blk) => putHl(blk, normHl(blockHl(blk))));
    const s = new XMLSerializer().serializeToString(doc.documentElement);
    editScript((sc) => { const l = findLine(sc, line.id)?.l; if (l) setVoice(sc, l, s); });
  };
  const clickMora = (w: Element, j: number) => {
    const blk = accentBlocks(doc).find((b) => b.includes(w));
    if (!blk) return;
    let off = 0;
    for (const x of blk) {
      if (x === w) break;
      off += (x.getAttribute("hl") || "").length;
    }
    putHl(blk, setMora(blockHl(blk), off + j));
    commit();
  };

  if (tab === "len") return <div className="accent">{head}<Lengths line={line} doc={doc} /></div>;

  // アクセント：単語ごとのモーラを、高・低の折れ線で
  const items: React.ReactNode[] = [];
  const pts: [number, number][][] = [];
  let seg: [number, number][] = [];
  let x = 8;
  const phrases = Array.from(doc.getElementsByTagName("acoustic_phrase"));
  phrases.forEach((ph, pi) => {
    if (pi > 0) items.push(<div key={`ph${pi}`} style={{ position: "absolute", left: x - 6, top: 4, bottom: 4, borderLeft: "1px solid #2a4062" }} />);
    let prevSpoken = false;
    for (const w of Array.from(ph.getElementsByTagName("word"))) {
      const hl = w.getAttribute("hl") || "";
      const orig = w.textContent || w.getAttribute("original") || "";
      if (!(w.getAttribute("phoneme") && hl)) {
        items.push(<div key={`s${x}`} style={{ position: "absolute", left: x, top: YL + 18, width: SEP, textAlign: "center", color: "var(--dim2)" }}>{orig}</div>);
        x += SEP;
        if (seg.length) pts.push(seg);
        seg = [];
        prevSpoken = false;
        continue;
      }
      const chained = prevSpoken && w.getAttribute("chain") === "1";
      if (prevSpoken) {
        const bx = x - WPAD / 2 - 5;
        items.push(
          <div
            key={`b${x}`}
            title={chained ? "ここでブロックを分割" : "前のブロックと結合"}
            onClick={() => { w.setAttribute("chain", chained ? "0" : "1"); commit(); }}
            style={{ position: "absolute", left: bx, top: 6, width: 10, height: YL + 8, cursor: "pointer",
                     borderLeft: chained ? "1px dotted #2a4062" : "2px solid #ff9e3d", marginLeft: 4 }}
          />,
        );
      }
      if (!chained) {
        if (seg.length) pts.push(seg);
        seg = [];
      }
      const ms = moras(w.getAttribute("pronunciation") || "");
      const labels = ms.length === hl.length ? ms : [...hl].map(() => "・");
      const splittable = ms.length === hl.length && (w.getAttribute("phoneme") || "").split("|").length === hl.length;
      const x0 = x;
      labels.forEach((m, j) => {
        if (j > 0 && splittable) {
          items.push(
            <div
              key={`c${x}`}
              className="acc-cut"
              title="ここで単語を分けて、ブロックを分割"
              onClick={() => { if (splitWordEl(w, j)) commit(); }}
              style={{ left: x - GAP / 2 - 4, top: 6, height: YL + 8 }}
            />,
          );
        }
        const y = hl[j] === "h" ? YH : YL;
        seg.push([x + MW / 2, y]);
        items.push(
          <div
            key={`m${x}`}
            onClick={() => clickMora(w, j)}
            title="高・低を切り替え"
            style={{ position: "absolute", left: x, top: y - 11, width: MW, height: 22, textAlign: "center", lineHeight: "22px",
                     cursor: "pointer", borderRadius: 3, background: hl[j] === "h" ? "var(--hl-on)" : "var(--hl-off)", fontSize: 12,
                     border: "1px solid " + (hl[j] === "h" ? "var(--accent)" : "var(--line2)") }}
          >
            {m}
          </div>,
        );
        x += MW + GAP;
      });
      items.push(
        <div key={`w${x0}`} style={{ position: "absolute", left: x0, top: YL + 18, width: x - x0 - GAP, textAlign: "center", fontSize: 11, color: "var(--dim)", whiteSpace: "nowrap", overflow: "hidden" }}>
          {orig}
        </div>,
      );
      x += WPAD;
      prevSpoken = true;
    }
    if (seg.length) pts.push(seg);
    seg = [];
    x += 8;
  });
  return (
    <div className="accent">
      {head}
      <div className="acc-scroll">
        <div style={{ position: "relative", width: x, height: YL + 38 }}>
          <svg width={x} height={YL + 38} style={{ position: "absolute", left: 0, top: 0, pointerEvents: "none" }}>
            {pts.map((p, i) => (
              <polyline key={i} points={p.map((q) => q.join(",")).join(" ")} fill="none" stroke="var(--accent)" strokeWidth={1.5} opacity={0.6} />
            ))}
          </svg>
          {items}
        </div>
      </div>
      <div className="acc-tools">
        {edited ? (
          <>
            <span style={{ color: "var(--caution)" }}>調整済み（_voice.json）</span>
            <button className="ghost" onClick={() => editScript((s) => { const l = findLine(s, line.id)?.l; if (l) setVoice(s, l, null); })}>元のアクセントに戻す</button>
          </>
        ) : (
          <span>モーラをクリックで高・低を切り替え、単語の境目をクリックでブロックを結合・分割、単語の中のモーラの間をクリックでそこで分割します</span>
        )}
        {!fresh && a && <span style={{ color: "var(--warn)" }}>調整後の音声はまだ作成されていません</span>}
        {a?.unread?.length ? <span style={{ color: "var(--warn)" }}>読めなかった言葉：{a.unread.join("、")}（｜漢字《よみ》）</span> : null}
      </div>
    </div>
  );
}

function Lengths({ line, doc }: { line: Line; doc: Document }) {
  const script = useS((s) => s.script)!;
  const item = useS((s) => s.layout.find((x) => x.line_id === line.id));
  const a = useS((s) => (item ? s.analysis[item.key] : undefined));
  const P = a?.phonemes;
  const D = a?.phoneme_durations;
  const pr = paramsOf(script, line) || {};
  const [own, setOwn] = useState<Record<number, number> | null>(null);
  const drag = useRef<{ i: number; x0: number; base: number } | null>(null);
  if (!P || !D) return <div className="dim">▶ で一度音声を作ると、音素の長さを調整できます</div>;
  const mismatch = !!pr.durations && pr.durations.n !== P.length;
  const saved: Record<number, number> = mismatch ? {} : { ...(pr.durations?.set || {}) };
  const cur = own ?? saved;
  const dur = (i: number) => cur[i] ?? D[i];
  const save = (o: Record<number, number>) => {
    editScript((s) => {
      const l = findLine(s, line.id)?.l;
      if (!l) return;
      const p = { ...(paramsOf(s, l) || {}) };
      if (Object.keys(o).length) p.durations = { n: P.length, set: o };
      else delete p.durations;
      setParams(s, l, p);
    });
    setOwn(null);
  };
  let x = 8;
  const boxes = P.map((ph, i) => {
    const w = dur(i) * LEN_PX;
    const el = (
      <div
        key={i}
        onDoubleClick={() => { const o = { ...cur }; delete o[i]; save(o); }}
        title="右端をドラッグで長さを変更、ダブルクリックで自動に戻します"
        style={{ position: "absolute", left: x, top: 6, width: Math.max(3, w - 1), height: 34, fontSize: 11, overflow: "hidden",
                 background: phKind(ph) === "pau" ? "var(--ph-pau)" : phKind(ph) === "v" ? "var(--hl-on)" : "var(--ph-con)",
                 border: "1px solid " + (i in cur ? "var(--caution)" : "var(--line2)"), color: "var(--text)", textAlign: "center" }}
      >
        {ph === "pau" ? "、" : ph}
        <div style={{ fontSize: 9, color: "var(--dim)" }}>{Math.round(dur(i) * 1000)}</div>
        <div
          onPointerDown={(e) => {
            e.preventDefault();
            e.stopPropagation();
            (e.target as HTMLElement).setPointerCapture(e.pointerId);
            drag.current = { i, x0: e.clientX, base: dur(i) };
          }}
          onPointerMove={(e) => {
            const d = drag.current;
            if (!d) return;
            const v = Math.round(Math.min(3, Math.max(0.01, d.base + (e.clientX - d.x0) / LEN_PX)) * 1000) / 1000;
            setOwn({ ...cur, [d.i]: v });
          }}
          onPointerUp={() => {
            if (drag.current) save({ ...cur });
            drag.current = null;
          }}
          style={{ position: "absolute", right: 0, top: 0, bottom: 0, width: 6, cursor: "ew-resize", background: "#ffffff22" }}
        />
      </div>
    );
    x += w;
    return el;
  });
  const n = Object.keys(saved).length;
  return (
    <>
      <div className="acc-scroll">
        <div style={{ position: "relative", width: x + 8, height: 46 }}>{boxes}</div>
      </div>
      <div className="acc-tools">
        {n || mismatch ? <button className="ghost" onClick={() => save({})}>長さを元に戻す</button> : null}
        <span>{n ? `${n} 個の音素の長さを変更しています（_voice_params.json）` : "音素の右端をドラッグで長さを変更、ダブルクリックで自動に戻します"}</span>
        {mismatch && <span style={{ color: "var(--warn)" }}>読みが変わり音素の並びがずれたため、以前の長さは使用していません</span>}
      </div>
    </>
  );
}
