// 読み上げ辞書（アプリのデータフォルダの voice_dict.json、動画をまたいで共通）。台本の文字を選んで右クリック →「辞書に登録」。
// 昔の台本エディタの「辞書に登録」と同じ決まり（表記・読み（カタカナ）・アクセント（モーラごとの h / l、省くと声のエンジンに任せる））。
import React, { useEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { create } from "zustand";
import { editScript, synthLine, toast, toastError, useS, voiceApi } from "../state";
import { player } from "../io/player";
import { akf, call } from "../io/api";
import { findLine, setVoice, spVoice, subText, ttsText, voicedLines, voiceOf } from "../logic/voice";
import { engineOf, has } from "../logic/voiceengine";
import { canRuby, wrapRuby } from "../logic/ruby";
import { Dialog } from "./TopBar";
import { cleanReading, flatHl, guessReading, moras, savedAccent, setMora, upsertEntry, type DictEntry } from "../logic/dict";

interface DictState {
  entries: DictEntry[] | null;
  menu: { x: number; y: number; surface: string; lineId: string; ruby: boolean; ta: HTMLTextAreaElement; start: number; end: number; text: string } | null;
  edit: { surface: string; reading: string; accent: string; lineId: string | null } | null;
  list: boolean;
}
export const useDict = create<DictState>(() => ({ entries: null, menu: null, edit: null, list: false }));

export async function loadDict(force = false) {
  if (useDict.getState().entries && !force) return useDict.getState().entries!;
  const r = await voiceApi<{ entries: DictEntry[] }>("/api/dict");
  useDict.setState({ entries: r.entries || [] });
  return r.entries || [];
}

/** 声のエンジンの解析（その行の analyzed_text）から、選んだ表記にあたる語の読みとアクセントを拾う（見つからなければ空） */
function guessFromAnalysis(lineId: string, surface: string): { reading: string; accent: string } {
  const st = useS.getState();
  const l = st.script && voicedLines(st.script).find((x) => x.l.id === lineId)?.l;
  const key = st.layout.find((x) => x.line_id === lineId)?.key;
  const xml = l && (voiceOf(st.script!, l) || (key ? st.analysis[key]?.analyzed_text : undefined));
  if (!xml) return { reading: "", accent: "" };
  const doc = new DOMParser().parseFromString(xml, "text/xml");
  const words = Array.from(doc.getElementsByTagName("word")).map((w) => ({
    orig: w.getAttribute("original") || "",
    pron: w.getAttribute("pronunciation") || "",
    hl: w.getAttribute("hl") || "",
  }));
  return guessReading(words, surface);
}

export async function openDict(surface: string, lineId: string | null) {
  useDict.setState({ menu: null });
  let entries: DictEntry[];
  try {
    entries = await loadDict(true);
  } catch (e) {
    toastError("辞書を読み込めませんでした", e, 8000);
    return;
  }
  const e = entries.find((x) => x.surface === surface);
  const g = !e && lineId ? guessFromAnalysis(lineId, surface) : { reading: "", accent: "" };
  useDict.setState({ edit: { surface, reading: e ? e.reading : g.reading, accent: e ? e.accent ?? "" : g.accent, lineId } });
}

/** 台本の textarea の右クリック。文字を選んでいなければ普通のメニューのまま */
export function dictContextMenu(e: React.MouseEvent<HTMLTextAreaElement>, lineId: string) {
  const t = e.currentTarget;
  const surface = subText(t.value.slice(t.selectionStart, t.selectionEnd)).replace(/[｜《》]/g, "").trim();
  if (!surface) return;
  e.preventDefault();
  // ルビ（｜漢字《よみ》）の中の字は読み上げの文に無いので、この行には辞書が効かない
  const ruby = !ttsText(t.value).includes(surface);
  useDict.setState({ menu: { x: e.clientX, y: e.clientY, surface, lineId, ruby, ta: t, start: t.selectionStart, end: t.selectionEnd, text: t.value.slice(t.selectionStart, t.selectionEnd) } });
  loadDict().catch((e) => toastError("辞書を読み込めませんでした", e));
}

// 辞書を変えたら、その言葉を含む行の音声を作り直す（手で直したアクセントは辞書を当てる前の解析なので外す。元に戻すで戻せる）
async function afterChange(surface: string) {
  const st = useS.getState();
  if (!st.script || !surface) return;
  const hit = voicedLines(st.script).map((x) => x.l).filter((l) => ttsText(l.text).includes(surface));
  const reset = hit.filter((l) => voiceOf(st.script!, l) != null).map((l) => l.id);
  if (reset.length) editScript((s) => voicedLines(s).forEach((x) => reset.includes(x.l.id) && setVoice(s, x.l, null)));
  // 解析は Rust が持つ。消してから作り直す（画面の写しも消す）
  const keys = st.layout.filter((it) => hit.some((l) => l.id === it.line_id)).map((it) => it.key);
  try {
    await call("voice_forget", { keys });
  } catch (e) {
    toastError("解析を消去できませんでした", e, 8000);
  }
  useS.setState((s) => {
    const an = { ...s.analysis };
    for (const k of keys) delete an[k];
    return { analysis: an };
  });
  toast(
    `辞書を保存しました（${surface}）。この言葉を含む ${hit.length} 行の音声を作り直します` +
      (reset.length ? `。そのうち ${reset.length} 行のアクセントの手直しは外しました（Ctrl+Z で戻せます）` : ""),
  );
  let failed = 0;
  for (const id of hit.map((l) => l.id)) {
    try {
      await synthLine(id);
    } catch (e) {
      // 1 行失敗しても残りは作る（途中で裏の解析が古い音声を書き戻していても、作り直せば上書きされる）
      if (!failed++) toastError("音声を作成できませんでした", e, 8000);
    }
  }
  if (failed > 1) toast(`${failed} 行の音声を作れませんでした`, "error", 8000);
}

async function saveEntries(entries: DictEntry[]) {
  const r = await voiceApi<{ entries: DictEntry[] }>("/api/dict", { entries });
  useDict.setState({ entries: r.entries || [] });
}

/** 選んだ字を ｜字《》 にして、カーソルを《》の間へ（元に戻すで 1 回ぶん） */
function rubySelection(m: NonNullable<DictState["menu"]>) {
  useDict.setState({ menu: null });
  const cur = findLine(useS.getState().script!, m.lineId)?.l.text ?? m.ta.value;
  const w = wrapRuby(cur, m.start, m.end);
  if (!w) return;
  editScript((s) => { findLine(s, m.lineId)!.l.text = w.value; }, "own");
  // 入れ直しのあとで、欄にカーソルを置く
  setTimeout(() => {
    const el = document.querySelector<HTMLTextAreaElement>(`[data-line="${m.lineId}"] textarea`) ?? m.ta;
    el.focus();
    el.setSelectionRange(w.caret, w.caret);
  }, 0);
}

function DictMenu() {
  const menu = useDict((s) => s.menu);
  const entries = useDict((s) => s.entries);
  useEffect(() => {
    if (!menu) return;
    const close = () => useDict.setState({ menu: null });
    const key = (e: KeyboardEvent) => e.key === "Escape" && close();
    // 選びが消えた（欄を離れた・選び直した）らメニューも閉じる
    const sel = () => {
      const t = menu.ta;
      if (document.activeElement !== t || t.selectionStart === t.selectionEnd) close();
    };
    const down = (e: MouseEvent) => {
      if (!(e.target as Element | null)?.closest?.(".dict-menu")) close();
    };
    window.addEventListener("keydown", key);
    window.addEventListener("scroll", close, true);
    document.addEventListener("selectionchange", sel);
    document.addEventListener("mousedown", down, true);
    return () => {
      window.removeEventListener("keydown", key);
      window.removeEventListener("scroll", close, true);
      document.removeEventListener("selectionchange", sel);
      document.removeEventListener("mousedown", down, true);
    };
  }, [menu]);
  if (!menu) return null;
  const has = !!entries?.some((x) => x.surface === menu.surface);
  const left = Math.min(menu.x, window.innerWidth - 300);
  const top = Math.min(menu.y, window.innerHeight - 110);
  return (
    <div className="menu dict-menu" style={{ left, top, position: "fixed" }} onMouseDown={(e) => e.preventDefault()}>
      {canRuby(menu.text) && (
        <div className="mi" onClick={() => rubySelection(menu)}>
          読みを指定
          <small>｜選んだ字《よみ》にする（字幕はそのまま、声のエンジンにだけ読みを渡す）</small>
        </div>
      )}
      <div className="mi" onClick={() => openDict(menu.surface, menu.lineId)}>
        {has ? `「${menu.surface}」の辞書を編集…` : `「${menu.surface}」を辞書に登録…`}
        <small>{menu.ruby ? "この行ではルビの読みが使われます（辞書はほかの行に効きます）" : "読み・アクセント（voice_dict.json、動画をまたいで共通）"}</small>
      </div>
      <div className="mi" onClick={() => { useDict.setState({ menu: null, list: true }); loadDict(true).catch((e) => toastError("辞書を読み込めませんでした", e)); }}>
        辞書の一覧
      </div>
    </div>
  );
}

function DictEditor() {
  const edit = useDict((s) => s.edit);
  const entries = useDict((s) => s.entries);
  const [surface, setSurface] = useState("");
  const [reading, setReading] = useState("");
  const canAccent = has(engineOf(useS((s) => s.voice?.info)), "accent");
  const [useAcc, setUseAcc] = useState(true);
  const [accent, setAccent] = useState("");
  const [busy, setBusy] = useState(false);
  const readRef = useRef<HTMLInputElement>(null);
  const exists = !!edit && !!entries?.some((x) => x.surface === edit.surface);

  useEffect(() => {
    if (!edit) return;
    const e = entries?.find((x) => x.surface === edit.surface);
    setSurface(edit.surface);
    setReading(edit.reading);
    setUseAcc(e ? !!e.accent : true);
    setAccent(edit.accent);
    setTimeout(() => readRef.current?.focus(), 0);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [edit]);
  if (!edit) return null;

  const ms = moras(reading);
  const acc = accent.length === ms.length ? accent : flatHl(ms.length);
  const close = () => useDict.setState({ edit: null });

  const build = (): DictEntry | null => {
    const sf = surface.trim();
    const rd = cleanReading(reading);
    if (!sf || !rd) {
      toast("表記と読みを入れてください", "error");
      return null;
    }
    const entry: DictEntry = { surface: sf, reading: rd };
    // アクセントを扱えないエンジンで保存しても、共通の辞書にある既存のアクセントは残す
    const accent = savedAccent(entries?.find((x) => x.surface === (edit?.surface ?? sf)), rd, { canAccent, useAcc, acc });
    if (accent) entry.accent = accent;
    return entry;
  };
  // 保存せずに、この言葉だけを今の入力で作って鳴らす（右クリックした行の話者の声。なければ トーク1）
  const tryListen = async () => {
    const entry = build();
    if (!entry) return;
    const st = useS.getState();
    const line = edit.lineId && st.script ? findLine(st.script, edit.lineId)?.l : null;
    const speaker = line?.sp || "トーク1";
    setBusy(true);
    try {
      const r = await voiceApi<{ id: string }>("/api/synth", {
        speaker,
        text: entry.surface,
        analyzed_text: null,
        params: null,
        speaker_params: (st.script && spVoice(st.script, speaker)) ?? {},
        dict_extra: [entry],
      });
      player.pause();
      player.playOne(akf(`/audio/${r.id}.wav`));
    } catch (e) {
      toastError("音声を作成できませんでした", e, 8000);
    }
    setBusy(false);
  };
  const commit = async () => {
    const entry = build();
    if (!entry) return;
    const sf = entry.surface;
    setBusy(true);
    try {
      await saveEntries(upsertEntry(await loadDict(true), entry, edit.surface));
    } catch (e) {
      setBusy(false);
      return toastError("辞書を保存できませんでした", e, 8000);
    }
    setBusy(false);
    close();
    // 表記を変えたときは、元の表記を含む行も作り直す（同時に走らせないよう順に）
    void (async () => {
      if (edit.surface && edit.surface !== sf) await afterChange(edit.surface);
      await afterChange(sf);
    })();
  };
  const remove = async () => {
    if (!confirm(`「${edit.surface}」を辞書から削除しますか？`)) return;
    try {
      await saveEntries((await loadDict(true)).filter((x) => x.surface !== edit.surface));
    } catch (e) {
      return toastError("辞書を保存できませんでした", e, 8000);
    }
    close();
    void afterChange(edit.surface);
  };

  return (
    <Dialog title={exists ? "辞書を編集" : "辞書に登録"} onClose={close}>
      <div style={{ display: "grid", gridTemplateColumns: "80px 1fr", gap: "10px 12px", alignItems: "center" }}>
        <span className="dim">表記</span>
        <input value={surface} onChange={(e) => setSurface(e.target.value)} placeholder="時雨" />
        <span className="dim">読み</span>
        <input
          ref={readRef}
          value={reading}
          onChange={(e) => setReading(e.target.value)}
          onKeyDown={(e) => e.key === "Enter" && !e.nativeEvent.isComposing && !busy && commit()}
          placeholder="しぐれ（ひらがな・カタカナ）"
        />
        {canAccent && <span className="dim">アクセント</span>}
        {canAccent && <div>
          <label style={{ fontSize: 12 }}>
            <input type="checkbox" checked={useAcc} onChange={(e) => setUseAcc(e.target.checked)} /> アクセントも指定する（外すと声のエンジンに任せる）
          </label>
          <div style={{ display: "flex", gap: 4, padding: "8px 0", flexWrap: "wrap", opacity: useAcc ? 1 : 0.35, pointerEvents: useAcc ? undefined : "none" }}>
            {ms.map((m, j) => {
              const h = acc[j] === "h";
              const click = () => setAccent(setMora([...acc], j).join(""));
              const box = (on: boolean) => (
                <div
                  onClick={click}
                  title="高・低を切り替え"
                  style={{ visibility: on ? "visible" : "hidden", width: 26, height: 22, lineHeight: "22px", textAlign: "center", cursor: "pointer", fontSize: 12, borderRadius: 3,
                           background: h ? "var(--hl-on)" : "var(--hl-off)", border: "1px solid " + (h ? "var(--accent)" : "var(--line2)") }}
                >
                  {m}
                </div>
              );
              return (
                <div key={j} style={{ display: "flex", flexDirection: "column", gap: 6 }}>
                  {box(h)}
                  {box(!h)}
                </div>
              );
            })}
            {!ms.length && <span className="dim2" style={{ fontSize: 12 }}>読みを入れるとモーラが出ます</span>}
          </div>
          <div className="dim2" style={{ fontSize: 11 }}>モーラのクリックで高・低（アクセント欄と同じ決まり）</div>
        </div>}
      </div>
      <div className="row" style={{ marginTop: 16 }}>
        {exists && <button className="danger" onClick={remove} disabled={busy}>削除</button>}
        <span className="grow" />
        <button onClick={close}>キャンセル</button>
        <button onClick={tryListen} disabled={busy} title="保存せずに、この言葉だけを今の入力で鳴らす">{busy ? "…" : "試聴"}</button>
        <button className="primary" onClick={commit} disabled={busy}>保存</button>
      </div>
    </Dialog>
  );
}

function DictList() {
  const list = useDict((s) => s.list);
  const entries = useDict((s) => s.entries);
  if (!list) return null;
  const hlText = (h?: string) => (h ? [...h].map((c) => (c === "h" ? "‾" : "_")).join("") : "（声のエンジンに任せる）");
  const close = () => useDict.setState({ list: false });
  return (
    <Dialog title="読み上げ辞書（voice_dict.json、動画をまたいで共通）" onClose={close}>
      <div style={{ maxHeight: "60vh", overflow: "auto" }}>
        {entries?.length ? (
          <table style={{ width: "100%", borderCollapse: "collapse", fontSize: 13 }}>
            <thead>
              <tr className="dim" style={{ textAlign: "left" }}><th>表記</th><th>読み</th><th>アクセント</th><th /></tr>
            </thead>
            <tbody>
              {entries.map((e) => (
                <tr key={e.surface} style={{ borderTop: "1px solid var(--line)" }}>
                  <td>{e.surface}</td>
                  <td>{e.reading}</td>
                  <td className="mono" style={{ letterSpacing: 1 }}>{hlText(e.accent)}</td>
                  <td style={{ textAlign: "right" }}>
                    <button className="ghost" onClick={() => { close(); openDict(e.surface, null); }}>編集</button>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        ) : (
          <p className="dim">まだ登録がありません。台本の文字を選んで右クリック →「辞書に登録」で追加できます。</p>
        )}
      </div>
      <div className="row" style={{ marginTop: 12 }}>
        <button onClick={() => { close(); openDict("", null); }}>＋ 追加</button>
        <span className="grow" />
        <button onClick={close}>閉じる</button>
      </div>
    </Dialog>
  );
}

// 台本の欄の中に置くと、欄の外へ出られない（position: fixed が欄に閉じ込められる）ので body に出す
export default function DictUI() {
  return createPortal(
    <>
      <DictMenu />
      <DictEditor />
      <DictList />
    </>,
    document.body,
  );
}
