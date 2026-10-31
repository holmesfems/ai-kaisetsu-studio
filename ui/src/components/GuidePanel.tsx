import React from "react";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { createGuide, createGuideFx, saveGuideBench, saveGuideFixedDefault, scanGuide, undoClaudeGuide, useS } from "../state";
import { editGuide } from "../state/guide";
import { player } from "../io/player";
import { analysisSummary, BENCH_ITEMS, claudeUndoLabel, claudeUndoTitle, fixedDiffersFromSettings, fixedRows, fmtSeconds, partRows, withFixedOwn } from "../logic/guide";
import { VIDEO_EXTS } from "../logic/media";
import { tc } from "../logic/util";
import GuideAnchors from "./guide/Anchors";
import GuideCuts from "./guide/Cuts";
import GuideFormation from "./guide/Formation";
import GuideOps from "./guide/Ops";
import { BenchInput, SecInput, td, th } from "./guide/inputs";

export default function GuidePanel() {
  const g = useS((s) => s.guide);
  const root = useS((s) => s.project?.root);
  const progress = useS((s) => s.progress["guide_scan"]);
  const hasFx = useS((s) => !!s.project?.doc.fx_timeline);
  const [undoing, setUndoing] = React.useState(false);
  if (!g) return <div className="empty">攻略動画の状態を読み込んでいます…</div>;

  const pickVideo = async () => {
    const r = await openDialog({ multiple: false, defaultPath: root, filters: [{ name: "動画", extensions: [...VIDEO_EXTS] }] });
    return typeof r === "string" ? r : null;
  };

  if (!g.guide) {
    return (
      <div className="col" style={{ flex: 1, minHeight: 0, padding: 10, overflow: "auto" }}>
        <div className="dim2" style={{ fontSize: 12, lineHeight: 1.7 }}>
          攻略動画（手動操作の通しと自動指揮の素材を、パートに組み立てる動画）にします。回のフォルダの中の動画を選ぶと、studio/guide.json を作ります。
        </div>
        {g.error && <div style={{ color: "var(--error)", whiteSpace: "pre-wrap", fontSize: 12 }}>{g.error}</div>}
        <div className="row">
          <button
            className="primary"
            onClick={async () => {
              const p = await pickVideo();
              if (p) await createGuide(p);
            }}
          >
            攻略動画にする…
          </button>
        </div>
      </div>
    );
  }

  const guide = g.guide;
  const fixed = fixedRows(g);
  const parts = partRows(g);
  const scanning = g.scanning;
  const pct = progress && progress.total ? Math.round((progress.done / progress.total) * 100) : null;

  return (
    <div className="col" style={{ flex: 1, minHeight: 0, padding: 10, overflow: "auto" }}>
      {g.claude_undo && (
        <div className="row" style={{ flexWrap: "wrap" }}>
          <span className="dim2" style={{ fontSize: 11 }}>Claude が攻略動画を直しました</span>
          <span className="grow" />
          <button
            disabled={undoing}
            onClick={async () => {
              // 押している間は無効にする（2 回押して、1 つ前の変更まで戻さないように）。本体にも見せている変更の時刻を渡す
              setUndoing(true);
              try {
                await undoClaudeGuide(g.claude_undo!.id);
              } finally {
                setUndoing(false);
              }
            }}
            title={claudeUndoTitle(g.claude_undo)}
          >
            {claudeUndoLabel(g.claude_undo)}
          </button>
        </div>
      )}
      {!hasFx && (
        <div className="row" style={{ flexWrap: "wrap" }}>
          <span className="dim2" style={{ fontSize: 11 }}>演出（NOW/NEXT・操作の帯など）がまだありません</span>
          <span className="grow" />
          <button
            className="primary"
            onClick={createGuideFx}
            title="fx/guide_fx.py を作って、攻略動画の決まった演出を読み込みます"
          >
            演出のファイルを作る
          </button>
        </div>
      )}
      <div className="section" style={{ marginTop: 0 }}>SOURCE</div>
      <div className="row" style={{ flexWrap: "wrap" }}>
        <span className="mono" style={{ fontSize: 12 }}>{guide.src}</span>
        <span className="grow" />
        <button
          disabled={scanning}
          onClick={async () => {
            const p = await pickVideo();
            if (p) await editGuide((g2) => ({ ...g2, src: p }));
          }}
        >
          素材を選び直す…
        </button>
        <button className="primary" disabled={scanning} onClick={scanGuide} title="素材を 1 回なめて、各コマの時刻とアンカーの候補を求めます（数分かかります）">
          {scanning ? "解析しています…" : "解析"}
        </button>
      </div>
      {scanning && (
        <div className="progress"><div style={{ width: `${pct ?? 0}%` }} /></div>
      )}
      <div className="dim2" style={{ fontSize: 11 }}>{analysisSummary(g)}</div>
      {g.analysis.warnings.length > 0 && (
        <div style={{ fontSize: 11, color: "var(--warn)", lineHeight: 1.6 }}>
          {g.analysis.warnings.map((w, i) => <div key={i}>⚠ {w}</div>)}
        </div>
      )}
      {g.error && <div style={{ color: "var(--error)", whiteSpace: "pre-wrap", fontSize: 12 }}>{g.error}</div>}
      {g.warnings.length > 0 && (
        <div style={{ fontSize: 11, color: "var(--warn)", lineHeight: 1.6 }}>
          {g.warnings.map((w, i) => <div key={i}>⚠ {w}</div>)}
        </div>
      )}

      <GuideAnchors g={g} />

      <div className="section">FIXED</div>
      <table style={{ borderCollapse: "collapse", width: "100%" }}>
        <thead>
          <tr><th style={th}>項目</th><th style={th}>効いている値（秒）</th><th style={th}>この回だけ（秒）</th><th style={th}>設定の値</th></tr>
        </thead>
        <tbody>
          {fixed.map((f) => (
            <tr key={f.key}>
              <td style={td} title={f.hint}>{f.label}</td>
              <td style={td} className="ui">{fmtSeconds(f.effective)}</td>
              <td style={td}>
                <SecInput
                  value={f.own}
                  placeholder={fmtSeconds(f.setting)}
                  onCommit={(v) => editGuide((g2) => withFixedOwn(g2, f.key, v))}
                />
              </td>
              <td style={td} className="ui dim">{fmtSeconds(f.setting)}</td>
            </tr>
          ))}
        </tbody>
      </table>
      <div className="row">
        <span className="dim2" style={{ fontSize: 11 }}>空欄にすると設定の値を使います</span>
        <span className="grow" />
        <button
          disabled={!fixedDiffersFromSettings(g)}
          onClick={() => saveGuideFixedDefault(g.fixed_effective)}
          title="今効いている値を設定に保存して、ほかの回でも使います"
        >
          設定に保存
        </button>
      </div>

      <div className="row" style={{ flexWrap: "wrap", marginTop: 6 }}>
        <span style={{ fontSize: 12 }} title="編成の下のキャラの待機欄（12 枠すべて）を、レイヤーの左下に小さく半透明で重ねます。設定に保存して、全部の回で使います">待機欄（編成）</span>
        {BENCH_ITEMS.map((it) => (
          <span key={it.key} className="row" title={it.hint}>
            <span className="dim2" style={{ fontSize: 11 }}>{it.label}</span>
            <BenchInput bench={g.bench} field={it.key} onSave={saveGuideBench} />
            <span className="dim2" style={{ fontSize: 11 }}>%</span>
          </span>
        ))}
      </div>

      <div className="section">PARTS</div>
      {parts.length === 0 ? (
        <div className="dim">パートを組み立てられていません。素材を解析して、アンカーを確かめてください</div>
      ) : (
        <table style={{ borderCollapse: "collapse", width: "100%" }}>
          <thead>
            <tr><th style={th}>パート</th><th style={th}>開始</th><th style={th}>長さ</th></tr>
          </thead>
          <tbody>
            {parts.map((p) => (
              <tr key={p.name} style={{ cursor: "pointer" }} onClick={() => player.seek(p.t0)}>
                <td style={td}>{p.label}</td>
                <td style={td} className="ui">{tc(p.t0, 60)}</td>
                <td style={td} className="ui">{tc(p.len, 60)}</td>
              </tr>
            ))}
          </tbody>
        </table>
      )}

      <GuideCuts g={g} />
      <GuideFormation g={g} />
      <GuideOps g={g} />
    </div>
  );
}
