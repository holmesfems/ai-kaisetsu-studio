import { useEffect, useMemo, useState } from "react";
import { createPortal } from "react-dom";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { call } from "../io/api";
import { editScript, toast, toastError, useS } from "../state";
import { importScript } from "../logic/importscript.ts";
import type { Script } from "../types";
import { nid, paramsOf, voiceOf } from "../logic/voice";
import { Dialog } from "./TopBar";

type Proj = { name: string; path: string; script: boolean; fx: boolean; mtime: number };

const norm = (p: string) => p.replace(/\\/g, "/").replace(/\/+$/, "").toLowerCase();
const same = (a: string, b: string) => norm(a) === norm(b);
const folderName = (p: string) => p.replace(/[\\/]+$/, "").split(/[\\/]/).pop() || p;

/** ほかの回の台本から、ブロック・行を（アクセントの調整ごと）末尾に取り込むダイアログ */
// 台本の欄の中に置くと、欄の外へ出られない（position: fixed が欄に閉じ込められる）ので body に出す
export default function ImportScriptDialog({ onClose }: { onClose: () => void }) {
  return createPortal(<ImportScriptBody onClose={onClose} />, document.body);
}

function ImportScriptBody({ onClose }: { onClose: () => void }) {
  const root = useS((s) => s.project?.root ?? "");
  const [list, setList] = useState<{ projects: Proj[] } | null>(null);
  const [q, setQ] = useState("");
  const [src, setSrc] = useState<{ name: string; script: Script } | null>(null);
  const [busy, setBusy] = useState(false);
  const [sel, setSel] = useState<Record<string, Set<string>>>({});
  const [open, setOpen] = useState<Set<string>>(new Set());
  const [memo, setMemo] = useState(true);
  const [notes, setNotes] = useState(true);
  const [speakers, setSpeakers] = useState(true);

  useEffect(() => {
    call("list_projects")
      .then(setList)
      .catch((e) => toastError("回の一覧を取得できませんでした", e));
  }, []);

  const pickProject = async (path: string) => {
    if (same(path, root)) {
      toast("今開いている回です。ほかの回を選んでください", "error");
      return;
    }
    setBusy(true);
    try {
      const script = await call("script_peek", { path });
      setSrc({ name: folderName(path), script });
      setSel({});
      setOpen(new Set());
    } catch (e) {
      toastError("台本を読み込めませんでした", e, 8000);
    } finally {
      setBusy(false);
    }
  };

  const total = useMemo(() => Object.values(sel).reduce((n, s) => n + s.size, 0), [sel]);

  const toggleBlock = (id: string, ids: string[]) =>
    setSel((s) => ({ ...s, [id]: (s[id]?.size ?? 0) === ids.length ? new Set() : new Set(ids) }));
  const toggleLine = (bid: string, lid: string) =>
    setSel((s) => {
      const cur = new Set(s[bid] ?? []);
      if (cur.has(lid)) cur.delete(lid);
      else cur.add(lid);
      return { ...s, [bid]: cur };
    });

  const doImport = () => {
    if (!src || !total) return;
    const pick = {
      blocks: src.script.blocks
        .filter((b) => sel[b.id]?.size)
        .map((b) => ({ id: b.id, lines: b.lines.filter((l) => sel[b.id].has(l.id)).map((l) => l.id) })),
      memo,
      notes,
      speakers,
    };
    if (!useS.getState().script) {
      // 台本が無い回は「台本を作成」と同じく空の台本を作ってから（序論の空の行は足さない）
      useS.setState({ script: { blocks: [], memo: "", voice: {}, params: {}, rev: 0 } });
    }
    let r = { added: 0, voice: 0, params: 0, speakers: [] as string[] };
    editScript((s) => { r = importScript(s, src.script, pick, nid); }, "own");
    const sp = r.speakers.length ? `、話者の声: ${r.speakers.join("・")}` : "";
    toast(`${src.name} から ${r.added} 行を取り込みました（アクセント ${r.voice} 行・抑揚 ${r.params} 行${sp}）`);
    onClose();
  };

  if (!src) {
    return (
      <Dialog title="ほかの回から台本を取り込む" onClose={onClose}>
        <div className="row" style={{ marginBottom: 8 }}>
          <input className="grow" placeholder="絞り込み（フォルダ名・題名の一部）" value={q} onChange={(e) => setQ(e.target.value)} autoFocus />
          <button
            disabled={busy}
            onClick={async () => {
              try {
                const d = await openDialog({ directory: true, title: "取り込み元の回のフォルダを選択" });
                if (typeof d === "string") void pickProject(d);
              } catch (e) {
                toastError("フォルダを選択できませんでした", e);
              }
            }}
          >
            フォルダを選択…
          </button>
        </div>
        {!list ? (
          <div className="dim">読み込み中…</div>
        ) : (
          <div className="plist">
            {list.projects
              .filter((p) => p.script && !same(p.path, root))
              .filter((p) => !q || p.name.toLowerCase().includes(q.toLowerCase()))
              .map((p) => (
                <div key={p.path} className="pi" onClick={() => !busy && void pickProject(p.path)}>
                  <span className="ui" style={{ minWidth: 220 }}>{p.name}</span>
                  <span className="grow" />
                  <span className="dim2 ui">{new Date(p.mtime * 1000).toLocaleDateString()}</span>
                </div>
              ))}
          </div>
        )}
        {busy && <div className="dim">台本を読み込み中…</div>}
      </Dialog>
    );
  }

  return (
    <Dialog title={`${src.name} から取り込む`} onClose={onClose} wide>
      <div className="dim" style={{ marginBottom: 8, fontSize: 12 }}>
        取り込むブロック・行を選んでください（展開すると行ごとに選べます）。「調整済み」の行は、アクセント・抑揚の調整ごと取り込みます。
      </div>
      <div style={{ maxHeight: "50vh", overflow: "auto", border: "1px solid var(--line)", borderRadius: 4 }}>
        {src.script.blocks.map((b) => {
          const ids = b.lines.map((l) => l.id);
          const n = sel[b.id]?.size ?? 0;
          const isOpen = open.has(b.id);
          return (
            <div key={b.id}>
              <div className="row" style={{ padding: "4px 8px", gap: 6 }}>
                <button
                  className="ghost"
                  title={isOpen ? "行を閉じる" : "行を展開"}
                  onClick={() => setOpen((o) => { const x = new Set(o); if (x.has(b.id)) x.delete(b.id); else x.add(b.id); return x; })}
                >
                  {isOpen ? "▾" : "▸"}
                </button>
                <input
                  type="checkbox"
                  checked={n > 0 && n === ids.length}
                  ref={(el) => { if (el) el.indeterminate = n > 0 && n < ids.length; }}
                  disabled={!ids.length}
                  onChange={() => toggleBlock(b.id, ids)}
                />
                <span className="grow">{b.title || "（見出しなし）"}</span>
                <span className="dim2" style={{ fontSize: 11 }}>{n ? `${n} / ` : ""}{ids.length} 行</span>
              </div>
              {isOpen &&
                b.lines.map((l) => {
                  const adj = voiceOf(src.script, l) != null || paramsOf(src.script, l) != null;
                  return (
                    <label key={l.id} className="row" style={{ padding: "2px 8px 2px 44px", gap: 6, fontSize: 12, cursor: "pointer" }}>
                      <input type="checkbox" checked={!!sel[b.id]?.has(l.id)} onChange={() => toggleLine(b.id, l.id)} />
                      <span className="grow">[{l.sp}] {l.text || "（空）"}</span>
                      {adj && <span className="chip" title="アクセントや抑揚の調整があります">調整済み</span>}
                    </label>
                  );
                })}
            </div>
          );
        })}
        {!src.script.blocks.length && <div className="dim" style={{ padding: 8 }}>この回の台本にはブロックがありません。</div>}
      </div>
      <div className="col" style={{ marginTop: 10, gap: 4 }}>
        <label><input type="checkbox" checked={memo} onChange={(e) => setMemo(e.target.checked)} /> 行のメモを写す</label>
        <label><input type="checkbox" checked={notes} onChange={(e) => setNotes(e.target.checked)} /> ブロックの notes を写す</label>
        <label><input type="checkbox" checked={speakers} onChange={(e) => setSpeakers(e.target.checked)} /> 話者の声の設定を写す（同じ名前は上書き）</label>
      </div>
      <div className="row" style={{ marginTop: 14, justifyContent: "flex-end" }}>
        <button onClick={() => setSrc(null)}>回を選び直す</button>
        <button onClick={onClose}>キャンセル</button>
        <button className="primary" disabled={!total} onClick={doImport}>
          取り込む{total ? `（${total} 行）` : ""}
        </button>
      </div>
    </Dialog>
  );
}
