import React, { useEffect, useRef, useState } from "react";
import { call } from "./io/api";
import { endCompare, fpsOf, init, redo, startCompare, stopSynthAll, total, undo, undoExternal, useS, patchOverride, toast, toastError } from "./state";
import { player } from "./io/player";
import TopBar from "./components/TopBar";
import ScriptPanel from "./components/ScriptPanel";
import Monitor from "./components/Monitor";
import Timeline from "./components/Timeline";
import Inspector, { MediaAdjust } from "./components/Inspector";
import ClaudePanel from "./components/ClaudePanel";
import Toasts from "./components/Toasts";
import MemoPanel from "./components/MemoPanel";
import MediaPanel from "./components/MediaPanel";
import GuidePanel from "./components/GuidePanel";
import NestEditor from "./components/NestEditor";
import { isFormControl, isTextEntry } from "./logic/util";
import { isSynthStopKey } from "./logic/synthall";

// 再生などのショートカットは、フォーム部品を触っている間は効かせない（スライドバーの矢印キー・チェックボックスの Space など）
const isTyping = (_e: KeyboardEvent) => isFormControl(document.activeElement as HTMLInputElement | null);
// 元に戻す・やり直しは、文字を打つ欄のときだけ欄に任せる（選択肢・チェックボックス・スライドバーを触った直後でも効くように）
const isTypingText = () => isTextEntry(document.activeElement as HTMLInputElement | null);

function useSize(key: string, init: number) {
  const [v, setV] = useState<number>(() => {
    const x = Number(localStorage.getItem(key));
    return x > 0 ? x : init;
  });
  const set = (n: number) => {
    setV(n);
    localStorage.setItem(key, String(n));
  };
  return [v, set] as const;
}

function Splitter({ dir, onDrag, className }: { dir: "v" | "h"; onDrag: (d: number) => void; className: string }) {
  return (
    <div
      className={className}
      onPointerDown={(e) => {
        const el = e.currentTarget;
        el.setPointerCapture(e.pointerId);
        let last = dir === "v" ? e.clientX : e.clientY;
        const move = (ev: PointerEvent) => {
          const p = dir === "v" ? ev.clientX : ev.clientY;
          onDrag(p - last);
          last = p;
        };
        const up = () => {
          el.removeEventListener("pointermove", move);
          el.removeEventListener("pointerup", up);
        };
        el.addEventListener("pointermove", move);
        el.addEventListener("pointerup", up);
      }}
    />
  );
}

export default function App() {
  const [leftW, setLeftW] = useSize("leftW", 440);
  const [rightW, setRightW] = useSize("rightW", 390);
  const [tlH, setTlH] = useSize("tlH", 330);
  const [leftTab, setLeftTab] = useState<"script" | "memo" | "media" | "adjust" | "guide">("script");
  const [rightTab, setRightTab] = useState<"insp" | "claude">("claude");
  const project = useS((s) => s.project);
  const nestKey = useS((s) => s.nestKey);
  const selection = useS((s) => s.selection);
  const pending = useS((s) => s.proposals.reduce((n, p) => n + p.items.filter((i) => i.status === "pending").length, 0));
  const unread = useS((s) => s.chat.length);
  const lastSeen = useRef(0);
  const [seen, setSeen] = useState(0);
  const sizes = useRef({ leftW, rightW, tlH });
  sizes.current = { leftW, rightW, tlH };

  useEffect(() => {
    init().catch((e) => toastError("起動できませんでした", e, 10000));
  }, []);

  useEffect(() => {
    if (selection?.kind === "fx" || selection?.kind === "se" || selection?.kind === "guide_audio") setRightTab("insp");
    // 素材の調整は左の「素材調整」タブ
    if (selection?.kind === "media") setLeftTab("adjust");
  }, [selection]);

  useEffect(() => {
    if (rightTab === "claude") {
      lastSeen.current = unread;
      setSeen(unread);
    }
  }, [rightTab, unread]);

  useEffect(() => {
    const onKey = async (e: KeyboardEvent) => {
      const s = useS.getState();
      const ctrl = e.ctrlKey || e.metaKey;
      // Ctrl+Alt+Z: Claude や外のエディタでの変更を戻す（自分の操作の Ctrl+Z とは別。ST-6）
      if (ctrl && e.altKey && (e.key === "z" || e.key === "Z") && !isTypingText()) {
        e.preventDefault();
        await undoExternal();
        return;
      }
      if (ctrl && (e.key === "z" || e.key === "Z") && !isTypingText()) {
        e.preventDefault();
        if (e.shiftKey) await redo();
        else await undo();
        return;
      }
      if (ctrl && (e.key === "y" || e.key === "Y") && !isTypingText()) {
        e.preventDefault();
        await redo();
        return;
      }
      // Esc: 音声の一括作成を中止（文字を打つ欄・選択肢・Claude のチャット・録画を差し込む画面では、そちらの Esc に任せる）
      if (s.synthAllRun && e.key === "Escape") {
        const a = document.activeElement as HTMLElement | null;
        const where = {
          typing: isTypingText() || a?.tagName === "SELECT",
          inChat: !!a?.closest(".chat, .chat-input"),
          nestOpen: !!s.nestKey,
        };
        if (isSynthStopKey(e, where)) {
          e.preventDefault();
          stopSynthAll();
          return;
        }
      }
      if (isTyping(e) || ctrl || e.altKey) return;
      const fps = fpsOf(s);
      const step = e.shiftKey ? 1 : 1 / fps;
      switch (e.key) {
        case " ":
          e.preventDefault();
          player.toggle();
          break;
        case "k":
        case "K":
          player.pause();
          break;
        case "l":
        case "L":
          player.play();
          break;
        case "j":
        case "J":
          player.seek(s.t - 2);
          break;
        case "ArrowLeft":
          e.preventDefault();
          player.seek(s.t - step);
          break;
        case "ArrowRight":
          e.preventDefault();
          player.seek(s.t + step);
          break;
        case "ArrowUp":
        case "ArrowDown": {
          e.preventDefault();
          const marks = [...new Set([...s.layout.map((x) => x.start), ...s.scenes.map((x) => x.t0)])].sort((a, b) => a - b);
          const t = e.key === "ArrowUp" ? [...marks].reverse().find((m) => m < s.t - 0.02) : marks.find((m) => m > s.t + 0.02);
          if (t !== undefined) player.seek(t);
          break;
        }
        case "Home":
          player.seek(0);
          break;
        case "End":
          player.seek(total());
          break;
        case "i":
        case "I":
          useS.setState({ inT: s.t });
          break;
        case "o":
        case "O":
          useS.setState({ outT: s.t });
          break;
        case "x":
        case "X":
          useS.setState({ inT: null, outT: null });
          break;
        case "b":
        case "B":
          // 押している間だけ、一番新しい外の変更より前の姿をモニターに出す（離すと戻る）
          if (!e.repeat) startCompare();
          break;
        case "Enter": {
          const a = s.inT ?? Math.max(0, s.t - 5);
          const b = s.outT ?? s.t + 15;
          call("prefetch", { t0: Math.min(a, b), t1: Math.max(a, b), scale: s.scale, tag: "render" });
          toast(`演出をレンダリングしています（${Math.min(a, b).toFixed(1)}〜${Math.max(a, b).toFixed(1)} 秒）`);
          break;
        }
        case "Delete":
          if (s.nestKey) break; // 録画を差し込む画面では、そちらのクリップを外す
          if (s.selection?.kind === "fx") {
            const key = s.selection.key;
            await patchOverride(key, { hidden: !s.overrides[key]?.hidden });
          }
          break;
        case "=":
        case "+":
          useS.setState({ zoom: Math.min(400, s.zoom * 1.25) });
          break;
        case "-":
          useS.setState({ zoom: Math.max(2, s.zoom / 1.25) });
          break;
      }
    };
    // 見比べは B を離したとき（とウィンドウから離れたとき）に終える
    const onUp = (e: KeyboardEvent) => {
      if (e.key === "b" || e.key === "B") endCompare();
    };
    const onBlur = () => endCompare();
    window.addEventListener("keydown", onKey);
    window.addEventListener("keyup", onUp);
    window.addEventListener("blur", onBlur);
    return () => {
      window.removeEventListener("keydown", onKey);
      window.removeEventListener("keyup", onUp);
      window.removeEventListener("blur", onBlur);
    };
  }, []);

  // Alt を単独で押して放すと、Windows（WebView2）はウィンドウを「メニュー操作モード」にして、
  // 次のキー（Ctrl など）をそちらに取られる。Alt+ホイールのあとで Ctrl+ホイールが効かなくなる原因（issue 旧#13）。
  // Alt キーそのものの既定動作だけを止める（Alt+F4 などは F4 のイベントなので止めない）。入力中も同じ
  useEffect(() => {
    const onAlt = (e: KeyboardEvent) => {
      if (e.key === "Alt") e.preventDefault();
    };
    window.addEventListener("keydown", onAlt, true);
    window.addEventListener("keyup", onAlt, true);
    return () => {
      window.removeEventListener("keydown", onAlt, true);
      window.removeEventListener("keyup", onAlt, true);
    };
  }, []);

  const style = { "--left-w": `${leftW}px`, "--right-w": `${rightW}px`, "--tl-h": `${tlH}px` } as React.CSSProperties;

  return (
    <div className="app" style={style}>
      <TopBar />
      <div className="pane left">
        <div className="tabs">
          <div className={`tab ${leftTab === "script" ? "active" : ""}`} onClick={() => setLeftTab("script")}>台本</div>
          <div className={`tab ${leftTab === "memo" ? "active" : ""}`} onClick={() => setLeftTab("memo")}>構成メモ</div>
          <div className={`tab ${leftTab === "media" ? "active" : ""}`} onClick={() => setLeftTab("media")}>素材</div>
          <div className={`tab ${leftTab === "adjust" ? "active" : ""}`} onClick={() => setLeftTab("adjust")}>素材調整</div>
          <div className={`tab ${leftTab === "guide" ? "active" : ""}`} onClick={() => setLeftTab("guide")}>攻略</div>
        </div>
        {!project ? (
          <div className="empty">上の「回を開く」から、回のフォルダ（動画 1 本ぶんのフォルダ）を選んでください</div>
        ) : leftTab === "script" ? (
          <ScriptPanel />
        ) : leftTab === "memo" ? (
          <MemoPanel />
        ) : leftTab === "media" ? (
          <MediaPanel />
        ) : leftTab === "guide" ? (
          <GuidePanel />
        ) : (
          <MediaAdjust />
        )}
      </div>
      <Splitter className="split-v" dir="v" onDrag={(d) => setLeftW(Math.max(260, Math.min(900, sizes.current.leftW + d)))} />
      <Monitor />
      <Splitter className="split-v" dir="v" onDrag={(d) => setRightW(Math.max(280, Math.min(800, sizes.current.rightW - d)))} />
      <div className="pane right">
        <div className="tabs">
          <div className={`tab ${rightTab === "insp" ? "active" : ""}`} onClick={() => setRightTab("insp")}>調整</div>
          <div className={`tab ${rightTab === "claude" ? "active" : ""}`} onClick={() => setRightTab("claude")}>
            Claude
            {pending > 0 && <span className="badge">{pending}</span>}
            {rightTab !== "claude" && unread > seen && <span className="badge">{unread - seen}</span>}
          </div>
        </div>
        {rightTab === "insp" ? <Inspector /> : <ClaudePanel />}
      </div>
      <Splitter className="split-h" dir="h" onDrag={(d) => setTlH(Math.max(150, Math.min(760, sizes.current.tlH - d)))} />
      <Timeline />
      {nestKey && <NestEditor />}
      <Toasts />
    </div>
  );
}
