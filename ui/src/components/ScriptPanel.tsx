import React, { useEffect, useMemo, useRef, useState } from "react";
import { akf } from "../io/api";
import { editDoc, editScript, redo, relayout, sendChat, stopSynthAll, synthAll, synthLine, toast, toastError, undo, useS } from "../state";
import { lineKeyAction } from "../logic/linekeys";
import { player } from "../io/player";
import type { Block, Line } from "../types";
import { dropStaleAdj, findLine, nid, paramsOf, relayoutFrom, staleAdj, ttsKeyText, voiceOf } from "../logic/voice";
import { SPEAKERS, spClass, tc } from "../logic/util";
import AccentEditor from "./AccentEditor";
import ImportScriptDialog from "./ImportScriptDialog";
import DictUI, { dictContextMenu, loadDict, useDict } from "./DictDialog";

function autosize(ta: HTMLTextAreaElement | null) {
  if (!ta) return;
  ta.style.height = "auto";
  ta.style.height = ta.scrollHeight + "px";
}

const LineRow = React.memo(function LineRow({ l, b, n, speakers }: { l: Line; b: Block; n: number; speakers: string[] }) {
  const sel = useS((s) => s.selLines.includes(l.id));
  const active = useS((s) => s.selection?.kind === "line" && s.selection.id === l.id);
  const item = useS((s) => s.layout.find((x) => x.line_id === l.id));
  const playing = useS((s) => s.playing && !!item && s.t >= item.start && s.t < item.start + item.dur);
  // 今の手直しで作った音声か（Rust の並びの audio が付いているか）
  const fresh = item?.audio != null;
  const a = useS((s) => (item ? s.analysis[item.key] : undefined));
  const busy = useS((s) => s.synthBusy.has(l.id));
  const edited = useS((s) => !!s.script && (voiceOf(s.script, l) != null || paramsOf(s.script, l) != null));
  const stale = useS((s) => !!s.script && staleAdj(s.script, l));
  const prop = useS((s) => s.proposals.some((p) => p.items.some((i) => i.status === "pending" && (i.line_id === l.id || i.after_id === l.id))));
  const [acc, setAcc] = useState(false);
  const [memoOpen, setMemoOpen] = useState(false);
  const ta = useRef<HTMLTextAreaElement>(null);
  const memoTa = useRef<HTMLTextAreaElement>(null);
  const showMemo = memoOpen || !!l.memo;
  const focusLine = useS((s) => s.focusLine);

  useEffect(() => autosize(ta.current), [l.text]);
  useEffect(() => autosize(memoTa.current), [l.memo, showMemo]);
  useEffect(() => {
    if (focusLine === l.id) {
      ta.current?.closest(".ln")?.scrollIntoView({ block: "center", behavior: "smooth" });
      useS.setState({ focusLine: null });
    }
  }, [focusLine, l.id]);
  useEffect(() => {
    if (playing && useS.getState().playing) ta.current?.closest(".ln")?.scrollIntoView({ block: "nearest" });
  }, [playing]);

  const play = async () => {
    try {
      const r = await synthLine(l.id);
      if (r) {
        player.pause();
        player.playOne(akf(`/audio/${r.id}.wav`));
        if (r.unread?.length) toast(`読めなかった語：${r.unread.join("、")}（｜漢字《よみ》で読みを指定できます）`);
      }
    } catch (e) {
      toastError("音声を作成できませんでした", e, 8000);
    }
  };

  const onKey = (e: React.KeyboardEvent<HTMLTextAreaElement>) => {
    const t = e.currentTarget;
    const act = lineKeyAction({
      key: e.key, ctrlKey: e.ctrlKey, metaKey: e.metaKey, shiftKey: e.shiftKey, altKey: e.altKey, repeat: e.repeat,
      isComposing: e.nativeEvent.isComposing, keyCode: e.keyCode, selStart: t.selectionStart, selEnd: t.selectionEnd,
    });
    if (act === "undo" || act === "redo") {
      // 欄の元に戻すではなくアプリの履歴で戻す（行の結合・分割も戻るように。旧#49）
      e.preventDefault();
      e.stopPropagation();
      (act === "undo" ? undo : redo)();
      return;
    }
    if (e.key === "Enter" && !e.shiftKey && !e.nativeEvent.isComposing) {
      e.preventDefault();
      const pos = t.selectionStart;
      const head = t.value.slice(0, pos);
      const tail = t.value.slice(pos);
      const id = nid("l");
      editScript((s) => {
        const f = findLine(s, l.id)!;
        f.l.text = head;
        const sp = SPEAKERS[(SPEAKERS.indexOf(l.sp) + 1) % SPEAKERS.length] || l.sp;
        f.b.lines.splice(f.i + 1, 0, { id, sp: tail ? l.sp : sp, text: tail, start: null }); // メモは頭の行に残す
      }, "own");
      setTimeout(() => {
        const el = document.querySelector<HTMLTextAreaElement>(`[data-line="${id}"] textarea`);
        el?.focus();
        el?.setSelectionRange(0, 0);
      }, 30);
    } else if (act === "mergePrev") {
      const s = useS.getState().script!;
      const f = findLine(s, l.id)!;
      if (f.i === 0) return;
      e.preventDefault();
      const prev = f.b.lines[f.i - 1];
      const at = prev.text.length;
      editScript((s2) => {
        const g = findLine(s2, l.id)!;
        const p = g.b.lines[g.i - 1];
        p.text += g.l.text;
        const memo = [p.memo, g.l.memo].filter(Boolean).join("\n");
        if (memo) p.memo = memo;
        g.b.lines.splice(g.i, 1);
      }, "own");
      setTimeout(() => {
        const el = document.querySelector<HTMLTextAreaElement>(`[data-line="${prev.id}"] textarea`);
        el?.focus();
        el?.setSelectionRange(at, at);
      }, 30);
    } else if (e.altKey && (e.key === "ArrowUp" || e.key === "ArrowDown")) {
      e.preventDefault();
      editScript((s) => {
        const f = findLine(s, l.id)!;
        const j = f.i + (e.key === "ArrowUp" ? -1 : 1);
        if (j < 0 || j >= f.b.lines.length) return;
        [f.b.lines[f.i], f.b.lines[j]] = [f.b.lines[j], f.b.lines[f.i]];
        f.b.lines[f.i].start = null;
        f.b.lines[j].start = null;
      }, "own");
    } else if (e.key === "Tab") {
      e.preventDefault();
      toggleSp();
    } else if (e.ctrlKey && e.key === "p") {
      e.preventDefault();
      play();
    }
  };

  const toggleSp = () =>
    editScript((s) => {
      const f = findLine(s, l.id)!;
      f.l.sp = speakers[(speakers.indexOf(f.l.sp) + 1) % speakers.length];
    }, "own");

  const clickNum = (e: React.MouseEvent) => {
    const st = useS.getState();
    if (e.shiftKey && st.selLines.length) {
      // 範囲選択
      const all = st.script!.blocks.flatMap((x) => x.lines.map((y) => y.id));
      const a0 = all.indexOf(st.selLines[st.selLines.length - 1]);
      const a1 = all.indexOf(l.id);
      const [lo, hi] = a0 < a1 ? [a0, a1] : [a1, a0];
      useS.setState({ selLines: [...new Set([...st.selLines, ...all.slice(lo, hi + 1)])] });
    } else if (e.ctrlKey) {
      useS.setState({ selLines: st.selLines.includes(l.id) ? st.selLines.filter((x) => x !== l.id) : [...st.selLines, l.id] });
    } else {
      useS.setState({ selLines: st.selLines.length === 1 && st.selLines[0] === l.id ? [] : [l.id] });
    }
    if (item) {
      player.seek(item.start);
      // 再生中でも、タイムラインを再生位置まで動かす
      window.dispatchEvent(new Event("ak-reveal-playhead"));
    }
  };

  const voiced = !!ttsKeyText(l.text);
  return (
    <div
      className={`ln ${sel ? "sel" : ""} ${active ? "active" : ""} ${playing ? "playing" : ""} ${prop ? "prop" : ""}`}
      data-line={l.id}
      onFocus={() => useS.setState({ selection: { kind: "line", id: l.id } })}
    >
      <div className="num" onClick={clickNum} title="クリックでその行へ移動して選択（Ctrl で追加、Shift で範囲）。選択した行は Claude への文脈になります">{n}</div>
      <div className={`sp ${spClass(l.sp, speakers)}`} onClick={toggleSp} title="話者を切り替え（Tab）">{l.sp}</div>
      <textarea
        ref={ta}
        rows={1}
        value={l.text}
        spellCheck={false}
        onChange={(e) => {
          const v = e.target.value.replace(/\n/g, "");
          // 打った文字も元に戻すの履歴に積む（続けて打った分は 1 回ぶんにまとまる）。Ctrl+Z はアプリの履歴で戻すため（旧#49）
          editScript((s) => { findLine(s, l.id)!.l.text = v; });
          autosize(e.target);
        }}
        onBlur={() => relayout()}
        onKeyDown={onKey}
        onContextMenu={(e) => dictContextMenu(e, l.id)}
      />
      <div className="acts">
        <button className="ghost" onClick={play} disabled={!voiced || busy} title="試聴（Ctrl+P）">{busy ? "…" : "▶"}</button>
        <button className={`ghost ${acc ? "on" : ""}`} onClick={() => { setAcc(!acc); if (!a && voiced) synthLine(l.id).catch((e) => toastError("音声を作成できませんでした", e)); }} disabled={!voiced} title="アクセント・長さ・抑揚">A</button>
        <button
          className={`ghost ${l.memo ? "on" : ""}`}
          style={{ fontSize: 11 }}
          title="Claude 向けのメモ（演出の参考。読み上げない）"
          onClick={() => {
            setMemoOpen(true);
            setTimeout(() => memoTa.current?.focus(), 0);
          }}
        >
          メモ
        </button>
        <button
          className="ghost danger"
          title="行を削除"
          onClick={() => editScript((s) => { const f = findLine(s, l.id)!; f.b.lines.splice(f.i, 1); })}
        >
          ✕
        </button>
      </div>
      <div className="meta">
        {item && <span>{tc(item.start)}</span>}
        {item && <span>{item.dur.toFixed(2)}s</span>}
        {voiced && !a && <span className="warn">音声なし</span>}
        {a && !fresh && <span className="warn">調整後の音声が未作成</span>}
        {edited && <span style={{ color: "var(--caution)" }}>調整あり</span>}
        {stale && (
          <span style={{ color: "var(--warn)" }} title="文や話者を直す前の調整が残っています。文を元に戻すとまた効きます">
            前の文の調整（使っていません）
            <button
              className="ghost"
              style={{ marginLeft: 2, padding: "0 4px", fontSize: 10 }}
              title="前の文の調整を消す"
              onClick={() => editScript((s) => { const f = findLine(s, l.id); if (f) dropStaleAdj(s, f.l); })}
            >
              ✕
            </button>
          </span>
        )}
        {a?.unread?.length ? <span className="warn">読めない語：{a.unread.join("、")}</span> : null}
        {!voiced && <span>（読み上げない行）</span>}
      </div>
      {showMemo && (
        <textarea
          ref={memoTa}
          className="memo"
          rows={1}
          value={l.memo ?? ""}
          spellCheck={false}
          placeholder="演出のメモ（例: ここで実際のPUメンバーを出す）。読み上げません"
          onChange={(e) => {
            const v = e.target.value;
            editScript((s) => { findLine(s, l.id)!.l.memo = v || undefined; }, false);
            autosize(e.target);
          }}
          onBlur={() => {
            // 空なら閉じる（memo は空のとき undefined にしてあるので、保存には出ない）
            if (!l.memo) setMemoOpen(false);
          }}
        />
      )}
      {acc && voiced && <AccentEditor line={l} />}
    </div>
  );
});

export default function ScriptPanel() {
  const script = useS((s) => s.script);
  const project = useS((s) => s.project);
  const selLines = useS((s) => s.selLines);
  const run = useS((s) => s.synthAllRun);
  const [importing, setImporting] = useState(false);
  const listRef = useRef<HTMLDivElement>(null);
  // 欄の幅が変わったら、行の欄の高さを測り直す（高さは px で持つので、狭いときの縦長の高さが残る）
  useEffect(() => {
    const el = listRef.current;
    if (!el) return;
    let w = el.clientWidth;
    const ro = new ResizeObserver(() => {
      if (el.clientWidth === w) return;
      w = el.clientWidth;
      el.querySelectorAll("textarea").forEach(autosize);
    });
    ro.observe(el);
    return () => ro.disconnect();
  }, [!!script, !!project]);
  const speakers = useMemo(
    () => [...new Set([...SPEAKERS, ...(script?.blocks.flatMap((b) => b.lines.map((l) => l.sp)) ?? [])])],
    [script],
  );
  if (!project) return null;
  if (!script) {
    return (
      <div className="empty">
        この回には script/ がありません。
        <br />
        <button
          onClick={() => {
            useS.setState({ script: { blocks: [], memo: "", voice: {}, params: {}, rev: 0 } });
            editScript((s) => s.blocks.push({ id: nid("b"), file: null, title: "序論", lines: [{ id: nid("l"), sp: "トーク1", text: "", start: null }], notes: "" }));
          }}
        >
          台本を作成
        </button>{" "}
        <button onClick={() => setImporting(true)}>ほかの回から取り込む…</button>
        {importing && <ImportScriptDialog onClose={() => setImporting(false)} />}
      </div>
    );
  }
  let n = 0;
  const doc = project.doc;
  return (
    <>
      <div className="row" style={{ padding: "6px 10px", borderBottom: "1px solid var(--line)", flexWrap: "wrap" }}>
        <button
          onClick={() => void synthAll()}
          disabled={!!run}
          title="音声がない行と調整後の行の音声をまとめて作成します（声のエンジン）"
        >
          音声を一括作成
        </button>
        {run && run.n > 0 && (
          <>
            <span className="dim" style={{ fontSize: 12 }}>
              {run.stopping ? `中止中 ${run.i + 1} / ${run.n}` : `音声を作成中 ${run.i + 1} / ${run.n}`}
            </span>
            <button onClick={stopSynthAll} disabled={run.stopping} title="作成中の行が終わったところで止めます（Esc）">
              中止
            </button>
          </>
        )}
        <button
          title="ほかの回の台本から、ブロック・行をアクセントの調整ごと末尾に足します"
          onClick={() => setImporting(true)}
        >
          取り込む…
        </button>
        <span className="grow" />
        <span className="dim" style={{ fontSize: 12 }}>間隔</span>
        <input type="number" step={0.05} min={0} value={doc.gap} onChange={(e) => editDoc((d) => { d.gap = Math.max(0, Number(e.target.value)); d.gaps_inferred = true; })} style={{ width: 56 }} title="セリフの間隔（秒）。開始時刻のない行の配置と「詰め直す」に使います" />
        <input type="number" step={0.1} min={0} value={doc.block_gap} onChange={(e) => editDoc((d) => { d.block_gap = Math.max(0, Number(e.target.value)); d.gaps_inferred = true; })} style={{ width: 56 }} title="ブロックの間隔（秒）。開始時刻のない行の配置と「詰め直す」に使います" />
        <button
          title="台本の先頭から間隔どおりに並べ直します"
          onClick={() => {
            const st = useS.getState();
            editScript((s) => relayoutFrom(s, new Map(st.layout.map((it) => [it.line_id, it.dur])), doc.gap, doc.block_gap));
          }}
        >
          詰め直す
        </button>
        <button
          title="読み上げ辞書（voice_dict.json）の一覧。台本の文字を選んで右クリックでも登録できます"
          onClick={() => { useDict.setState({ list: true }); loadDict(true).catch((e) => toastError("辞書を読み込めませんでした", e)); }}
        >
          辞書
        </button>
      </div>
      <DictUI />
      {importing && <ImportScriptDialog onClose={() => setImporting(false)} />}
      {selLines.length > 0 && (
        <div className="row" style={{ padding: "4px 10px", background: "var(--n-25)", fontSize: 12 }}>
          <span>{selLines.length} 行を選択中</span>
          <span className="grow" />
          <button onClick={() => sendChat("選んだ行を推敲してください（言い回し・テンポ・数字の確認）。案は script_propose でお願いします", { kind: "script_review" })}>Claude に推敲を頼む</button>
          <button className="ghost" onClick={() => useS.setState({ selLines: [] })}>解除</button>
        </div>
      )}
      <div className="script" ref={listRef}>
        {script.blocks.map((b, bi) => (
          <div className="blk" key={b.id}>
            <div className="blk-head">
              <input
                value={b.title}
                onChange={(e) => editScript((s) => { s.blocks[bi].title = e.target.value; }, false)}
                title="ブロックの見出し（## の行）"
              />
              <span className="dim2" style={{ fontSize: 11 }}>{b.file ?? "（新規）"}</span>
              <button className="ghost" title="上へ移動" disabled={bi === 0} onClick={() => editScript((s) => { [s.blocks[bi - 1], s.blocks[bi]] = [s.blocks[bi], s.blocks[bi - 1]]; })}>↑</button>
              <button className="ghost" title="下へ移動" disabled={bi === script.blocks.length - 1} onClick={() => editScript((s) => { [s.blocks[bi + 1], s.blocks[bi]] = [s.blocks[bi], s.blocks[bi + 1]]; })}>↓</button>
              <button
                className="ghost danger"
                title="ブロックを削除（ファイルは _backup に残ります）"
                onClick={() => { if (confirm(`ブロック「${b.title}」を削除しますか？`)) editScript((s) => { s.blocks.splice(bi, 1); }); }}
              >
                ✕
              </button>
            </div>
            {b.lines.map((l) => {
              n++;
              return <LineRow key={l.id} l={l} b={b} n={n} speakers={speakers} />;
            })}
            <div className="row" style={{ padding: "2px 0 0 44px" }}>
              <button
                className="ghost"
                onClick={() => {
                  const id = nid("l");
                  editScript((s) => {
                    const last = s.blocks[bi].lines[s.blocks[bi].lines.length - 1];
                    const sp = last ? SPEAKERS[(SPEAKERS.indexOf(last.sp) + 1) % SPEAKERS.length] : "トーク1";
                    s.blocks[bi].lines.push({ id, sp, text: "", start: null });
                  });
                  setTimeout(() => document.querySelector<HTMLTextAreaElement>(`[data-line="${id}"] textarea`)?.focus(), 30);
                }}
              >
                ＋ 行
              </button>
            </div>
            {b.notes && <div className="notes">{b.notes}</div>}
          </div>
        ))}
        <button
          onClick={() => editScript((s) => s.blocks.push({ id: nid("b"), file: null, title: "新しいブロック", lines: [{ id: nid("l"), sp: "トーク1", text: "", start: null }], notes: "" }))}
        >
          ＋ ブロック
        </button>
        <div className="dim2" style={{ marginTop: 18, fontSize: 11, lineHeight: 1.8 }}>
          <span className="kbd">Enter</span> 行を分割　<span className="kbd">Backspace</span>（行頭）前の行と結合　<span className="kbd">Tab</span> 話者の切り替え
          <span className="kbd">Alt+↑↓</span> 行を移動　<span className="kbd">Ctrl+P</span> 試聴　ルビは <span className="mono">｜漢字《よみ》</span>
        </div>
      </div>
    </>
  );
}
