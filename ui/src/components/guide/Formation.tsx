import React, { useEffect, useState } from "react";
import type { GuideState } from "../../gen/GuideState";
import { akf } from "../../io/api";
import { toast } from "../../state";
import { slotRows, slotToggleError, withAllUsed, withSlotNote, withSlotToggled } from "../../logic/guide";
import { editGuide } from "./actions";
import { TextInput } from "./inputs";

/** 編成: 枠の絵（編成パートの暗転の 1 秒前のコマから切り抜き）を 6×2 で並べる。クリックで「使う / 使わない」 */
export default function GuideFormation({ g }: { g: GuideState }) {
  const guide = g.guide;
  // 暗転のアンカーや素材を変えたら絵を取り直す（URL の v で）
  const ver = `${guide?.src ?? ""}|${g.anchor_out["formation_dark"] ?? ""}`;
  // 出せなかった枠の絵の理由（本体の返した文。黙って壊れた絵にしない）
  const [bad, setBad] = useState<Record<number, string>>({});
  useEffect(() => setBad({}), [ver]);
  if (!guide) return null;
  const rows = slotRows(guide);
  // 枠の絵は、組み立てられていて、解析が読めているときだけ出せる
  const ready = g.plan_parts.length > 0;
  const allUsed = guide.formation.used.length === 0;
  const onBad = (i: number, url: string) => {
    fetch(url)
      .then((r) => r.text())
      .catch((e) => String(e))
      .then((t) => setBad((b) => ({ ...b, [i]: t || "理由が分かりません" })));
  };
  return (
    <>
      <div className="section">FORMATION</div>
      <div className="row">
        <span className="dim2" style={{ fontSize: 11 }}>
          使わない枠は編成パートで暗くなります（暗転の手前で元に戻ります）
        </span>
        <span className="grow" />
        <button disabled={allUsed} onClick={() => editGuide(withAllUsed)} title="全部の枠を使う（暗くしない）に戻します">全部使う</button>
      </div>
      <div className="slot-grid">
        {rows.map((r) => (
          <div key={r.index} className={`slot ${r.used ? "" : "off"}`}>
            <div
              className="slot-img"
              title={`枠 ${r.index + 1}（クリックで「${r.used ? "使わない" : "使う"}」に切り替え）`}
              onClick={() => {
                const err = slotToggleError(guide, r.index);
                if (err) toast(err, "error");
                else editGuide((g2) => withSlotToggled(g2, r.index));
              }}
            >
              {ready && bad[r.index] ? (
                <div className="dim2" style={{ fontSize: 10, padding: 6, color: "var(--error)" }} title={bad[r.index]}>
                  絵を出せませんでした
                </div>
              ) : ready ? (
                <img
                  src={akf("/guide_slot", { i: r.index, s: 0.3, v: ver })}
                  draggable={false}
                  alt={`枠 ${r.index + 1}`}
                  onError={(e) => onBad(r.index, e.currentTarget.src)}
                />
              ) : (
                <div className="dim2" style={{ fontSize: 10, padding: 6 }}>絵なし</div>
              )}
              <span className="slot-no">{r.index + 1}</span>
              <span className="slot-state">{r.used ? "使う" : "使わない"}</span>
            </div>
            <TextInput
              value={r.note}
              placeholder="補足（Lv・凸・モジュール）"
              title="この枠のキャラに求める値など（演出で表示します）"
              style={{ width: "100%", fontSize: 11 }}
              onCommit={async (v) => editGuide((g2) => withSlotNote(g2, r.index, v))}
            />
          </div>
        ))}
      </div>
    </>
  );
}
