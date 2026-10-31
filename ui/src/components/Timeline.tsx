import React, { useEffect, useMemo, useRef, useState } from "react";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { call, fileUrl } from "../io/api";
import { editDoc, editScript, exportTalk, patchOverride, refreshCache, refreshSeBank, selectMark, sendChat, toast, toastError, total, upsertMark, useS } from "../state";
import { player } from "../io/player";
import { addMedia, addTrack } from "../state/media";
import { moveLineTo } from "../logic/voice";
import { SPEAKERS, clamp, spClass, tc, wheelAct } from "../logic/util";
import type { Group, Override, Track } from "../types";
import type { GuideState } from "../gen/GuideState";
import { DRIFT_HELP, driftText, useCueDrift } from "./drift";
import { groupSpan, isRecGroup } from "../logic/nest";
import { TRANS_KINDS, transDur } from "../logic/trans";
import { LOCK_HELP } from "../logic/pick";
import { seLabel } from "../logic/sebank";
import { mediaClass } from "../logic/media";
import { TRACK_KIND } from "../logic/terms.ts";
import { manualSpan, opAddError, opMarkKind, opMarkTitle, opRows } from "../logic/guide";
import { guideAudioRow } from "../logic/guideAudio.ts";
import { bandLanes, circled, listedMarks, markHex } from "../logic/marks";
import { addOpAt, addOpHere, dragOpTo, removeOp, seekOp } from "./guide/actions";

const HEAD = 150;
const RULER = 34;
const H = { fx: 18, voice: 28, video: 32, image: 32, audio: 30, se: 16, gaudio: 24, cue: 16, subs: 24, ops: 22 };
/** 印の帯 1 段の高さ */
const MARK_LANE = 14;

type Row =
  | { kind: "track"; track: Track; h: number }
  | { kind: "fx"; lanes: Group[][]; h: number; track: Track }
  | { kind: "voice"; sp: string; h: number }
  | { kind: "se"; h: number }
  | { kind: "gaudio"; h: number }
  | { kind: "ops"; h: number }
  | { kind: "marks"; h: number }
  | { kind: "subs"; h: number }
  | { kind: "cue"; h: number };

const span = groupSpan;

function Playhead() {
  const t = useS((s) => s.t);
  const zoom = useS((s) => s.zoom);
  return <div className="playhead" style={{ left: HEAD + t * zoom }} />;
}

/** スナップ先（cue の頭・同期点・再生位置・セリフの頭） */
function snapTargets(): number[] {
  const s = useS.getState();
  const out = [s.t];
  for (const c of s.desc?.cues ?? []) out.push(c.start);
  for (const y of s.desc?.sync ?? []) if (typeof y.t === "number") out.push(y.t);
  for (const it of s.layout) out.push(it.start, it.start + it.dur);
  if (s.inT != null) out.push(s.inT);
  if (s.outT != null) out.push(s.outT);
  return out;
}

function snap(t: number, on: boolean, zoom: number, extra: number[] = []): number {
  if (!on) return t;
  const lim = 7 / zoom;
  let best = t;
  let bd = lim;
  for (const x of [...snapTargets(), ...extra]) {
    const d = Math.abs(x - t);
    if (d < bd) {
      bd = d;
      best = x;
    }
  }
  return best;
}

export default function Timeline() {
  const desc = useS((s) => s.desc);
  const seBank = useS((s) => s.seBank);
  const overrides = useS((s) => s.overrides);
  const doc = useS((s) => s.project?.doc);
  const layout = useS((s) => s.layout);
  const zoom = useS((s) => s.zoom);
  const selection = useS((s) => s.selection);
  const cacheRanges = useS((s) => s.cacheRanges);
  const scenes = useS((s) => s.scenes);
  const inT = useS((s) => s.inT);
  const outT = useS((s) => s.outT);
  const playing = useS((s) => s.playing);
  const subCheck = useS((s) => s.subCheck);
  const subView = useS((s) => s.subView); // 字幕の区間・鍵・見た目（Rust が決める）
  const subViewErr = useS((s) => s.subViewErr);
  const guide = useS((s) => s.guide); // 攻略動画の回だけ「操作」の行が出る
  const guideOp = useS((s) => s.guideOp);
  const guideOn = !!guide?.guide;
  const allMarks = useS((s) => s.marks.marks);
  const markShow = useS((s) => s.markShow);
  const markShowDone = useS((s) => s.markShowDone);
  const markSel = useS((s) => s.markSel);
  const listed = useMemo(() => listedMarks(allMarks, markShow, markShowDone), [allMarks, markShow, markShowDone]);
  const [snapOn, setSnapOn] = useState(true);
  const [fxRows, setFxRows] = useState(() => Number(localStorage.getItem("fxRows") || 6));
  const [follow, setFollow] = useState(true);
  const [menu, setMenu] = useState<{ x: number; y: number; key: string } | null>(null);
  // 攻略動画: クリップの無い所の右クリック（t = 出力の秒）・操作の印の右クリック（op = 操作の id）
  const [gmenu, setGmenu] = useState<{ x: number; y: number; t?: number; op?: string } | null>(null);
  const [seBusy, setSeBusy] = useState(false);
  const [talkBusy, setTalkBusy] = useState(false);
  const drift = useCueDrift();
  const scroll = useRef<HTMLDivElement>(null);
  const T = total();
  const W = HEAD + T * zoom + 200;
  // 動画の終わりの印と、最後の字幕が消えてから終わりまでの余白（声の終わり + 1 秒などで決まる）
  const subEnd = subView?.end ?? 0;
  const padFrom = subEnd > 0 && subEnd < T - 1e-3 ? subEnd : null;
  const endWhy = `動画の終わり ${tc(T)}\n（演出の終わり・最後の声の終わり + 1 秒・繰り返しでない素材の終わりのうち、いちばん遅いところ）` +
    (padFrom != null ? `\n最後の字幕が消えてから ${(T - padFrom).toFixed(2)} 秒の余白` : "");

  // 効果音の印のツールチップに種類の表示名を出すため、設定（SeInspector が開くまで読まれない）を先に読む
  const hasDesc = !!desc;
  useEffect(() => {
    if (hasDesc && !seBank) refreshSeBank();
  }, [hasDesc, seBank === null]);

  // 右クリックのメニューは Esc・外のクリックで閉じる
  const anyMenu = !!menu || !!gmenu;
  useEffect(() => {
    if (!anyMenu) return;
    const close = () => { setMenu(null); setGmenu(null); };
    const onKey = (e: KeyboardEvent) => { if (e.key === "Escape") close(); };
    const onDown = (e: PointerEvent) => { if (!(e.target as Element | null)?.closest?.(".menu")) close(); };
    window.addEventListener("keydown", onKey);
    window.addEventListener("pointerdown", onDown, true);
    return () => {
      window.removeEventListener("keydown", onKey);
      window.removeEventListener("pointerdown", onDown, true);
    };
  }, [anyMenu]);

  // 再生中は再生位置を追いかける
  useEffect(() => {
    if (!playing || !follow) return;
    const id = setInterval(() => {
      const sc = scroll.current;
      if (!sc) return;
      const x = HEAD + useS.getState().t * useS.getState().zoom;
      if (x > sc.scrollLeft + sc.clientWidth - 60 || x < sc.scrollLeft + HEAD) sc.scrollLeft = Math.max(0, x - HEAD - 80);
    }, 200);
    return () => clearInterval(id);
  }, [playing, follow]);

  // 止まっているときも、再生位置が見えていなければそこまでスクロール（Claude が画面を動かしたときなど）
  const tNow = useS((s) => (s.playing ? -1 : s.t));
  useEffect(() => {
    if (tNow < 0) return;
    const sc = scroll.current;
    if (!sc) return;
    const x = HEAD + tNow * zoom;
    if (x > sc.scrollLeft + sc.clientWidth - 40 || x < sc.scrollLeft + HEAD) sc.scrollLeft = Math.max(0, x - HEAD - sc.clientWidth / 3);
  }, [tNow, zoom]);

  // 台本の行番号クリックなどで再生位置へ飛んだとき（再生中も）、見えていなければそこまでスクロール
  useEffect(() => {
    const on = () => {
      const sc = scroll.current;
      if (!sc) return;
      const st = useS.getState();
      const x = HEAD + st.t * st.zoom;
      if (x > sc.scrollLeft + sc.clientWidth - 40 || x < sc.scrollLeft + HEAD) sc.scrollLeft = Math.max(0, x - HEAD - sc.clientWidth / 3);
    };
    window.addEventListener("ak-reveal-playhead", on);
    return () => window.removeEventListener("ak-reveal-playhead", on);
  }, []);

  // ホイール：そのまま＝左右、Shift＝上下、Alt＝マウス位置を中心にズーム（Ctrl も同じ。ピンチ用）。割り当ては wheelAct
  // （React の onWheel は passive で preventDefault が効かないので、ネイティブで受ける）
  useEffect(() => {
    const sc = scroll.current;
    if (!sc) return;
    const onWheel = (e: WheelEvent) => {
      e.preventDefault();
      const a = wheelAct(e, { w: sc.clientWidth, h: sc.clientHeight });
      if (a.kind === "zoom") {
        const zoom = useS.getState().zoom;
        const r = sc.getBoundingClientRect();
        const tAt = (e.clientX - r.left + sc.scrollLeft - HEAD) / zoom;
        const nz = clamp(zoom * (a.zoomIn ? 1.2 : 1 / 1.2), 2, 400);
        useS.setState({ zoom: nz });
        requestAnimationFrame(() => { sc.scrollLeft = Math.max(0, tAt * nz - (e.clientX - r.left - HEAD)); });
      } else if (a.kind === "v") sc.scrollTop += a.d;
      else sc.scrollLeft += a.d;
    };
    sc.addEventListener("wheel", onWheel, { passive: false });
    return () => sc.removeEventListener("wheel", onWheel);
  }, []);

  // 演出の描き済み区間（レンダリングの帯）
  useEffect(() => {
    refreshCache();
    const id = setInterval(refreshCache, playing ? 1500 : 4000);
    return () => clearInterval(id);
  }, [playing, desc, overrides]);

  // エクスプローラーからのドロップ
  useEffect(() => {
    const un = getCurrentWebview().onDragDropEvent((e) => {
      if (e.payload.type !== "drop") return;
      if (useS.getState().nestKey) return; // 録画を差し込む画面が受ける
      const sc = scroll.current;
      if (!sc) return;
      const r = sc.getBoundingClientRect();
      const dpr = window.devicePixelRatio || 1;
      const px = e.payload.position.x / dpr;
      const py = e.payload.position.y / dpr;
      const inside = px >= r.left && px <= r.right && py >= r.top && py <= r.bottom;
      const st = useS.getState();
      const t = inside ? Math.max(0, (px - r.left + sc.scrollLeft - HEAD) / st.zoom) : st.t;
      let trackId: string | undefined;
      if (inside) {
        const el = document.elementsFromPoint(px, py).find((x) => (x as HTMLElement).dataset?.track) as HTMLElement | undefined;
        trackId = el?.dataset.track;
      }
      addMedia(e.payload.paths, t, trackId);
    });
    return () => { un.then((f) => f()); };
  }, []);

  // 行の並び
  const rows: Row[] = useMemo(() => {
    const out: Row[] = [];
    if (listed.length) out.push({ kind: "marks", h: Math.max(1, bandLanes(listed).n) * MARK_LANE + 4 }); // ルーラーのすぐ下
    const tracks = doc?.tracks ?? [];
    const vis = [...tracks.filter((x) => x.kind !== "audio")].reverse();
    for (const tr of vis) {
      if (tr.kind === "fx") {
        const lanes: Group[][] = [];
        const ends: number[] = [];
        const gs = desc?.groups ?? [];
        const sorted = [...gs].sort((a, b) => span(a, overrides[a.key])[0] - span(b, overrides[b.key])[0]);
        for (const g of sorted) {
          const [a, b] = span(g, overrides[g.key]);
          let li = ends.findIndex((e) => e <= a + 0.001);
          if (li < 0 && ends.length < fxRows) {
            li = ends.length;
            ends.push(0);
            lanes.push([]);
          }
          if (li < 0) li = ends.indexOf(Math.min(...ends)); // レーンが足りないときは、一番早く空くレーンに重ねる
          ends[li] = Math.max(ends[li], b);
          lanes[li].push(g);
        }
        out.push({ kind: "fx", lanes, h: Math.max(1, Math.min(lanes.length, fxRows)) * H.fx + 4, track: tr });
        if (guideOn) out.push({ kind: "ops", h: H.ops });
      } else out.push({ kind: "track", track: tr, h: H[tr.kind as "video"] });
    }
    if (guideOn && !out.some((r) => r.kind === "ops")) out.unshift({ kind: "ops", h: H.ops });
    out.push({ kind: "cue", h: H.cue });
    out.push({ kind: "subs", h: H.subs });
    const sps = [...new Set([...SPEAKERS, ...layout.map((x) => x.sp)])];
    for (const sp of sps) out.push({ kind: "voice", sp, h: H.voice });
    out.push({ kind: "se", h: H.se });
    if (guideOn) out.push({ kind: "gaudio", h: H.gaudio });
    for (const tr of tracks.filter((x) => x.kind === "audio")) out.push({ kind: "track", track: tr, h: H.audio });
    return out;
  }, [doc, desc, overrides, layout, fxRows, guideOn, listed]);

  const speakers = useMemo(() => [...new Set([...SPEAKERS, ...layout.map((x) => x.sp)])], [layout]);

  // ---- 目盛り
  const ticks: React.ReactNode[] = [];
  const major = zoom >= 80 ? 1 : zoom >= 30 ? 5 : zoom >= 12 ? 10 : zoom >= 5 ? 30 : 60;
  const minor = major / 5;
  for (let s = 0; s <= T; s += minor) {
    const isMajor = Math.abs(s / major - Math.round(s / major)) < 1e-6;
    ticks.push(
      <div key={s} className={`tick ${isMajor ? "" : "minor"}`} style={{ left: s * zoom }}>
        {isMajor ? tc(s).replace(/\.\d+$/, "") : ""}
      </div>,
    );
  }

  const seekFromEvent = (e: React.PointerEvent | PointerEvent, el: HTMLElement) => {
    const r = el.getBoundingClientRect();
    const t = clamp((e.clientX - r.left) / useS.getState().zoom, 0, total());
    player.seek(snap(t, snapOn && !(e as PointerEvent).altKey, useS.getState().zoom));
  };

  // ---- クリップのドラッグ（共通）
  function dragClip(
    e: React.PointerEvent,
    onMove: (dt: number, ev: PointerEvent) => void,
    onUp: (dt: number, moved: boolean, ev: PointerEvent) => void,
  ) {
    if (e.button !== 0) return;
    e.stopPropagation();
    const el = e.currentTarget as HTMLElement;
    el.setPointerCapture(e.pointerId);
    const x0 = e.clientX;
    let moved = false;
    const move = (ev: PointerEvent) => {
      if (Math.abs(ev.clientX - x0) > 3) moved = true;
      if (moved) onMove((ev.clientX - x0) / zoom, ev);
    };
    const up = (ev: PointerEvent) => {
      el.removeEventListener("pointermove", move);
      el.removeEventListener("pointerup", up);
      onUp((ev.clientX - x0) / zoom, moved, ev);
    };
    el.addEventListener("pointermove", move);
    el.addEventListener("pointerup", up);
  }

  const [preview, setPreview] = useState<{ id: string; a: number; b: number } | null>(null);
  // 同期点（CUE 行の◆）のドラッグ中の位置（鍵は cue と キーワード）
  const [sprev, setSprev] = useState<{ cue: number; keyword: string; t: number } | null>(null);
  const sprevRef = useRef(sprev);
  sprevRef.current = sprev;
  // 離したあとは、演出を読み直した新しい位置（desc）が届くまでプレビューの位置に置いておく（元の位置へ一瞬戻らないように）
  useEffect(() => setSprev(null), [desc]);
  // 動かした先の t だけ渡す。t_end のずらし方・丸めは Rust（syncman::moved）が決める
  const setSync = (cue: number, keyword: string, t: number | null) =>
    call("sync_set", { cue, keyword, t }).then(() => true, (e) => (toastError("同期点を動かせませんでした", e), false));

  const fxClip = (g: Group, lane: number) => {
    const o = overrides[g.key];
    let [a, b] = span(g, o);
    if (preview?.id === g.key) [a, b] = [preview.a, preview.b];
    const sel = selection?.kind === "fx" && selection.key === g.key;
    const adj = !!o && Object.keys(o).some((k) => k !== "hidden" && k !== "note" && k !== "no_placeholder");
    const rec = isRecGroup(g);
    const nestN = rec ? (doc?.nests?.[g.key]?.tracks ?? []).reduce((n, tr) => n + tr.clips.length, 0) : 0;
    const startDrag = (mode: "move" | "l" | "r") => (e: React.PointerEvent) => {
      useS.setState({ selection: { kind: "fx", key: g.key } });
      const [a0, b0] = span(g, o);
      const others = (desc?.groups ?? []).filter((x) => x.key !== g.key).flatMap((x) => span(x, overrides[x.key]));
      dragClip(
        e,
        (dt, ev) => {
          const sn = snapOn && !ev.altKey;
          if (mode === "move") {
            const na = snap(a0 + dt, sn, zoom, others);
            setPreview({ id: g.key, a: na, b: b0 + (na - a0) });
          } else if (mode === "r") setPreview({ id: g.key, a: a0, b: Math.max(a0 + 0.1, snap(b0 + dt, sn, zoom, others)) });
          else setPreview({ id: g.key, a: Math.min(b0 - 0.1, snap(a0 + dt, sn, zoom, others)), b: b0 });
        },
        async (_dt, moved) => {
          const p = previewRef.current;
          setPreview(null);
          if (!moved || !p) {
            // 再生位置がクリップの外なら、クリップの中へ（入りのアニメが終わったあたり）
            const tt = useS.getState().t;
            if (!moved && (tt < a0 || tt >= b0)) player.seek(a0 + Math.min(0.5, (b0 - a0) / 2));
            return;
          }
          const sh = (o?.shift || 0) + (p.a - a0);
          const en = (o?.end || 0) + (p.b - b0) - (p.a - a0);
          await patchOverride(g.key, { shift: Math.round(sh * 1000) / 1000 || null, end: Math.round(en * 1000) / 1000 || null } as any);
        },
      );
    };
    return (
      <div
        key={g.key}
        className={`clip fx ${g.gui ? "gui" : ""} ${adj ? "adj" : ""} ${o?.hidden ? "hid" : ""} ${sel ? "sel" : ""}`}
        style={{ left: a * zoom, width: Math.max(3, (b - a) * zoom - 1), top: 2 + lane * H.fx, height: H.fx - 2, fontSize: 10, padding: "0 4px" }}
        title={`${g.key}\n${tc(a)} – ${tc(b)}\n${g.src.split("\n")[0]}\n\nドラッグで移動・端で長さを変更、ダブルクリックで${rec ? "録画の差し込み" : "コードを開く"}、右クリックでメニュー${rec ? `\n差し込んだ録画: ${nestN} 個` : ""}`}
        onPointerDown={startDrag("move")}
        onDoubleClick={() => {
          if (rec) useS.setState({ nestKey: g.key, selection: { kind: "fx", key: g.key } });
          else if (desc) call("open_in_editor", { path: desc.path, line: g.line });
        }}
        onContextMenu={(e) => {
          e.preventDefault();
          useS.setState({ selection: { kind: "fx", key: g.key } });
          setMenu({ x: e.clientX, y: e.clientY, key: g.key });
        }}
      >
        <div className="edge l" onPointerDown={startDrag("l")} />
        {g.label}
        <span style={{ opacity: 0.6 }}> {g.key.split("@")[1]}</span>
        {rec && <span className={`nest-badge ${nestN ? "" : "empty"}`}>{nestN ? `▣${nestN}` : "▣ 空"}</span>}
        <div className="edge r" onPointerDown={startDrag("r")} />
      </div>
    );
  };
  const previewRef = useRef(preview);
  previewRef.current = preview;

  // 操作の印のドラッグ中の位置。離したあとは、保存した新しい位置（guide）が届くまでそこに置く
  const [oprev, setOprev] = useState<{ id: string; t: number } | null>(null);
  const oprevRef = useRef(oprev);
  oprevRef.current = oprev;
  useEffect(() => setOprev(null), [guide]);

  // 印の帯の両端のドラッグ中の区間
  const [mkPrev, setMkPrev] = useState<{ id: number; a: number; b: number } | null>(null);
  const mkPrevRef = useRef(mkPrev);
  mkPrevRef.current = mkPrev;

  const [vprev, setVprev] = useState<{ id: string; start: number } | null>(null);
  const vprevRef = useRef(vprev);
  vprevRef.current = vprev;
  const [mprev, setMprev] = useState<{ id: string; start: number; dur: number; src_in: number } | null>(null);
  const mprevRef = useRef(mprev);
  mprevRef.current = mprev;

  // トラックのロック（プレビューのクリックで選ばない。issue 旧#11）
  const lockBtn = (tr: Track) => (
    <button className={tr.locked ? "on" : ""} title={LOCK_HELP} onClick={() => editDoc((d) => { const t = d.tracks.find((x) => x.id === tr.id)!; t.locked = !t.locked; })}>
      {tr.locked ? "🔒" : "🔓"}
    </button>
  );

  const renderRow = (row: Row, i: number) => {
    let head: React.ReactNode;
    let body: React.ReactNode = null;
    if (row.kind === "fx") {
      head = (
        <>
          <span className="k">{TRACK_KIND.fx.badge}</span>
          <span className="nm" title={`クリップ数: ${(desc?.groups.length ?? 1) - 1}`}>{TRACK_KIND.fx.name}</span>
          <button title="レーンを減らす" onClick={() => { const v = Math.max(1, fxRows - 1); setFxRows(v); localStorage.setItem("fxRows", String(v)); }}>−</button>
          <button title="レーンを増やす" onClick={() => { const v = Math.min(40, fxRows + 1); setFxRows(v); localStorage.setItem("fxRows", String(v)); }}>＋</button>
          <button className={row.track.hidden ? "" : "on"} title="表示" onClick={() => editDoc((d) => { const t = d.tracks.find((x) => x.id === row.track.id)!; t.hidden = !t.hidden; })}>👁</button>
          {lockBtn(row.track)}
        </>
      );
      body = row.lanes.flatMap((lane, li) => lane.map((g) => fxClip(g, li)));
    } else if (row.kind === "track") {
      const tr = row.track;
      const isAudio = tr.kind === "audio";
      head = (
        <>
          <span className="k">{TRACK_KIND[isAudio ? "audio" : "video"].badge}</span>
          <input
            className="nm"
            value={tr.name}
            onChange={(e) => editDoc((d) => { d.tracks.find((x) => x.id === tr.id)!.name = e.target.value; }, false)}
            style={{ background: "transparent", border: 0, padding: 0, color: "inherit", width: 60 }}
          />
          {isAudio ? (
            <button className={tr.muted ? "" : "on"} title="ミュート" onClick={() => editDoc((d) => { const t = d.tracks.find((x) => x.id === tr.id)!; t.muted = !t.muted; })}>♪</button>
          ) : (
            <button className={tr.hidden ? "" : "on"} title="表示" onClick={() => editDoc((d) => { const t = d.tracks.find((x) => x.id === tr.id)!; t.hidden = !t.hidden; })}>👁</button>
          )}
          {!isAudio && lockBtn(tr)}
        </>
      );
      body = tr.clips.map((c) => {
        const p = mprev?.id === c.id ? mprev : null;
        const start = p ? p.start : c.start;
        const dur = p ? p.dur : c.dur;
        const sel = selection?.kind === "media" && selection.clip === c.id;
        const cls = mediaClass(c.src);
        const drag = (mode: "move" | "l" | "r") => (e: React.PointerEvent) => {
          useS.setState({ selection: { kind: "media", track: tr.id, clip: c.id } });
          dragClip(
            e,
            (dt, ev) => {
              const sn = snapOn && !ev.altKey;
              if (mode === "move") setMprev({ id: c.id, start: Math.max(0, snap(c.start + dt, sn, zoom)), dur: c.dur, src_in: c.src_in });
              else if (mode === "r") setMprev({ id: c.id, start: c.start, dur: Math.max(0.1, snap(c.start + c.dur + dt, sn, zoom) - c.start), src_in: c.src_in });
              else {
                const ns = clamp(snap(c.start + dt, sn, zoom), Math.max(0, c.start - c.src_in), c.start + c.dur - 0.1);
                setMprev({ id: c.id, start: ns, dur: c.dur - (ns - c.start), src_in: c.src_in + (ns - c.start) });
              }
            },
            (_dt, moved) => {
              const p2 = mprevRef.current;
              setMprev(null);
              if (!moved || !p2) return;
              editDoc((d) => {
                const cc = d.tracks.find((x) => x.id === tr.id)!.clips.find((x) => x.id === c.id)!;
                cc.start = Math.round(p2.start * 1000) / 1000;
                cc.dur = Math.round(p2.dur * 1000) / 1000;
                cc.src_in = Math.round(p2.src_in * 1000) / 1000;
              });
            },
          );
        };
        // トランジションの印（入りは左端、抜けは右端の斜線）
        const tIn = cls === "aud" ? 0 : transDur(c.trans_in, dur);
        const tOut = cls === "aud" ? 0 : transDur(c.trans_out, dur);
        const tLabel = (k?: string) => TRANS_KINDS.find((x) => x.k === k)?.label ?? k;
        const tText = (tIn ? `\n入り: ${tLabel(c.trans_in?.kind)} ${tIn.toFixed(2)}s` : "") + (tOut ? `\n抜け: ${tLabel(c.trans_out?.kind)} ${tOut.toFixed(2)}s` : "");
        return (
          <div
            key={c.id}
            className={`clip ${cls} ${sel ? "sel" : ""}`}
            style={{ left: start * zoom, width: Math.max(4, dur * zoom - 1), top: 3, height: row.h - 6 }}
            title={`${c.name}\n${tc(start)} – ${tc(start + dur)}${c.loop ? "（ループ）" : ""}${tText}`}
            onPointerDown={drag("move")}
            onContextMenu={(e) => {
              e.preventDefault();
              if (confirm(`「${c.name}」をタイムラインから削除しますか？`)) {
                editDoc((d) => {
                  const t2 = d.tracks.find((x) => x.id === tr.id)!;
                  t2.clips = t2.clips.filter((x) => x.id !== c.id);
                });
              }
            }}
          >
            {tIn > 0 && <div className="trans-mk in" style={{ width: tIn * zoom }} />}
            {tOut > 0 && <div className="trans-mk out" style={{ width: tOut * zoom }} />}
            <div className="edge l" onPointerDown={drag("l")} />
            {cls === "img" && <img src={fileUrl(c.src)} style={{ height: row.h - 8, verticalAlign: "middle", marginRight: 4 }} />}
            {c.name}
            <div className="edge r" onPointerDown={drag("r")} />
          </div>
        );
      });
    } else if (row.kind === "voice") {
      head = (
        <>
          <span className="k">{TRACK_KIND.voice.badge}</span>
          <span className={`nm sp ${spClass(row.sp, speakers)}`} style={{ padding: "0 4px", borderRadius: 3, fontSize: 11 }}>{row.sp}</span>
          <button
            className={doc?.voice_muted ? "" : "on"}
            title="ミュート"
            onClick={() => editDoc((d) => { d.voice_muted = !d.voice_muted; })}
          >
            ♪
          </button>
          <button
            className={selection?.kind === "subs" && selection.sp === row.sp ? "on" : ""}
            title={`${row.sp} の声と字幕スタイル`}
            onClick={() => useS.setState({ selection: { kind: "subs", sp: row.sp } })}
          >
            ⚙
          </button>
        </>
      );
      body = layout
        .filter((it) => it.sp === row.sp)
        .map((it) => {
          const start = vprev?.id === it.line_id ? vprev.start : it.start;
          const sel = selection?.kind === "line" && selection.id === it.line_id;
          return (
            <div
              key={it.line_id}
              className={`clip voice ${spClass(it.sp, speakers)} ${it.audio ? "" : "est"} ${sel ? "sel" : ""}`}
              style={{ left: start * zoom, width: Math.max(4, it.dur * zoom - 1), top: 3, height: row.h - 6 }}
              title={`${it.n}. ${it.text}\n${tc(it.start)}（${it.dur.toFixed(2)}s）${it.audio ? "" : "\n音声がまだありません（長さは推定）"}\nドラッグで移動（台本の順番も変わります）`}
              onPointerDown={(e) =>
                dragClip(
                  e,
                  (dt, ev) => setVprev({ id: it.line_id, start: Math.max(0, snap(it.start + dt, snapOn && !ev.altKey, zoom)) }),
                  (_dt, moved) => {
                    const p = vprevRef.current;
                    setVprev(null);
                    useS.setState({ selection: { kind: "line", id: it.line_id }, focusLine: it.line_id });
                    if (!moved || !p) {
                      player.seek(it.start);
                      return;
                    }
                    editScript((s) => moveLineTo(s, it.line_id, p.start));
                  },
                )
              }
            >
              <span style={{ opacity: 0.7, marginRight: 4 }} className="ui">{it.n}</span>
              {it.text}
            </div>
          );
        });
    } else if (row.kind === "subs") {
      const hidSubs = !!doc?.subtitle?.hidden;
      head = (
        <>
          <span className="k">{TRACK_KIND.sub.badge}</span>
          <span className="nm" title="セリフの字幕です。モニター・完成版 mp4・字幕の透過 .mov で同じ見た目になります">{TRACK_KIND.sub.name}</span>
          <button
            className={hidSubs ? "" : "on"}
            title="表示（モニター・書き出しの両方）"
            onClick={() => editDoc((d) => { d.subtitle = { ...d.subtitle, hidden: hidSubs ? undefined : true }; })}
          >
            👁
          </button>
          <button
            className={selection?.kind === "subs" && !selection.sp ? "on" : ""}
            title="字幕スタイル（フォント・色・縁・大きさ・位置・余白）"
            onClick={() => useS.setState({ selection: { kind: "subs" } })}
          >
            ⚙
          </button>
        </>
      );
      body = subViewErr && !subView ? (
        <span className="dim2" style={{ position: "sticky", left: 6, top: 6, fontSize: 11, color: "var(--error)" }}>字幕の見た目を取得できませんでした: {subViewErr}</span>
      ) : [...layout].sort((p, q) => p.start - q.start || p.n - q.n).flatMap((it) => {
        const sv = subView?.lines[it.line_id];
        if (!sv) return []; // 見た目を取り直している間は出さない
        const { a, b } = sv;
        const ck = subCheck?.lines[it.line_id];
        const own = doc?.subtitle_lines?.[sv.key];
        const sel = selection?.kind === "line" && selection.id === it.line_id;
        const ve = it.start + it.dur;
        const lines = ck?.texts?.length ? ck.texts : sv.text.split("\n");
        const flags = [ck?.overflow ? "はみ出し" : "", own?.text ? (sv.custom ? "字幕のみ変更" : "手動改行") : "",
          own && (own.font || own.size) ? "フォント・大きさ" : ""].filter(Boolean);
        return (
          <div
            key={it.line_id}
            className={`clip sub ${spClass(it.sp, speakers)} ${ck?.overflow ? "over" : ""} ${own ? (sv.custom ? "custom" : "own") : ""} ${hidSubs ? "hid" : ""} ${sel ? "sel" : ""}`}
            style={{ left: a * zoom, width: Math.max(3, (b - a) * zoom - 1), top: 3, height: row.h - 6 }}
            title={`${it.n}. [${it.sp}]\n${lines.join("\n")}\n${tc(a)} – ${tc(b)}（声の終わり ${tc(ve)}）${flags.length ? "\n（" + flags.join("・") + "）" : ""}\nクリックで選択（右の「調整」で改行・フォント・大きさを変更）`}
            onPointerDown={(e) => {
              if (e.button !== 0) return;
              e.stopPropagation();
              useS.setState({ selection: { kind: "line", id: it.line_id }, focusLine: it.line_id });
              const tt = useS.getState().t;
              if (tt < a || tt >= b) player.seek(a + Math.min(0.3, (b - a) / 2));
            }}
          >
            {ck?.overflow ? "⚠ " : ""}
            {lines.join(" ⏎ ")}
            {/* 声（WAV）が終わってからの余韻は薄く描き、左の線で声の終わりを示す（枠線 1px のぶん引く） */}
            {ve < b - 1e-3 && <span className="tail" style={{ left: (ve - a) * zoom - 1 }} />}
          </div>
        );
      });
    } else if (row.kind === "se") {
      head = (
        <>
          <span className="k">{TRACK_KIND.se.badge}</span>
          <span className="nm">{TRACK_KIND.se.name}</span>
          <button className={doc?.se_muted ? "" : "on"} title="ミュート" onClick={() => editDoc((d) => { d.se_muted = !d.se_muted; })}>♪</button>
          <button
            title="効果音を作り直します（演出の調整後の時刻で）"
            disabled={seBusy || !desc}
            onClick={async () => {
              setSeBusy(true);
              try {
                const r: any = await call("export", { kind: "se" });
                player.seVersion++;
                toast(`効果音を作り直しました（${r.count} 個）`, "ok");
              } catch (e) {
                toastError("効果音を作成できませんでした", e);
              } finally {
                setSeBusy(false);
              }
            }}
          >
            {seBusy ? "…" : "↻"}
          </button>
          <button
            className={selection?.kind === "se" ? "on" : ""}
            title="効果音の設定"
            onClick={() => useS.setState({ selection: { kind: "se" } })}
          >
            ⚙
          </button>
        </>
      );
      body = (desc?.groups ?? []).flatMap((g) => {
        const o = overrides[g.key];
        if (o?.hidden) return [];
        return g.se.map((ev, k) => (
          <div
            key={`${g.key}${k}`}
            className={`se-mk ${selection?.kind === "se" && selection.se === ev.kind ? "on" : ""}`}
            style={{ left: (ev.t + (o?.shift || 0)) * zoom, top: 2, height: row.h - 4 }}
            title={`${seLabel(ev.kind, seBank)}（${ev.kind}・${g.key}）
クリックでこの種類の効果音を設定`}
            onPointerDown={(e) => e.stopPropagation()}
            onClick={(e) => {
              e.stopPropagation();
              useS.setState({ selection: { kind: "se", se: ev.kind } });
            }}
          />
        ));
      });
    } else if (row.kind === "gaudio" && guide) {
      // 攻略動画の素材の音（通しの素材の音を、映像と同じ並びにした wav）。♪ で消音、⚙ で音量
      const ga = guideAudioRow(guide.audio_rev, guide.warnings, !!doc?.guide_audio_muted, guide.audio_err);
      const end = guide.plan_parts.at(-1)?.t1 ?? 0;
      head = (
        <>
          <span className="k">{TRACK_KIND.guide_audio.badge}</span>
          <span className="nm" title={ga.title}>{TRACK_KIND.guide_audio.name}</span>
          <button className={doc?.guide_audio_muted ? "" : "on"} title="ミュート" onClick={() => editDoc((d) => { d.guide_audio_muted = !d.guide_audio_muted; })}>♪</button>
          <button
            className={selection?.kind === "guide_audio" ? "on" : ""}
            title="素材の音の設定（音量）"
            onClick={() => useS.setState({ selection: { kind: "guide_audio" } })}
          >
            ⚙
          </button>
        </>
      );
      body = ga.empty || end <= 0 ? [] : [
        <div
          key="ga"
          className={`gaudio-bar ${doc?.guide_audio_muted ? "muted" : ""}`}
          style={{ left: 0, width: end * zoom, top: 3, height: row.h - 6 }}
          title={ga.title}
          onPointerDown={(e) => e.stopPropagation()}
          onClick={() => useS.setState({ selection: { kind: "guide_audio" } })}
        />,
      ];
    } else if (row.kind === "marks") {
      // モニターに描いた印の区間 [t0, t1)。両端のドラッグで区間を変え、クリックでその印の位置へ移って選ぶ
      head = (
        <>
          <span className="k">✎</span>
          <span className="nm" title="モニターに描いた印の区間です。両端のドラッグで区間を変え、クリックでその印へ移動します">印</span>
        </>
      );
      const { lane } = bandLanes(listed);
      const r3 = (v: number) => Math.round(v * 1000) / 1000;
      body = listed.map((m) => {
        const pv = mkPrev?.id === m.id ? mkPrev : null;
        const a = pv ? pv.a : m.t0;
        const b = pv ? pv.b : m.t1;
        const hex = markHex(m.color);
        const edge = (mode: "l" | "r") => (e: React.PointerEvent) => {
          const a0 = m.t0;
          const b0 = m.t1;
          dragClip(
            e,
            (dt, ev) => {
              const sn = snapOn && !ev.altKey;
              if (mode === "l") setMkPrev({ id: m.id, a: Math.max(0, Math.min(b0 - 0.1, snap(a0 + dt, sn, zoom))), b: b0 });
              else setMkPrev({ id: m.id, a: a0, b: Math.max(a0 + 0.1, snap(b0 + dt, sn, zoom)) });
            },
            async (_dt, moved) => {
              const p = mkPrevRef.current;
              if (!moved || !p) {
                setMkPrev(null);
                return;
              }
              await upsertMark({ ...m, t0: r3(p.a), t1: r3(p.b) });
              setMkPrev(null);
            },
          );
        };
        return (
          <div
            key={m.id}
            className={`clip mark-band ${m.state === "done" ? "done" : ""} ${m.by === "claude" ? "claude" : ""} ${markSel === m.id ? "sel" : ""}`}
            style={{ left: a * zoom, width: Math.max(8, (b - a) * zoom - 1), top: 2 + (lane.get(m.id) ?? 0) * MARK_LANE, height: MARK_LANE - 2, background: `${hex}55`, borderColor: hex }}
            title={`印${circled(m.id)} ${m.note}\n${tc(a)} – ${tc(b)}${m.state === "done" ? "（済）" : m.state === "sent" ? "（送信済み）" : ""}\nクリックでその印へ移動・端のドラッグで区間を変更`}
            onPointerDown={(e) =>
              dragClip(e, () => {}, (_dt, moved) => {
                if (moved) return;
                player.seek(m.t);
                selectMark(m.id, true);
              })
            }
          >
            <div className="edge l" onPointerDown={edge("l")} />
            <b style={{ color: hex }}>{circled(m.id)}</b> {m.note}
            <div className="edge r" onPointerDown={edge("r")} />
          </div>
        );
      });
    } else if (row.kind === "ops" && guide) {
      // 攻略動画の操作（手動パートの出力の時刻に印。精密は ◆、大まかは ●、信頼枠は灰色の点）
      const span0 = manualSpan(guide);
      head = (
        <>
          <span className="k">{TRACK_KIND.ops.badge}</span>
          <span className="nm" title="手動パートの操作の時刻です。印をドラッグすると時刻（素材のコマ）を直せます">{TRACK_KIND.ops.name}</span>
          <button disabled={!span0} title="再生位置に操作を足します（手動パートの中だけ）" onClick={addOpHere}>＋</button>
        </>
      );
      body = [
        // カットの継ぎ目（クロスフェードの長さ。フェードが無ければ縦の線）
        ...guide.cuts_out.map(([c0, c1], k) => (
          <div
            key={`cut${k}`}
            className="cut-mk"
            style={{ left: c0 * zoom, width: Math.max(2, (c1 - c0) * zoom), height: row.h }}
            title={`カットの継ぎ目 ${tc(c0, 60)}${c1 > c0 ? ` – ${tc(c1, 60)}（クロスフェード）` : ""}`}
          />
        )),
        ...opRows(guide).flatMap((o) => {
          const p = oprev?.id === o.id ? oprev.t : o.tOut;
          if (p === null) return []; // カットの中・手動の外の操作は出力に無い（一覧に理由を出している）
          const kind = opMarkKind(o);
          return [
            <div
              key={o.id}
              className={`op-mk ${kind} ${guideOp === o.id ? "sel" : ""}`}
              style={{ left: p * zoom, top: 2, height: row.h - 4 }}
              title={`${opMarkTitle(o)}
出力 ${tc(p, 60)}${o.cost !== null ? ` / コスト ${o.cost}` : ""}${o.game ? ` / ゲーム内 ${o.game}` : ""}
クリックでその位置へ・ドラッグで時刻を直す・右クリックでメニュー`}
              onContextMenu={(e) => {
                e.preventDefault();
                e.stopPropagation();
                setMenu(null);
                setGmenu({ x: e.clientX, y: e.clientY, op: o.id });
              }}
              onPointerDown={(e) => {
                if (!span0) return;
                const t0 = p;
                dragClip(
                  e,
                  (dt, ev) => {
                    const nt = clamp(snap(t0 + dt, snapOn && !ev.altKey, zoom), span0[0], span0[1]);
                    setOprev({ id: o.id, t: nt });
                  },
                  (_dt, moved) => {
                    const pv = oprevRef.current;
                    if (!moved || !pv) {
                      setOprev(null);
                      seekOp(o.id);
                      return;
                    }
                    useS.setState({ guideOp: o.id });
                    // 保存できたら guide が変わって印が新しい位置に来る。断られたら元の位置へ戻す
                    dragOpTo(o.id, pv.t).then((ok) => ok || setOprev(null));
                  },
                );
              }}
            >
              <span className="op-sym">{kind === "precise" ? "◆" : "●"}</span>
            </div>,
          ];
        }),
      ];
    } else {
      head = (
        <>
          <span className="k">{TRACK_KIND.cue.badge}</span>
          <span className="nm" title="演出が読み込む files/script.srt の cue と同期点">{TRACK_KIND.cue.name}</span>
          {drift && !drift.ok && (
            // 台本の声と演出の cue がずれている。押すと台本 → 演出の入力を書き出す
            <button
              className="drift-btn"
              disabled={talkBusy}
              title={`${driftText(drift)}\n\n${DRIFT_HELP}\n\nクリックで書き出します`}
              onClick={async () => {
                setTalkBusy(true);
                await exportTalk();
                setTalkBusy(false);
              }}
            >
              {talkBusy ? "…" : drift.offs.length ? `⚠${drift.max.toFixed(2)}s` : "⚠行数"}
            </button>
          )}
        </>
      );
      body = [
        // 声の頭と cue の頭がずれている所（cue から声までの帯。赤 = 声のほうが遅い）
        ...(drift?.offs ?? []).map((o) => (
          <div
            key={`d${o.i}`}
            className={`drift-mk ${o.d > 0 ? "late" : ""}`}
            style={{ left: Math.min(o.cue, o.voice) * zoom, width: Math.max(2, Math.abs(o.d) * zoom) }}
            title={`cue ${o.i + 1}: 演出 ${tc(o.cue)} / 声（行 ${o.n}）${tc(o.voice)}（${o.d > 0 ? "+" : ""}${o.d.toFixed(2)} 秒）`}
          />
        )),
        ...(desc?.cues ?? []).map((c) => (
          <div key={`c${c.n}`} style={{ position: "absolute", left: c.start * zoom, top: 0, height: row.h, borderLeft: "1px solid #ffffff30", fontSize: 9, paddingLeft: 2, color: "var(--dim)", fontFamily: "var(--ui)", whiteSpace: "nowrap" }} title={`cue ${c.n} [${c.speaker}] ${c.text}`}>
            {zoom > 6 ? c.n : ""}
          </div>
        )),
        ...(desc?.sync ?? []).map((y, k) => {
          const manual = y.method === "manual";
          const t = sprev && sprev.cue === y.cue && sprev.keyword === y.keyword ? sprev.t : y.t;
          return (
            <div
              key={`s${k}`}
              className="sync-hit"
              style={{ left: t * zoom, top: 1 }}
              title={`同期点 cue ${y.cue}「${y.keyword}」${y.method}
ドラッグで位置を調整${manual ? "・右クリックで自動の位置に戻す" : ""}`}
              onPointerDown={(e) =>
                dragClip(
                  e,
                  (dt, ev) => setSprev({ cue: y.cue, keyword: y.keyword, t: Math.max(0, snap(y.t + dt, snapOn && !ev.altKey, zoom)) }),
                  (_dt, moved) => {
                    const p = sprevRef.current;
                    if (!moved || !p) {
                      setSprev(null);
                      player.seek(y.t);
                      return;
                    }
                    setSync(y.cue, y.keyword, p.t).then((ok) => ok || setSprev(null));
                  },
                )
              }
              onContextMenu={(e) => {
                e.preventDefault();
                e.stopPropagation();
                if (!manual) return;
                setSync(y.cue, y.keyword, null).then((ok) => ok && toast(`同期点 cue ${y.cue}「${y.keyword}」を自動の位置に戻しました`, "ok"));
              }}
            >
              <div className={`sync-mk ${y.method === "ratio" ? "ratio" : ""} ${manual ? "manual" : ""}`} style={{ left: 4, top: 4 }} />
            </div>
          );
        }),
      ];
    }
    const trackId = row.kind === "track" ? row.track.id : row.kind === "fx" ? "fx" : undefined;
    const locked = (row.kind === "track" || row.kind === "fx") && row.track.locked;
    return (
      <div key={i} style={{ display: "flex", height: row.h, borderBottom: "1px solid var(--n-18)", width: W }} className={`${i % 2 ? "alt" : ""} ${locked ? "locked" : ""}`}>
        <div className="tl-head" style={{ width: HEAD, height: row.h, position: "sticky", left: 0, zIndex: 5, background: "var(--panel)" }} data-track={trackId}>
          {head}
        </div>
        <div style={{ position: "relative", flex: "none", width: W - HEAD, height: row.h }} data-track={trackId}
          onContextMenu={(e) => {
            // 攻略動画: クリップ・印の無い所を右クリックで、その位置に操作を足すメニュー
            if (!guideOn || e.target !== e.currentTarget) return;
            e.preventDefault();
            setMenu(null);
            setGmenu({ x: e.clientX, y: e.clientY, t: clamp((e.clientX - e.currentTarget.getBoundingClientRect().left) / useS.getState().zoom, 0, total()) });
          }}
          onPointerDown={(e) => {
            if (e.target === e.currentTarget) {
              useS.setState({ selection: null });
              seekFromEvent(e, e.currentTarget);
            }
          }}
        >
          {body}
        </div>
      </div>
    );
  };

  return (
    <div className="timeline-wrap">
      <div className="tl-tools">
        <span className="tag">// TIMELINE</span>
        <span className="dim2 ui">{tc(T)}</span>
        <span className="grow" />
        <button className={snapOn ? "on" : ""} onClick={() => setSnapOn(!snapOn)} title="cue・同期点・セリフの開始・再生位置にスナップします（Alt で一時的に無効）">スナップ</button>
        <button className={follow ? "on" : ""} onClick={() => setFollow(!follow)} title="再生中に再生位置を追ってスクロールします">追従</button>
        <span className="dim" style={{ fontSize: 12 }}>ズーム</span>
        <input type="range" min={2} max={400} step={1} value={zoom} onChange={(e) => {
          const sc = scroll.current;
          const t = useS.getState().t;
          const nz = Number(e.target.value);
          useS.setState({ zoom: nz });
          if (sc) requestAnimationFrame(() => { sc.scrollLeft = Math.max(0, t * nz - sc.clientWidth / 2 + HEAD); });
        }} style={{ width: 140 }} />
        <button onClick={() => addTrack("video")} title="映像トラックを追加">＋V</button>
        <button onClick={() => addTrack("image")} title="画像（立ち絵）トラックを追加">＋画像</button>
        <button onClick={() => addTrack("audio")} title="音声（BGM）トラックを追加">＋A</button>
      </div>
      <div
        className="tl-scroll"
        ref={scroll}
      >
        <div className="tl-inner" style={{ width: W }}>
          <div style={{ display: "flex", height: RULER, position: "sticky", top: 0, zIndex: 7, width: W }}>
            <div className="tl-head" style={{ width: HEAD, height: RULER, position: "sticky", left: 0, zIndex: 8, background: "var(--panel)" }}>
              <span className="tc" style={{ fontSize: 13, minWidth: 0 }}><TimeLabel /></span>
            </div>
            <div
              className="ruler"
              style={{ position: "relative", width: W - HEAD, height: RULER }}
              onContextMenu={(e) => {
                if (!guideOn) return;
                e.preventDefault();
                setMenu(null);
                setGmenu({ x: e.clientX, y: e.clientY, t: clamp((e.clientX - e.currentTarget.getBoundingClientRect().left) / useS.getState().zoom, 0, total()) });
              }}
              onPointerDown={(e) => {
                if (e.button !== 0) return;
                const el = e.currentTarget;
                el.setPointerCapture(e.pointerId);
                seekFromEvent(e, el);
                const mv = (ev: PointerEvent) => seekFromEvent(ev, el);
                const up = () => { el.removeEventListener("pointermove", mv); el.removeEventListener("pointerup", up); };
                el.addEventListener("pointermove", mv);
                el.addEventListener("pointerup", up);
              }}
            >
              {ticks}
              {zoom > 4 && scenes.map(({ scene: id, t0: a }) => (
                <div key={id} className="scene-mk" style={{ left: a * zoom }}>{zoom > 15 || !id.includes("-") ? id : ""}</div>
              ))}
              {cacheRanges.map(([a, b, s], k) =>
                s === 0 ? null : <div key={k} className={`renderbar ${s > 0 ? "ok" : "no"}`} style={{ left: a * zoom, width: Math.max(1, (b - a) * zoom) }} />,
              )}
              {inT != null && outT != null && outT > inT && <div className="inout" style={{ left: inT * zoom, width: (outT - inT) * zoom }} />}
              {padFrom != null && <div className="end-pad" style={{ left: padFrom * zoom, width: (T - padFrom) * zoom }} />}
              <div className="end-mk" style={{ left: T * zoom }} title={endWhy}>END {tc(T)}</div>
            </div>
          </div>
          <div style={{ position: "relative" }}>
            {rows.map(renderRow)}
            {padFrom != null && <div className="end-pad" style={{ left: HEAD + padFrom * zoom, width: (T - padFrom) * zoom }} />}
            <div className="end-line" style={{ left: HEAD + T * zoom }} />
            {inT != null && outT != null && outT > inT && (
              <div className="inout" style={{ left: HEAD + inT * zoom, width: (outT - inT) * zoom, background: "color-mix(in srgb, var(--accent) 5.10%, transparent)" }} />
            )}
          </div>
          <Playhead />
        </div>
      </div>
      {gmenu && guide && <GuideMenu m={gmenu} g={guide} onClose={() => setGmenu(null)} />}
      {menu && (
        <div className="menu" style={{ left: menu.x, top: menu.y, position: "fixed" }} onMouseLeave={() => setMenu(null)}>
          <div className="mi" onClick={() => { patchOverride(menu.key, { hidden: !overrides[menu.key]?.hidden }); setMenu(null); }}>{overrides[menu.key]?.hidden ? "表示" : "非表示"}<small>コードは変わりません（Delete キーでも可）</small></div>
          <div className="mi" onClick={() => { patchOverride(menu.key, null); setMenu(null); }}>調整をリセット</div>
          {desc?.groups.some((x) => x.key === menu.key && isRecGroup(x)) && (
            <div className="mi" onClick={() => { useS.setState({ nestKey: menu.key }); setMenu(null); }}>録画を差し込む…<small>窓の大きさを見ながら素材を配置（ネスト）</small></div>
          )}
          <div className="mi" onClick={() => { const g = desc?.groups.find((x) => x.key === menu.key); if (g && desc) call("open_in_editor", { path: desc.path, line: g.line }); setMenu(null); }}>コードを開く<small>VS Code で該当の行を開きます</small></div>
          <hr />
          <div className="mi" onClick={() => {
            const text = prompt(`${menu.key} について Claude に伝える内容`);
            if (text) sendChat(text, { kind: "clip_note", clip: menu.key });
            setMenu(null);
          }}>Claude に相談…<small>このクリップと再生位置を添えて送信</small></div>
        </div>
      )}
    </div>
  );
}

/** 攻略動画のタイムラインの右クリックメニュー（空いた所: 操作を足す / 操作の印: 消す・移動） */
function GuideMenu({ m, g, onClose }: { m: { x: number; y: number; t?: number; op?: string }; g: GuideState; onClose: () => void }) {
  const done = (f: () => void) => () => { onClose(); f(); };
  if (m.op) {
    const id = m.op;
    return (
      <div className="menu" style={{ left: m.x, top: m.y, position: "fixed" }}>
        <div className="mi" onClick={done(() => seekOp(id))}>ここへ移動<small>モニターをこの操作の位置へ動かします</small></div>
        <div className="mi" onClick={done(() => { if (confirm(`操作 ${id} を消しますか？`)) removeOp(id); })}>この操作を消す<small>ほかの操作の番号は変わりません</small></div>
      </div>
    );
  }
  const t = m.t ?? 0;
  const here = opAddError(g, t);
  const now = opAddError(g, useS.getState().t);
  return (
    <div className="menu" style={{ left: m.x, top: m.y, position: "fixed" }}>
      <div className="mi" style={here ? { opacity: 0.5, cursor: "default" } : undefined} onClick={here ? undefined : done(() => { addOpAt(t); })}>
        ここに操作を追加（{tc(t, 60)}）<small>{here ?? "この位置に操作を足して選びます"}</small>
      </div>
      <div className="mi" style={now ? { opacity: 0.5, cursor: "default" } : undefined} onClick={now ? undefined : done(() => { addOpHere(); })}>
        再生位置に操作を追加<small>{now ?? "モニターの今の位置に足します"}</small>
      </div>
    </div>
  );
}

function TimeLabel() {
  const t = useS((s) => s.t);
  return <>{tc(t)}</>;
}
