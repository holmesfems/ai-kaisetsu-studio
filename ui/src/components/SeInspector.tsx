// 効果音（SE）の設定。演出の動きから自動で付く効果音を、種類（シチュエーション）ごとに
// 候補の音を聞き比べ（▶ 単体・前後を聞く＝声・BGM と一緒に）、音量を変えて、鳴らす音を選ぶ。
// 保存すると fx/se/bank.json に書き、効果音を作り直す（再生・書き出しの両方に効く）。
import React, { useEffect, useMemo, useRef, useState } from "react";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { call, fileUrl } from "../io/api";
import { editDoc, refreshSeBank, toast, toastError, total, useS } from "../state";
import { player } from "../io/player";
import type { SeBank, SeKind } from "../types";
import {
  candLabel, contextSpan, dbToGain, draftOf, draftPatch, fmtDb, nearestIndex, pickUse, rebaseDraft, seCounts, seLabel,
  seTimes, type SeDraft,
} from "../logic/sebank";
import { tc } from "../logic/util";

// 試聴の音（候補ごとに 1 回だけ作る。標準の大きさ 0 dB で作り、音量は再生でかける）
const auditionCache = new Map<string, Promise<string>>();

function auditionUrl(bankPath: string, kind: string, src: string): Promise<string> {
  const key = `${bankPath}\u0001${kind}\u0001${src}`;
  let p = auditionCache.get(key);
  if (!p) {
    p = call("se_audition", { kind, src }).then((r) => fileUrl(r.wav));
    p.catch(() => auditionCache.delete(key));
    auditionCache.set(key, p);
  }
  return p;
}

const ONLY_USED_KEY = "seOnlyUsed";

export default function SeInspector({ focus }: { focus?: string }) {
  const bank = useS((s) => s.seBank);
  const desc = useS((s) => s.desc);
  const overrides = useS((s) => s.overrides);
  const doc = useS((s) => s.project?.doc);
  const [draft, setDraft] = useState<SeDraft>({});
  const [busy, setBusy] = useState("");
  const [onlyUsed, setOnlyUsed] = useState(() => localStorage.getItem(ONLY_USED_KEY) !== "0");
  const prevBank = useRef<SeBank | null>(null);

  // 開いたとき・演出が変わったときに読み直す
  useEffect(() => {
    if (desc) refreshSeBank();
  }, [desc?.path]);
  // 設定が届いたら下書きを合わせる（保存していない変更は残す）
  useEffect(() => {
    if (!bank) return;
    const old = prevBank.current?.path === bank.path ? prevBank.current : null; // 別の回を開いたら下書きは捨てる
    setDraft((d) => (old ? rebaseDraft(old, bank, d) : rebaseDraft(null, bank, {})));
    prevBank.current = bank;
  }, [bank]);

  const counts = useMemo(() => seCounts(desc, overrides), [desc, overrides]);
  const patch = useMemo(() => (bank ? draftPatch(bank, draft) : {}), [bank, draft]);
  const dirty = Object.keys(patch).length > 0;

  if (!desc) return <div className="empty">演出（fx/*_fx.py）を読み込むと、効果音を設定できます</div>;
  if (!bank) return <div className="empty">効果音の設定を読み込んでいます…</div>;

  const save = async (): Promise<boolean> => {
    if (!dirty) return true;
    setBusy("保存して効果音を作り直しています…");
    try {
      const r = await call("se_bank_set", { kinds: patch, render: true });
      // 知らせ（se_bank）より先に戻っても、同じ印で効果音を読み直す（二度読みしない）
      if (r.rendered) player.seVersion = r.rev;
      toast(`効果音の設定を保存しました（${r.count ?? "?"} 個）`, "ok");
      return true;
    } catch (e) {
      // 設定は保存できて作り直しだけ失敗・中止したとき（partly_saved）は、「保存できませんでした」を付けずにそのまま出す
      toastError("効果音の設定を保存できませんでした", e, 10000);
      return false;
    } finally {
      setBusy("");
    }
  };
  // 候補の一覧を変える（鳴る音は変わらないので作り直さない。鳴らしている候補を外すときだけ作り直す）
  const editCands = async (kind: string, p: { add?: string[]; remove?: string[] }, render = false) => {
    try {
      await call("se_bank_set", { kinds: { [kind]: p }, render });
    } catch (e) {
      toastError("候補を変更できませんでした", e, 8000);
    }
  };
  const importFile = async (kind: string) => {
    const r = await openDialog({ multiple: true, filters: [{ name: "効果音（wav・mp3 など）", extensions: ["wav", "mp3", "ogg", "flac", "m4a"] }] });
    if (!r) return;
    try {
      for (const p of Array.isArray(r) ? r : [r]) await call("se_import", { kind, path: p });
      await refreshSeBank();
    } catch (e) {
      toastError("ファイルを追加できませんでした", e, 8000);
    }
  };
  const setKindDraft = (kind: string, f: (d: SeDraft[string]) => SeDraft[string]) =>
    setDraft((all) => ({ ...all, [kind]: f(all[kind] ?? { use: ["synth"], db: {} }) }));

  const kinds = bank.kinds.filter((k) => !onlyUsed || (counts[k.kind] ?? 0) > 0 || k.kind === focus);
  const hidden = bank.kinds.length - kinds.length;

  return (
    <div className="insp se-insp">
      <h3>効果音の設定<span className="dim2" style={{ fontSize: 12 }}>演出から自動で付く効果音</span></h3>
      <div className="dim2" style={{ fontSize: 11, lineHeight: 1.7, margin: "4px 0 6px" }}>
        演出の動きの種類ごとに、鳴らす音と音量を選びます。▶ で音だけ、「前後を聞く」で声・BGM と一緒に試聴できます。
        音量は標準の大きさ（セリフの邪魔をしない大きさ）からの dB です。保存先は <code>{bank.path.replace(/^.*[\\/]fx[\\/]/, "fx/").replace(/\\/g, "/")}</code> で、
        保存すると効果音を作り直します（再生・書き出しの両方に反映）。候補は <code>fx/se/</code> の .wav です（mp3 などは 16bit の wav に変換して置きます）。<code>se/&lt;種類&gt;/</code> に置くと、その種類の候補になります。
      </div>
      {doc?.se_muted && (
        <div className="se-warn">
          効果音はミュート中です（前後を聞いても鳴りません）
          <button style={{ marginLeft: 6 }} onClick={() => editDoc((d) => { d.se_muted = false; })}>ミュート解除</button>
        </div>
      )}
      <div className="row" style={{ gap: 8, margin: "6px 0" }}>
        <label className="row" style={{ gap: 4, fontSize: 12 }}>
          <input
            type="checkbox"
            checked={onlyUsed}
            onChange={(e) => {
              setOnlyUsed(e.target.checked);
              localStorage.setItem(ONLY_USED_KEY, e.target.checked ? "1" : "0");
            }}
          />
          この回で鳴る種類のみ{hidden > 0 ? `（ほか ${hidden}）` : ""}
        </label>
        <span className="grow" />
        <button onClick={() => { auditionCache.clear(); refreshSeBank(); }} title="fx/se/ のファイルと bank.json を再読み込みします">↻ 再読み込み</button>
      </div>
      {kinds.map((k) => (
        <KindRow
          key={k.kind}
          k={k}
          bank={bank}
          open={focus === k.kind}
          count={counts[k.kind] ?? 0}
          d={draft[k.kind] ?? draftOf(k)}
          dirty={!!patch[k.kind]}
          busy={!!busy}
          trackDb={doc?.se_db ?? 0}
          onDraft={(f) => setKindDraft(k.kind, f)}
          onSave={save}
          onCands={(p, render) => editCands(k.kind, p, render)}
          onImport={() => importFile(k.kind)}
        />
      ))}
      <div className="se-foot">
        {busy ? (
          <span className="dim">{busy}</span>
        ) : dirty ? (
          <span style={{ color: "var(--warn)", fontSize: 12 }}>未保存の変更: {Object.keys(patch).map((k) => seLabel(k, bank)).join("・")}</span>
        ) : (
          <span className="dim2" style={{ fontSize: 12 }}>保存済み</span>
        )}
        <span className="grow" />
        <button disabled={!dirty || !!busy} onClick={() => setDraft(Object.fromEntries(bank.kinds.map((k) => [k.kind, draftOf(k)])))}>元に戻す</button>
        <button className="primary" disabled={!dirty || !!busy} onClick={save}>保存</button>
      </div>
    </div>
  );
}

function KindRow({ k, bank, open, count, d, dirty, busy, trackDb, onDraft, onSave, onCands, onImport }: {
  k: SeKind; bank: SeBank; open: boolean; count: number; d: SeDraft[string]; dirty: boolean; busy: boolean; trackDb: number;
  onDraft: (f: (d: SeDraft[string]) => SeDraft[string]) => void;
  onSave: () => Promise<boolean>;
  onCands: (p: { add?: string[]; remove?: string[] }, render?: boolean) => void;
  onImport: () => void;
}) {
  const desc = useS((s) => s.desc);
  const overrides = useS((s) => s.overrides);
  const [multi, setMulti] = useState(d.use.length > 1);
  const [idx, setIdx] = useState(-1);
  const [playing, setPlaying] = useState<string | null>(null);
  useEffect(() => { if (d.use.length > 1) setMulti(true); }, [d.use.length]);
  const times = useMemo(() => (open ? seTimes(desc, overrides, k.kind) : []), [open, desc, overrides, k.kind]);
  const cur = idx >= 0 && idx < times.length ? idx : nearestIndex(times, useS.getState().t);
  const select = () => useS.setState({ selection: { kind: "se", se: open ? undefined : k.kind } });
  const [lo, hi] = bank.db_range;
  const using = d.use.map(candLabel).join(" → ");

  const playN = useRef(0);
  const audition = async (src: string) => {
    const n = ++playN.current;
    setPlaying(src);
    try {
      const url = await auditionUrl(bank.path, k.kind, src);
      // 前に鳴らしていた音の終わりの知らせで、今鳴らしている音の印を消さないように番号で見分ける
      const ok = await player.playOne(url, () => { if (playN.current === n) setPlaying(null); }, dbToGain((d.db[src] ?? 0) + trackDb));
      if (!ok) {
        auditionCache.clear();
        throw new Error("試聴用の音を読み込めませんでした");
      }
    } catch (e) {
      if (playN.current === n) setPlaying(null);
      toastError("試聴用の音を作成できませんでした", e, 8000);
    }
  };
  const listen = async (i: number) => {
    if (i < 0 || i >= times.length) return;
    setIdx(i);
    if (dirty && !(await onSave())) return;
    const [a, b] = contextSpan(times[i], total());
    player.playSpan(a, b);
  };
  const addable = bank.files.filter((f) => !k.cands.some((c) => c.src === f));
  const shelf = !!k.shelf; // 棚の種類（全部の回で使う）。選ぶのは 1 つだけ・候補は棚の音が自動で並ぶ

  return (
    <div className={`se-kind ${open ? "open" : ""} ${count ? "" : "unused"}`}>
      <div className="se-kind-h" onClick={select} title={k.stem ? `書き出しのステム: ${k.stem}` : undefined}>
        <span className="se-caret">{open ? "▾" : "▸"}</span>
        <span className="grow">
          {seLabel(k.kind, bank)} <span className="dim2 ui" style={{ fontSize: 11 }}>{k.kind}</span>
          {dirty && <span style={{ color: "var(--warn)" }}> *</span>}
        </span>
        <span className="dim2 ui" style={{ fontSize: 11 }}>{count ? `${count} か所` : "未使用"}</span>
      </div>
      {!open && <div className="se-kind-sub dim2" onClick={select}>{using}{Object.values(d.db).some((v) => Math.abs(v) > 0.05) ? "（音量変更あり）" : ""}</div>}
      {open && (
        <div className="se-kind-b">
          <div className="dim2" style={{ fontSize: 11, lineHeight: 1.6 }}>
            標準の大きさ {k.level} dBFS（ピーク）{k.stem ? `・ステム ${k.stem}` : ""}{k.lite ? "・要所（lite）にも入る" : ""}
            {!k.bankable && <><br />数え上げのカチカチ音は合成音のみで、変えられるのは音量だけです。確定音は「数え上げの確定」で選びます。</>}
          </div>
          {shelf && (
            <div className="dim2" style={{ fontSize: 11, lineHeight: 1.6, margin: "4px 0" }}>
              選んだ音と音量は全部の攻略動画の回で使います（アプリの効果音の棚）。この回だけ変えたいときは <code>fx/se/bank.json</code> に書きます。
            </div>
          )}
          {k.bankable && !shelf && (
            <label className="row" style={{ gap: 4, fontSize: 12, margin: "4px 0" }}>
              <input
                type="checkbox"
                checked={multi}
                onChange={(e) => {
                  setMulti(e.target.checked);
                  if (!e.target.checked && d.use.length > 1) onDraft((x) => ({ ...x, use: [x.use[0]] }));
                }}
              />
              複数の音を順番に使う（1 → 2 → … の順に交代）
            </label>
          )}
          {k.cands.map((c) => {
            const on = d.use.indexOf(c.src);
            const db = d.db[c.src] ?? 0;
            return (
              <div key={c.src} className={`se-cand ${on >= 0 ? "use" : ""} ${c.exists && !c.bad ? "" : "missing"}`}>
                <input
                  type={multi ? "checkbox" : "radio"}
                  name={`se-use-${k.kind}`}
                  checked={on >= 0}
                  disabled={!k.bankable || !c.exists || !!c.bad}
                  title="この音を使う"
                  onChange={() => onDraft((x) => ({ ...x, use: pickUse(x.use, c.src, multi) }))}
                />
                <span className="se-cand-n" title={[c.src, c.source, c.exists ? c.bad ?? "" : "ファイルがありません"].filter(Boolean).join("\n")}>
                  {multi && on >= 0 && <b className="se-ord">{on + 1}</b>}
                  {candLabel(c.src)}
                  {c.folder && !shelf && <span className="dim2"> 📁</span>}
                </span>
                <button className="ghost" title="試聴" disabled={!c.exists || !!c.bad} onClick={() => audition(c.src)}>
                  {playing === c.src ? "♪" : "▶"}
                </button>
                <input
                  type="range"
                  min={lo}
                  max={hi}
                  step={0.5}
                  value={db}
                  disabled={shelf && on < 0}
                  title={shelf && on < 0 ? "棚の種類は選んでいる音だけ音量を持ちます。この音を選ぶと音量を変えられます" : "音量（dB）。ダブルクリックで 0 dB に戻します"}
                  onChange={(e) => {
                    const v = Number(e.target.value);
                    onDraft((x) => ({ ...x, db: { ...x.db, [c.src]: v } }));
                    // この候補を鳴らしている間は、その音のまま音量を変える
                    if (playing === c.src) player.setOneGain(dbToGain(v + trackDb));
                  }}
                  onPointerUp={() => audition(c.src)}
                  onDoubleClick={() => onDraft((x) => ({ ...x, db: { ...x.db, [c.src]: 0 } }))}
                />
                <span className="se-db ui">{fmtDb(db)}</span>
                {c.extra ? (
                  <button className="ghost" title="候補から削除（ファイルは残ります）" onClick={() => onCands({ remove: [c.src] }, on >= 0)}>✕</button>
                ) : (
                  <span style={{ width: 22 }} />
                )}
              </div>
            );
          })}
          {k.bankable && (
            <div className="row" style={{ gap: 6, marginTop: 6, flexWrap: "wrap" }}>
              {!shelf && (
                <select value="" onChange={(e) => e.target.value && onCands({ add: [e.target.value] })} disabled={!addable.length}>
                  <option value="">{addable.length ? "fx/se/ の音を候補に追加…" : "（追加できる音がありません）"}</option>
                  {addable.map((f) => <option key={f} value={f}>{f}</option>)}
                </select>
              )}
              {shelf ? (
                <button onClick={onImport} title={`wav・mp3 などを選んで効果音の棚（${k.kind}/）に置き、候補に追加します。全部の回で使います（mp3 などは 16bit の wav に変換します）`}>ファイルから足す…</button>
              ) : (
                <button onClick={onImport} title={`wav・mp3 などを選んで fx/se/${k.kind}/ に置き、候補に追加します（mp3 などは 16bit の wav に変換します）`}>ファイルを追加…</button>
              )}
            </div>
          )}
          <div className="section">IN CONTEXT</div>
          {times.length ? (
            <div className="row" style={{ gap: 4, flexWrap: "wrap" }}>
              <button disabled={busy} title="前の箇所" onClick={() => listen(cur <= 0 ? times.length - 1 : cur - 1)}>◀</button>
              <button className="primary" disabled={busy} onClick={() => listen(cur)}
                title={"鳴る位置の前後（1.5 秒前〜2.5 秒後）を声・BGM と一緒に再生します" + (dirty ? "\n未保存の変更は、先に保存してから再生します" : "")}>
                前後を聞く
              </button>
              <button disabled={busy} title="次の箇所" onClick={() => listen(cur >= times.length - 1 ? 0 : cur + 1)}>▶</button>
              <span className="dim ui" style={{ fontSize: 12, cursor: "pointer" }} title="この位置へ移動" onClick={() => player.seek(times[cur])}>
                {cur + 1} / {times.length}（{tc(times[cur])}）
              </span>
            </div>
          ) : (
            <div className="dim2" style={{ fontSize: 12 }}>この回では使われていません</div>
          )}
          {k.kind === "confirm" || k.kind === "confirm_neg" ? (
            <div className="dim2" style={{ fontSize: 11, marginTop: 4 }}>確定音は数え上げの終わりに鳴ります（箇所は数え上げの始まり）。</div>
          ) : null}
        </div>
      )}
    </div>
  );
}
