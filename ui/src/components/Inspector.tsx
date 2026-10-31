import React, { useEffect, useRef, useState } from "react";
import { call } from "../io/api";
import { failText } from "../logic/errors.ts";
import { sizeOf } from "./Monitor";
import { editDoc, holdHist, loadSpeakersInfo, patchOverride, sendChat, setSpeakerVoice, toast, useS } from "../state";
import { VOICE_PARAMS, effectiveVoice, styleShares, withStyleWeight } from "../logic/speakervoice";
import { engineOf, has, paramFields } from "../logic/voiceengine";
import { SCRUB_START_PX, clampRange, fmtNum, nudge, parseNum, scrubMove, scrubMult, scrubValue } from "../logic/scrub";
import { player } from "../io/player";
import type { FontInfo, Group, MediaClip, Override, SubStyle, VoiceItem } from "../types";
import { nid } from "../logic/voice";
import { isAud, isImg as isImgFile } from "../logic/media";
import { SUB_IDLE_MS, fontLabel, fontName, normSubText, subEditKey, syncSubEdit } from "../logic/subs";
import {
  loadFonts,
  patchSubLine,
  patchSubStyle,
  refreshSubCheck,
} from "../state/subs";
import { SPEAKERS, premiereTc, tc } from "../logic/util";
import { groupSpan, isRecGroup } from "../logic/nest";
import SeInspector from "./SeInspector";
import { guideAudioRow } from "../logic/guideAudio.ts";
import { TRANS_DEFAULT_DUR, TRANS_DIRS, TRANS_KINDS, linkSwitch, prevClip, setTrans, transHasDir } from "../logic/trans";

const SCRUB_TITLE = "左右にドラッグで変更します（Shift で粗く、Ctrl で細かく）。クリックで入力、↑↓ で増減できます";

/**
 * 数値そのものを左右にドラッグして変える欄（Premiere の数値と同じ）。クリックで直接入力（Enter で確定・Esc で取り消し）。
 * ドラッグ量は step で決まり（1px ＝ 1 step）、min / max は値を丸めるのにだけ使う
 */
function ScrubNum({ value, step, min, max, onCommit, onReset, width = 58 }: {
  value: number; step: number; min?: number; max?: number;
  onCommit: (v: number) => void; onReset?: () => void; width?: number;
}) {
  // ドラッグ中の値（null のときは保存されている値を出す）
  const [live, setLive] = useState<number | null>(null);
  // 直接入力中の文字（null のときは入力欄を出さない）
  const [edit, setEdit] = useState<string | null>(null);
  const timer = useRef<number | undefined>(undefined);
  const pending = useRef<number | null>(null);
  const drag = useRef<{ x0: number; x: number; cur: number; on: boolean } | null>(null);
  const skipBlur = useRef(false);
  const commitRef = useRef(onCommit);
  commitRef.current = onCommit;
  useEffect(() => () => {
    // 途中で消えたとき（選択が変わったなど）も、ドラッグのまとめを解く
    clearTimeout(timer.current);
    if (drag.current?.on) { holdHist(false); document.body.classList.remove("scrubbing"); }
  }, []);
  const send = (n: number) => {
    if (!Number.isFinite(n)) return;
    commitRef.current(clampRange(n, min, max));
  };
  // 動かしている間も 0.1 秒おきに反映する（モニターで効果を見ながら決められるように）
  const slide = (n: number) => {
    setLive(n);
    pending.current = n;
    if (timer.current === undefined) {
      timer.current = window.setTimeout(() => {
        timer.current = undefined;
        if (pending.current !== null) send(pending.current);
        pending.current = null;
      }, 100);
    }
  };
  const endDrag = () => {
    const d = drag.current;
    drag.current = null;
    if (!d?.on) return;
    clearTimeout(timer.current);
    timer.current = undefined;
    if (pending.current !== null) send(pending.current);
    pending.current = null;
    setLive(null);
    holdHist(false);
    document.body.classList.remove("scrubbing");
  };
  // 入力欄を開いたときの値と確定先。入力中にほかのクリップを選ぶと、blur より先にこの欄が次のクリップの値で描き直されるので、
  // そのときの value / onCommit で比べると、変えていない値が次のクリップに書き込まれ、やり直しの履歴も消えていた（旧#9）
  const editFor = useRef<{ value: number; commit: (v: number) => void } | null>(null);
  // 入力欄を開く。Enter / Esc で閉じたあとは blur が来ないことがあり、読み飛ばしの印が残ると
  // 次の入力が blur（ほかをクリック）で確定しなくなるので、開くたびに戻す
  const startEdit = () => {
    skipBlur.current = false;
    editFor.current = { value, commit: onCommit };
    setEdit(fmtNum(value));
  };
  const finishEdit = (txt: string | null) => {
    const f = editFor.current ?? { value, commit: onCommit };
    editFor.current = null;
    setEdit(null);
    if (txt === null) return;
    const n = parseNum(txt);
    if (n !== null && Number.isFinite(n) && Math.abs(n - f.value) > 1e-9) f.commit(clampRange(n, min, max));
  };

  if (edit !== null) {
    return (
      <input
        className="scrub-input"
        autoFocus
        value={edit}
        onFocus={(e) => e.currentTarget.select()}
        onChange={(e) => setEdit(e.target.value)}
        onBlur={() => { if (skipBlur.current) skipBlur.current = false; else finishEdit(edit); }}
        onKeyDown={(e) => {
          if (e.key === "Enter") { e.stopPropagation(); skipBlur.current = true; finishEdit(edit); }
          else if (e.key === "Escape") { e.stopPropagation(); skipBlur.current = true; finishEdit(null); }
          else if (e.key === "ArrowUp" || e.key === "ArrowDown") {
            e.preventDefault();
            e.stopPropagation();
            const n = nudge(parseNum(edit) ?? value, e.key === "ArrowUp" ? 1 : -1, step, scrubMult(e), min, max);
            setEdit(fmtNum(n));
            send(n);
            if (editFor.current) editFor.current.value = n;
          }
        }}
        style={{ width }}
      />
    );
  }
  const shown = live ?? value;
  return (
    <span
      className={`scrub ${live !== null ? "on" : ""}`}
      tabIndex={0}
      title={SCRUB_TITLE + (onReset ? "。右クリックでリセットできます" : "")}
      style={{ minWidth: width }}
      onPointerDown={(e) => {
        if (e.button !== 0) return;
        e.preventDefault();
        e.currentTarget.focus();
        e.currentTarget.setPointerCapture(e.pointerId);
        drag.current = { x0: e.clientX, x: e.clientX, cur: value, on: false };
      }}
      onPointerMove={(e) => {
        const d = drag.current;
        if (!d) return;
        if (!d.on) {
          if (Math.abs(e.clientX - d.x0) < SCRUB_START_PX) return;
          d.on = true;
          d.x = e.clientX;
          holdHist(true);
          document.body.classList.add("scrubbing");
          return;
        }
        const m = scrubMult(e);
        d.cur = scrubMove(d.cur, e.clientX - d.x, step, m, min, max);
        d.x = e.clientX;
        slide(scrubValue(d.cur, step, m, min, max));
      }}
      onPointerUp={() => {
        const d = drag.current;
        if (d && !d.on) {
          // 動かさずに離した → 直接入力
          drag.current = null;
          startEdit();
        } else endDrag();
      }}
      onPointerCancel={endDrag}
      onLostPointerCapture={endDrag}
      onContextMenu={(e) => { if (onReset) { e.preventDefault(); onReset(); } }}
      onKeyDown={(e) => {
        // App のキー操作（↑↓ でセリフ移動・Enter でレンダリング）とぶつからないように止める
        if (e.key === "Enter" || e.key === "F2") { e.preventDefault(); e.stopPropagation(); startEdit(); }
        else if (e.key === "ArrowUp" || e.key === "ArrowDown") {
          e.preventDefault();
          e.stopPropagation();
          send(nudge(value, e.key === "ArrowUp" ? 1 : -1, step, scrubMult(e), min, max));
        }
      }}
    >
      {fmtNum(shown)}
    </span>
  );
}

/** 既定値があるか（-1e9 は「既定なし」の印） */
const hasDef = (def: number) => def > -1e8;
const isDef = (v: number, def: number) => Math.abs(v - def) < 1e-9;

export function Num({ label, value, def, step = 1, unit = "", onCommit, min, max }: {
  label: string; value: number | undefined; def: number; step?: number; unit?: string;
  onCommit: (v: number | null) => void; min?: number; max?: number;
}) {
  const v = value ?? def;
  const reset = hasDef(def) ? () => onCommit(null) : undefined;
  return (
    <div className="field">
      <label>{label}</label>
      <ScrubNum value={v} step={step} min={min} max={max} onReset={reset} onCommit={(n) => onCommit(isDef(n, def) ? null : n)} />
      <div className="row" style={{ gap: 3 }}>
        <span className="unit">{unit}</span>
        {reset && !isDef(v, def) && <button className="ghost reset" title="リセット" onClick={reset}>↺</button>}
      </div>
    </div>
  );
}

type NumPart = { tag: string; value: number | undefined; def: number; onCommit: (v: number | null) => void };

/** 2 つの数値を 1 行に（位置の x / y など） */
export function Num2({ label, a, b, step = 1, unit = "", min, max }: {
  label: string; a: NumPart; b: NumPart; step?: number; unit?: string; min?: number; max?: number;
}) {
  const part = (p: NumPart) => {
    const v = p.value ?? p.def;
    return (
      <span className="num2-part">
        <span className="tag">{p.tag}</span>
        <ScrubNum value={v} step={step} min={min} max={max} width={50}
          onReset={hasDef(p.def) ? () => p.onCommit(null) : undefined}
          onCommit={(n) => p.onCommit(isDef(n, p.def) ? null : n)} />
      </span>
    );
  };
  const changed = [a, b].filter((p) => hasDef(p.def) && !isDef(p.value ?? p.def, p.def));
  return (
    <div className="field">
      <label>{label}</label>
      <div className="row num2">{part(a)}{part(b)}</div>
      <div className="row" style={{ gap: 3 }}>
        <span className="unit">{unit}</span>
        {changed.length > 0 && <button className="ghost reset" title="リセット" onClick={() => changed.forEach((p) => p.onCommit(null))}>↺</button>}
      </div>
    </div>
  );
}

/** クロップの欄（本体の素材とネストの素材で共通）。children は行の先頭に足すボタン */
export function CropFields({ c, set, reset, children }: {
  c: MediaClip; set: (k: "crop_l" | "crop_t" | "crop_r" | "crop_b" | "feather") => (v: any) => void; reset: () => void; children?: React.ReactNode;
}) {
  return (
    <>
      <div className="section">CROP</div>
      <Num2 label="左右" step={0.5} unit="%" min={0} max={100}
        a={{ tag: "左", value: c.crop_l, def: 0, onCommit: (v) => set("crop_l")(v ?? 0) }}
        b={{ tag: "右", value: c.crop_r, def: 0, onCommit: (v) => set("crop_r")(v ?? 0) }} />
      <Num2 label="上下" step={0.5} unit="%" min={0} max={100}
        a={{ tag: "上", value: c.crop_t, def: 0, onCommit: (v) => set("crop_t")(v ?? 0) }}
        b={{ tag: "下", value: c.crop_b, def: 0, onCommit: (v) => set("crop_b")(v ?? 0) }} />
      <Num label="ぼかし" value={c.feather} def={0} step={1} unit="px" onCommit={(v) => set("feather")(v ?? 0)} min={0} max={200} />
      <div className="row" style={{ flexWrap: "wrap", marginTop: 4 }}>
        {children}
        <button className="ghost" onClick={reset}>クロップをリセット</button>
      </div>
    </>
  );
}

// ------------------------------------------------------------------ 字幕
function ColorField({ label, value, base, onCommit }: {
  label: string; value: string; base: string; onCommit: (v: string | null) => void;
}) {
  const [txt, setTxt] = useState(value);
  const timer = useRef<number | undefined>(undefined);
  useEffect(() => setTxt(value), [value]);
  const commit = (v: string) => {
    if (!/^#[0-9a-fA-F]{6}([0-9a-fA-F]{2})?$/.test(v)) return setTxt(value);
    if (v.toLowerCase() === value.toLowerCase()) return;
    onCommit(v.toLowerCase() === base.toLowerCase() ? null : v.toLowerCase());
  };
  return (
    <div className="field">
      <label>{label}</label>
      <input
        type="color"
        value={(/^#[0-9a-fA-F]{6}/.test(txt) ? txt : value).slice(0, 7)}
        onChange={(e) => {
          // 色を選んでいる間は何度も来るので、止まってから保存する
          const v = e.target.value;
          setTxt(v);
          clearTimeout(timer.current);
          timer.current = window.setTimeout(() => commit(v), 350);
        }}
        style={{ width: "100%", height: 22, padding: 0 }}
      />
      <div className="row" style={{ gap: 3 }}>
        <input value={txt} onChange={(e) => setTxt(e.target.value)} onBlur={() => commit(txt)}
          onKeyDown={(e) => { if (e.key === "Enter") commit(txt); }} style={{ width: 62 }} className="mono" />
        {value.toLowerCase() !== base.toLowerCase() && <button className="ghost" title="リセット" onClick={() => onCommit(null)} style={{ padding: "0 3px" }}>↺</button>}
      </div>
    </div>
  );
}

/** 入っているフォントから選ぶ。inherit があれば「上の設定のまま」を選べる */
function FontSelect({ value, onChange, inherit }: { value: string; onChange: (v: string | null) => void; inherit?: string }) {
  const fonts = useS((s) => s.fonts);
  useEffect(() => { loadFonts(); }, []);
  const jp = (fonts ?? []).filter((f) => f.jp !== false);
  const other = (fonts ?? []).filter((f) => f.jp === false);
  const known = (fonts ?? []).some((f) => f.spec.toLowerCase() === value.toLowerCase());
  const opt = (f: FontInfo) => <option key={f.spec} value={f.spec} title={f.spec}>{fontLabel(f)}</option>;
  return (
    <div className="field">
      <label>フォント</label>
      <select value={value} onChange={(e) => onChange(e.target.value || null)} style={{ gridColumn: "2 / 4", minWidth: 0 }}>
        {inherit !== undefined && <option value="">{inherit}</option>}
        {value && !known && <option value={value}>{fonts ? "（未インストール）" : ""}{fontName(value, fonts)}</option>}
        {!fonts && <option disabled>フォントを調べています…</option>}
        {jp.length > 0 && <optgroup label="日本語">{jp.map(opt)}</optgroup>}
        {other.length > 0 && <optgroup label="その他">{other.map(opt)}</optgroup>}
      </select>
    </div>
  );
}

/** 字幕の見た目の項目。style は今の値、base は外したときに戻る値（共通なら既定、話者なら共通） */
function SubStyleFields({ style, base, onPatch, inheritFont }: {
  style: SubStyle; base: SubStyle; onPatch: (p: Partial<Record<keyof SubStyle, any>>) => void; inheritFont?: string;
}) {
  const fonts = useS((s) => s.fonts);
  const f = fonts?.find((x) => x.spec.toLowerCase() === style.font.toLowerCase());
  const N = (k: keyof SubStyle, label: string, min: number, max: number, step = 1, unit = "px") => (
    <Num label={label} value={style[k] as number} def={base[k] as number} step={step} unit={unit} min={min} max={max}
      onCommit={(v) => onPatch({ [k]: v })} />
  );
  return (
    <>
      <FontSelect value={inheritFont !== undefined && style.font === base.font ? "" : style.font} inherit={inheritFont}
        onChange={(v) => onPatch({ font: v === base.font ? null : v })} />
      {(!f || f.weight_axis) ? N("weight", "太さ", f?.weight_axis?.[0] ?? 100, f?.weight_axis?.[1] ?? 900, 100, "") : (
        <div className="dim2" style={{ fontSize: 11, margin: "0 0 4px 0" }}>太さは可変フォントのときだけ変更できます</div>
      )}
      {N("size", "サイズ", 12, 160)}
      <ColorField label="文字の色" value={style.color} base={base.color} onCommit={(v) => onPatch({ color: v })} />
      <ColorField label="縁の色" value={style.stroke} base={base.stroke} onCommit={(v) => onPatch({ stroke: v })} />
      {N("stroke_w", "縁の太さ", 0, 30)}
      {N("shadow", "影の濃さ", 0, 1, 0.05, "")}
      {style.shadow > 0 && (
        <>
          <ColorField label="影の色" value={style.shadow_color} base={base.shadow_color} onCommit={(v) => onPatch({ shadow_color: v })} />
          {N("shadow_x", "影 横", -40, 40)}
          {N("shadow_y", "影 縦", -40, 40)}
          {N("shadow_blur", "影 ぼかし", 0, 40)}
        </>
      )}
      <div className="section">LAYOUT</div>
      {N("y", "下の位置", 300, 1080)}
      {N("line_h", "行間", 20, 200)}
      {N("margin_l", "左余白", 0, 900)}
      {N("margin_r", "右余白", 0, 900)}
      <div className="dim2 ui" style={{ fontSize: 11 }}>
        字幕を x={style.margin_l}〜{1920 - style.margin_r}（幅 {1920 - style.margin_l - style.margin_r}）に収めます（モニターの「ガイド」の緑の線）
      </div>
      <div className="field">
        <label>揃え</label>
        <select value={style.align} onChange={(e) => onPatch({ align: e.target.value === base.align ? null : e.target.value })} style={{ gridColumn: "2 / 4" }}>
          <option value="center">中央</option>
          <option value="left">左</option>
          <option value="right">右</option>
        </select>
      </div>
      <div className="field">
        <label>自動折り返し</label>
        <input type="checkbox" checked={style.wrap} onChange={(e) => onPatch({ wrap: e.target.checked === base.wrap ? null : e.target.checked })} />
        <span className="dim2" style={{ fontSize: 11 }}>手動で改行した行は折り返しません</span>
      </div>
    </>
  );
}

// ------------------------------------------------------------------ 話者の声
/** スタイルの帯の色（VoiSona の円グラフに近い色。スタイルの順） */
const STYLE_COLORS = ["#2ac6cc", "#b04a58", "#a8243a", "#9a9420", "#3438a8", "#2f8f5a", "#a0602a", "#7a4aa8"];

/** トークトラックの話者の声（声のエンジンの話者の値とスタイルの重み）。_voice_speakers.json に保存 */
function SpeakerVoiceFields({ sp }: { sp: string }) {
  const over = useS((s) => s.script?.speakers?.[sp]);
  const hasScript = useS((s) => !!s.script);
  const info = useS((s) => s.speakersInfo);
  const voiceOk = useS((s) => s.voice?.ok);
  const engine = engineOf(useS((s) => s.voice?.info));
  const run = useS((s) => s.synthAllRun);
  useEffect(() => { if (voiceOk && !info) loadSpeakersInfo(); }, [voiceOk, info]);
  const si = info?.speakers?.[sp];
  const note = (text: React.ReactNode) => <div className="dim2" style={{ fontSize: 11, lineHeight: 1.7 }}>{text}</div>;
  if (!hasScript) return note("台本を開くと設定できます。");
  if (!si) {
    if (info?.error) return note(<>話者の声を読み込めませんでした: {info.error} <button onClick={loadSpeakersInfo}>再読み込み</button></>);
    if (!voiceOk) return note("声のエンジンにつながると設定できます。");
    if (info?.speakers) return note(`${sp} は声のエンジン（${engine.label}）の話者のプリセットにありません。`);
    return note("（話者の声を読み込み中…）");
  }
  const fields = info?.range ? paramFields(info.range) : VOICE_PARAMS;
  const cur = effectiveVoice(si.base, over, fields);
  const w = cur.style_weights ?? [];
  const shares = styleShares(w);
  const changed = !!over && Object.keys(over).length > 0;
  return (
    <>
      {note(<>
        {sp} のすべてのセリフの声です（声のエンジン（{engine.label}）の話者の値）。共通のプリセットから変更した項目を、この回の
        script/_voice_speakers.json に保存します。変更すると {sp} の音声を作り直します。セリフごとに抑揚を変えた行は、そちらが優先されます。
      </>)}
      {fields.map((p) => (
        <div key={p.key} title={p.abbr ? `${p.abbr}: ${p.tip}` : p.tip}>
          <Num label={p.label} value={over?.[p.key] as number | undefined} def={(si.base[p.key] as number | undefined) ?? p.def} step={p.step} unit={p.unit}
            min={p.min} max={p.max} onCommit={(v) => setSpeakerVoice(sp, { [p.key]: v })} />
        </div>
      ))}
      {has(engine, "styles") && w.length > 0 && (
        <>
          <div className="dim" style={{ fontSize: 12, margin: "10px 0 4px" }}>声のスタイル</div>
          <div className="voice-mix" title="スタイルの配合（重みの割合）">
            {shares.map((x, i) => x > 0 && (
              <span key={i} style={{ flexGrow: x, background: STYLE_COLORS[i % STYLE_COLORS.length] }} />
            ))}
          </div>
          {w.map((_, i) => (
            <div key={i} className="voice-style" title={si.style_names[i]}>
              <Num label={si.style_labels[i] ?? si.style_names[i] ?? `スタイル ${i + 1}`} value={over?.style_weights?.[i]}
                def={si.base.style_weights?.[i] ?? 0} step={0.01} min={0} max={1} unit={`${Math.round(shares[i] * 100)}%`}
                onCommit={(v) => setSpeakerVoice(sp, { style_weights: withStyleWeight(w, i, v, si.base.style_weights) })} />
              <i style={{ background: STYLE_COLORS[i % STYLE_COLORS.length] }} />
            </div>
          ))}
        </>
      )}
      {si.error && note(`声のエンジンからスタイルの名前を受け取れなかったため、プリセットの名前で表示しています（${si.error}）`)}
      <div className="row" style={{ gap: 6, marginTop: 6 }}>
        {run && <span className="dim2" style={{ fontSize: 11 }}>音声を作成中（{run.i + 1}/{run.n}）</span>}
        <span className="grow" />
        <button className="danger" disabled={!changed} onClick={() => setSpeakerVoice(sp, null)}>声をリセット</button>
      </div>
    </>
  );
}

/** subs_view の結果がまだ無いときの表示（失敗していればその理由） */
const subWait = (err: string | null) => err ? failText("字幕の見た目を取得できませんでした", err) : "（字幕の見た目を読み込み中…）";

/** 字幕のトラックの見た目（共通）と、話者ごとの上書き。話者を選んでいるときは、その話者の声も */
function SubtitleInspector({ sp }: { sp?: string }) {
  const doc = useS((s) => s.project?.doc);
  const layout = useS((s) => s.layout);
  const check = useS((s) => s.subCheck);
  const view = useS((s) => s.subView);
  const viewErr = useS((s) => s.subViewErr);
  useEffect(() => { refreshSubCheck(0); }, []);
  if (!doc) return <div className="empty">回を開いてください</div>;
  if (!view) return <div className="empty">{subWait(viewErr)}</div>;
  const speakers = [...new Set([...SPEAKERS, ...layout.map((x) => x.sp)])];
  const common = view.track;
  const ov = sp ? doc.subtitle?.speakers?.[sp] ?? {} : null;
  const nOver = check ? Object.values(check.lines).filter((x) => x.overflow).length : 0;
  const nOwn = Object.keys(doc.subtitle_lines ?? {}).length;
  const firstOver = check ? layout.find((it) => check.lines[it.line_id]?.overflow) : undefined;
  return (
    <div className="insp">
      <h3>{sp ? "声と字幕" : "字幕の見た目"}<span className="dim2" style={{ fontSize: 12 }}>{sp ?? "共通（すべてのセリフ）"}</span></h3>
      <div className="row" style={{ flexWrap: "wrap", gap: 4 }}>
        <button className={!sp ? "on" : ""} onClick={() => useS.setState({ selection: { kind: "subs" } })}>共通</button>
        {speakers.map((x) => (
          <button key={x} className={sp === x ? "on" : ""} onClick={() => useS.setState({ selection: { kind: "subs", sp: x } })}>
            {x}{doc.subtitle?.speakers?.[x] && Object.keys(doc.subtitle.speakers[x]).length ? " *" : ""}
          </button>
        ))}
        <span className="grow" />
        {sp ? (
          <button className="danger" disabled={!Object.keys(ov!).length} onClick={() => editDoc((d) => {
            const spk = { ...(d.subtitle.speakers || {}) };
            delete spk[sp];
            d.subtitle = { ...d.subtitle, speakers: spk };
          })}>字幕の上書きを削除</button>
        ) : (
          <button className="danger" onClick={() => {
            if (confirm("共通の字幕の見た目を既定（チャンネルの標準）に戻しますか？話者ごと・セリフごとの設定はそのまま残ります。")) {
              editDoc((d) => { d.subtitle = { hidden: d.subtitle?.hidden, speakers: d.subtitle?.speakers }; });
            }
          }}>既定に戻す</button>
        )}
      </div>
      {sp && (
        <>
          <div className="section">VOICE</div>
          <SpeakerVoiceFields sp={sp} />
        </>
      )}
      <div className="section">STYLE</div>
      <div className="dim2" style={{ fontSize: 11, lineHeight: 1.7, margin: "6px 0" }}>
        {sp ? `${sp} のセリフだけ、変更した項目で共通の字幕の設定を上書きします。` : "すべてのセリフの字幕の見た目です。話者ごと（上のボタン）・セリフごと（セリフを選んで右の「字幕」）に上書きできます。"}
        モニター・完成版 mp4・確認画像・字幕の透過 .mov が同じ見た目になります。
      </div>
      {sp ? (
        <SubStyleFields style={view.speakers[sp] ?? view.track} base={common} onPatch={(p) => patchSubStyle(sp, p)} inheritFont={`共通のまま（${fontName(common.font, useS.getState().fonts)}）`} />
      ) : (
        <SubStyleFields style={common} base={view.default} onPatch={(p) => patchSubStyle(null, p)} />
      )}
      <div className="section">CHECK</div>
      <div className="dim" style={{ fontSize: 12, lineHeight: 1.8 }}>
        {check ? (
          <>
            はみ出している字幕: <b style={{ color: nOver ? "var(--error)" : "var(--ok)" }}>{nOver}</b> 行
            {firstOver && <button style={{ marginLeft: 6 }} onClick={() => { useS.setState({ selection: { kind: "line", id: firstOver.line_id }, focusLine: firstOver.line_id }); player.seek(firstOver.start + 0.1); }}>最初の行へ</button>}
            <br />
            セリフごとの設定: {nOwn} 行
            {view.orphans.length > 0 && (
              <>
                （うち台本の変更で無効になったもの {view.orphans.length}
                <button style={{ marginLeft: 6 }} title={view.orphans.map((k) => k.replace("\u0001", "：")).join("\n")}
                  onClick={() => editDoc((d) => {
                    const all = { ...(d.subtitle_lines || {}) };
                    for (const k of view.orphans) delete all[k];
                    d.subtitle_lines = all;
                  })}>削除</button>）
              </>
            )}
          </>
        ) : "（字幕の確認中…）"}
      </div>
    </div>
  );
}

/** セリフの字幕（改行・フォント・大きさ）。鍵は話者＋字幕の文なので、台本の文を直すと外れる */
function LineSubtitle({ it }: { it: VoiceItem }) {
  const doc = useS((s) => s.project!.doc);
  const ck = useS((s) => s.subCheck?.lines[it.line_id]);
  const view = useS((s) => s.subView);
  const viewErr = useS((s) => s.subViewErr);
  const lv = view?.lines[it.line_id];
  const key = lv?.key ?? "";
  const own = (key && doc.subtitle_lines?.[key]) || {};
  const cur = own.text ?? it.text;
  const [txt, setTxt] = useState(cur);
  // 待っている確定（2 秒の待ち）。アンマウント・key が変わるときに流すので、その時点の key・値を持つ
  const pend = useRef<{ key: string; v: string; cur: string; text: string } | null>(null);
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const composing = useRef(false);
  const enterPending = useRef(false);
  const prevKey = useRef(key);
  const txtRef = useRef(txt);
  txtRef.current = txt;
  const write = (k: string, v: string, saved: string, text: string) => {
    const n = normSubText(v);
    if (n === saved) return;
    patchSubLine(k, { text: !n.trim() || n === text ? null : n });
  };
  const cancelWait = () => {
    if (timer.current) clearTimeout(timer.current);
    timer.current = null;
    pend.current = null;
  };
  const flush = () => {
    const p = pend.current;
    cancelWait();
    if (p) write(p.key, p.v, p.cur, p.text);
  };
  const flushRef = useRef(flush);
  flushRef.current = flush;
  useEffect(() => () => flushRef.current(), [key]);
  useEffect(() => {
    if (prevKey.current !== key) setTxt(cur);
    else if (syncSubEdit(txtRef.current, cur) !== txtRef.current) {
      // 外（Claude・元に戻す）から変わった。待っている確定を流すと外の変更を古い値で消すので捨てる
      cancelWait();
      setTxt(cur);
    }
    prevKey.current = key;
  }, [cur, key]);
  // すぐ確定する（待っている確定は取り消す）。空の行・行末の空白は保存で消えるが、欄は syncSubEdit でそのまま残る
  const commit = (v: string) => {
    cancelWait();
    write(key, v, cur, it.text);
  };
  // 打つのが止まって SUB_IDLE_MS たったら確定する
  const later = (v: string) => {
    cancelWait();
    pend.current = { key, v, cur, text: it.text };
    timer.current = setTimeout(() => flushRef.current(), SUB_IDLE_MS);
  };
  const custom = txt.replace(/\n/g, "") !== it.text;
  if (!view || !lv) return <div className="empty">{subWait(viewErr)}</div>;
  const base = view.speakers[it.sp] ?? view.track;
  const style = lv.style;
  return (
    <>
      <div className="section">SUBTITLE（字幕）</div>
      <div className="row dim" style={{ fontSize: 12, flexWrap: "wrap" }}>
        {ck ? (
          ck.overflow ? <span style={{ color: "var(--error)" }}>⚠ 左右の線からはみ出しています。改行を入れるか、サイズを小さくしてください</span>
            : <span style={{ color: "var(--ok)" }}>✓ 線の内側に収まっています（{ck.texts.length} 行）</span>
        ) : <span>確認中…</span>}
        {own.text && <span className="dim2">{lv.custom ? "・字幕だけ書き換え" : "・手動で改行"}</span>}
      </div>
      <textarea
        value={txt}
        onChange={(e) => {
          const v = e.target.value;
          setTxt(v);
          const enter = enterPending.current;
          enterPending.current = false;
          // 空にしただけでは確定しない（台本の文に戻って欄が埋まり、打ち直せなくなる）。空での確定は blur・Ctrl+Enter だけ
          if (!normSubText(v).trim()) cancelWait();
          else if (enter) commit(v);
          else if (!composing.current) later(v);
        }}
        // 変換中は待ちのタイマーだけ止める。変換前の編集は compositionend の確定か、key の切り替えで流れる
        onCompositionStart={() => { composing.current = true; if (timer.current) clearTimeout(timer.current); timer.current = null; }}
        onCompositionEnd={(e) => { composing.current = false; commit(e.currentTarget.value); }}
        onBlur={() => commit(txt)}
        onKeyDown={(e) => {
          const m = subEditKey({ key: e.key, ctrlKey: e.ctrlKey, isComposing: e.nativeEvent.isComposing, keyCode: e.keyCode });
          if (m === "now") { e.preventDefault(); commit(txt); }
          else if (m === "after-input") enterPending.current = true;
        }}
        rows={Math.max(2, txt.split("\n").length + 1)}
        style={{ width: "100%", fontSize: 14, lineHeight: 1.6, marginTop: 4 }}
        placeholder={it.text}
        title="Enter で改行します。改行・変換の確定のたびと、入力が止まって 2 秒後にモニターに反映します"
      />
      {!own.text && ck && ck.texts.length > 1 && (
        <div className="dim2" style={{ fontSize: 11 }}>自動改行: {ck.texts.join(" ⏎ ")}</div>
      )}
      {custom && txt.trim() && (
        <div style={{ fontSize: 11, color: "var(--warn)", lineHeight: 1.6 }}>
          台本の文と違うため「字幕だけ書き換え」になります（声は台本のまま）。台本の文を変更すると、この設定は無効になります
        </div>
      )}
      <div className="row" style={{ marginTop: 4, flexWrap: "wrap" }}>
        <button disabled={!ck || ck.texts.length < 2}
          onClick={() => { const v = ck!.texts.join("\n"); setTxt(v); commit(v); }}>自動改行を挿入</button>
        <button disabled={!own.text} onClick={() => { setTxt(it.text); patchSubLine(key, { text: null }); }}>台本の文に戻す</button>
        <span className="grow" />
        <button className="danger" disabled={!Object.keys(own).length} onClick={() => patchSubLine(key, null)}>字幕をリセット</button>
      </div>
      <FontSelect value={own.font ?? ""} inherit={`${it.sp} の設定のまま（${fontName(base.font, useS.getState().fonts)}）`}
        onChange={(v) => patchSubLine(key, { font: v })} />
      <Num label="サイズ" value={style.size} def={base.size} step={1} unit="px" min={12} max={160} onCommit={(v) => patchSubLine(key, { size: v })} />
      <div className="row" style={{ marginTop: 2 }}>
        <span className="dim2" style={{ fontSize: 11 }}>色・縁・位置は {it.sp} または共通の設定で変更します</span>
        <span className="grow" />
        <button onClick={() => useS.setState({ selection: { kind: "subs", sp: it.sp } })}>{it.sp} の字幕の見た目…</button>
      </div>
    </>
  );
}

function FxInspector({ g }: { g: Group }) {
  const o: Override = useS((s) => s.overrides[g.key]) ?? {};
  const desc = useS((s) => s.desc)!;
  const guiClips = useS((s) => s.guiClips);
  const [ask, setAsk] = useState("");
  const set = (k: keyof Override) => (v: any) => patchOverride(g.key, { [k]: v });
  const [a, b] = groupSpan(g, o);
  const gui = g.key.startsWith("gui:") ? guiClips.find((c) => "gui:" + c.id === g.key) : null;
  return (
    <div className="insp">
      <h3>{g.label}<span className="dim2 mono" style={{ fontSize: 11 }}>{g.key}</span></h3>
      <div className="row dim" style={{ fontSize: 12, flexWrap: "wrap" }}>
        <span className="ui">{tc(a)} – {tc(b)}（{(b - a).toFixed(2)}s）</span>
        <span>{g.kinds.join("・")}</span>
        {g.se.length > 0 && <span>効果音 {g.se.length}</span>}
      </div>
      <div className="row" style={{ marginTop: 8 }}>
        <button onClick={() => player.seek(a + 0.05)}>開始位置へ</button>
        <button onClick={() => player.seek(Math.max(a, b - 0.4))}>終了位置へ</button>
        {!gui && <button onClick={() => call("open_in_editor", { path: desc.path, line: g.line })}>コードを開く（{g.line} 行目）</button>}
        {isRecGroup(g) && <button className="primary" onClick={() => useS.setState({ nestKey: g.key })} title="タイムラインでダブルクリックしても開けます">録画を差し込む</button>}
        <span className="grow" />
        <button className="danger" onClick={() => patchOverride(g.key, null)} disabled={!Object.keys(o).length}>リセット</button>
      </div>

      <div className="section">TIMING</div>
      <Num label="オフセット" value={o.shift} def={0} step={0.033} unit="秒" onCommit={set("shift")} min={-30} max={30} />
      <Num label="長さ ±" value={o.end} def={0} step={0.033} unit="秒" onCommit={set("end")} min={-30} max={60} />

      <div className="section">TRANSFORM</div>
      <Num2 label="位置 ±" unit="px" a={{ tag: "横", value: o.dx, def: 0, onCommit: set("dx") }} b={{ tag: "縦", value: o.dy, def: 0, onCommit: set("dy") }} />
      <Num label="スケール" value={o.scale} def={1} step={0.01} unit="倍" onCommit={set("scale")} min={0.1} max={4} />
      <Num label="不透明度" value={o.opacity} def={1} step={0.01} unit="" onCommit={set("opacity")} min={0} max={1} />
      <Num label="重なり順" value={o.z} def={g.z} step={1} unit={`元${g.z}`} onCommit={set("z")} min={-100} max={1000} />
      <div className="field">
        <label>非表示</label>
        <input type="checkbox" checked={!!o.hidden} onChange={(e) => set("hidden")(e.target.checked || null)} />
        <span />
      </div>
      <div className="field">
        <label>メモ</label>
        <input value={o.note ?? ""} onChange={(e) => set("note")(e.target.value || null)} placeholder="（Claude にも表示されます）" style={{ gridColumn: "2 / 4" }} />
      </div>
      <div className="dim2" style={{ fontSize: 11, lineHeight: 1.7 }}>
        モニターでは枠のドラッグで移動、右下の角で拡大縮小できます。タイムラインではクリップのドラッグで移動、端で長さを変更できます。
        調整は fx/studio_overrides.json に保存され、コードは書き換えません。
      </div>

      {gui && <GuiClipEditor clip={gui} />}

      {!gui && (
        <>
          <div className="section">SOURCE</div>
          <div className="src">{g.src || "（コードの場所が不明です）"}</div>
        </>
      )}

      <div className="section">ASK CLAUDE</div>
      <textarea
        value={ask}
        onChange={(e) => setAsk(e.target.value)}
        placeholder="このクリップをどうしたいか（例: 数字が出るタイミングを「24%」の発話に合わせたい）"
        style={{ width: "100%", minHeight: 60 }}
        onKeyDown={(e) => {
          if (e.key === "Enter" && e.ctrlKey && ask.trim()) {
            sendChat(ask.trim(), { kind: "clip_note", clip: g.key });
            setAsk("");
          }
        }}
      />
      <div className="row" style={{ justifyContent: "flex-end", marginTop: 4 }}>
        <button
          className="primary"
          disabled={!ask.trim()}
          onClick={() => {
            sendChat(ask.trim(), { kind: "clip_note", clip: g.key });
            setAsk("");
            toast("Claude に送りました", "ok");
          }}
        >
          送信（Ctrl+Enter）
        </button>
      </div>
    </div>
  );
}

function GuiClipEditor({ clip }: { clip: any }) {
  const [p, setP] = useState<any>(clip.params);
  useEffect(() => setP(clip.params), [clip]);
  const save = (np: any) => {
    const clips = useS.getState().guiClips.map((c) => (c.id === clip.id ? { ...c, params: np } : c));
    call("set_gui_clips", { clips });
  };
  const F = (k: string, label: string, type = "text") => (
    <div className="field">
      <label>{label}</label>
      <input
        type={type}
        value={p[k] ?? ""}
        onChange={(e) => setP({ ...p, [k]: type === "number" ? Number(e.target.value) : e.target.value })}
        onBlur={() => save(p)}
        style={{ gridColumn: "2 / 4" }}
      />
    </div>
  );
  return (
    <>
      <div className="section">CONTENT（画面で追加した演出）</div>
      {clip.type === "telop" && <>{F("text", "文字")}{F("sub", "2行目")}{F("tag", "見出し")}{F("size", "サイズ", "number")}{F("color", "色")}{F("accent", "枠の色")}{F("cx", "中心 x", "number")}{F("cy", "中心 y", "number")}</>}
      {clip.type === "note" && <>{F("text", "注釈")}</>}
      {clip.type === "stamp" && <>{F("text", "文字")}{F("color", "色")}{F("size", "サイズ", "number")}{F("angle", "角度", "number")}{F("cx", "中心 x", "number")}{F("cy", "中心 y", "number")}</>}
      {clip.type === "chips" && (
        <div className="field">
          <label>キーワード</label>
          <input value={(p.words || []).join("、")} onChange={(e) => setP({ ...p, words: e.target.value.split(/[、,]/).filter(Boolean) })} onBlur={() => save(p)} style={{ gridColumn: "2 / 4" }} />
        </div>
      )}
      {clip.type === "image" && <>{F("path", "画像")}{F("x", "x", "number")}{F("y", "y", "number")}{F("scale", "スケール", "number")}</>}
      <div className="row" style={{ marginTop: 6 }}>
        <span className="dim2" style={{ fontSize: 11 }}>色は cyan / amber / red / gold / green / #rrggbb で指定します。変更すると演出を再読み込みします（数秒）</span>
        <span className="grow" />
        <button className="danger" onClick={() => call("set_gui_clips", { clips: useS.getState().guiClips.filter((c) => c.id !== clip.id) })}>削除</button>
      </div>
    </>
  );
}

/** 入り・抜けのトランジション（素材クリップ。本体のトラックとネストで共通）。onSwitch があれば「前のクリップから切り替え」も出す */
export function TransFields({ c, edit, onSwitch }: {
  c: MediaClip; edit: (fn: (cc: MediaClip) => void) => void; onSwitch?: (kind: string, dur: number) => void;
}) {
  const side = (s: "in" | "out", label: string) => {
    const tr = s === "in" ? c.trans_in : c.trans_out;
    const help = TRANS_KINDS.find((k) => k.k === tr?.kind)?.help;
    return (
      <>
        <div className="field">
          <label>{label}</label>
          <select value={tr?.kind ?? ""} title={help} onChange={(e) => edit((cc) => setTrans(cc, s, { kind: e.target.value }))} style={{ gridColumn: "2 / 4" }}>
            <option value="">（なし）</option>
            {TRANS_KINDS.map((k) => <option key={k.k} value={k.k} title={k.help}>{k.label}</option>)}
          </select>
        </div>
        {tr && (
          <Num label="長さ" value={tr.dur > 0 ? tr.dur : TRANS_DEFAULT_DUR} def={-1e9} step={0.05} unit="秒" min={0.05} max={3}
            onCommit={(v) => edit((cc) => setTrans(cc, s, { dur: v ?? TRANS_DEFAULT_DUR }))} />
        )}
        {tr && transHasDir(tr.kind) && (
          <div className="field">
            <label>{tr.kind === "wipe" ? "走り出す辺" : s === "in" ? "来る側" : "去る側"}</label>
            <select value={tr.dir || "left"} onChange={(e) => edit((cc) => setTrans(cc, s, { dir: e.target.value }))} style={{ gridColumn: "2 / 4" }}>
              {TRANS_DIRS.map((d) => <option key={d.k} value={d.k}>{d.label}</option>)}
            </select>
          </div>
        )}
      </>
    );
  };
  return (
    <>
      <div className="section">TRANSITION</div>
      {side("in", "入り")}
      {side("out", "抜け")}
      {onSwitch && (
        <div className="row" style={{ flexWrap: "wrap", marginTop: 4 }}>
          <button
            title="前のクリップと切れ目で重ね、前の「抜け」とこの「入り」に同じトランジションを付けます（入りがなしならフェード）"
            onClick={() => onSwitch(c.trans_in?.kind || "fade", c.trans_in && c.trans_in.dur > 0 ? c.trans_in.dur : TRANS_DEFAULT_DUR)}
          >
            前のクリップから切り替え
          </button>
        </div>
      )}
      <div className="dim2" style={{ fontSize: 11 }}>フェードイン・アウトとは別に掛かります。プレビューと mp4 に反映されます（演出の透過 .mov には入りません）</div>
    </>
  );
}

export function MediaInspector({ trackId, c }: { trackId: string; c: MediaClip }) {
  const doc = useS((s) => s.project!.doc);
  const layout = useS((s) => s.layout);
  const speakers = [...new Set(["トーク1", "トーク2", ...layout.map((x) => x.sp)])];
  const set = (k: keyof MediaClip) => (v: any) =>
    editDoc((d) => {
      const cc = d.tracks.find((x) => x.id === trackId)?.clips.find((x) => x.id === c.id);
      if (cc) (cc as any)[k] = v;
    });
  const isAudio = isAud(c.src);
  const isImg = isImgFile(c.src);
  return (
    <div className="insp">
      <h3>{c.name}</h3>
      <div className="src">{c.src}</div>
      <div className="row" style={{ marginTop: 8 }}>
        <button onClick={() => player.seek(c.start)}>開始位置へ</button>
        <button onClick={() => call("reveal", { path: c.src })}>エクスプローラーで表示</button>
        <span className="grow" />
        <button className="danger" onClick={() => editDoc((d) => { const t = d.tracks.find((x) => x.id === trackId)!; t.clips = t.clips.filter((x) => x.id !== c.id); useS.setState({ selection: null }); })}>タイムラインから削除</button>
      </div>
      <div className="section">TIME</div>
      <Num label="開始" value={c.start} def={-1e9} step={0.033} unit="秒" onCommit={(v) => set("start")(v ?? 0)} min={0} max={36000} />
      <Num label="長さ" value={c.dur} def={-1e9} step={0.033} unit="秒" onCommit={(v) => set("dur")(v ?? 1)} min={0.05} max={36000} />
      {!isImg && <Num label="素材の開始" value={c.src_in} def={0} step={0.033} unit="秒" onCommit={(v) => set("src_in")(v ?? 0)} min={0} max={36000} />}
      {!isImg && (
        <div className="field"><label>ループ</label><input type="checkbox" checked={c.loop} onChange={(e) => set("loop")(e.target.checked)} /><span /></div>
      )}
      <Num label="フェードイン" value={c.fade_in} def={0} step={0.05} unit="秒" onCommit={(v) => set("fade_in")(v ?? 0)} min={0} max={10} />
      <Num label="フェードアウト" value={c.fade_out} def={0} step={0.05} unit="秒" onCommit={(v) => set("fade_out")(v ?? 0)} min={0} max={10} />
      {isAudio ? (
        <>
          <div className="section">AUDIO</div>
          <Num label="音量" value={c.volume_db} def={0} step={0.5} unit="dB" onCommit={(v) => set("volume_db")(v ?? 0)} min={-60} max={20} />
          <div className="dim2" style={{ fontSize: 11 }}>トラック音量（{doc.tracks.find((x) => x.id === trackId)?.volume_db} dB）に加算されます。BGM は声より 15〜20dB 小さくするのが目安です（bgm/BGM_LIST.md）</div>
          <Num label="トラック音量" value={doc.tracks.find((x) => x.id === trackId)?.volume_db} def={0} step={0.5} unit="dB" min={-60} max={20}
            onCommit={(v) => editDoc((d) => { d.tracks.find((x) => x.id === trackId)!.volume_db = v ?? 0; })} />
        </>
      ) : (
        <>
          <div className="section">MOTION</div>
          <Num2 label="位置" unit="px" min={-2000} max={4000}
            a={{ tag: "x", value: c.x, def: 960, onCommit: (v) => set("x")(v ?? 960) }}
            b={{ tag: "y", value: c.y, def: 540, onCommit: (v) => set("y")(v ?? 540) }} />
          <Num label="スケール" value={c.scale} def={-1e9} step={0.005} unit="倍" onCommit={(v) => set("scale")(v ?? 1)} min={0.01} max={8} />
          <Num label="不透明度" value={c.opacity} def={-1e9} step={0.01} onCommit={(v) => set("opacity")(v ?? 1)} min={0} max={1} />
          <div className="dim2 ui" style={{ fontSize: 11 }}>Premiere: 位置 ({Math.round(c.x)}, {Math.round(c.y)})・スケール {(c.scale * 100).toFixed(3)}%</div>
          <TransFields
            c={c}
            edit={(fn) => editDoc((d) => {
              const cc = d.tracks.find((x) => x.id === trackId)?.clips.find((x) => x.id === c.id);
              if (cc) fn(cc);
            })}
            onSwitch={(kind, dur) => {
              const tr = doc.tracks.find((x) => x.id === trackId);
              if (!tr || !prevClip(tr.clips, c)) return toast("同じトラックの前にクリップがありません", "error");
              editDoc((d) => { linkSwitch(d.tracks.find((x) => x.id === trackId)!.clips, c.id, kind, dur); });
            }}
          />
          <CropFields c={c} set={set}
            reset={() => editDoc((d) => {
              const cc = d.tracks.find((x) => x.id === trackId)?.clips.find((x) => x.id === c.id);
              if (cc) { cc.crop_l = 0; cc.crop_t = 0; cc.crop_r = 0; cc.crop_b = 0; cc.feather = 0; }
            })}>
            <button
              title="右端が x = 600（立ち絵の領域の境目）になるように右側をクロップします"
              onClick={async () => {
                const sz = await sizeOf(c.src);
                const w = sz.w * c.scale;
                const right = c.x + w / 2;
                set("crop_r")(Math.round(Math.max(0, Math.min(100, ((right - 600) / w) * 100)) * 10) / 10);
              }}
            >
              立ち絵の領域に合わせる
            </button>
          </CropFields>
          {isImg && (
            <>
              <div className="field"><label>左右反転</label><input type="checkbox" checked={c.flip} onChange={(e) => set("flip")(e.target.checked)} /><span /></div>
              <div className="field">
                <label>話者</label>
                <select value={c.speaker} onChange={(e) => set("speaker")(e.target.value)} style={{ gridColumn: "2 / 4" }}>
                  <option value="">（なし）</option>
                  {speakers.map((s) => <option key={s} value={s}>{s}</option>)}
                </select>
              </div>
              <div className="dim2" style={{ fontSize: 11 }}>話者を選ぶと、その人が話していないときに少し暗くなります（掛け合いの立ち絵用）</div>
            </>
          )}
        </>
      )}
    </div>
  );
}

/** 攻略動画の素材の音の設定（タイムラインの「素材の音」の行の ⚙）。値は回の project.json に残り、モニターと書き出しの両方に効く */
function GuideAudioInspector() {
  const doc = useS((s) => s.project?.doc);
  const guide = useS((s) => s.guide);
  if (!doc) return <div className="empty">回を開いてください</div>;
  const ga = guideAudioRow(guide?.audio_rev, guide?.warnings ?? [], doc.guide_audio_muted, guide?.audio_err);
  return (
    <div className="insp">
      <h3>素材の音<span className="dim2" style={{ fontSize: 12 }}>攻略動画の通しの素材の音</span></h3>
      <div className="dim2" style={{ fontSize: 11, lineHeight: 1.7, margin: "4px 0 6px" }}>
        通しの素材（戦闘画面）の音を、映像と同じ並び（カットのクロスフェードも合わせて）にしたものです。
        モニターの再生と完成版 mp4・音声ミックス wav に入ります。補足素材の小窓の音は入りません。
      </div>
      {doc.guide_audio_muted && (
        <div className="se-warn">
          素材の音はミュート中です
          <button style={{ marginLeft: 6 }} onClick={() => editDoc((d) => { d.guide_audio_muted = false; })}>ミュート解除</button>
        </div>
      )}
      <Num label="素材の音" value={doc.guide_audio_db} def={0} step={0.5} unit="dB" min={-40} max={20} onCommit={(v) => editDoc((d) => { d.guide_audio_db = v ?? 0; })} />
      <div className="dim2 ui" style={{ fontSize: 11 }}>{ga.empty ? ga.title : "素材の音を作りました。再生と書き出しに入ります"}</div>
    </div>
  );
}

function ProjectInspector() {
  const doc = useS((s) => s.project?.doc);
  const t = useS((s) => s.t);
  const guiClips = useS((s) => s.guiClips);
  const subView = useS((s) => s.subView);
  const subViewErr = useS((s) => s.subViewErr);
  if (!doc) return <div className="empty">回を開いてください</div>;
  const addGui = (type: string, params: any, dur = 3) => {
    const id = nid("g");
    const clips = [...guiClips, { id, type, start: { t: Math.round(t * 1000) / 1000 }, end: { t: Math.round((t + dur) * 1000) / 1000 }, params }];
    call("set_gui_clips", { clips });
    setTimeout(() => useS.setState({ selection: { kind: "fx", key: "gui:" + id } }), 4000);
    toast("演出を追加しました（再読み込み中）", "ok");
  };
  return (
    <div className="insp">
      <h3>この回の設定</h3>
      <div className="section">AUDIO</div>
      <Num label="声の音量" value={doc.voice_db} def={0} step={0.5} unit="dB" min={-30} max={20} onCommit={(v) => editDoc((d) => { d.voice_db = v ?? 0; })} />
      <Num label="効果音" value={doc.se_db} def={0} step={0.5} unit="dB" min={-40} max={20} onCommit={(v) => editDoc((d) => { d.se_db = v ?? 0; })} />
      <div className="section">PREMIERE</div>
      <Num label="配置位置" value={doc.premiere_t0} def={0} step={0.01} unit="秒" min={0} max={600} onCommit={(v) => editDoc((d) => { d.premiere_t0 = v ?? 0; })} />
      <div className="dim2 ui" style={{ fontSize: 11 }}>= {premiereTc(doc.premiere_t0, doc.premiere_fps)}（イントロの長さ。Premiere のタイムコード表示に使います）</div>
      <Num label="fps" value={doc.premiere_fps} def={60} step={1} min={1} max={240} onCommit={(v) => editDoc((d) => { d.premiere_fps = v ?? 60; })} />
      <div className="section">SUBTITLE</div>
      <div className="row" style={{ flexWrap: "wrap" }}>
        <span className="dim" style={{ fontSize: 12 }}>{subView ? `${fontName(subView.track.font, useS.getState().fonts)}・${subView.track.size}px` : subViewErr ? "（取得できませんでした）" : "（読み込み中…）"}{doc.subtitle?.hidden ? "・非表示" : ""}</span>
        <span className="grow" />
        <button onClick={() => useS.setState({ selection: { kind: "subs" } })}>字幕の見た目…</button>
      </div>
      <div className="section">ADD（再生位置に簡単な演出を追加）</div>
      <div className="row" style={{ flexWrap: "wrap" }}>
        <button onClick={() => addGui("telop", { text: "ここに文字", tag: "KEY POINT", size: 64, color: "ice", cy: 450 })}>テロップ</button>
        <button onClick={() => addGui("note", { text: "※注釈" })}>注釈</button>
        <button onClick={() => addGui("stamp", { text: "おすすめ", color: "red", cx: 1500, cy: 300 })}>ハンコ</button>
        <button onClick={() => addGui("chips", { words: ["キーワード", "もう1つ"], cy: 440 })}>キーワード</button>
      </div>
      <div className="dim2" style={{ fontSize: 11, lineHeight: 1.7, marginTop: 8 }}>
        画面で追加した演出は fx/studio_clips.json に保存されます（fxparts の telop・note・stamp・keyword_chips）。
        凝った演出は Claude に頼むと、演出のコードとして作ってくれます。
      </div>
      <div className="section">KEYS</div>
      <div className="dim" style={{ fontSize: 12, lineHeight: 2 }}>
        <span className="kbd">Space</span> 再生　<span className="kbd">J K L</span>　<span className="kbd">← →</span> 1コマ（Shift で 1 秒）　<span className="kbd">↑ ↓</span> 前後のセリフ・シーン<br />
        <span className="kbd">I</span> <span className="kbd">O</span> イン・アウト　<span className="kbd">X</span> 解除　<span className="kbd">Enter</span> レンダリング　<span className="kbd">Delete</span> 選んだ演出を非表示<br />
        <span className="kbd">Ctrl+Z</span> 元に戻す　<span className="kbd">Alt+ホイール</span> ズーム　<span className="kbd">Alt</span>+ドラッグ スナップなし<br />
        RecWindow（録画の窓）をダブルクリックで、録画を差し込む画面（ネスト）
      </div>
    </div>
  );
}

function LineInspector({ id }: { id: string }) {
  const it = useS((s) => s.layout.find((x) => x.line_id === id));
  if (!it) return <ProjectInspector />;
  return (
    <div className="insp">
      <h3>セリフ {it.n}<span className="dim2" style={{ fontSize: 12 }}>{it.sp}・{it.block}</span></h3>
      <div style={{ fontSize: 14, lineHeight: 1.7, margin: "6px 0" }}>{it.text}</div>
      <div className="dim ui">{tc(it.start)} – {tc(it.start + it.dur)}（{it.dur.toFixed(2)}s）{it.audio ? "" : "・音声なし（長さは推定）"}</div>
      <div className="row" style={{ marginTop: 8 }}>
        <button onClick={() => player.seek(it.start)}>開始位置へ</button>
        <button onClick={() => sendChat(`行 ${it.n} の言い回しを推敲してください`, { kind: "line_review", lines: [it.n] })}>Claude に推敲を頼む</button>
      </div>
      <LineSubtitle it={it} />
    </div>
  );
}

/** 左の「素材調整」タブ：選んでいる素材（タイムラインの素材トラックのクリップ）の調整 */
export function MediaAdjust() {
  const selection = useS((s) => s.selection);
  const doc = useS((s) => s.project?.doc);
  if (selection?.kind === "media" && doc) {
    const tr = doc.tracks.find((x) => x.id === selection.track);
    const c = tr?.clips.find((x) => x.id === selection.clip);
    if (tr && c) return <MediaInspector trackId={tr.id} c={c} />;
  }
  return <div className="empty">タイムラインかモニターで素材（立ち絵・録画・BGM・画像）を選ぶと、ここで調整できます</div>;
}

export default function Inspector() {
  const selection = useS((s) => s.selection);
  const desc = useS((s) => s.desc);
  // 素材（media）の調整は左の「素材調整」タブ（MediaAdjust）。こちらは全体の設定を出す
  if (selection?.kind === "fx") {
    const g = desc?.groups.find((x) => x.key === selection.key);
    if (g) return <FxInspector g={g} />;
  }
  if (selection?.kind === "line") return <LineInspector id={selection.id} />;
  if (selection?.kind === "subs") return <SubtitleInspector sp={selection.sp} />;
  if (selection?.kind === "se") return <SeInspector focus={selection.se} />;
  if (selection?.kind === "guide_audio") return <GuideAudioInspector />;
  return <ProjectInspector />;
}
