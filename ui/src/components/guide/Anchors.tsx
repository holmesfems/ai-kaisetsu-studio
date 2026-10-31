import React from "react";
import type { GuideState } from "../../gen/GuideState";
import { player } from "../../io/player";
import { anchorRows, fmtSeconds, withAnchorT } from "../../logic/guide";
import { tc } from "../../logic/util";
import { editGuide, seekAnchor, setAnchorHere, stepAnchor } from "./actions";
import { SecInput, td, th } from "./inputs";

/** アンカー（素材の基準のコマ）の表。◀ ▶ で素材の 1 コマずつ（Shift で 10 コマ）動かせる */
export default function GuideAnchors({ g }: { g: GuideState }) {
  const guide = g.guide;
  if (!guide) return null;
  const rows = anchorRows(g);
  const step = (name: string, dir: 1 | -1) => (e: React.MouseEvent) => stepAnchor(name, dir * (e.shiftKey ? 10 : 1));
  return (
    <>
      <div className="section">ANCHORS</div>
      <table style={{ borderCollapse: "collapse", width: "100%" }}>
        <thead>
          <tr><th style={th}>アンカー</th><th style={th}>候補（秒）</th><th style={th}>使う値（秒）</th><th style={th}>出力</th><th style={th}></th></tr>
        </thead>
        <tbody>
          {rows.map((a) => (
            <tr key={a.name}>
              <td style={td} title={a.name}>{a.label}</td>
              <td style={td} className="ui dim">{fmtSeconds(a.auto) || "—"}</td>
              <td style={td}>
                <SecInput
                  value={a.t}
                  placeholder={fmtSeconds(a.auto)}
                  onCommit={async (v) => {
                    const ok = await editGuide((g2) => withAnchorT(g2, a.name, v));
                    if (ok) seekAnchor(a.name);
                    return ok;
                  }}
                />
              </td>
              <td style={td} className="ui dim">{a.out === null ? "—" : tc(a.out, 60)}</td>
              <td style={td}>
                <div className="row" style={{ gap: 4, flexWrap: "wrap" }}>
                  <button className="ghost" disabled={a.value === null} onClick={step(a.name, -1)} title="素材の 1 コマ前へ（Shift で 10 コマ）">◀</button>
                  <button className="ghost" disabled={a.value === null} onClick={step(a.name, 1)} title="素材の 1 コマ先へ（Shift で 10 コマ）">▶</button>
                  <button className="ghost" onClick={() => setAnchorHere(a.name)} title="モニターの今の位置のコマをこのアンカーにします（そのアンカーのパートの中だけ）">今のコマにする</button>
                  <button className="ghost" disabled={a.out === null} onClick={() => a.out !== null && player.seek(a.out)} title="モニターをこのアンカーの位置へ動かします">ここへ</button>
                  <button className="ghost" disabled={a.t === null} onClick={() => editGuide((g2) => withAnchorT(g2, a.name, null))} title="手で決めた値を消して、解析の候補を使います">候補に戻す</button>
                </div>
              </td>
            </tr>
          ))}
        </tbody>
      </table>
    </>
  );
}
