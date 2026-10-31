// 録画を差し込む画面（RecWindow をダブルクリックで開く）。Premiere のネストと同じく、
// 自分のキャンバス（既定 1920x1080）の上にトラックで素材を並べ、キャンバスの外は捨てて、窓にはめる。
// 再生・再生位置・元に戻すは本体と共通（useS.t・player・editDoc）。
import React, { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { akf, call } from "../io/api";
import { useS } from "../state";
import { errorMessage } from "../logic/errors.ts";
import { player } from "../io/player";
import { clamp, isFormControl, isTextEntry, releaseFocus, tc, wheelAct } from "../logic/util";
import { CropFields, Num, Num2, TransFields } from "./Inspector";
import { linkSwitch, prevClip } from "../logic/trans";
import { IMAGE_EXTS, VIDEO_EXTS, isImg } from "../logic/media";
import type { Group, MediaClip, Nest, NestFit, Override, Track } from "../types";
import {
  FIT_LABEL,
  findClip,
  fitScale,
  nestMap,
  newTrack,
  windowSpan,
} from "../logic/nest";
import {
  SPEED_MAX,
  SPEED_MIN,
  clipSpeed,
  commitDrag,
  maxEnd,
  minStart,
  setSpeed,
  slipSrcIn,
  sourceSpan,
  trimSrcIn,
} from "../logic/nestcalc";
import {
  addNestMedia,
  deleteClip,
  duplicateClip,
  editNest,
  ensureNest,
  mediaInfo,
  mediaInfoNow,
  splitClip,
  windowBox,
} from "../state/nest";
import { LOCK_HELP, selectable } from "../logic/pick";

const HEAD = 130;
const ROW = 42;
const RULER = 38;
const MEDIA_EXT: string[] = [...VIDEO_EXTS, ...IMAGE_EXTS];

// Esc・Delete・Ctrl+D などは、文字を打つ欄のときだけ欄に任せる（選択肢・スライドバーを触った直後でも効くように）。
// S（分割）だけは、選択肢の頭文字で選ぶ操作とぶつかるので、フォーム部品にフォーカスがあるときは使わない
const isTyping = () => isTextEntry(document.activeElement as HTMLInputElement | null);

const r3 = (v: number) => Math.round(v * 1000) / 1000;
const MAX_PX_PER_SEC = 600; // ネストのタイムラインの最大ズーム（1 フレーム 20px）

export default function NestEditor() {
  const key = useS((s) => s.nestKey);
  const desc = useS((s) => s.desc);
  const o = useS((s) => (key ? s.overrides[key] : undefined));
  const nest = useS((s) => (key ? s.project?.doc.nests?.[key] : undefined));
  const [sel, setSel] = useState<string | null>(null);
  const [showFx, setShowFx] = useState(true);
  const g = desc?.groups.find((x) => x.key === key);
  const close = useCallback(() => useS.setState({ nestKey: null }), []);

  // 開いたとき: ネストを用意して、再生位置を窓の中へ
  useEffect(() => {
    if (!key || !g) return;
    ensureNest(key);
    const [a, b] = windowSpan(g, useS.getState().overrides[key]);
    const t = useS.getState().t;
    if (t < a || t >= b) player.seek(a + Math.min(0.5, (b - a) / 2));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [key, !!g]);

  // キー: Esc 閉じる・Delete 外す・Ctrl+D 複製・Ctrl+K / S 再生位置で分割（本体の Delete＝演出を隠す、より先に取る）
  useEffect(() => {
    if (!key) return;
    const h = (e: KeyboardEvent) => {
      if (isTyping()) return;
      const ctrl = e.ctrlKey || e.metaKey;
      const stop = () => { e.preventDefault(); e.stopImmediatePropagation(); };
      if (e.key === "Escape") { stop(); close(); return; }
      if (!sel) return;
      if (e.key === "Delete" || e.key === "Backspace") { stop(); deleteClip(key, sel); setSel(null); }
      else if (ctrl && (e.key === "d" || e.key === "D")) { stop(); const id = duplicateClip(key, sel); if (id) setSel(id); }
      else if ((ctrl && (e.key === "k" || e.key === "K")) || (!ctrl && !e.altKey && !isFormControl(document.activeElement as HTMLInputElement | null) && (e.key === "s" || e.key === "S"))) {
        stop();
        const id = splitClip(key, sel, useS.getState().t);
        if (id) setSel(id);
      }
    };
    window.addEventListener("keydown", h, true);
    return () => window.removeEventListener("keydown", h, true);
  }, [key, sel, close]);

  // エクスプローラーからのドロップ（本体のタイムラインはこの画面を開いている間は受けない）
  useEffect(() => {
    if (!key) return;
    const un = getCurrentWebview().onDragDropEvent(async (e) => {
      if (e.payload.type !== "drop") return;
      const dpr = window.devicePixelRatio || 1;
      const px = e.payload.position.x / dpr;
      const py = e.payload.position.y / dpr;
      const el = document.elementsFromPoint(px, py).find((x) => (x as HTMLElement).dataset?.ntrack) as HTMLElement | undefined;
      let at: number | undefined;
      if (el?.dataset.nt0 && el.dataset.nzoom) {
        const r = el.getBoundingClientRect();
        at = Number(el.dataset.nt0) + (px - r.left) / Number(el.dataset.nzoom);
      }
      const id = await addNestMedia(key, e.payload.paths, at, el?.dataset.ntrack);
      if (id) setSel(id);
    });
    return () => { un.then((f) => f()); };
  }, [key]);

  if (!key) return null;
  if (!g) {
    return (
      <div className="nest-bg">
        <div className="nest-empty">
          <div>演出のクリップ {key} が見つかりません。演出の読み込み中か、コードの変更でキーが変わった可能性があります。</div>
          <button onClick={close}>閉じる</button>
        </div>
      </div>
    );
  }
  if (!nest) return <div className="nest-bg"><div className="nest-empty">準備中…</div></div>;

  const [a, b] = windowSpan(g, o);
  const box = windowBox(g, nest, o);
  const bw = Math.round(box[2] - box[0]);
  const bh = Math.round(box[3] - box[1]);
  const segs = g.rec?.[0]?.segs ?? [];
  const pickFiles = async () => {
    const r = await openDialog({ multiple: true, title: "素材を追加", filters: [{ name: "動画・画像", extensions: MEDIA_EXT }] });
    const paths = Array.isArray(r) ? r : typeof r === "string" ? [r] : [];
    if (paths.length) {
      const id = await addNestMedia(key, paths);
      if (id) setSel(id);
    }
  };

  return (
    <div className="nest-bg">
      <div className="nest">
        <div className="nest-head">
          <span className="tag">// NEST</span>
          <span className="nest-title">録画を差し込む</span>
          <span className="mono dim2" style={{ fontSize: 11 }}>{key}</span>
          {segs.map((s) => (
            <span key={s[0]} className="chip" title={`${s[2]}\n${s[3]}`}>PLAYBACK {s[1]}</span>
          ))}
          <span className="dim ui" style={{ fontSize: 12 }}>{tc(a)} – {tc(b)}（{(b - a).toFixed(2)}s）・窓 {bw}×{bh}</span>
          <span className="grow" />
          <span className="dim" style={{ fontSize: 12 }}>はめ方</span>
          <div className="seg">
            {(["contain", "stretch", "cover"] as NestFit[]).map((f) => (
              <button key={f} className={nest.fit === f ? "on" : ""} onClick={() => editNest(key, (n) => { n.fit = f; })}
                title={f === "contain" ? "縦横比を保って窓に収めます（余白は透明）" : f === "stretch" ? "窓いっぱいに引き伸ばします（縦横比が変わります）" : "縦横比を保って窓を埋めます（はみ出しは切り取り）"}>
                {FIT_LABEL[f]}
              </button>
            ))}
          </div>
          <button className={showFx ? "on" : ""} onClick={() => setShowFx(!showFx)} title="演出（窓の枠・チップ・テロップ）を重ねて表示">演出</button>
          <button onClick={() => desc && call("open_in_editor", { path: desc.path, line: g.line })} title="VS Code で RecWindow の行を開きます">コードを開く</button>
          <button className="primary" onClick={close} title="Esc">閉じる</button>
        </div>
        <div className="nest-body">
          <div className="nest-main">
            <NestMonitor g={g} nest={nest} o={o} sel={sel} setSel={setSel} showFx={showFx} />
            <div className="transport">
              <button className="ghost" onClick={() => player.seek(a)} title="窓の開始へ">⏮</button>
              <button className="ghost" onClick={() => player.seek(useS.getState().t - 1 / (desc?.fps ?? 30))} title="1 コマ戻る（←）">◀</button>
              <button className="primary" onClick={() => player.toggle()} title="再生・停止（Space）" style={{ minWidth: 44 }}><PlayIcon /></button>
              <button className="ghost" onClick={() => player.seek(useS.getState().t + 1 / (desc?.fps ?? 30))} title="1 コマ進む（→）">▶</button>
              <button className="ghost" onClick={() => player.seek(Math.max(a, b - 0.05))} title="窓の終了へ">⏭</button>
              <span className="tc"><Tc /></span>
              <span className="grow" />
              <button onClick={pickFiles} title="録画・画像を再生位置に追加します（タイムラインへのドロップでも追加できます）">＋ 素材</button>
              <button disabled={!sel} onClick={() => { const id = sel && splitClip(key, sel, useS.getState().t); if (id) setSel(id); }} title="再生位置で分割（S / Ctrl+K）">分割</button>
              <button disabled={!sel} onClick={() => { const id = sel && duplicateClip(key, sel); if (id) setSel(id); }} title="すぐ後ろに複製（Ctrl+D）">複製</button>
              <button className="danger" disabled={!sel} onClick={() => { if (sel) deleteClip(key, sel); setSel(null); }} title="Delete">削除</button>
            </div>
          </div>
          <NestInspector g={g} nest={nest} o={o} nkey={key} sel={sel} setSel={setSel} />
        </div>
        <NestTimeline g={g} nest={nest} o={o} nkey={key} sel={sel} setSel={setSel} onAdd={pickFiles} />
      </div>
    </div>
  );
}

function PlayIcon() {
  const playing = useS((s) => s.playing);
  return <>{playing ? "❚❚" : "▶"}</>;
}

function Tc() {
  const t = useS((s) => s.t);
  const fps = useS((s) => s.desc?.fps ?? 30);
  return <>{tc(t, fps)}</>;
}

// ====================================================================== モニター（窓のまわりだけを切り出して見る）
type MonProps = { g: Group; nest: Nest; o?: Override; sel: string | null; setSel: (id: string | null) => void; showFx: boolean };

function NestMonitor({ g, nest, o, sel, setSel, showFx }: MonProps) {
  const t = useS((s) => s.t);
  const playing = useS((s) => s.playing);
  const overrides = useS((s) => s.overrides);
  const doc = useS((s) => s.project?.doc);
  const engineGen = useS((s) => s.engine?.status.gen);
  const fps = useS((s) => s.desc?.fps ?? 30);
  const stage = useRef<HTMLDivElement>(null);
  const canvas = useRef<HTMLCanvasElement>(null);
  const [area, setArea] = useState({ w: 800, h: 450 });
  const [err, setErr] = useState("");
  const [, setInfoTick] = useState(0);
  const seq = useRef(0);
  const inflight = useRef(0);
  const shown = useRef(-1);
  const [ghost, setGhost] = useState<{ id: string; dx: number; dy: number; s: number } | null>(null);

  useLayoutEffect(() => {
    const el = stage.current!;
    const ro = new ResizeObserver(() => setArea({ w: el.clientWidth - 24, h: el.clientHeight - 24 }));
    ro.observe(el);
    return () => ro.disconnect();
  }, []);

  // 見る範囲（1920x1080 の座標）: 窓のまわり＋キャンバスの外枠
  const box = windowBox(g, nest, o);
  const m = nestMap(nest, box);
  const cr = [m.ox, m.oy, m.ox + nest.w * m.sx, m.oy + nest.h * m.sy];
  const M = 40;
  const vx0 = Math.max(0, Math.min(box[0] - M, cr[0] - 12));
  const vy0 = Math.max(0, Math.min(box[1] - M, cr[1] - 12));
  const vx1 = Math.min(1920, Math.max(box[2] + M, cr[2] + 12));
  const vy1 = Math.min(1080, Math.max(box[3] + M, cr[3] + 12));
  const vw = Math.max(10, vx1 - vx0);
  const vh = Math.max(10, vy1 - vy0);
  const dispW = Math.max(160, Math.min(area.w, (area.h * vw) / vh));
  const dispH = (dispW * vh) / vw;
  const k = dispW / vw; // 1920 座標 → 表示
  const X = (x: number) => (x - vx0) * k;
  const Y = (y: number) => (y - vy0) * k;
  const view = useRef({ vx0, vy0, vw, vh, dispW });
  view.current = { vx0, vy0, vw, vh, dispW };

  const draw = useCallback(async (tt: number, play = false) => {
    const my = ++seq.current;
    inflight.current++;
    try {
      const v = view.current;
      const need = (v.dispW / v.vw) * (window.devicePixelRatio || 1);
      const s = [0.5, 0.6667, 1].find((x) => x >= need - 0.02) ?? 1;
      // 字幕は窓の外（画面の下）なので、窓の切り出しには描かない
      const r = await fetch(akf("/frame", { t: tt.toFixed(4), s, fx: showFx ? 1 : 0, play: play ? 1 : 0, subs: 0 }));
      if (!r.ok) {
        setErr(await r.text());
        return;
      }
      const bmp = await createImageBitmap(await r.blob());
      if (!play && my < seq.current) return;
      const c = canvas.current;
      if (!c) return;
      const sc = bmp.width / 1920;
      const cw = Math.max(2, Math.round(v.vw * sc));
      const ch = Math.max(2, Math.round(v.vh * sc));
      if (c.width !== cw || c.height !== ch) {
        c.width = cw;
        c.height = ch;
      }
      c.getContext("2d")!.drawImage(bmp, v.vx0 * sc, v.vy0 * sc, v.vw * sc, v.vh * sc, 0, 0, cw, ch);
      setErr("");
    } catch (e) {
      setErr(errorMessage(e));
    } finally {
      inflight.current--;
    }
  }, [showFx]);

  useEffect(() => {
    if (!playing) return;
    let raf = 0;
    const loop = () => {
      const f = Math.floor(player.now() * fps);
      if (f !== shown.current && inflight.current < 2) {
        shown.current = f;
        draw((f + 1) / fps, true);
      }
      raf = requestAnimationFrame(loop);
    };
    raf = requestAnimationFrame(loop);
    return () => cancelAnimationFrame(raf);
  }, [playing, draw, fps]);

  useEffect(() => {
    if (playing) return;
    const id = setTimeout(() => draw(t), 16);
    return () => clearTimeout(id);
  }, [t, playing, overrides, doc, draw, engineGen, vx0, vy0, vw, vh, dispW]);

  // 今見えているクリップの枠（キャンバスの座標 → 画面）
  const rectOf = (c: MediaClip, d = { dx: 0, dy: 0, s: 1 }) => {
    const info = mediaInfoNow(c.src);
    if (!info) return null;
    const cx = m.ox + (c.x + d.dx) * m.sx;
    const cy = m.oy + (c.y + d.dy) * m.sy;
    const w = info.w * c.scale * d.s * m.sx;
    const h = info.h * c.scale * d.s * m.sy;
    // 枠はクロップ後の範囲（本体の Monitor と同じ）。cx, cy は素材の中心（拡大の基準）
    const [cl, ct, cr, cb] = [c.crop_l ?? 0, c.crop_t ?? 0, c.crop_r ?? 0, c.crop_b ?? 0].map((v) => v / 100);
    return { x: cx - w / 2 + w * cl, y: cy - h / 2 + h * ct, w: w * (1 - cl - cr), h: h * (1 - ct - cb), cx, cy };
  };
  const visible: { tr: Track; c: MediaClip }[] = [];
  for (const tr of nest.tracks) {
    if (tr.hidden) continue;
    for (const c of tr.clips) if (t >= c.start && t < c.start + c.dur) visible.push({ tr, c });
  }
  // 大きさが分かったら描き直す
  useEffect(() => {
    let dead = false;
    Promise.all(visible.map((v) => mediaInfo(v.c.src))).then(() => { if (!dead) setInfoTick((n) => n + 1); });
    return () => { dead = true; };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [visible.map((v) => v.c.src).join("|")]);

  const pick = (e: React.MouseEvent) => {
    const r = e.currentTarget.getBoundingClientRect();
    const x = vx0 + (e.clientX - r.left) / k;
    const y = vy0 + (e.clientY - r.top) / k;
    for (const v of [...visible].reverse()) {
      if (!selectable(v.tr)) continue; // ロックしたトラックは画面のクリックで選ばない（issue 旧#11）
      const rc = rectOf(v.c);
      if (rc && x >= rc.x && x <= rc.x + rc.w && y >= rc.y && y <= rc.y + rc.h) {
        setSel(v.c.id);
        return;
      }
    }
    setSel(null);
  };

  const startDrag = (e: React.PointerEvent, c: MediaClip, mode: "move" | "scale") => {
    if (playing) return;
    e.preventDefault();
    // preventDefault でフォーカスが移らないので、調整欄に残ったフォーカスを外す（Ctrl+Z がすぐ効くように）
    releaseFocus();
    e.stopPropagation();
    const el = e.currentTarget as HTMLElement;
    el.setPointerCapture(e.pointerId);
    const rc = rectOf(c);
    if (!rc) return;
    const x0 = e.clientX;
    const y0 = e.clientY;
    const boxEl = el.closest(".nest-canvas-box") as HTMLElement;
    const rect = boxEl.getBoundingClientRect();
    const world = (px: number, py: number) => ({ x: vx0 + (px - rect.left) / k, y: vy0 + (py - rect.top) / k });
    const w0 = world(x0, y0);
    const r0 = Math.max(1, Math.hypot(w0.x - rc.cx, w0.y - rc.cy));
    let cur = { id: c.id, dx: 0, dy: 0, s: 1 };
    setGhost(cur);
    const move = (ev: PointerEvent) => {
      if (mode === "move") {
        let dx = (ev.clientX - x0) / k / m.sx;
        let dy = (ev.clientY - y0) / k / m.sy;
        if (ev.shiftKey) {
          if (Math.abs(dx) > Math.abs(dy)) dy = 0;
          else dx = 0;
        }
        // キャンバスの中心にくっつく（Alt で外す）
        const snapPx = 8 / k / m.sx;
        if (!ev.altKey) {
          if (Math.abs(c.x + dx - nest.w / 2) < snapPx) dx = nest.w / 2 - c.x;
          if (Math.abs(c.y + dy - nest.h / 2) < snapPx) dy = nest.h / 2 - c.y;
        }
        cur = { id: c.id, dx: Math.round(dx), dy: Math.round(dy), s: 1 };
      } else {
        const w = world(ev.clientX, ev.clientY);
        cur = { id: c.id, dx: 0, dy: 0, s: clamp(Math.hypot(w.x - rc.cx, w.y - rc.cy) / r0, 0.05, 20) };
      }
      setGhost({ ...cur });
    };
    const up = () => {
      el.removeEventListener("pointermove", move);
      el.removeEventListener("pointerup", up);
      setGhost(null);
      const key = useS.getState().nestKey;
      if (!key) return;
      if (mode === "move" && (cur.dx || cur.dy)) {
        editNest(key, (n) => { const f = findClip(n, c.id); if (f) { f.c.x = Math.round(f.c.x + cur.dx); f.c.y = Math.round(f.c.y + cur.dy); } });
      } else if (mode === "scale" && Math.abs(cur.s - 1) > 1e-4) {
        editNest(key, (n) => { const f = findClip(n, c.id); if (f) f.c.scale = Math.round(f.c.scale * cur.s * 100000) / 100000; });
      }
    };
    el.addEventListener("pointermove", move);
    el.addEventListener("pointerup", up);
  };

  const selC = sel ? visible.find((v) => v.c.id === sel)?.c : undefined;
  const fitLabel = nest.fit === "stretch" ? `引き伸ばし ${(m.sx * 100).toFixed(1)}% × ${(m.sy * 100).toFixed(1)}%` : `${FIT_LABEL[nest.fit]} ${(m.sx * 100).toFixed(3)}%`;

  return (
    <div className="nest-stage" ref={stage}>
      <div className="nest-canvas-box" style={{ width: dispW, height: dispH }} onMouseDown={(e) => { if (e.target === canvas.current) pick(e); }}>
        <canvas ref={canvas} width={10} height={10} />
        {/* キャンバス（素材を置く台）の外枠 */}
        <div className="nest-frame canvas" style={{ left: X(cr[0]), top: Y(cr[1]), width: (cr[2] - cr[0]) * k, height: (cr[3] - cr[1]) * k }}>
          <span>キャンバス {nest.w}×{nest.h}・{fitLabel}</span>
        </div>
        {/* 窓（RecWindow の box）。この外は映らない */}
        <div className="nest-frame win" style={{ left: X(box[0]), top: Y(box[1]), width: (box[2] - box[0]) * k, height: (box[3] - box[1]) * k }}>
          <span>窓 {Math.round(box[2] - box[0])}×{Math.round(box[3] - box[1])}</span>
        </div>
        {visible.filter((v) => v.c.id !== sel).map((v) => {
          const rc = rectOf(v.c);
          return rc && <div key={v.c.id} className="nest-clipbox" style={{ left: X(rc.x), top: Y(rc.y), width: rc.w * k, height: rc.h * k }} />;
        })}
        {selC && (() => {
          const d = ghost?.id === selC.id ? ghost : undefined;
          const rc = rectOf(selC, d);
          if (!rc) return null;
          return (
            <div className="sel-box" style={{ left: X(rc.x), top: Y(rc.y), width: rc.w * k, height: rc.h * k }}
              onPointerDown={(e) => startDrag(e, selC, "move")}
              title="ドラッグで移動、右下の角で拡大・縮小（Shift: 縦か横だけ動かす / Alt: 中心にスナップしない）">
              <div className="label">
                {d ? (d.s !== 1 ? `×${d.s.toFixed(3)}` : `${d.dx >= 0 ? "+" : ""}${d.dx}, ${d.dy >= 0 ? "+" : ""}${d.dy}`) : `${selC.name}  (${Math.round(selC.x)}, ${Math.round(selC.y)}) ${(selC.scale * 100).toFixed(1)}%`}
              </div>
              <div className="handle" style={{ right: -6, bottom: -6, cursor: "nwse-resize" }} onPointerDown={(e) => startDrag(e, selC, "scale")} />
            </div>
          );
        })()}
        {!visible.length && !playing && (
          <div className="nest-hint">再生位置 {tc(t, fps)} に素材がありません。下のタイムラインに録画をドロップするか、「＋ 素材」で追加してください</div>
        )}
        {err && <div className="nest-err">{err}</div>}
      </div>
    </div>
  );
}

// ====================================================================== タイムライン（窓が出ている区間だけ）
type TlProps = { g: Group; nest: Nest; o?: Override; nkey: string; sel: string | null; setSel: (id: string | null) => void; onAdd: () => void };

function NestTimeline({ g, nest, o, nkey, sel, setSel, onAdd }: TlProps) {
  const t = useS((s) => s.t);
  const wrap = useRef<HTMLDivElement>(null);
  const [W, setW] = useState(900);
  const [zm, setZm] = useState(1);
  const zmRef = useRef(1);
  const view = useRef({ ra: 0, span: 1, base: 200, zoom: 1, zmMin: 1, zmMax: 1 });
  const keep = useRef<{ t: number; px: number } | null>(null); // ズーム後に画面の同じ所へ置く時刻と位置
  const leftT = useRef<number | null>(null); // 画面の左端の時刻（範囲や幅が変わっても同じ所を映し続ける）
  const [snapOn, setSnapOn] = useState(true);
  const [prev, setPrev] = useState<{ id: string; start: number; dur: number; src_in: number; track: string } | null>(null);
  const prevRef = useRef(prev);
  prevRef.current = prev;

  useLayoutEffect(() => {
    const el = wrap.current!;
    const ro = new ResizeObserver(() => setW(el.clientWidth));
    ro.observe(el);
    return () => ro.disconnect();
  }, []);

  // 録画の素材の長さ（素材全体の影と、縮めたときの範囲に使う。画像・ループは無し）
  const [srcDur, setSrcDur] = useState<Record<string, number>>({});
  const srcs = [...new Set(nest.tracks.flatMap((tr) => tr.clips).filter((c) => !isImg(c.src) && !c.loop).map((c) => c.src))].join("\n");
  useEffect(() => {
    let dead = false;
    for (const src of srcs ? srcs.split("\n") : []) {
      mediaInfo(src).then((i) => { if (!dead && i && !i.image && i.dur > 0) setSrcDur((m) => (m[src] === i.dur ? m : { ...m, [src]: i.dur })); });
    }
    return () => { dead = true; };
  }, [srcs]);
  const durOf = (c: MediaClip) => (c.loop || isImg(c.src) ? 0 : srcDur[c.src] ?? 0);

  const [a, b] = windowSpan(g, o);
  // 倍率 1 で画面に収まるのは窓の区間 ±1 秒。窓の外にはみ出したクリップがあれば、その端まで（掴めるように）
  let ra0 = Math.max(0, a - 1);
  let rb0 = b + 1;
  // スクロールできる範囲は、録画の素材全体まで（縮めて 10 分の録画のどこを使うか選べるように）
  let fa = ra0;
  let fb = rb0;
  for (const tr of nest.tracks) {
    for (const c of tr.clips) {
      ra0 = Math.min(ra0, Math.max(0, c.start));
      rb0 = Math.max(rb0, c.start + c.dur);
      const d = durOf(c);
      if (d > 0) {
        const [ga, gb] = sourceSpan(c, d);
        fa = Math.min(fa, ga);
        fb = Math.max(fb, gb);
      }
    }
  }
  fa = Math.min(fa, ra0);
  fb = Math.max(fb, rb0);
  const base = Math.max(200, W - HEAD - 24);
  // 倍率の上限は 1 秒あたり 600px（1 フレーム 20px）になる値。下限は素材全体が収まる値
  const zmMax = Math.max(1, (MAX_PX_PER_SEC * (rb0 - ra0)) / base);
  const [ra, rb] = [fa, fb];
  const zmMin = Math.min(1, (rb0 - ra0) / (fb - fa));
  const zoom = (base * clamp(zm, zmMin, zmMax)) / (rb0 - ra0);
  view.current = { ra, span: rb - ra, base, zoom, zmMin, zmMax };

  // ズーム。anchorX（画面の x）の時刻が同じ所に残るように scrollLeft を合わせる（本体の Timeline と同じ）。省くと再生位置
  const zoomTo = useCallback((nzm: number, anchorX?: number) => {
    const sc = wrap.current;
    const v = view.current;
    if (!sc) return;
    const r = sc.getBoundingClientRect();
    const px = anchorX !== undefined ? anchorX - r.left : HEAD + (useS.getState().t - v.ra) * v.zoom - sc.scrollLeft;
    keep.current = { t: v.ra + (px + sc.scrollLeft - HEAD) / v.zoom, px };
    zmRef.current = clamp(nzm, v.zmMin, v.zmMax);
    setZm(zmRef.current);
  }, []);
  useLayoutEffect(() => {
    const k = keep.current;
    const sc = wrap.current;
    keep.current = null;
    if (!sc) return;
    // ズームはマウスの位置を、それ以外（開いたとき・素材の長さが分かって範囲が広がったとき）は左端の時刻を保つ。最初は窓の区間から
    if (k) sc.scrollLeft = Math.max(0, HEAD + (k.t - ra) * zoom - k.px);
    else sc.scrollLeft = Math.max(0, ((leftT.current ?? ra0) - ra) * zoom);
    leftT.current = ra + sc.scrollLeft / zoom;
  }, [zoom, ra]);

  // ホイール: Alt でズーム（Ctrl も同じ。ピンチ用）、ふつうは横スクロール（Shift で縦）。割り当ては本体と共通の wheelAct。
  // 本体のタイムラインと同じく passive でない native のリスナーで
  useEffect(() => {
    const sc = wrap.current;
    if (!sc) return;
    const onWheel = (e: WheelEvent) => {
      e.preventDefault();
      const a = wheelAct(e, { w: sc.clientWidth, h: sc.clientHeight });
      if (a.kind === "zoom") zoomTo(zmRef.current * (a.zoomIn ? 1.2 : 1 / 1.2), e.clientX);
      else if (a.kind === "v") sc.scrollTop += a.d;
      else sc.scrollLeft += a.d;
    };
    sc.addEventListener("wheel", onWheel, { passive: false });
    return () => sc.removeEventListener("wheel", onWheel);
  }, [zoomTo]);

  const X = (tt: number) => (tt - ra) * zoom;
  const inner = HEAD + (rb - ra) * zoom + 20;
  const shift = o?.shift || 0;
  const segs = (g.rec ?? []).flatMap((r) => r.segs).map((s) => ({ t: s[0] + shift, id: s[1], fn: s[2], desc: s[3] }));
  const tracks = [...nest.tracks].reverse(); // 上のトラックを上に

  const snapList = (except: string) => {
    const out = [a, b, useS.getState().t, ...segs.map((s) => s.t)];
    for (const tr of nest.tracks) for (const c of tr.clips) if (c.id !== except) out.push(c.start, c.start + c.dur);
    return out;
  };
  const snap = (v: number, on: boolean, list: number[]) => {
    if (!on) return v;
    let best = v;
    let bd = 8 / zoom;
    for (const x of list) {
      const d = Math.abs(x - v);
      if (d < bd) { bd = d; best = x; }
    }
    return best;
  };

  const seekAt = (clientX: number, el: HTMLElement, alt = false) => {
    const r = el.getBoundingClientRect();
    const tt = clamp(ra + (clientX - r.left) / zoom, Math.max(0, ra), rb);
    player.seek(snap(tt, snapOn && !alt, [a, b, ...segs.map((s) => s.t)]));
  };

  const dragClip = (e: React.PointerEvent, tr: Track, c: MediaClip, mode0: "move" | "l" | "r") => {
    if (e.button !== 0) return;
    e.stopPropagation();
    setSel(c.id);
    const x0 = e.clientX;
    let moved = false;
    const list = snapList(c.id);
    const sd = durOf(c) || (() => {
      const info = mediaInfoNow(c.src);
      return info && !info.image && info.dur > 0 && !c.loop ? info.dur : 0;
    })();
    // Ctrl を押して本体を掴むと、位置と長さはそのままで中の素材だけずらす（スリップ。Premiere と同じ）
    const mode = mode0 === "move" && (e.ctrlKey || e.metaKey) && !isImg(c.src) ? "slip" : mode0;
    const move = (ev: PointerEvent) => {
      if (Math.abs(ev.clientX - x0) > 3) moved = true;
      if (!moved) return;
      const dt = (ev.clientX - x0) / zoom;
      const sn = snapOn && !ev.altKey;
      if (mode === "slip") {
        setPrev({ id: c.id, start: c.start, dur: c.dur, src_in: slipSrcIn(c, dt, sd), track: tr.id });
      } else if (mode === "move") {
        // 先頭と終わりのどちらかがくっつく所へ
        let ns = snap(c.start + dt, sn, list);
        if (ns === c.start + dt) ns = snap(c.start + c.dur + dt, sn, list) - c.dur;
        ns = Math.max(0, ns);
        const under = document.elementsFromPoint(ev.clientX, ev.clientY).find((x) => (x as HTMLElement).dataset?.ntrack) as HTMLElement | undefined;
        setPrev({ id: c.id, start: ns, dur: c.dur, src_in: c.src_in, track: under?.dataset.ntrack ?? tr.id });
      } else if (mode === "r") {
        const end = Math.min(snap(c.start + c.dur + dt, sn, list), sd ? maxEnd(c, sd) : Infinity);
        setPrev({ id: c.id, start: c.start, dur: Math.max(0.1, end - c.start), src_in: c.src_in, track: tr.id });
      } else {
        const img = isImg(c.src);
        const ns = clamp(snap(c.start + dt, sn, list), minStart(c, img), c.start + c.dur - 0.1);
        setPrev({ id: c.id, start: ns, dur: c.dur - (ns - c.start), src_in: img ? 0 : trimSrcIn(c, ns), track: tr.id });
      }
    };
    // 別のトラックへ動かすとクリップが描き直されて掴んだ要素が DOM から外れ、要素に付けたリスナーだと pointerup が届かず
    // 移動が確定しなかった（issue 旧#70）。window で受ける
    const end = () => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
      window.removeEventListener("pointercancel", cancel);
    };
    const up = () => {
      end();
      const p = prevRef.current;
      setPrev(null);
      if (!moved || !p) {
        const tt = useS.getState().t;
        if (tt < c.start || tt >= c.start + c.dur) player.seek(c.start);
        return;
      }
      editNest(nkey, (n) => { commitDrag(n, p); });
    };
    const cancel = () => {
      end();
      setPrev(null);
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
    window.addEventListener("pointercancel", cancel);
  };

  // 目盛り（大きい目盛りは 1 フレーム・0.1 秒から。間の小さい目盛りは、0.1 秒以下では 1 フレームごと）
  const fps = useS.getState().desc?.fps ?? 30;
  const major = [1 / fps, 0.1, 0.5, 1, 2, 5, 10, 30, 60, 120, 300, 600].find((s) => s * zoom >= 56) ?? 600;
  const minor = major <= 0.1 + 1e-9 ? 1 / fps : major / 5;
  const ticks: React.ReactNode[] = [];
  const isMajor = (s: number) => Math.abs(s / major - Math.round(s / major)) < 1e-6;
  const minorN = Math.floor(rb / minor) - Math.ceil(ra / minor - 1e-9);
  // 縮めて 10 分を映すと、0.1 秒の目盛りが数千本になるので、多すぎるときは小さい目盛りを描かない
  const step = minorN > 3000 ? Math.round(major / minor) : 1;
  for (let i = Math.ceil(ra / minor / step - 1e-9) * step; i * minor <= rb; i += step) {
    const s = i * minor;
    const mj = isMajor(s);
    // 素材全体の影は 0 秒より前にもはみ出すので、負の時刻も目盛りに出す
    ticks.push(<div key={`m${i}`} className={`tick ${mj ? "" : "minor"}`} style={{ left: X(s) }}>{mj ? (s < -1e-9 ? "−" + tc(-s, fps) : tc(Math.abs(s), fps)) : ""}</div>);
  }

  return (
    <div className="nest-tl">
      <div className="tl-tools">
        <span className="tag">// NEST TIMELINE</span>
        <span className="dim2 ui" style={{ fontSize: 11, minWidth: 0, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}
          title="映るのは窓が出ている区間（水色）だけです。黄色の線は PLAYBACK の切り替わり、選んだ録画の点線の帯は素材全体です。Ctrl+ドラッグで、位置と長さはそのままで中の素材だけずらせます">
          映るのは水色の区間だけ。黄色は PLAYBACK の切り替わり、点線は素材全体（Ctrl+ドラッグで中身をずらす）
        </span>
        <span className="grow" />
        <button className={snapOn ? "on" : ""} onClick={() => setSnapOn(!snapOn)} title="PLAYBACK の切り替わり・窓の開始と終了・再生位置・ほかのクリップの端にスナップします（Alt で一時的に無効）">スナップ</button>
        <span className="dim" style={{ fontSize: 12, whiteSpace: "nowrap" }}>ズーム</span>
        <input type="range" min={Math.log(zmMin)} max={Math.log(zmMax)} step="any" value={Math.log(clamp(zm, zmMin, zmMax))}
          onChange={(e) => zoomTo(Math.exp(Number(e.target.value)))} style={{ width: 110 }}
          title="Alt+ホイールでも。左いっぱいで、録画の素材全体が見えるまで縮めます" />
        <button onClick={() => editNest(nkey, (n) => { n.tracks.push(newTrack(n.tracks.length + 1)); })} title="一番上にトラックを追加">＋トラック</button>
        <button onClick={onAdd}>＋ 素材</button>
      </div>
      <div className="tl-scroll" ref={wrap} onScroll={(e) => { const v = view.current; leftT.current = v.ra + e.currentTarget.scrollLeft / v.zoom; }}>
        <div className="tl-inner" style={{ width: inner, position: "relative" }}>
          <div style={{ display: "flex", height: RULER, position: "sticky", top: 0, zIndex: 7 }}>
            <div className="tl-head" style={{ width: HEAD, height: RULER, position: "sticky", left: 0, zIndex: 8, background: "var(--panel)" }}>
              <span className="tc" style={{ fontSize: 13, minWidth: 0 }}>{tc(t)}</span>
            </div>
            <div className="ruler" style={{ position: "relative", flex: "none", width: inner - HEAD, height: RULER }}
              onPointerDown={(e) => {
                const el = e.currentTarget;
                el.setPointerCapture(e.pointerId);
                seekAt(e.clientX, el, e.altKey);
                const mv = (ev: PointerEvent) => seekAt(ev.clientX, el, ev.altKey);
                const up = () => { el.removeEventListener("pointermove", mv); el.removeEventListener("pointerup", up); };
                el.addEventListener("pointermove", mv);
                el.addEventListener("pointerup", up);
              }}>
              <div className="nest-span" style={{ left: X(a), width: (b - a) * zoom }} />
              {ticks}
              {segs.map((s) => (
                <div key={`${s.t}-${s.id}`} className="nest-seg" style={{ left: X(s.t) }} title={`PLAYBACK ${s.id}\n${s.fn}\n${s.desc}`}>
                  PLAYBACK {s.id} <span className="dim2">{s.fn}</span>
                </div>
              ))}
            </div>
          </div>
          {tracks.map((tr) => {
            const n = nest.tracks.length;
            return (
              <div key={tr.id} className={tr.locked ? "locked" : ""} style={{ display: "flex", height: ROW, borderBottom: "1px solid var(--n-18)" }}>
                <div className="tl-head" style={{ width: HEAD, height: ROW, position: "sticky", left: 0, zIndex: 5, background: "var(--panel)" }}>
                  <span className="k">V</span>
                  <input className="nm" value={tr.name} onChange={(e) => editNest(nkey, (nn) => { nn.tracks.find((x) => x.id === tr.id)!.name = e.target.value; }, false)}
                    style={{ background: "transparent", border: 0, padding: 0, color: "inherit", width: 36 }} />
                  <button className={tr.hidden ? "" : "on"} title="表示" onClick={() => editNest(nkey, (nn) => { const x = nn.tracks.find((y) => y.id === tr.id)!; x.hidden = !x.hidden; })}>👁</button>
                  <button className={tr.locked ? "on" : ""} title={LOCK_HELP} onClick={() => editNest(nkey, (nn) => { const x = nn.tracks.find((y) => y.id === tr.id)!; x.locked = !x.locked; })}>{tr.locked ? "🔒" : "🔓"}</button>
                  <button title="トラックを削除" disabled={n <= 1} onClick={() => {
                    if (tr.clips.length && !confirm(`トラック「${tr.name}」とその素材 ${tr.clips.length} 個を削除しますか？`)) return;
                    editNest(nkey, (nn) => { nn.tracks = nn.tracks.filter((x) => x.id !== tr.id); });
                  }}>✕</button>
                </div>
                <div className={`nest-row ${prev && prev.track === tr.id ? "drop" : ""}`} style={{ position: "relative", flex: "none", width: inner - HEAD, height: ROW }}
                  data-ntrack={tr.id} data-nt0={ra} data-nzoom={zoom}
                  onPointerDown={(e) => {
                    if (e.target === e.currentTarget) {
                      setSel(null);
                      seekAt(e.clientX, e.currentTarget, e.altKey);
                    }
                  }}>
                  <div className="nest-out" style={{ left: 0, width: Math.max(0, X(a)) }} />
                  <div className="nest-out" style={{ left: X(b), right: 0 }} />
                  {segs.map((s) => <div key={`${s.t}-${s.id}`} className="nest-seg-line" style={{ left: X(s.t) }} />)}
                  {(() => {
                    // 選んだ録画（ドラッグ中ならその値）の素材全体の影。クリップ本体は、そのうち使っている所
                    const f = sel ? findClip(nest, sel) : null;
                    const d = f ? durOf(f.c) : 0;
                    if (!f || !d) return null;
                    const p = prev?.id === f.c.id ? prev : null;
                    if ((p ? p.track : f.tr.id) !== tr.id) return null;
                    const [ga, gb] = sourceSpan({ ...f.c, ...(p ?? {}) }, d);
                    return (
                      <div className="nest-ghost" style={{ left: X(ga), width: Math.max(1, (gb - ga) * zoom), top: 3, height: ROW - 6 }}
                        title={`${f.c.name}（素材全体 ${tc(d)}）`}>
                        <span>素材 0:00</span><span>{tc(d)}</span>
                      </div>
                    );
                  })()}
                  {nest.tracks.flatMap((tt) => tt.clips.map((c) => ({ tt, c }))).filter(({ tt, c }) => (prev?.id === c.id ? prev.track === tr.id : tt.id === tr.id)).map(({ tt, c }) => {
                    const p = prev?.id === c.id ? prev : null;
                    const start = p ? p.start : c.start;
                    const dur = p ? p.dur : c.dur;
                    const img = isImg(c.src);
                    return (
                      <div key={c.id} className={`clip ${img ? "img" : "vid"} ${sel === c.id ? "sel" : ""} ${tt.hidden ? "hid" : ""}`}
                        style={{ left: X(start), width: Math.max(4, dur * zoom - 1), top: 3, height: ROW - 6 }}
                        title={`${c.name}\n${tc(start)} – ${tc(start + dur)}（${dur.toFixed(2)}s）\n素材の ${c.src_in.toFixed(2)}s から\nドラッグで移動（上下でトラックを移動）・端で長さを変更・Ctrl+ドラッグで中の素材だけずらす\nCtrl+D 複製・S 分割・Delete 削除`}
                        onPointerDown={(e) => dragClip(e, tt, c, "move")}
                        onDoubleClick={() => player.seek(c.start)}>
                        <div className="edge l" onPointerDown={(e) => dragClip(e, tt, c, "l")} />
                        {dur * zoom > 60 && <img draggable={false} src={akf("/thumb", { p: c.src, t: c.src_in.toFixed(2) })} style={{ height: ROW - 10, verticalAlign: "middle", marginRight: 4 }} />}
                        {c.name}
                        {clipSpeed(c) !== 1 && <span className="spd">×{clipSpeed(c).toFixed(2)}</span>}
                        <div className="edge r" onPointerDown={(e) => dragClip(e, tt, c, "r")} />
                      </div>
                    );
                  })}
                </div>
              </div>
            );
          })}
          <div className="playhead" style={{ left: HEAD + X(t) }} />
        </div>
      </div>
    </div>
  );
}

// ====================================================================== 調整（選んだクリップ・ネストの設定）
type InspProps = { g: Group; nest: Nest; o?: Override; nkey: string; sel: string | null; setSel: (id: string | null) => void };

function NestInspector({ g, nest, o, nkey, sel, setSel }: InspProps) {
  const t = useS((s) => s.t);
  const f = sel ? findClip(nest, sel) : null;
  const [info, setInfo] = useState<{ w: number; h: number; dur: number; image: boolean } | null>(null);
  useEffect(() => {
    setInfo(null);
    if (f) mediaInfo(f.c.src).then(setInfo);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [f?.c.src]);
  const box = windowBox(g, nest, o);
  const m = nestMap(nest, box);
  const [a, b] = windowSpan(g, o);
  const shift = o?.shift || 0;

  if (f) {
    const c = f.c;
    const set = (k: keyof MediaClip) => (v: any) => editNest(nkey, (n) => { const x = findClip(n, c.id); if (x) (x.c as any)[k] = v; });
    const fit = (how: "contain" | "cover" | "width" | "height") => info && editNest(nkey, (n) => {
      const x = findClip(n, c.id);
      if (x) { x.c.scale = fitScale(n, info.w, info.h, how); x.c.x = n.w / 2; x.c.y = n.h / 2; }
    });
    const px = m.ox + c.x * m.sx;
    const py = m.oy + c.y * m.sy;
    return (
      <div className="insp nest-insp">
        <h3>{c.name}</h3>
        <div className="src">{c.src}</div>
        <div className="row" style={{ marginTop: 8, flexWrap: "wrap" }}>
          <button onClick={() => player.seek(c.start)}>先頭へ</button>
          <button onClick={() => call("reveal", { path: c.src })}>エクスプローラーで表示</button>
          <span className="grow" />
          <button className="danger" onClick={() => { deleteClip(nkey, c.id); setSel(null); }}>削除</button>
        </div>
        <div className="section">TIME</div>
        <Num label="開始" value={c.start} def={-1e9} step={0.033} unit="秒" onCommit={(v) => set("start")(r3(v ?? a))} min={0} max={36000} />
        <Num label="長さ" value={c.dur} def={-1e9} step={0.033} unit="秒" onCommit={(v) => set("dur")(r3(v ?? 1))} min={0.05} max={36000} />
        {!isImg(c.src) && <Num label="素材の開始"value={c.src_in} def={0} step={0.033} unit="秒" onCommit={(v) => set("src_in")(r3(v ?? 0))} min={0} max={36000} />}
        {!isImg(c.src) && (
          <Num label="速度" value={Math.round(clipSpeed(c) * 1000) / 10} def={100} step={5} unit="%" min={SPEED_MIN * 100} max={SPEED_MAX * 100}
            onCommit={(v) => editNest(nkey, (n) => { const x = findClip(n, c.id); if (x) setSpeed(x.c, (v ?? 100) / 100); })} />
        )}
        <Num label="フェードイン" value={c.fade_in} def={0} step={0.05} unit="秒" onCommit={(v) => set("fade_in")(v ?? 0)} min={0} max={10} />
        <Num label="フェードアウト" value={c.fade_out} def={0} step={0.05} unit="秒" onCommit={(v) => set("fade_out")(v ?? 0)} min={0} max={10} />
        <TransFields
          c={c}
          edit={(fn) => editNest(nkey, (n) => { const x = findClip(n, c.id); if (x) fn(x.c); })}
          onSwitch={prevClip(f.tr.clips, c) ? (kind, dur) => editNest(nkey, (n) => {
            const x = findClip(n, c.id);
            if (x) linkSwitch(x.tr.clips, c.id, kind, dur);
          }) : undefined}
        />
        <div className="section">MOTION（キャンバス {nest.w}×{nest.h} の座標）</div>
        <Num2 label="位置" unit="px" min={-4000} max={8000}
          a={{ tag: "x", value: c.x, def: nest.w / 2, onCommit: (v) => set("x")(v ?? nest.w / 2) }}
          b={{ tag: "y", value: c.y, def: nest.h / 2, onCommit: (v) => set("y")(v ?? nest.h / 2) }} />
        <Num label="スケール" value={c.scale} def={-1e9} step={0.005} unit="倍" onCommit={(v) => set("scale")(v ?? 1)} min={0.01} max={20} />
        <Num label="不透明度" value={c.opacity} def={-1e9} step={0.01} onCommit={(v) => set("opacity")(v ?? 1)} min={0} max={1} />
        <div className="row" style={{ flexWrap: "wrap", marginTop: 4 }}>
          <button disabled={!info} onClick={() => fit("contain")} title="縦横比を保ってキャンバスに収めます">収める</button>
          <button disabled={!info} onClick={() => fit("cover")} title="縦横比を保ってキャンバスを埋めます">埋める</button>
          <button disabled={!info} onClick={() => fit("width")}>幅に合わせる</button>
          <button disabled={!info} onClick={() => fit("height")}>高さに合わせる</button>
        </div>
        {info && <div className="dim2 ui" style={{ fontSize: 11, marginTop: 6 }}>素材 {info.w}×{info.h}{info.dur ? `・${info.dur.toFixed(2)} 秒` : ""}</div>}
        <CropFields c={c} set={set}
          reset={() => editNest(nkey, (n) => { const x = findClip(n, c.id); if (x) Object.assign(x.c, { crop_l: 0, crop_t: 0, crop_r: 0, crop_b: 0, feather: 0 }); })} />
        <div className="dim2 ui" style={{ fontSize: 11, marginTop: 4, lineHeight: 1.7 }}>
          画面（1920×1080）では: 位置 ({Math.round(px)}, {Math.round(py)})・スケール {nest.fit === "stretch" ? `${(c.scale * m.sx * 100).toFixed(3)}% × ${(c.scale * m.sy * 100).toFixed(3)}%` : `${(c.scale * m.sx * 100).toFixed(3)}%`}
          <br />（Premiere で窓に合わせるときの値です。窓とキャンバスの外は切り取られます）
        </div>
      </div>
    );
  }

  const segs = (g.rec ?? []).flatMap((r) => r.segs);
  const covered = (ta: number, tb: number) => nest.tracks.some((tr) => !tr.hidden && tr.clips.some((c) => c.start < tb - 0.05 && c.start + c.dur > ta + 0.05));
  return (
    <div className="insp nest-insp">
      <h3>ネストの設定</h3>
      <div className="section">CANVAS</div>
      <Num label="幅" value={nest.w} def={-1e9} step={1} unit="px" min={16} max={7680} onCommit={(v) => editNest(nkey, (n) => { n.w = Math.round(v ?? 1920); })} />
      <Num label="高さ" value={nest.h} def={-1e9} step={1} unit="px" min={16} max={4320} onCommit={(v) => editNest(nkey, (n) => { n.h = Math.round(v ?? 1080); })} />
      <div className="row" style={{ flexWrap: "wrap" }}>
        {[[1920, 1080], [1920, 768], [1280, 720], [2560, 1080]].map(([w, h]) => (
          <button key={`${w}x${h}`} className={nest.w === w && nest.h === h ? "on" : ""} onClick={() => editNest(nkey, (n) => { n.w = w; n.h = h; })}>{w}×{h}</button>
        ))}
      </div>
      <div className="dim2" style={{ fontSize: 11, lineHeight: 1.7, marginTop: 6 }}>
        素材はキャンバスの上に置き、キャンバスの外は切り取られます（Premiere のネストと同じ）。キャンバス全体を窓（{Math.round(box[2] - box[0])}×{Math.round(box[3] - box[1])}）に「{FIT_LABEL[nest.fit]}」ではめます
        （{nest.fit === "stretch" ? `横 ${(m.sx * 100).toFixed(3)}%・縦 ${(m.sy * 100).toFixed(3)}%` : `${(m.sx * 100).toFixed(3)}%`}）。
      </div>
      <div className="section">PLAYBACK</div>
      {segs.map((s, i) => {
        const ta = s[0] + shift;
        const tb = i + 1 < segs.length ? segs[i + 1][0] + shift : b;
        const ok = covered(ta, tb);
        return (
          <div key={i} className={`nest-segrow ${t >= ta && t < tb ? "cur" : ""}`} onClick={() => player.seek(ta + 0.05)}>
            <div className="row">
              <span className="chip">PLAYBACK {s[1]}</span>
              <span className="ui dim" style={{ fontSize: 11 }}>{tc(ta)} – {tc(tb)}</span>
              <span className="grow" />
              <span className={ok ? "ok" : "warn"}>{ok ? "素材あり" : "素材なし"}</span>
            </div>
            <div className="mono" style={{ fontSize: 11 }}>{s[2]}</div>
            <div className="dim" style={{ fontSize: 11 }}>{s[3]}</div>
          </div>
        );
      })}
      <div className="section">HOW TO</div>
      <div className="dim" style={{ fontSize: 12, lineHeight: 1.9 }}>
        録画は下のタイムラインにドロップするか、「＋ 素材」で追加します。1920×1080 の録画はキャンバスいっぱいに置かれ、窓では 66.667% になります（Premiere の 位置 (1240, 460)・スケール 66.667% と同じ）。<br />
        モニターの枠はドラッグで移動、右下の角で拡大します。クリップは端をドラッグで長さ、上下にドラッグでトラックを移動します。<br />
        <span className="kbd">S</span>/<span className="kbd">Ctrl+K</span> 分割　<span className="kbd">Ctrl+D</span> 複製　<span className="kbd">Delete</span> 削除　<span className="kbd">Ctrl+Z</span> 元に戻す　<span className="kbd">Esc</span> 閉じる<br />
        素材がある区間では、窓の仮表示（ファイル名・説明）は自動で非表示になります。録画トラックだけの mp4 は「書き出し → 録画トラック mp4」で書き出せます。
      </div>
      <div className="dim2" style={{ fontSize: 11, marginTop: 8 }}>{a.toFixed(2)}〜{b.toFixed(2)} 秒・{nest.tracks.reduce((n, tr) => n + tr.clips.length, 0)} クリップ</div>
    </div>
  );
}
