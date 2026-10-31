import React, { useCallback, useEffect, useLayoutEffect, useRef, useState } from "react";
import { akf, call } from "../io/api";
import { editDoc, fpsOf, patchOverride, removeMark, sceneAt, selectMark, sendChat, setMarkMode, setMarkShow, setMarkState, total, upsertMark, useS } from "../state";
import { errorMessage } from "../logic/errors.ts";
import { player } from "../io/player";
import { clamp, isTextEntry, premiereTc, releaseFocus, tc } from "../logic/util";
import type { MediaClip } from "../types";
import { fxSelectable, pickMedia } from "../logic/pick";
import { groupSpan, windowBoxOf } from "../logic/nest";
import { isAud } from "../logic/media";
import { parseFrameInfo, shortError } from "../logic/frameinfo";
import {
  MARK_COLORS, MARK_TOOLS, badgePos, circled, defaultSpan, drawMarks, hitMark, markMeaning, newMark, shapeFromDrag, visibleMarks,
  type P,
} from "../logic/marks";
import type { Mark } from "../gen/Mark";

/** /frame の X-Info で届く、今の字幕の行の位置（1920 の座標）とはみ出し */
type SubInfo = {
  line: string;
  n: number;
  lines: [number, number, number, number][];
  overflow: boolean[];
  l: number;
  r: number;
  manual: boolean;
  custom: boolean;
};

type Box = { x: number; y: number; w: number; h: number }; // 1920x1080 の座標

const mediaSize = new Map<string, { w: number; h: number }>();
export async function sizeOf(src: string) {
  if (mediaSize.has(src)) return mediaSize.get(src)!;
  const r = await call("probe_media", { path: src });
  const v = { w: r.w || 1920, h: r.h || 1080 };
  mediaSize.set(src, v);
  return v;
}

/** 描いた直後の、その場の一言の入力欄（Enter で決定・Esc は一言なし・外をクリックで決定） */
function MarkNoteInput({ mark, left, top }: { mark: Mark; left: number; top: number }) {
  const done = useRef(false);
  const close = () => useS.setState({ markNoteFor: null });
  const commit = async (text: string) => {
    if (done.current) return;
    done.current = true;
    close();
    const v = text.trim();
    if (!v) return;
    const cur = useS.getState().marks.marks.find((x) => x.id === mark.id) ?? mark;
    await upsertMark({ ...cur, note: v });
  };
  return (
    <input
      className="mark-note"
      style={{ left, top }}
      autoFocus
      placeholder="一言（Enter で決定・Esc でなし）"
      onMouseDown={(e) => e.stopPropagation()}
      onKeyDown={(e) => {
        e.stopPropagation();
        if (e.nativeEvent.isComposing || e.keyCode === 229) return;
        if (e.key === "Enter") {
          e.preventDefault();
          commit(e.currentTarget.value);
        } else if (e.key === "Escape") {
          e.preventDefault();
          done.current = true;
          close();
        }
      }}
      onBlur={(e) => commit(e.currentTarget.value)}
    />
  );
}

const STATE_LABEL: Record<string, string> = { draft: "未送信", sent: "送信済み", done: "済" };

/** 描くモードの道具の列（道具・色・済も表示）と、選んでいる印の直し */
function MarkBar() {
  const tool = useS((s) => s.markTool);
  const color = useS((s) => s.markColor);
  const showDone = useS((s) => s.markShowDone);
  const sel = useS((s) => s.marks.marks.find((m) => m.id === s.markSel) ?? null);
  const dot = (cur: string, pick: (k: string) => void) =>
    MARK_COLORS.map((c) => (
      <button
        key={c.key}
        className={`mark-dot ${cur === c.key ? "on" : ""}`}
        style={{ background: c.hex }}
        title={`${c.name}＝${c.meaning}`}
        aria-label={`${c.name}＝${c.meaning}`}
        onClick={() => pick(c.key)}
      />
    ));
  return (
    <div className="mark-bar">
      <div className="row">
        {MARK_TOOLS.map((x) => (
          <button key={x.key} className={tool === x.key ? "on" : ""} title={x.help} onClick={() => useS.setState({ markTool: x.key })}>{x.label}</button>
        ))}
        <span className="mark-sep" />
        {dot(color, (k) => useS.setState({ markColor: k }))}
        <span className="dim2" style={{ fontSize: 11 }}>{markMeaning(color)}</span>
        <span className="mark-sep" />
        <label className="row dim" style={{ fontSize: 12, gap: 4 }} title="済にした印もモニターとタイムラインに薄く出します">
          <input type="checkbox" checked={showDone} onChange={(e) => useS.setState({ markShowDone: e.target.checked })} />済も表示
        </label>
        <span className="grow" />
        <span className="dim2" style={{ fontSize: 11 }}>Esc で終わります</span>
      </div>
      {sel && (
        <div className="row">
          <b className="mark-no" style={{ color: MARK_COLORS.find((c) => c.key === sel.color)?.hex }}>印{circled(sel.id)}</b>
          <span className="dim2" style={{ fontSize: 11 }}>{sel.by === "claude" ? "Claude の印・" : ""}{STATE_LABEL[sel.state] ?? sel.state}</span>
          <input
            key={`${sel.id}:${sel.note}`}
            className="mark-edit-note"
            defaultValue={sel.note}
            placeholder="一言"
            onKeyDown={(e) => {
              e.stopPropagation();
              if (e.key === "Enter" && !e.nativeEvent.isComposing && e.keyCode !== 229) e.currentTarget.blur();
              if (e.key === "Escape") {
                e.currentTarget.value = sel.note;
                e.currentTarget.blur();
              }
            }}
            onBlur={(e) => {
              if (e.currentTarget.value !== sel.note) upsertMark({ ...sel, note: e.currentTarget.value });
            }}
          />
          {dot(sel.color, (k) => k !== sel.color && upsertMark({ ...sel, color: k }))}
          {sel.state === "done" ? (
            <button onClick={() => setMarkState(sel.id, sel.msg != null || sel.by === "claude" ? "sent" : "draft")} title="済を取り消して、元の状態（送信済み・下書き）に戻します">戻す</button>
          ) : (
            <button onClick={() => setMarkState(sel.id, "done")} title="直し終わった印として薄く表示します">済にする</button>
          )}
          <button className="ghost" onClick={() => removeMark(sel.id)} title="この印を削除します（Delete）。番号は詰めません">削除</button>
        </div>
      )}
    </div>
  );
}

export default function Monitor() {
  const t = useS((s) => s.t);
  const ts = useS((s) => s.subView?.track) ?? null; // 字幕の目安線の既定（共通の見た目。Rust が決める）
  const playing = useS((s) => s.playing);
  const scale = useS((s) => s.scale);
  const desc = useS((s) => s.desc);
  const playFps = useS(fpsOf);
  const overrides = useS((s) => s.overrides);
  const doc = useS((s) => s.project?.doc);
  const selection = useS((s) => s.selection);
  const inT = useS((s) => s.inT);
  const outT = useS((s) => s.outT);
  const engineGen = useS((s) => s.engine?.status.gen);
  const backRev = useS((s) => s.backRev);
  const compare = useS((s) => s.compare);
  const marks = useS((s) => s.marks.marks);
  const markMode = useS((s) => s.markMode);
  const markShow = useS((s) => s.markShow);
  const markShowDone = useS((s) => s.markShowDone);
  const markSel = useS((s) => s.markSel);
  const markNoteFor = useS((s) => s.markNoteFor);
  const markTool = useS((s) => s.markTool);
  const markColor = useS((s) => s.markColor);
  const layer = useRef<HTMLCanvasElement>(null);
  // 描いている途中の点（1920 の座標）
  const [stroke, setStroke] = useState<P[] | null>(null);
  useS((s) => s.scenes); // シーン番号の表示を更新するため
  const [guides, setGuides] = useState(() => localStorage.getItem("guides") !== "0");
  const [showSubs, setShowSubs] = useState(true);
  const [loop, setLoop] = useState(false);
  const stage = useRef<HTMLDivElement>(null);
  const canvas = useRef<HTMLCanvasElement>(null);
  const [disp, setDisp] = useState({ w: 960, h: 540 });
  const seq = useRef(0);
  const inflight = useRef(0);
  const shownFrame = useRef(-1);
  const [hide, setHide] = useState<string[]>([]);
  const [selBox, setSelBox] = useState<Box | null>(null);
  const [ghost, setGhost] = useState<{ url: string; box: Box; dx: number; dy: number; s: number } | null>(null);
  const [err, setErr] = useState<string>("");
  // 演出のコマが描けなかったときのエラー（絵は演出を抜いて出ている。BR-15）
  const [fxErr, setFxErr] = useState<string>("");
  // 今の字幕の行の位置とはみ出し（Rust が描いたものと同じ。止まっているときだけ届く）
  const [subInfo, setSubInfo] = useState<SubInfo | null>(null);

  // 表示の大きさ（16:9 に収める）
  useLayoutEffect(() => {
    const el = stage.current!;
    const ro = new ResizeObserver(() => {
      const W = el.clientWidth - 16;
      const H = el.clientHeight - 16;
      const w = Math.max(160, Math.min(W, (H * 16) / 9));
      setDisp({ w: Math.round(w), h: Math.round((w * 9) / 16) });
    });
    ro.observe(el);
    return () => ro.disconnect();
  }, []);

  const k = disp.w / 1920; // 1920 座標 → 表示

  // 印の重ね（canvas 1 枚。表示の大きさ × devicePixelRatio で描き、確認画像と同じ drawMarks を使う）
  useEffect(() => {
    const c = layer.current;
    if (!c) return;
    const dpr = window.devicePixelRatio || 1;
    const w = Math.round(disp.w * dpr);
    const h = Math.round(disp.h * dpr);
    if (c.width !== w || c.height !== h) {
      c.width = w;
      c.height = h;
    }
    const ctx = c.getContext("2d");
    if (!ctx) return;
    ctx.clearRect(0, 0, w, h);
    const list = visibleMarks(marks, t, markShow, markShowDone);
    const sel = markShow ? marks.find((m) => m.id === markSel) : undefined;
    if (sel && !list.includes(sel)) list.push(sel); // 選んでいる印は、済を隠していても見えるように
    if (stroke) list.push(newMark(markColor, shapeFromDrag(markTool, stroke), "", t, [0, 0]));
    drawMarks(ctx, list, w / 1920, { note: true, selectedId: markSel });
  }, [marks, t, markShow, markShowDone, markSel, stroke, markTool, markColor, disp.w, disp.h]);

  // 描くモード: Esc で抜ける・Delete で選んだ印を削除する（選んでいなくても App の「演出を隠す」には渡さない）（文字を打つ欄では素通し。Space などはそのまま App に届く）
  useEffect(() => {
    if (!markMode) return;
    const h = (e: KeyboardEvent) => {
      if (useS.getState().nestKey || isTextEntry(document.activeElement as HTMLInputElement | null)) return;
      if (e.key === "Escape") {
        e.preventDefault();
        e.stopPropagation();
        setMarkMode(false);
      } else if (e.key === "Delete") {
        e.preventDefault();
        e.stopPropagation(); // App の「演出を隠す」に渡さない
        if (markSel !== null) removeMark(markSel);
      }
    };
    window.addEventListener("keydown", h, true);
    return () => window.removeEventListener("keydown", h, true);
  }, [markMode, markSel]);

  // 描くモードのクリック・ドラッグ（pick は呼ばない）。既存の印の線の上なら選ぶ
  const onMarkDown = (e: React.PointerEvent<HTMLDivElement>) => {
    if (e.button !== 0) return;
    e.preventDefault();
    releaseFocus();
    if (useS.getState().playing) player.pause();
    const rect = e.currentTarget.getBoundingClientRect();
    const toP = (ev: { clientX: number; clientY: number }): P => [(ev.clientX - rect.left) / k, (ev.clientY - rect.top) / k];
    const p0 = toP(e);
    const st = useS.getState();
    const hit = hitMark(visibleMarks(st.marks.marks, st.t, st.markShow, st.markShowDone), p0[0], p0[1], 10 / k);
    if (hit !== null) {
      selectMark(hit);
      useS.setState({ markNoteFor: null });
      return;
    }
    const pts: P[] = [p0];
    setStroke([p0]);
    const move = (ev: PointerEvent) => {
      pts.push(toP(ev));
      setStroke([...pts]);
    };
    const end = () => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
      window.removeEventListener("pointercancel", cancel);
    };
    const cancel = () => {
      end();
      setStroke(null);
    };
    const up = async (ev: PointerEvent) => {
      end();
      pts.push(toP(ev));
      setStroke(null);
      const s2 = useS.getState();
      const at = Math.round(s2.t * 1000) / 1000;
      const m = await upsertMark(newMark(s2.markColor, shapeFromDrag(s2.markTool, pts), "", at, defaultSpan(at, s2.scenes)));
      if (m) {
        selectMark(m.id);
        useS.setState({ markNoteFor: m.id });
      }
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
    window.addEventListener("pointercancel", cancel);
  };
  const noteMark = markNoteFor !== null ? marks.find((m) => m.id === markNoteFor) : undefined;

  const nestKey = useS((s) => s.nestKey);

  const draw = useCallback(async (tt: number, play = false) => {
    if (useS.getState().nestKey) return; // 録画を差し込む画面が開いている間は、そちらだけ描く
    const my = ++seq.current;
    inflight.current++;
    try {
      const url = akf("/frame", { t: tt.toFixed(4), s: useS.getState().scale, hide: hide.join(","), play: play ? 1 : 0, subs: showSubs ? 1 : 0 });
      const r = await fetch(url);
      if (!r.ok) {
        setErr(await r.text());
        setFxErr("");
        return;
      }
      const bmp = await createImageBitmap(await r.blob());
      if (!play && my < seq.current) return; // 古い依頼（スクラブ中）
      const info = parseFrameInfo<SubInfo>(r.headers.get("X-Info"));
      if (!play) {
        setSubInfo(info.sub);
        setFxErr(info.fxError);
      } else if (info.fxError && useS.getState().playing) {
        // 再生中は出たら残す（コマごとに点滅させない）。止めたときのコマで決め直す
        setFxErr(info.fxError);
      }
      const c = canvas.current;
      if (!c) return;
      if (c.width !== bmp.width || c.height !== bmp.height) {
        c.width = bmp.width;
        c.height = bmp.height;
      }
      c.getContext("2d")!.drawImage(bmp, 0, 0);
      setErr("");
    } catch (e) {
      setErr(errorMessage(e));
    } finally {
      inflight.current--;
    }
  }, [hide, showSubs]);

  // 再生が未準備の手前で待つかの判断に、モニターで隠している演出・字幕の状態を渡す（隠した分は待たない）
  useEffect(() => {
    player.setMonitor({ hide, subs: showSubs });
  }, [hide, showSubs]);
  const playWait = useS((s) => s.playWait);

  // 再生中: 音の時計に合わせてコマを取りに行く（間に合わなければ飛ばす）
  useEffect(() => {
    if (!playing) return;
    let raf = 0;
    const fps = playFps;
    const loopFn = () => {
      const now = player.now();
      const f = Math.floor(now * fps);
      if (f !== shownFrame.current && inflight.current < 2) {
        shownFrame.current = f;
        draw((f + 1) / fps, true);
      }
      raf = requestAnimationFrame(loopFn);
    };
    raf = requestAnimationFrame(loopFn);
    return () => cancelAnimationFrame(raf);
  }, [playing, draw, playFps]);

  // 再生中は字幕の位置が届かないので、目安線は共通の設定で出す
  useEffect(() => {
    if (playing) {
      setSubInfo(null);
      setFxErr(""); // 止めていたときの演出のエラーは消す（再生中に出たら出す）
    }
  }, [playing]);

  // 止まっているとき: 位置や中身が変わったら描き直す
  useEffect(() => {
    if (playing) return;
    const id = setTimeout(() => draw(t), 16);
    return () => clearTimeout(id);
  }, [t, playing, overrides, doc, desc, scale, draw, engineGen, nestKey, backRev]);

  // 選んでいるクリップの枠
  useEffect(() => {
    setSelBox(null);
    if (playing || !selection) return;
    let dead = false;
    (async () => {
      if (selection.kind === "fx") {
        const r = await fetch(akf("/clip", { t: t.toFixed(4), s: scale, key: selection.key }));
        const bb = r.headers.get("X-Rect"); // 絵の透明でない所 x,y,幅,高さ
        if (dead || !bb) return;
        const [x, y, w, h] = bb.split(",").map(Number);
        setSelBox({ x: x / scale, y: y / scale, w: w / scale, h: h / scale });
      } else if (selection.kind === "media" && doc) {
        const c = doc.tracks.find((x) => x.id === selection.track)?.clips.find((x) => x.id === selection.clip);
        if (!c || isAud(c.src)) return;
        const sz = await sizeOf(c.src);
        if (dead) return;
        const w = sz.w * c.scale;
        const h = sz.h * c.scale;
        const [cl, ct, cr, cb] = [c.crop_l ?? 0, c.crop_t ?? 0, c.crop_r ?? 0, c.crop_b ?? 0].map((v) => v / 100);
        setSelBox({ x: c.x - w / 2 + w * cl, y: c.y - h / 2 + h * ct, w: w * (1 - cl - cr), h: h * (1 - ct - cb) });
      }
    // 再生位置を動かすたびに走るので、トーストにはせずコンソールにだけ出す（枠が出ないだけ）
    })().catch((e) => console.warn("選んだクリップの枠を取れませんでした", e));
    return () => { dead = true; };
  }, [selection, t, playing, overrides, doc, scale]);

  // ---- ドラッグで移動・角で拡大
  const startDrag = async (e: React.PointerEvent, mode: "move" | "scale") => {
    if (!selection || !selBox || playing) return;
    e.preventDefault();
    // preventDefault でフォーカスが移らないので、調整欄に残ったフォーカスを外す（Ctrl+Z がすぐ効くように）
    releaseFocus();
    e.stopPropagation();
    const el = e.currentTarget as HTMLElement;
    el.setPointerCapture(e.pointerId);
    const x0 = e.clientX;
    const y0 = e.clientY;
    const box = selBox;
    let url = "";
    if (selection.kind === "fx") {
      // 絵を取ってくる間に離されたら、ドラッグを始めない（このあと window で受けるので、次のクリックまで枠が付いてこないように）
      let released = false;
      const early = () => { released = true; };
      window.addEventListener("pointerup", early);
      const r = await fetch(akf("/clip", { t: t.toFixed(4), s: scale, key: selection.key }));
      const blob = await r.blob();
      window.removeEventListener("pointerup", early);
      if (released) return;
      url = URL.createObjectURL(blob);
      setHide([selection.key]);
    }
    const cx = box.x + box.w / 2;
    const cy = box.y + box.h / 2;
    const d0 = Math.hypot(e.clientX - (stage.current!.getBoundingClientRect().left), 1);
    void d0;
    let cur = { dx: 0, dy: 0, s: 1 };
    setGhost({ url, box, ...cur });
    const rect = (el.closest(".mon-canvas-box") as HTMLElement).getBoundingClientRect();
    const toWorld = (px: number, py: number) => ({ x: (px - rect.left) / k, y: (py - rect.top) / k });
    const startW = toWorld(x0, y0);
    const r0 = Math.hypot(startW.x - cx, startW.y - cy);
    const move = (ev: PointerEvent) => {
      if (mode === "move") {
        let dx = (ev.clientX - x0) / k;
        let dy = (ev.clientY - y0) / k;
        if (ev.shiftKey) {
          if (Math.abs(dx) > Math.abs(dy)) dy = 0;
          else dx = 0;
        }
        cur = { dx: Math.round(dx), dy: Math.round(dy), s: 1 };
      } else {
        const w = toWorld(ev.clientX, ev.clientY);
        const s = clamp(Math.hypot(w.x - cx, w.y - cy) / Math.max(1, r0), 0.1, 5);
        cur = { dx: 0, dy: 0, s: Math.round(s * 1000) / 1000 };
      }
      setGhost((g) => (g ? { ...g, ...cur } : g));
    };
    const up = async () => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
      const st = useS.getState();
      if (selection.kind === "fx") {
        const o = st.overrides[selection.key] || {};
        if (mode === "move" && (cur.dx || cur.dy)) {
          await patchOverride(selection.key, { dx: (o.dx || 0) + cur.dx, dy: (o.dy || 0) + cur.dy });
        } else if (mode === "scale" && cur.s !== 1) {
          const pivot = o.pivot ?? [Math.round(cx - (o.dx || 0)), Math.round(cy - (o.dy || 0))];
          await patchOverride(selection.key, { scale: Math.round((o.scale || 1) * cur.s * 1000) / 1000, pivot });
        }
      } else if (selection.kind === "media") {
        editDoc((d) => {
          const c = d.tracks.find((x) => x.id === selection.track)?.clips.find((x) => x.id === selection.clip);
          if (!c) return;
          if (mode === "move") {
            c.x = Math.round(c.x + cur.dx);
            c.y = Math.round(c.y + cur.dy);
          } else c.scale = Math.round(c.scale * cur.s * 1000) / 1000;
        });
      }
      setHide([]);
      setGhost(null);
      if (url) URL.revokeObjectURL(url);
    };
    // ドラッグが始まると sel-box はゴースト（動かし中の枠）に入れ替わって DOM から外れ、el に付けたままだと
    // pointermove / pointerup が届かず、枠が「+0, +0」のまま止まっていた。window で受ける
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
  };

  // 画面のクリックで、そこに出ている演出を選ぶ（だいたいの当たり判定）
  const pick = (e: React.MouseEvent) => {
    const rect = e.currentTarget.getBoundingClientRect();
    const x = (e.clientX - rect.left) / k;
    const y = (e.clientY - rect.top) / k;
    const st = useS.getState();
    // 字幕の上ならそのセリフを選ぶ（右の調整で改行・フォント・大きさ）
    if (subInfo && showSubs && subInfo.lines.some(([x0, y0, x1, y1]) => x >= x0 - 8 && x <= x1 + 8 && y >= y0 && y <= y1)) {
      useS.setState({ selection: { kind: "line", id: subInfo.line }, focusLine: subInfo.line });
      return;
    }
    // 演出（FX トラック）をロック・非表示にしているときは、演出を選ばずに素材を探す（issue 旧#11）
    const fxOk = !!desc && fxSelectable(doc?.tracks ?? []);
    const cands = (fxOk ? desc!.groups : []).filter((g) => {
      const o = st.overrides[g.key] || {};
      const [a, b] = groupSpan(g, o);
      if (o.hidden || st.t < a || st.t >= b || !g.bbox) return false;
      // 微調整（倍率・中心・移動）の入れ方は RecWindow の窓と同じ（見本 ui_rules.json の window_box で縛っている）
      const [X0, Y0, X1, Y1] = windowBoxOf(g.bbox, o);
      return x >= X0 && x <= X1 && y >= Y0 && y <= Y1;
    });
    if (!cands.length) {
      // 立ち絵などの素材（隠した・ロックしたトラックは選ばない。背景はロックしたトラックに置く）
      const hit = pickMedia(doc?.tracks ?? [], st.t, x, y, (src) => {
        const sz = mediaSize.get(src);
        if (!sz) sizeOf(src);
        return sz;
      });
      useS.setState({ selection: hit ? { kind: "media", track: hit.track.id, clip: hit.clip.id } : null });
      return;
    }
    const area = (g: (typeof cands)[number]) => (g.bbox![2] - g.bbox![0]) * (g.bbox![3] - g.bbox![1]);
    cands.sort((p, q) => area(p) - area(q) || q.z - p.z);
    useS.setState({ selection: { kind: "fx", key: cands[0].key } });
  };

  const st0 = desc?.stage ?? [600, 6, 1880, 880];
  const fps = playFps;
  const pfps = doc?.premiere_fps ?? 60;
  // 字幕の左右の目安線（セリフの字幕はこの内側に収める）。今の字幕があればその行の設定、無ければ共通の設定
  const safeL = subInfo && showSubs ? subInfo.l : ts ? ts.margin_l : 210;
  const safeR = subInfo && showSubs ? subInfo.r : ts ? 1920 - ts.margin_r : 1710;
  const subOver = !!(showSubs && subInfo?.overflow.some(Boolean));
  const safeTop = Math.min(880, ...(showSubs && subInfo ? subInfo.lines.map((l) => l[1] - 6) : []));
  const selLabel = selection?.kind === "fx" ? selection.key : selection?.kind === "media" ? doc?.tracks.find((x) => x.id === selection.track)?.clips.find((c) => c.id === selection.clip)?.name : "";

  return (
    <div className="monitor-wrap">
      <div className="mon-head">
        <span className="tag">// PROGRAM</span>
        {compare && <span className="compare-badge" title="B を離すと今の状態に戻ります">変更前: {compare}</span>}
        <span className="grow" />
        <select value={scale} onChange={(e) => useS.setState({ scale: Number(e.target.value) })} title="プレビューの解像度">
          <option value={0.25}>1/4（480p）</option>
          <option value={0.5}>1/2（540p）</option>
          <option value={0.6667}>2/3（720p）</option>
          <option value={1}>フル（1080p）</option>
        </select>
        <button className={guides ? "on" : ""} onClick={() => { setGuides(!guides); localStorage.setItem("guides", guides ? "0" : "1"); }} title="立ち絵・字幕・演出の領域と字幕の左右の目安線を表示します（はみ出すと赤）">ガイド</button>
        <button className={showSubs ? "on" : ""} onClick={() => setShowSubs(!showSubs)} title="モニターに字幕を表示します。書き出しには影響しません（書き出しで非表示にするにはタイムラインの「字幕」の 👁）">字幕</button>
        <button className={markMode ? "on" : ""} disabled={!doc} onClick={() => setMarkMode(!markMode)} title={!doc ? "回を開いてから" : "モニターに印（ピン・矩形・矢印・ペン）を描いて、Claude に場所つきで伝えます（Esc で終わります）"}>✎ 印</button>
        <button className={markShow ? "on" : ""} onClick={() => setMarkShow(!markShow)} title="描いた印をモニターとタイムラインに表示します。書き出しには出ません">印</button>
        <button
          className={doc?.placeholder ? "on" : ""}
          onClick={() => editDoc((d) => { d.placeholder = !d.placeholder; })}
          title="録画・スクショの窓の仮表示"
        >
          仮表示
        </button>
      </div>
      {markMode && <MarkBar />}
      <div className="mon-stage" ref={stage}>
        {playWait && (
          <div className="play-wait">
            <span>{playWait.text}</span>
            <button onClick={() => player.skipPlayWait()} title="準備を待たずに、このまま再生を続けます（コマが飛んだり、声が無音になったりします）">待たずに再生</button>
          </div>
        )}
        <div className="mon-canvas-box" style={{ width: disp.w, height: disp.h }} onMouseDown={(e) => { if (e.target === canvas.current) pick(e); }}>
          <canvas ref={canvas} width={960} height={540} />
          {guides && (
            <>
              <div className="guide" style={{ left: st0[0] * k, top: st0[1] * k, width: (st0[2] - st0[0]) * k, height: (st0[3] - st0[1]) * k, borderColor: "color-mix(in srgb, var(--accent) 33.33%, transparent)" }} />
              <div className="guide" style={{ left: 0, top: 0, width: 600 * k, height: disp.h, borderColor: "color-mix(in srgb, var(--warn) 25.10%, transparent)", borderTop: 0, borderLeft: 0, borderBottom: 0 }} />
              <div className="guide" style={{ left: 0, top: 880 * k, width: disp.w, height: 1, borderColor: "color-mix(in srgb, var(--error) 33.33%, transparent)", borderLeft: 0, borderRight: 0, borderBottom: 0 }} />
              {[safeL, safeR].map((x, i) => (
                <div key={i} className={`sub-safe ${subOver ? "over" : ""}`} style={{ left: x * k, top: safeTop * k, height: (1080 - safeTop) * k }}
                  title="字幕の左右の目安です（この内側に収めます）。字幕の設定の「左右の余白」で変更できます" />
              ))}
              {showSubs && subInfo?.lines.map(([x0, y0, x1, y1], i) => subInfo.overflow[i] && (
                <div key={i} className="sub-over" style={{ left: x0 * k, top: y0 * k, width: (x1 - x0) * k, height: (y1 - y0) * k }} />
              ))}
              {subOver && (
                <div className="sub-badge" style={{ top: safeTop * k - 20 }}
                  onMouseDown={(e) => { e.stopPropagation(); useS.setState({ selection: { kind: "line", id: subInfo!.line }, focusLine: subInfo!.line }); }}>
                  ⚠ 字幕がはみ出しています（セリフ {subInfo!.n}）。クリックして改行を入れてください
                </div>
              )}
            </>
          )}
          {desc && <div className="scene-label">SCENE {sceneAt(t)}</div>}
          <canvas ref={layer} className="mark-layer" />
          {markMode && <div className="mark-capture" onPointerDown={onMarkDown} />}
          {noteMark && markMode && (
            <MarkNoteInput
              key={noteMark.id}
              mark={noteMark}
              left={Math.max(0, Math.min((badgePos(noteMark.shape)[0] + 23) * k, disp.w - 250))}
              top={Math.max(0, Math.min((badgePos(noteMark.shape)[1] - 14) * k, disp.h - 30))}
            />
          )}
          {selBox && !ghost && !markMode && (
            <div
              className="sel-box"
              style={{ left: selBox.x * k, top: selBox.y * k, width: selBox.w * k, height: selBox.h * k }}
              onPointerDown={(e) => startDrag(e, "move")}
              title="ドラッグで移動（Shift で縦横に固定）、右下の角で拡大・縮小"
            >
              <div className="label">{selLabel}</div>
              <div className="handle" style={{ right: -6, bottom: -6, cursor: "nwse-resize" }} onPointerDown={(e) => startDrag(e, "scale")} />
            </div>
          )}
          {ghost && (
            <>
              {ghost.url && (
                <img
                  src={ghost.url}
                  style={{
                    position: "absolute",
                    left: (ghost.box.x + ghost.dx + (ghost.box.w * (1 - ghost.s)) / 2) * k,
                    top: (ghost.box.y + ghost.dy + (ghost.box.h * (1 - ghost.s)) / 2) * k,
                    width: ghost.box.w * ghost.s * k,
                    height: ghost.box.h * ghost.s * k,
                    pointerEvents: "none",
                  }}
                />
              )}
              <div className="sel-box" style={{
                left: (ghost.box.x + ghost.dx + (ghost.box.w * (1 - ghost.s)) / 2) * k,
                top: (ghost.box.y + ghost.dy + (ghost.box.h * (1 - ghost.s)) / 2) * k,
                width: ghost.box.w * ghost.s * k, height: ghost.box.h * ghost.s * k, pointerEvents: "none" }}>
                <div className="label">{ghost.s !== 1 ? `×${ghost.s.toFixed(2)}` : `${ghost.dx >= 0 ? "+" : ""}${ghost.dx}, ${ghost.dy >= 0 ? "+" : ""}${ghost.dy}`}</div>
              </div>
            </>
          )}
          {fxErr && !err && (
            <div title={fxErr} style={{ position: "absolute", top: 8, right: 8, maxWidth: "60%", color: "var(--e-05)", fontSize: 11, background: "#000a", padding: "2px 6px", whiteSpace: "nowrap", overflow: "hidden", textOverflow: "ellipsis" }}>
              ⚠ {shortError(fxErr)}
            </div>
          )}
          {err && <div style={{ position: "absolute", left: 8, bottom: 8, right: 8, color: "var(--e-05)", fontSize: 11, background: "#000a", padding: 4, whiteSpace: "pre-wrap", maxHeight: "40%", overflow: "auto" }}>{err}</div>}
        </div>
      </div>
      <div className="transport">
        <button className="ghost" onClick={() => player.seek(0)} title="先頭へ（Home）">⏮</button>
        <button className="ghost" onClick={() => player.seek(t - 1 / fps)} title="前のフレーム（←）">◀</button>
        <button className="primary" onClick={() => player.toggle()} title="再生・停止（Space）" style={{ minWidth: 44 }}>{playing ? "❚❚" : "▶"}</button>
        <button className="ghost" onClick={() => player.seek(t + 1 / fps)} title="次のフレーム（→）">▶</button>
        <button className="ghost" onClick={() => player.seek(total())} title="末尾へ（End）">⏭</button>
        <span className="tc">{tc(t, fps)}</span>
        <span className="dim2 ui tcpr" style={{ fontSize: 11 }} title="Premiere のタイムコード（オーバーレイの配置位置を反映）">
          Pr {premiereTc(t + (doc?.premiere_t0 ?? 0), pfps)}
        </span>
        <div className="tr-right">
        <button className="ghost" onClick={() => useS.setState({ inT: t })} title="イン（I）">[ {inT != null ? tc(inT, fps) : ""}</button>
        <button className="ghost" onClick={() => useS.setState({ outT: t })} title="アウト（O）">{outT != null ? tc(outT, fps) : ""} ]</button>
        <button
          className={loop ? "on" : ""}
          onClick={() => {
            const v = !loop;
            setLoop(v);
            player.loopRange = v && inT != null && outT != null && outT > inT ? [inT, outT] : null;
          }}
          title="イン〜アウトを繰り返し再生"
        >
          ⟲
        </button>
        <button
          onClick={() => {
            const a = inT ?? Math.max(0, t - 2);
            const b = outT ?? t + 10;
            call("prefetch", { t0: Math.min(a, b), t1: Math.max(a, b), scale, tag: "render" });
          }}
          title="イン〜アウト（未設定なら現在位置の前後）の演出を先にレンダリングします（Enter）"
        >
          レンダリング
        </button>
        <button
          onClick={() => {
            const text = prompt("この場面について Claude に送るメッセージ（例：このテロップをもう少し右に）");
            if (text) sendChat(text, { kind: "scene_note" });
          }}
          title="現在の場面（再生位置・シーン・選択中のクリップ）を添えて Claude に送信します"
        >
          💬 この場面
        </button>
        </div>
      </div>
    </div>
  );
}

export type { MediaClip };
