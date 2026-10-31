import React from "react";
import type { GuideState } from "../../gen/GuideState";
import { useS } from "../../state";
import { player } from "../../io/player";
import { cutRows, withCutFade, withoutCut } from "../../logic/guide";
import { tc } from "../../logic/util";
import { addCutFromMarks, editGuide } from "./actions";
import { SecInput, td, th } from "./inputs";

/** 手動パートのカット（素材の時刻の区間を抜いて詰める。継ぎ目はクロスフェード） */
export default function GuideCuts({ g }: { g: GuideState }) {
  const guide = g.guide;
  const inT = useS((s) => s.inT);
  const outT = useS((s) => s.outT);
  if (!guide) return null;
  const rows = cutRows(g);
  return (
    <>
      <div className="section">CUTS</div>
      <div className="row" style={{ flexWrap: "wrap" }}>
        <button
          disabled={inT === null || outT === null}
          onClick={addCutFromMarks}
          title="タイムラインのイン（I）・アウト（O）の範囲を、手動パートからカットします（両端が手動パートの中のとき）"
        >
          I・O の範囲をカット
        </button>
        <span className="dim2" style={{ fontSize: 11 }}>
          {inT !== null && outT !== null ? `${tc(inT, 60)} 〜 ${tc(outT, 60)}` : "タイムラインにイン（I）とアウト（O）を打つと使えます"}
        </span>
      </div>
      {rows.length === 0 ? (
        <div className="dim2" style={{ fontSize: 11 }}>カットはありません</div>
      ) : (
        <table style={{ borderCollapse: "collapse", width: "100%" }}>
          <thead>
            <tr><th style={th}>始まり（素材）</th><th style={th}>終わり（素材）</th><th style={th}>長さ</th><th style={th}>フェード（秒）</th><th style={th}></th></tr>
          </thead>
          <tbody>
            {rows.map((c) => (
              <tr key={c.index}>
                <td style={td} className="ui">{tc(c.srcIn, 60)}</td>
                <td style={td} className="ui">{tc(c.srcOut, 60)}</td>
                <td style={td} className="ui dim">{c.len.toFixed(2)} 秒</td>
                <td style={td}>
                  <SecInput width={60} value={c.fade} onCommit={(v) => editGuide((g2) => withCutFade(g2, c.index, v ?? 0))} />
                </td>
                <td style={td}>
                  <div className="row" style={{ gap: 4 }}>
                    <button className="ghost" disabled={c.outIn === null} onClick={() => c.outIn !== null && player.seek(c.outIn)} title="モニターをこの継ぎ目へ動かします">ここへ</button>
                    <button className="ghost" onClick={() => editGuide((g2) => withoutCut(g2, c.index))} title="このカットを消して、抜いた区間を戻します">削除</button>
                  </div>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
    </>
  );
}
