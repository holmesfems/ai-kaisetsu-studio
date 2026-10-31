import React, { useEffect, useMemo, useRef, useState } from "react";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { call, fileUrl } from "../io/api";
import { cancelChat, chatContext, lastSentChat, reactChat, sceneAt, selectMark, sendChat, toast, toastError, undoExternal, useS } from "../state";
import { CANCEL_HINT, cancelTarget, cancelTitle, isCancelKey, noticeNote, restoreDraft, shortcutTarget, stateNote } from "../logic/chatcancel";
import { player } from "../io/player";
import type { ChatMsg, Proposal } from "../types";
import { charDiff, tc } from "../logic/util";
import { IMG_EXTS, imagePaths, mimeExt, IMG_ONLY_TEXT } from "../logic/chatimg";
import { ClaudeConnect } from "./ClaudeConnect.tsx";
import { DELIVERY, PROPOSAL_STATUS } from "../logic/terms.ts";
import { NO_BLOCK, proposalFocusLine, proposalHead } from "../logic/proposals.ts";
import { fmtElapsed, statusView, verbAt } from "../logic/claudestatus";
import { MARK_ONLY_TEXT, bodyWithMarks, bytesToBase64, circled, circledList, clipsUnder, dropMarkImages, drawMarks, groupByT, sendableMarks } from "../logic/marks";
import type { MarkFound } from "../gen/MarkFound";
import { sizeOf } from "./Monitor";

function Diff({ a, b }: { a: string; b: string }) {
  const parts = useMemo(() => charDiff(a, b), [a, b]);
  return (
    <div className="diff" style={{ lineHeight: 1.7 }}>
      {parts.map((p, i) => (p.t === "eq" ? <span key={i}>{p.s}</span> : p.t === "del" ? <del key={i}>{p.s}</del> : <ins key={i}>{p.s}</ins>))}
    </div>
  );
}

function lineNo(id: string | null): number | null {
  const s = useS.getState().script;
  if (!s || !id) return null;
  let n = 0;
  for (const b of s.blocks) for (const l of b.lines) {
    n++;
    if (l.id === id) return n;
  }
  return null;
}

function blockTitleOf(id: string | null): string {
  const b = useS.getState().script?.blocks.find((x) => x.id === id);
  return b ? b.title : NO_BLOCK;
}

function firstLineOf(blockId: string | null): string | null {
  return useS.getState().script?.blocks.find((x) => x.id === blockId)?.lines[0]?.id ?? null;
}

function ProposalBodyView({ body }: { body: ReturnType<typeof proposalHead>["body"] }) {
  switch (body.t) {
    case "diff": return <Diff a={body.a} b={body.b} />;
    case "ins": return <div className="diff">{body.lines.map((s, i) => <div key={i}><ins>{s}</ins></div>)}</div>;
    case "del": return <div className="diff">{body.lines.map((s, i) => <div key={i}><del>{s}</del></div>)}</div>;
    case "plain": return <div>{body.text}</div>;
  }
}

function ProposalCard({ p }: { p: Proposal }) {
  const pending = p.items.filter((i) => i.status === "pending").length;
  const resolve = async (idx: number | null, accept: boolean) => {
    try {
      const r: any = await call("proposal_resolve", { id: p.id, idx, accept });
      if (r.errors?.length) toast(r.errors.join("\n"), "error", 8000);
      const after = useS.getState().proposals.find((x) => x.id === p.id);
      if (after && after.items.every((i) => i.status !== "pending")) {
        // 不採用の案は送らない。すべて不採用なら何も送らない（旧#48）
        if (await call("proposal_report", { id: p.id })) toast("結果を Claude に送信しました", "ok");
      }
    } catch (e) {
      toastError("推敲案を反映できませんでした", e);
    }
  };
  return (
    <div className="prop-card">
      <h4>
        ✎ {p.title}
        <span className="dim2" style={{ fontSize: 11, fontWeight: 400 }}>{p.items.length} 件</span>
        <span className="grow" />
        {pending > 0 && (
          <>
            <button onClick={() => resolve(null, true)}>すべて採用</button>
            <button className="ghost" onClick={() => resolve(null, false)}>すべて不採用</button>
          </>
        )}
      </h4>
      {p.note && <div className="dim" style={{ fontSize: 12, whiteSpace: "pre-wrap", marginBottom: 4 }}>{p.note}</div>}
      {p.items.map((it, i) => {
        const { head, body } = proposalHead(it, lineNo, blockTitleOf);
        return (
          <div key={i} className={`prop-item ${it.status}`}>
            <div className="row" style={{ fontSize: 11 }}>
              <span className="chip" style={{ cursor: "pointer" }} onClick={() => {
                const id = proposalFocusLine(it, firstLineOf);
                if (id) useS.setState({ focusLine: id, selection: { kind: "line", id } });
              }}>
                {head}
              </span>
              <span className="dim2">{it.sp}</span>
              <span className="grow" />
              {it.status === "pending" ? (
                <>
                  <button onClick={() => resolve(i, true)} title="採用">✓</button>
                  <button className="ghost" onClick={() => resolve(i, false)} title="不採用">✕</button>
                </>
              ) : (
                <span className="dim2">{PROPOSAL_STATUS[it.status]}</span>
              )}
            </div>
            <ProposalBodyView body={body} />
            {it.reason && <div className="why">{it.reason}</div>}
          </div>
        );
      })}
    </div>
  );
}

/** 押すとその印の位置へ移って、印を選ぶ（描くモードに入る） */
function focusMark(id: number) {
  const m = useS.getState().marks.marks.find((x) => x.id === id);
  if (!m) return toast(`印${circled(id)}は削除されています`, "info");
  player.seek(m.t);
  selectMark(id, true);
}

function Attachment({ a }: { a: any }) {
  const marks = useS((s) => s.marks.marks);
  if (typeof a.mark === "number") {
    const m = marks.find((x) => x.id === a.mark);
    return <span className="att" onClick={() => focusMark(a.mark)}>✎ 印{circled(a.mark)} {a.label ?? m?.note ?? ""}</span>;
  }
  if (a.image) {
    return <img src={fileUrl(a.image)} onClick={() => call("reveal", { path: a.image })} title={a.image} />;
  }
  if (typeof a.t === "number") {
    return <span className="att" onClick={() => player.seek(a.t)}>▶ {tc(a.t)} {a.label ?? ""}</span>;
  }
  if (a.clip) {
    return (
      <span className="att" onClick={() => {
        const g = useS.getState().desc?.groups.find((x) => x.key === a.clip);
        useS.setState({ selection: { kind: "fx", key: a.clip } });
        if (g) player.seek(g.t0 + (useS.getState().overrides[a.clip]?.shift || 0) + 0.4);
      }}>◆ {a.label ?? a.clip}</span>
    );
  }
  if (a.line !== undefined) {
    // 行の指し方（通し番号・"#12"・id）は Rust が引いて line_id にしてある。引けていなければ押せない表示にする
    const id: string | null = typeof a.line_id === "string" ? a.line_id : null;
    if (!id) return <span className="att">¶ 行 {String(a.line)} {a.label ?? ""}</span>;
    return (
      <span className="att" onClick={() => useS.setState({ focusLine: id, selection: { kind: "line", id } })}>¶ 行 {lineNo(id) ?? String(a.line)} {a.label ?? ""}</span>
    );
  }
  return null;
}

function rich(text: string) {
  // **太字** と `コード` だけ
  const out: React.ReactNode[] = [];
  const re = /(\*\*[^*]+\*\*|`[^`]+`)/g;
  let last = 0;
  let m: RegExpExecArray | null;
  let k = 0;
  while ((m = re.exec(text))) {
    if (m.index > last) out.push(text.slice(last, m.index));
    const s = m[0];
    out.push(s.startsWith("**") ? <b key={k++}>{s.slice(2, -2)}</b> : <code key={k++} className="mono" style={{ background: "#0006", padding: "0 3px" }}>{s.slice(1, -1)}</code>);
    last = m.index + s.length;
  }
  out.push(text.slice(last));
  return out;
}

/** Claude の返事への「好き！」。押した分は次のメッセージで Claude に伝わる */
function LikeButton({ m }: { m: ChatMsg }) {
  const liked = m.reaction === "like";
  const [pop, setPop] = useState(false);
  return (
    <button
      className={`like${liked ? " on" : ""}${pop ? " pop" : ""}`}
      title={liked ? "好き！を取り消す" : "好き！（次のメッセージで Claude に伝わります）"}
      onClick={() => {
        setPop(!liked);
        reactChat(m.id, !liked).catch((e) => toastError("好き！を付けられませんでした", e));
      }}
      onAnimationEnd={() => setPop(false)}
    >
      <span className="heart">{liked ? "♥" : "♡"}</span>
      {liked && <span className="like-lbl">好き！</span>}
    </button>
  );
}

function Msg({ m, onCancel }: { m: ChatMsg; onCancel?: () => void }) {
  const ctx = m.ctx || {};
  if (ctx.kind === "cancel_notice") {
    // 届いた後に取り消したとき、Claude に送った知らせ（本文は title で見られる）
    return <div className="chat-notice" title={m.text}>{noticeNote(m)}</div>;
  }
  const note = m.role === "user" ? stateNote(m) : "";
  const cancelled = m.state?.startsWith("cancel");
  return (
    <div className={`msg ${m.role}${m.state === "queued" ? " queued" : ""}${cancelled ? " cancelled" : ""}`}>
      {m.role === "claude" && <div className="tag" style={{ color: "var(--accent)", marginBottom: 2 }}>CLAUDE</div>}
      <div className="body">{rich(m.text)}</div>
      {m.attachments?.map((a, i) => <Attachment key={i} a={a} />)}
      {m.role === "user" && (ctx.t !== undefined || ctx.clip || ctx.lines || ctx.marks?.length) && (
        <div className="ctx">
          {ctx.t !== undefined && <span style={{ cursor: "pointer" }} onClick={() => player.seek(ctx.t)}>{tc(ctx.t)}・SCENE {ctx.scene} </span>}
          {ctx.clip && <span>・{ctx.clip} </span>}
          {ctx.lines?.length ? <span>・行 {ctx.lines.join(", ")}</span> : null}
          {ctx.marks?.length ? <span>・✎ 印{circledList(ctx.marks)}</span> : null}
        </div>
      )}
      {(note || onCancel) && (
        <div className="sent-state">
          <span className="grow">{note}</span>
          {onCancel && <button className="ghost" title={cancelTitle(m)} onClick={onCancel}>取り消し</button>}
        </div>
      )}
      <div className="ctx msg-foot">
        {m.role === "claude" && <LikeButton m={m} />}
        <span>{m.ts.slice(11, 16)}</span>
      </div>
    </div>
  );
}

/** Claude や外のエディタでの変更の履歴（ST-6）。1 件ずつ戻せる。一番新しいものは Ctrl+Alt+Z でも戻せる */
function ExtHistory() {
  const ext = useS((s) => s.ext);
  const [open, setOpen] = useState(false);
  const left = ext.filter((x) => !x.undone).length;
  if (!ext.length) return null;
  const hhmm = (at: number) => new Date(at).toLocaleTimeString("ja-JP", { hour: "2-digit", minute: "2-digit", second: "2-digit" });
  return (
    <div className="ext-hist">
      <button className="ghost" style={{ fontSize: 11 }} onClick={() => setOpen(!open)} title="一番新しい変更は Ctrl+Alt+Z で戻せます。B を押している間は、その変更の前の姿をモニターで見比べられます">
        {open ? "▾" : "▸"} 外での変更（{left}）
      </button>
      {open && (
        <div className="col" style={{ gap: 2 }}>
          {[...ext].reverse().slice(0, 20).map((x) => (
            <div key={x.id} className="row dim" style={{ fontSize: 11, gap: 6 }}>
              <span className="mono">{hhmm(x.at)}</span>
              <span className="grow" style={{ overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>{x.label}</span>
              {x.undone ? <span>戻しました</span> : <button className="ghost" style={{ fontSize: 11, padding: "0 6px" }} onClick={() => undoExternal(x.id)}>戻す</button>}
            </div>
          ))}
        </div>
      )}
    </div>
  );
}

function Connect() {
  const delivery = useS((s) => s.settings?.delivery ?? "mod");
  return (
    <div className="prop-card" style={{ borderColor: "var(--line2)", background: "var(--n-14)" }}>
      <h4 style={{ color: "var(--accent)" }}>Claude Code と接続</h4>
      <ClaudeConnect delivery={delivery} />
      <div className="dim2" style={{ fontSize: 11, marginTop: 6, lineHeight: 1.6 }}>
        送信方法は ⚙ 設定で変更できます（現在: {DELIVERY[delivery].name}）。<br />
        どの方法でも、Claude から AK Studio を操作するツール（台本・演出・確認画像）を使えます。
      </div>
    </div>
  );
}

/** ピクセルの Clawd 風マスコット（オリジナルの絵）。13x9 のマス。脚・目・体の上下は CSS の steps() で動かす */
function Clawd() {
  const O = "#e8814f", D = "#2a1408";
  return (
    <svg className="clawd" viewBox="0 0 13 9" width="39" height="27" shapeRendering="crispEdges" aria-hidden="true">
      <g className="clawd-legs-a" fill={O}>
        <rect x="3" y="5" width="1" height="4" /><rect x="9" y="5" width="1" height="4" />
        <rect x="5" y="5" width="1" height="3" /><rect x="7" y="5" width="1" height="3" />
      </g>
      <g className="clawd-legs-b" fill={O}>
        <rect x="3" y="5" width="1" height="3" /><rect x="9" y="5" width="1" height="3" />
        <rect x="5" y="5" width="1" height="4" /><rect x="7" y="5" width="1" height="4" />
      </g>
      <g className="clawd-body">
        <g fill={O}>
          <rect x="2" y="1" width="9" height="5" />
          <rect x="0" y="3" width="2" height="2" /><rect x="11" y="3" width="2" height="2" />
        </g>
        <g className="clawd-eyes" fill={D}>
          <rect x="4" y="2" width="1" height="2" /><rect x="8" y="2" width="1" height="2" />
        </g>
      </g>
    </svg>
  );
}

/** チャットの下（入力欄の上）の「考え中」の行（issue 旧#51） */
function Thinking() {
  const chat = useS((s) => s.chat);
  const claude = useS((s) => s.claude);
  const [now, setNow] = useState(() => Date.now());
  const v = statusView(chat, claude, now);
  const on = v !== null;
  useEffect(() => {
    if (!on) return;
    setNow(Date.now());
    const id = setInterval(() => setNow(Date.now()), 500);
    return () => clearInterval(id);
  }, [on]);
  if (!v) return null;
  const el = v.since !== null ? now - v.since : 0;
  return (
    <div className={`thinking${v.held ? " held" : ""}`} role="status">
      <Clawd />
      <span className={`thinking-text${v.text ? "" : " shimmer"}`}>{v.text || verbAt(el)}</span>
      {v.since !== null && <span className="thinking-sec mono">{fmtElapsed(el)}</span>}
    </div>
  );
}

const QUICK = [
  ["この場面を見て", "今の場面（確認画像）を見て、見づらい所や直した方がいい所があれば教えて"],
  ["台本を推敲", "台本全体を読んで、テンポ・言い回し・数字の確認をして、推敲案を script_propose で出して"],
  ["演出のアイデア", "今のシーンのセリフに合う演出のアイデアを 3 つ出して（作る前に相談して）"],
  ["同期を確認", "このあたりの演出の出るタイミングがセリフと合っているか確認して、ずれていたら clip_adjust で直して"],
];

export default function ClaudePanel() {
  const chat = useS((s) => s.chat);
  const proposals = useS((s) => s.proposals);
  const claude = useS((s) => s.claude);
  const t = useS((s) => s.t);
  const selection = useS((s) => s.selection);
  const selLines = useS((s) => s.selLines);
  const [text, setText] = useState("");
  const [withCtx, setWithCtx] = useState(true);
  const [showOld, setShowOld] = useState(false);
  const box = useRef<HTMLDivElement>(null);
  useEffect(() => {
    box.current?.scrollTo({ top: box.current.scrollHeight });
  }, [chat.length, proposals]);
  const pendingProps = proposals.filter((p) => p.items.some((i) => i.status === "pending"));
  const oldProps = proposals.filter((p) => !p.items.some((i) => i.status === "pending"));
  const input = useRef<HTMLTextAreaElement>(null);
  // 添える画像（<回>/studio/chat/ に保存済みのパス）
  const [imgs, setImgs] = useState<string[]>([]);
  const hasProject = useS((s) => s.project !== null);
  // 添える印（自分の draft）。✕ で外したものはこの送信から除く
  const allMarks = useS((s) => s.marks.marks);
  const [skipMarks, setSkipMarks] = useState<ReadonlySet<number>>(new Set());
  const chosen = useMemo(() => sendableMarks(allMarks, skipMarks), [allMarks, skipMarks]);
  const skipped = useMemo(() => sendableMarks(allMarks, new Set()).filter((m) => skipMarks.has(m.id)), [allMarks, skipMarks]);
  const [sending, setSending] = useState(false);
  /** 印を添える: 下にある演出・素材を調べ、印の t ごとに確認画像（コマ + 印の層）を作る。作れなければ例外 */
  const prepareMarks = async () => {
    const st = useS.getState();
    const tracks = st.project?.doc.tracks ?? [];
    const sizes = new Map<string, { w: number; h: number }>();
    const unknown: string[] = [];
    await Promise.all(
      tracks.filter((tr) => tr.kind === "image" || tr.kind === "video").flatMap((tr) => tr.clips).map(async (c) => {
        try {
          sizes.set(c.src, await sizeOf(c.src));
        } catch (e) {
          console.warn("素材の大きさを調べられませんでした", c.src, e);
          unknown.push(c.src.split(/[\/]/).pop() || c.src);
        }
      }),
    );
    const found: MarkFound[] = chosen.map((m) => ({
      id: m.id,
      ...clipsUnder(m.shape, m.t, { groups: st.desc?.groups ?? [], overrides: st.overrides, tracks, size: (src) => sizes.get(src) }),
    }));
    const images: string[] = [];
    for (const g of groupByT(chosen)) {
      const cv = new OffscreenCanvas(1920, 1080);
      const ctx = cv.getContext("2d");
      if (!ctx) throw new Error("印の層を描く canvas を作れませんでした");
      drawMarks(ctx, g.marks, 1, { note: false });
      const png = new Uint8Array(await (await cv.convertToBlob({ type: "image/png" })).arrayBuffer());
      images.push(await call("chat_mark_image", { t: g.t, layer: bytesToBase64(png) }));
    }
    return { found, images, unknown: [...new Set(unknown)] };
  };
  const send = async (s = text) => {
    const useImgs = s === text ? imgs : [];
    const marksNow = chosen;
    if (sending || (!s.trim() && !useImgs.length && !marksNow.length)) return;
    setSending(true);
    try {
      let found: MarkFound[] = [];
      let markImgs: string[] = [];
      let unknownSizes: string[] = [];
      if (marksNow.length) {
        try {
          ({ found, images: markImgs, unknown: unknownSizes } = await prepareMarks());
        } catch (e) {
          // 印を添えられないまま、印の無いメッセージとして送らない
          toastError("印の確認画像を作れませんでした", e);
          return;
        }
      }
      // 画面の状況を添えるを切っても、印ははっきり描いたものなので添える
      const base = withCtx ? {} : { t: undefined, scene: undefined, clip: undefined, lines: undefined };
      await sendChat(
        bodyWithMarks(s, useImgs.length + markImgs.length, marksNow.length, IMG_ONLY_TEXT),
        // 送信済みの記録は Rust が chat_send の中でする（下のクリップは mark_found）
        marksNow.length ? { ...base, marks: marksNow.map((x) => x.id), mark_found: found } : base,
        [...useImgs, ...markImgs].map((image) => ({ image })),
      );
      if (unknownSizes.length) toast(`印の下の素材を調べられませんでした: ${unknownSizes.join("、")}`, "error", 8000);
      if (marksNow.length) setSkipMarks(new Set());
      setText("");
      if (useImgs.length) setImgs([]);
    } catch (e) {
      toastError("メッセージを送信できませんでした", e);
    } finally {
      setSending(false);
    }
  };
  const attach = async (job: () => Promise<string>) => {
    try {
      const p = await job();
      setImgs((a) => [...a, p]);
    } catch (e) {
      toastError("画像を添付できませんでした", e);
    }
  };
  const onPaste = (e: React.ClipboardEvent) => {
    const files = [...e.clipboardData.items].filter((i) => i.kind === "file" && i.type.startsWith("image/")).map((i) => i.getAsFile());
    if (!files.length) return;
    // 文字も一緒にあれば、そちらはふつうに貼り付く
    if (!hasProject) return toast("回を開いてから", "info");
    for (const f of files) {
      const ext = f ? mimeExt(f.type) : null;
      if (!f || !ext) continue;
      attach(
        () =>
          new Promise<string>((ok, ng) => {
            const r = new FileReader();
            r.onerror = () => ng(r.error);
            r.onload = () => call("chat_attach_image", { data: String(r.result).split(",")[1] ?? "", ext }).then(ok, ng);
            r.readAsDataURL(f);
          }),
      );
    }
    if (!e.clipboardData.getData("text")) e.preventDefault();
  };
  const pickImages = async () => {
    const r = await openDialog({ multiple: true, filters: [{ name: "画像", extensions: IMG_EXTS }] });
    if (!r) return;
    for (const path of Array.isArray(r) ? r : [r]) attach(() => call("chat_attach_file", { path }));
  };
  // 送り間違いの取り消し（issue 旧#21）。文は入力欄に戻して、直して送り直せるようにする
  const target = cancelTarget(chat);
  const cancel = async (m: ChatMsg) => {
    try {
      const r = await cancelChat(m.id);
      if (m.text !== IMG_ONLY_TEXT && m.text !== MARK_ONLY_TEXT) setText((d) => restoreDraft(m.text, d));
      setImgs((a) => [...dropMarkImages(imagePaths(m.attachments)), ...a]);
      // 届く前に取り消したので、送った印は下書きに戻す（チップにまた出る）。届いていたときは Claude の作業待ちのまま
      if (r === "retracted" && m.ctx?.marks?.length) {
        try {
          await call("marks_unsend", { msg: m.id });
        } catch (e) {
          toastError("印を下書きに戻せませんでした", e);
        }
      }
      input.current?.focus();
      if (r === "retracted") toast("取り消しました（Claude には届いていません）。メッセージは入力欄に戻しました", "ok");
      else toast("Claude に届いていたため、「直前のメッセージは無視して」と伝えました。メッセージは入力欄に戻しました", "info", 6000);
    } catch (e) {
      toastError("メッセージを取り消せませんでした", e);
    }
  };
  // チャット（入力欄・履歴）にフォーカスがあるときだけ。Esc は IME の変換中なら IME に任せ、
  // Ctrl+C は文字を選んでいればふつうにコピー
  const onKey = (e: React.KeyboardEvent) => {
    const ta = input.current;
    const inInput = ta !== null && document.activeElement === ta;
    const selected = inInput && ta.selectionStart !== ta.selectionEnd ? "x" : window.getSelection()?.toString() ?? "";
    const k = {
      key: e.key, ctrlKey: e.ctrlKey, metaKey: e.metaKey, shiftKey: e.shiftKey, altKey: e.altKey,
      isComposing: e.nativeEvent.isComposing, keyCode: e.keyCode,
    };
    if (!isCancelKey(k, selected, text)) return;
    const m = shortcutTarget(useS.getState().chat, lastSentChat(), Date.now());
    if (!m) return;
    e.preventDefault();
    e.stopPropagation();
    cancel(m);
  };
  void chatContext;
  return (
    <>
      <ExtHistory />
      <div className="chat" ref={box} tabIndex={-1} onKeyDown={onKey}>
        {!claude.connected && !claude.watching && <Connect />}
        {oldProps.length > 0 && (
          <button className="ghost" style={{ alignSelf: "center", fontSize: 11 }} onClick={() => setShowOld(!showOld)}>
            {showOld ? "処理済みの推敲案を非表示" : `処理済みの推敲案（${oldProps.length}）`}
          </button>
        )}
        {showOld && oldProps.map((p) => <ProposalCard key={p.id} p={p} />)}
        {chat.length === 0 && (claude.connected || claude.watching) && (
          <div className="empty">Claude に接続しています。<br />台本の推敲や演出の相談をここから依頼できます。</div>
        )}
        {chat.map((m) => <Msg key={m.id} m={m} onCancel={m === target ? () => cancel(m) : undefined} />)}
        {pendingProps.map((p) => <ProposalCard key={p.id} p={p} />)}
      </div>
      <Thinking />
      <div className="chat-input" onKeyDown={onKey}>
        <div className="row" style={{ flexWrap: "wrap", gap: 4 }}>
          {QUICK.map(([label, msg]) => (
            <button key={label} className="ghost" style={{ fontSize: 11, padding: "1px 6px", borderColor: "var(--line2)" }} onClick={() => send(msg)}>{label}</button>
          ))}
        </div>
        {(chosen.length > 0 || skipped.length > 0) && (
          <div className="ctx-chips">
            {chosen.length > 0 && (
              <span className="chip mark" title="モニターに描いた印を、確認画像と場所のデータにして送ります（「画面の状況を添える」を切っても添えます）">
                ✎ 印 {circledList(chosen.map((m) => m.id))}
                <button className="ghost chip-x" title="この送信から印を外します" onClick={() => setSkipMarks(new Set([...skipMarks, ...chosen.map((m) => m.id)]))}>✕</button>
              </span>
            )}
            {skipped.length > 0 && (
              <span className="chip mark off" title="押すと、また添えます" onClick={() => setSkipMarks(new Set())}>
                ✎ 印 {circledList(skipped.map((m) => m.id))}（外しています）
              </span>
            )}
          </div>
        )}
        {withCtx && (
          <div className="ctx-chips">
            <span className="chip">SCENE {sceneAt(t)}</span>
            {selection?.kind === "fx" && <span className="chip">◆ {selection.key}</span>}
            {(selLines.length > 0 || selection?.kind === "line") && <span className="chip">¶ 行 {chatContext().lines?.join(", ")}</span>}
            <span className="chip">{tc(t)}</span>
          </div>
        )}
        {imgs.length > 0 && (
          <div className="chat-thumbs">
            {imgs.map((p) => (
              <div key={p} className="chat-thumb">
                <img src={fileUrl(p)} title={p} draggable={false} />
                <button className="ghost" title="外す" onClick={() => setImgs((a) => a.filter((x) => x !== p))}>✕</button>
              </div>
            ))}
          </div>
        )}
        <textarea
          ref={input}
          onPaste={onPaste}
          value={text}
          placeholder={
            target && !text && target.id === lastSentChat()?.id
              ? `送り間違えたら ${CANCEL_HINT}`
              : claude.connected ? "Claude へのメッセージ（Ctrl+Enter で送信）" : "Claude へのメッセージ（接続後に届きます。Ctrl+Enter で送信）"
          }
          onChange={(e) => setText(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter" && e.ctrlKey) {
              e.preventDefault();
              send();
            }
          }}
        />
        <div className="row">
          <button className="ghost" style={{ fontSize: 13, padding: "1px 6px" }} onClick={pickImages} disabled={!hasProject} title={hasProject ? "画像を添える（貼り付け Ctrl+V でも）" : "回を開いてから"}>📎</button>
          <label className="row dim" style={{ fontSize: 12, gap: 4 }}>
            <input type="checkbox" checked={withCtx} onChange={(e) => setWithCtx(e.target.checked)} />画面の状況を添える
          </label>
          <span className="grow" />
          <button className="ghost" style={{ fontSize: 11 }} onClick={async () => {
            if (!confirm("チャットの履歴を削除しますか？（添えた画像も消えます）")) return;
            try {
              await call("chat_clear");
              useS.setState({ chat: [] });
              setImgs([]);
            } catch (e) {
              toastError("チャットの履歴を削除できませんでした", e);
            }
          }}>履歴を削除</button>
          <button className="primary" onClick={() => send()} disabled={sending || (!text.trim() && !imgs.length && !chosen.length)}>送信</button>
        </div>
      </div>
    </>
  );
}
