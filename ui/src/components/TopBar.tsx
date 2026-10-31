import React, { useEffect, useState } from "react";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { call } from "../io/api";
import { editDoc, openProject, toast, toastError, useS } from "../state";
import { errorMessage, failText } from "../logic/errors.ts";
import { player } from "../io/player";
import { splitWarnings } from "../logic/warnings.ts";
import { settingsPatch } from "../logic/settings.ts";
import { engineChoice, engineOf, type EngineChoice } from "../logic/voiceengine.ts";
import { branding } from "../theme/index.ts";
import { DriftWarning } from "./drift";
import { ClaudeConnect } from "./ClaudeConnect.tsx";
import { DELIVERY, DELIVERY_ORDER } from "../logic/terms.ts";
import type { Delivery } from "../gen/Delivery.ts";

function Dialog({ title, onClose, children, wide }: { title: string; onClose: () => void; children: React.ReactNode; wide?: boolean }) {
  return (
    <div className="overlay-bg" onMouseDown={(e) => e.target === e.currentTarget && onClose()}>
      <div className="dialog" style={wide ? { minWidth: 760 } : undefined}>
        <div className="row" style={{ marginBottom: 10 }}>
          <h2 style={{ margin: 0 }} className="grow">{title}</h2>
          <button className="ghost" onClick={onClose}>✕</button>
        </div>
        {children}
      </div>
    </div>
  );
}

export { Dialog };

function ProjectPicker({ onClose }: { onClose: () => void }) {
  const [list, setList] = useState<any>(null);
  const [q, setQ] = useState("");
  const load = () => call("list_projects").then(setList);
  useEffect(() => {
    load().catch((e) => toastError("回の一覧を読めませんでした", e));
  }, []);
  // 回のフォルダが並んでいる所を選んで設定に保存する（初めて使うとき。既定の場所は持たない）
  const pickRoot = async () => {
    const d = await openDialog({ directory: true, title: "回のフォルダが並んでいる所を選択" });
    if (typeof d !== "string") return;
    try {
      await call("settings_patch", { p: { an_root: d } });
      const st = await call("get_state");
      useS.setState({ settings: st.settings, voisonaPassSet: st.voisona_pass_set });
      await load();
    } catch (e) {
      toastError("回のフォルダを保存できませんでした", e);
    }
  };
  const go = async (p: string) => {
    onClose();
    await openProject(p);
  };
  return (
    <Dialog title="回を開く" onClose={onClose}>
      <div className="row" style={{ marginBottom: 8 }}>
        <input className="grow" placeholder="絞り込み（フォルダ名・題名の一部）" value={q} onChange={(e) => setQ(e.target.value)} autoFocus />
        <button
          onClick={async () => {
            const d = await openDialog({ directory: true, title: "回のフォルダを選択" });
            if (typeof d === "string") go(d);
          }}
        >
          フォルダを選択…
        </button>
      </div>
      {!list ? (
        <div className="dim">読み込み中…</div>
      ) : !list.root ? (
        <div className="col" style={{ gap: 8 }}>
          <div className="dim">回のフォルダが並んでいる所がまだ決まっていません。選ぶと、ここに一覧が出ます（上の「フォルダを選択…」で 1 つの回を直接開くこともできます）。</div>
          <div><button className="primary" onClick={pickRoot}>回のフォルダを選択…</button></div>
        </div>
      ) : (
        <div className="plist">
          {(list.projects as any[])
            .filter((p) => !q || p.name.toLowerCase().includes(q.toLowerCase()))
            .map((p) => (
              <div key={p.path} className="pi" onClick={() => go(p.path)}>
                <span className="ui" style={{ minWidth: 220 }}>{p.name}</span>
                {p.script && <span className="chip">台本</span>}
                {p.fx && <span className="chip">演出</span>}
                <span className="grow" />
                <span className="dim2 ui">{new Date(p.mtime * 1000).toLocaleDateString()}</span>
              </div>
            ))}
        </div>
      )}
    </Dialog>
  );
}

function EngineLog({ onClose }: { onClose: () => void }) {
  const [log, setLog] = useState<string[]>([]);
  const engine = useS((s) => s.engine);
  const warnInfo = splitWarnings(engine?.warnings);
  useEffect(() => {
    const f = () => call("engine_log").then(setLog);
    f();
    const id = setInterval(f, 1500);
    return () => clearInterval(id);
  }, []);
  return (
    <Dialog title="演出エンジン" onClose={onClose} wide>
      <div className="row" style={{ marginBottom: 8 }}>
        <span className="dim">状態: {engine?.status.state}・ワーカー {engine?.status.ready_workers}/{engine?.status.workers}・読み込み {engine?.status.build_sec}s・キャッシュ {engine?.cache_frames} フレーム（{engine?.cache_mb.toFixed(0)} MB）</span>
        <span className="grow" />
        <button onClick={() => call("reload_fx")}>再読み込み</button>
      </div>
      {engine?.fxkit && <div className="dim" style={{ marginBottom: 8 }}>fxkit: {engine.fxkit}{engine.kit ? `（${engine.kit}）` : ""}</div>}
      {engine?.status.error && <div className="log" style={{ color: "var(--e-05)", marginBottom: 8 }}>{engine.status.error}</div>}
      {!!warnInfo.warns.length && (
        <div className="log" style={{ color: "var(--warn-text)", marginBottom: 8 }}>
          {`警告 ${warnInfo.warns.length} 件:\n` + warnInfo.warns.map((w) => "・" + w.message).join("\n")}
        </div>
      )}
      {!!warnInfo.infos.length && (
        <div className="dim" style={{ marginBottom: 8, whiteSpace: "pre-wrap" }}>{warnInfo.infos.map((w) => "・" + w.message).join("\n")}</div>
      )}
      <div className="log">{log.join("\n") || "ログはありません"}</div>
    </Dialog>
  );
}

/** 数の入力欄の範囲（表示のためだけ。保存の丸めは Rust がする） */
const NUM_RANGE: Record<string, { min: number; max: number; int: boolean }> = {
  workers: { min: 1, max: 32, int: true },
  cache_mb: { min: 100, max: 65536, int: true },
  cancel_grace: { min: 0, max: 30, int: false },
};

function SettingsDialog({ onClose }: { onClose: () => void }) {
  const settings = useS((s) => s.settings);
  const passSet = useS((s) => s.voisonaPassSet);
  const [v, setV] = useState<any>(settings ? { ...settings } : null);
  // 開いたときの値（変えた項目だけを送るための比べ元）
  const [orig] = useState(settings);
  // 「パスワードを削除」を押した（保存で消す）
  const [delPass, setDelPass] = useState(false);
  const [engMode, setEngMode] = useState<EngineChoice>(engineChoice(settings?.voice_engine));
  const [dictPath, setDictPath] = useState("");
  useEffect(() => {
    call("voice_dict_path").then(setDictPath).catch((e) => setDictPath(failText("辞書の場所を取得できませんでした", e)));
  }, []);
  if (!v) return null;
  const F = (k: string, label: string, type = "text", hint = "", placeholder = "") => (
    <div className="field" style={{ gridTemplateColumns: "150px 1fr" }}>
      <label>{label}</label>
      <div className="col" style={{ gap: 2 }}>
        <input type={type} value={v[k] ?? ""} placeholder={placeholder || undefined} min={NUM_RANGE[k]?.min} max={NUM_RANGE[k]?.max} step={NUM_RANGE[k]?.int ? 1 : undefined}
          onChange={(e) => setV({ ...v, [k]: type === "number" ? Number(e.target.value) : e.target.value })} style={{ width: "100%" }} />
        {hint && <small className="dim2">{hint}</small>}
      </div>
    </div>
  );
  return (
    <Dialog title="設定" onClose={onClose}>
      {F("an_root", "回のフォルダ", "text", "回のフォルダが並んでいる所。空欄のときは「回を開く」で選べます")}
      {F("template_dir", "ひな形のフォルダ", "text", "回のフォルダの中にあっても、回の一覧に出しません。空欄なら除外しません")}
      {F("python", "Python", "text", "fxkit・声の中継（voice_server）を動かす Python。保存すると演出のワーカーを作り直します。声の中継には再起動（上の「声」の丸をクリック）で反映されます")}
      {F("ffmpeg", "ffmpeg", "text", "空欄なら PATH、imageio-ffmpeg の順に探します")}
      <div className="field" style={{ gridTemplateColumns: "150px 1fr" }}>
        <label>読み上げ辞書</label>
        <div className="col" style={{ gap: 2 }}>
          <input type="text" readOnly value={dictPath} style={{ width: "100%" }} onFocus={(e) => e.currentTarget.select()} />
          <small className="dim2">全部の回で共通です。場所は変えられません</small>
        </div>
      </div>
      <div className="field" style={{ gridTemplateColumns: "150px 1fr" }}>
        <label>声のエンジン</label>
        <div className="col" style={{ gap: 2 }}>
          <select value={engMode} onChange={(e) => {
            const m = e.target.value as EngineChoice;
            setEngMode(m);
            setV({ ...v, voice_engine: m === "file" ? (engineChoice(v.voice_engine) === "file" ? v.voice_engine : "") : m });
          }}>
            <option value="voisona">VoiSona Talk</option>
            <option value="voicevox">VOICEVOX</option>
            <option value="file">ファイルを指定…</option>
          </select>
          {engMode === "file" && (
            <input type="text" value={v.voice_engine ?? ""} placeholder="声のエンジンの .py ファイルの場所（絶対パス）"
              onChange={(e) => setV({ ...v, voice_engine: e.target.value })} style={{ width: "100%" }} />
          )}
          <small className="dim2">保存すると声の中継を作り直します</small>
        </div>
      </div>
      {engMode === "voicevox" && F("voicevox_url", "VOICEVOX の場所（URL）", "text", "空欄なら http://127.0.0.1:50021 です", "http://127.0.0.1:50021")}
      {engMode === "voisona" && F("voisona_user", "VoiSona のメールアドレス")}
      {engMode === "voisona" && <div className="field" style={{ gridTemplateColumns: "150px 1fr" }}>
        <label>VoiSona の API パスワード</label>
        <div className="row" style={{ gap: 6 }}>
          <input type="password" value={v.voisona_pass ?? ""} autoComplete="new-password"
            placeholder={delPass ? "保存すると削除します" : passSet ? "設定済み（変えるときだけ入力）" : ""}
            onChange={(e) => { setDelPass(false); setV({ ...v, voisona_pass: e.target.value }); }} style={{ flex: 1 }} />
          {passSet && (
            <button disabled={delPass} onClick={() => { setDelPass(true); setV({ ...v, voisona_pass: "" }); }}>パスワードを削除</button>
          )}
        </div>
      </div>}
      {F("workers", "演出ワーカー数", "number", "多いほど速くなりますが、メモリを多く使います（1 本 300MB 前後）。再起動後に反映されます")}
      {F("cache_mb", "演出キャッシュ（MB）", "number")}
      <div className="section">CLAUDE CODE</div>
      <div className="col">
        <div className="field" style={{ gridTemplateColumns: "150px 1fr" }}>
          <label>メッセージの送信方法</label>
          <select value={v.delivery ?? "mod"} onChange={(e) => setV({ ...v, delivery: e.target.value as Delivery })}>
            {DELIVERY_ORDER.map((d) => <option key={d} value={d}>{DELIVERY[d].label}</option>)}
          </select>
        </div>
        {F("cancel_grace", "取り消しの猶予（秒）", "number",
          "送信後、この秒数は Claude に渡さずに待ちます。その間に取り消す（Esc / Ctrl+C）と届きません。0 で待ちません（最大 30）")}
        <ClaudeConnect delivery={v.delivery ?? "mod"} saved={orig?.delivery ?? "mod"} />
      </div>
      <div className="row" style={{ marginTop: 16, justifyContent: "flex-end" }}>
        <button onClick={onClose}>キャンセル</button>
        <button
          className="primary"
          onClick={async () => {
            const p = orig ? settingsPatch(orig, v) : {};
            if (delPass) p.voisona_pass = "";
            if (Object.keys(p).length === 0) {
              onClose();
              return;
            }
            try {
              await call("settings_patch", { p });
            } catch (e) {
              toastError("設定を保存できませんでした", e);
              return;
            }
            // 保存後の設定は本体から取り直す（自分で組み立てた値は入れない）。取り直せなくても保存はできている
            try {
              const st = await call("get_state");
              useS.setState({ settings: st.settings, voisonaPassSet: st.voisona_pass_set });
              toast("設定を保存しました", "ok");
            } catch (e) {
              toast(`設定は保存しましたが、画面の表示を更新できませんでした（設定を開き直すと反映されます）: ${errorMessage(e)}`, "error");
            }
            onClose();
          }}
        >
          保存
        </button>
      </div>
    </Dialog>
  );
}

function ExportMenu({ onClose }: { onClose: () => void }) {
  const inT = useS((s) => s.inT);
  const outT = useS((s) => s.outT);
  const hasRange = inT != null && outT != null && outT > inT;
  const isGuide = useS((s) => !!s.guide?.guide);
  const run = async (kind: string, label: string, ranged = false) => {
    onClose();
    player.pause();
    toast(`${label}の書き出しを開始しました`);
    try {
      const r: any = await call("export", { kind, from: ranged && hasRange ? inT : null, to: ranged && hasRange ? outT : null });
      if (kind === "se") player.seVersion++;
      const lines = [`${label}の書き出しが完了しました`];
      if (r?.path) lines.push(String(r.path));
      if (r?.srt) lines.push(`${r.srt}（${r.cues} cue）`);
      if (r?.wav) lines.push(`${r.wav}（${r.count} 個）`);
      for (const w of r?.warnings ?? []) lines.push("⚠ " + w);
      toast(lines.join("\n"), r?.warnings?.length ? "info" : "ok", 12000);
    } catch (e) {
      toastError(`${label}を書き出せませんでした`, e, 12000);
    }
  };
  // コンポーネントにせず関数で描く（ExportMenu の中で定義したコンポーネントは描き直しのたびに別物になり、
  // 押している間に TopBar が描き直されると要素が差し替わってクリックが届かない）
  const item = ({ k, label, sub, ranged }: { k: string; label: string; sub: string; ranged?: boolean }) => (
    <div key={k} className="mi" onClick={() => run(k, label, ranged)}>
      <span>{label}{ranged && hasRange ? "（イン〜アウト）" : ""}</span>
      <small>{sub}</small>
    </div>
  );
  return (
    <div className="menu" style={{ right: 60, top: 38 }} onMouseLeave={onClose}>
      {item({ k: "talk", label: "台本 → 演出の入力", sub: "script.srt・cue ごとの声（files/）・フル WAV（voice_out/）" })}
      {item({ k: "se", label: "効果音", sub: "fx/out/<名前>_se.wav・se_stems/（再生にも使用）" })}
      <hr />
      {item({ k: "mov", label: "演出 .mov（確認用）", sub: "仮表示・シーン番号・調整を反映", ranged: true })}
      {item({ k: "mov_clean", label: "演出 .mov（本番用）", sub: "Premiere 用の透過 qtrle（_clean.mov）", ranged: true })}
      <hr />
      {item({ k: "preview_mp4", label: "確認用 mp4（1280×720）", sub: "全トラックを重ねた縮小版（studio/export/）", ranged: true })}
      {item({ k: "mp4", label: "完成版 mp4（1920×1080）", sub: "背景・録画・立ち絵・演出・字幕・声・効果音・BGM", ranged: true })}
      {item({ k: "mix", label: "音声ミックス wav", sub: isGuide ? "素材の音・効果音・BGM（studio/export/mix.wav）" : "声・効果音・BGM（studio/export/mix.wav）" })}
      {isGuide && item({ k: "guide_audio", label: "素材の音（wav）", sub: "通しの素材の音だけ・音量は掛けない（studio/export/<回>_guide_audio.wav）", ranged: true })}
      {item({ k: "rec_track", label: "録画トラック mp4", sub: "RecWindow の録画だけ・黒背景 1920×1080（studio/export/material_track.mp4）" })}
      <hr />
      {item({ k: "subs_mov", label: "字幕 .mov（透過）", sub: "字幕だけの透過 qtrle ＋ .srt（fx/out/<名前>_subs.mov）", ranged: true })}
      {item({ k: "subs_srt", label: "字幕 .srt", sub: "改行・折り返し・字幕の書き換えを反映（fx/out/<名前>_subs.srt）", ranged: true })}
    </div>
  );
}

export default function TopBar() {
  const project = useS((s) => s.project);
  const engine = useS((s) => s.engine);
  const voice = useS((s) => s.voice);
  // 丸の名前: つながったらエンジンの名前、つながる前は「声」
  const voiceLabel = voice?.ok ? engineOf(voice.info).label : "声";
  const claude = useS((s) => s.claude);
  const progress = useS((s) => s.progress);
  const playSynthN = useS((s) => s.playSynthN);
  const [dlg, setDlg] = useState<null | "project" | "log" | "settings">(null);
  const [menu, setMenu] = useState(false);
  // どの版か（ビルドのコミットと時刻。旧#163）
  const [build, setBuild] = useState("");
  useEffect(() => {
    call("build_info").then(setBuild, () => setBuild("版不明"));
  }, []);
  const st = engine?.status;
  const eDot = !st || st.state === "none" ? "" : st.state === "ready" ? "ok" : st.state === "loading" ? "busy" : "err";
  const { warns, infos } = splitWarnings(engine?.warnings);
  const eTip = [
    st?.error ?? (st?.failed_workers ? `${st.failed_workers} 本のワーカーを読み込めませんでした:\n${st.worker_error ?? ""}` : ""),
    warns.length ? `警告 ${warns.length} 件:\n` + warns.map((w) => "・" + w.message).join("\n") : "",
    infos.length ? infos.map((w) => "・" + w.message).join("\n") : "",
  ].filter(Boolean).join("\n\n");
  return (
    <div className="topbar">
      <div className="brand">
        <svg width="22" height="22" viewBox="0 0 22 22">
          <path d="M5 1h16v15l-5 5H1V6z" style={{ fill: "var(--logo-bg)", stroke: "var(--accent)" }} strokeWidth="1.5" />
          <text x="11" y="15" textAnchor="middle" style={{ fill: "var(--text)", fontFamily: "var(--ui)" }} fontSize="9" fontWeight="700">AK</text>
        </svg>
        {branding.title}
        {branding.sub && <span className="sub">{branding.sub}</span>}
        {build && <span className="sub build" title="ビルドしたときのコミット（+ はコミットしていない変更入り）と日時">{build}</span>}
        {engine?.kit && <span className="sub build" title="演出に使っている kit（非公開の kit のコミットと、kit の約束の版）">{engine.kit}</span>}
      </div>
      <button onClick={() => setDlg("project")}>{project ? `回: ${project.name}` : "回を開く"} ▾</button>
      {project && (
        <select
          value={project.doc.fx_timeline ?? ""}
          onChange={(e) => editDoc((d) => { d.fx_timeline = e.target.value || null; })}
          title="演出のタイムライン（fx/*.py）"
        >
          {!project.doc.fx_timeline && <option value="">（演出なし）</option>}
          {project.timelines.map((t) => (
            <option key={t} value={t}>{t}</option>
          ))}
        </select>
      )}
      <span className="pill" style={{ cursor: "pointer" }} onClick={() => setDlg("log")} title={eTip}>
        <span className={`dot ${eDot}`} />
        演出 {st?.state === "ready" ? `${st.ready_workers}/${st.workers}` : st?.state === "loading" ? "読み込み中" : st?.state === "error" ? "エラー" : "—"}
        {engine && <span className="dim2 ui">· {engine.cache_mb.toFixed(0)}MB</span>}
        {warns.length > 0 && <span className="chip">警告 {warns.length}</span>}
      </span>
      <span
        className="pill"
        style={{ cursor: "pointer" }}
        title={voice?.error ?? `${voiceLabel}の中継。クリックで再起動します`}
        onClick={async () => {
          try {
            const v = await call("voice_restart");
            useS.setState({ voice: { ok: true, info: v } });
            toast("声の中継を起動しました", "ok");
          } catch (e) {
            useS.setState({ voice: { ok: false, error: errorMessage(e) } });
            toastError("声の中継を起動できませんでした", e, 9000);
          }
        }}
      >
        <span className={`dot ${voice?.ok ? "ok" : voice ? "err" : ""}`} />
        {voiceLabel}
        {playSynthN > 0 && <span className="dim2 ui">· 再生しながら音声を作成中（{playSynthN} 行）</span>}
      </span>
      <span className="pill" title={claude.api_error ? claude.api_error : claude.watching ? "Claude Code が画面からのメッセージを待っています" : claude.connected ? "Claude Code が作業中です" : "Claude Code に接続していません。Claude タブの「Claude を起動」で起動します"}>
        <span className={`dot ${claude.status ? "busy" : claude.watching || claude.connected ? "ok" : ""}`} />
        Claude {claude.status || (claude.watching ? "待機中" : claude.connected ? "接続中" : "未接続")}
        {claude.pending > 0 && <span className="chip">未読 {claude.pending}</span>}
      </span>
      <span className="grow" />
      <DriftWarning />
      {Object.values(progress).map((p) => (
        <div key={p.task} className="col" style={{ width: 200, gap: 2 }}>
          <div className="row" style={{ fontSize: 11 }}>
            <span className="grow dim" style={{ overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>{p.msg || p.task}</span>
            {!p.finished && p.task !== "render" && p.task !== "guide_scan" && (
              <button className="ghost" style={{ padding: "0 4px" }} onClick={() => call("export_cancel")}>中止</button>
            )}
          </div>
          <div className="progress"><div style={{ width: `${(p.total ? p.done / p.total : 0) * 100}%` }} /></div>
        </div>
      ))}
      <div style={{ position: "relative" }}>
        <button className="primary" disabled={!project} onClick={() => setMenu(!menu)}>書き出し ▾</button>
      </div>
      {menu && <ExportMenu onClose={() => setMenu(false)} />}
      <button className="ghost" onClick={() => setDlg("settings")} title="設定">⚙</button>
      {dlg === "project" && <ProjectPicker onClose={() => setDlg(null)} />}
      {dlg === "log" && <EngineLog onClose={() => setDlg(null)} />}
      {dlg === "settings" && <SettingsDialog onClose={() => setDlg(null)} />}
    </div>
  );
}
