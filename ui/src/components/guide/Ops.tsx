import React from "react";
import type { GuideState } from "../../gen/GuideState";
import { toast, useS } from "../../state";
import { nowLeadFromHere, nowLeadInputError, nowLeadText, nowStartLabel, OP_KINDS, opRows } from "../../logic/guide";
import { tc } from "../../logic/util";
import { addOpHere, editOp, removeOp, seekOp, stepOp } from "./actions";
import { SecInput, TextInput } from "./inputs";

/** 手動操作解説の操作の一覧。行を押すとそのコマへ飛ぶ。◀ ▶ で素材の 1 コマずつ（Shift で 10 コマ）動かせる */
export default function GuideOps({ g }: { g: GuideState }) {
  const guide = g.guide;
  const sel = useS((s) => s.guideOp);
  if (!guide) return null;
  const rows = opRows(g);
  const step = (id: string, dir: 1 | -1) => (e: React.MouseEvent) => {
    e.stopPropagation();
    stepOp(id, dir * (e.shiftKey ? 10 : 1));
  };
  return (
    <>
      <div className="section">OPERATIONS</div>
      <div className="row">
        <button onClick={addOpHere} disabled={g.plan_parts.length === 0} title="モニターの今の位置（手動パートの中）に操作を足します">＋操作</button>
        <span className="dim2" style={{ fontSize: 11 }}>{rows.length} 件。信頼枠の配置は薄く表示し、精密な操作には ◆ が付きます</span>
      </div>
      {rows.length === 0 && <div className="dim2" style={{ fontSize: 11 }}>操作はありません。手動パートの好きな位置にモニターを置いて「＋操作」を押してください</div>}
      {rows.map((r) => (
        <div
          key={r.id}
          className={`op-card ${sel === r.id ? "sel" : ""} ${r.trust ? "trust" : ""}`}
          onClick={() => seekOp(r.id)}
        >
          <div className="row" style={{ gap: 4, flexWrap: "wrap" }}>
            <span className="ui op-t" title={`素材の時刻 ${tc(r.t, 60)}（${r.id}）`}>
              {r.precise && !r.trust ? "◆ " : ""}{r.tOut === null ? "—" : tc(r.tOut, 60)}
            </span>
            <select
              value={OP_KINDS.some((k) => k.kind === r.kind) ? r.kind : ""}
              onClick={(e) => e.stopPropagation()}
              onChange={(e) => editOp(r.id, { kind: e.target.value })}
              title="操作の種類"
            >
              {!OP_KINDS.some((k) => k.kind === r.kind) && <option value="">（未設定）</option>}
              {OP_KINDS.map((k) => <option key={k.kind} value={k.kind}>{k.label}</option>)}
            </select>
            <span onClick={(e) => e.stopPropagation()} style={{ display: "contents" }}>
              <TextInput value={r.op} placeholder="キャラ" style={{ width: 84 }} onCommit={async (v) => editOp(r.id, { op: v })} />
              <TextInput value={r.where} placeholder="置き場所・向き（R3C5 ↑）" style={{ width: 130 }} onCommit={async (v) => editOp(r.id, { where: v })} />
              <TextInput value={r.info} placeholder="補足（S2 は溜めてから）" title="動画の NOW / NEXT と下の帯に小さく出る補足です" style={{ width: 150 }} onCommit={async (v) => editOp(r.id, { info: v })} />
            </span>
            <label className="op-precise" onClick={(e) => e.stopPropagation()} title="フレーム単位で合わせる操作（コストバーを拡大して強調します）">
              <input type="checkbox" checked={r.precise} onChange={(e) => editOp(r.id, { precise: e.target.checked })} />
              精密
            </label>
            <span className="grow" />
            <button className="ghost" onClick={step(r.id, -1)} title="素材の 1 コマ前へ（Shift で 10 コマ）">◀</button>
            <button className="ghost" onClick={step(r.id, 1)} title="素材の 1 コマ先へ（Shift で 10 コマ）">▶</button>
            <button className="ghost" onClick={(e) => { e.stopPropagation(); seekOp(r.id); }} title="モニターをこの操作のコマへ動かします">ここへ</button>
            <button
              className="ghost"
              onClick={(e) => {
                e.stopPropagation();
                removeOp(r.id);
              }}
              title="この操作を消します（ほかの操作の番号は変わりません）"
            >
              削除
            </button>
          </div>
          <div className="row" style={{ gap: 6, flexWrap: "wrap", marginTop: 3 }} onClick={(e) => e.stopPropagation()}>
            <span className="ui dim op-cost" title="このコマのコストと、ゲーム内の時間（分:秒:フレーム）">
              {r.cost === null ? "コスト —" : `コスト ${r.cost}`}　{r.game || "—"}
            </span>
            <TextInput value={r.cue_text} placeholder="コストでの指定（Kill15 の敵が出た数字 +12）" style={{ flex: 1, minWidth: 140 }} onCommit={async (v) => editOp(r.id, { cue_text: v })} />
            <TextInput value={r.note} placeholder="注意書き" style={{ flex: 1, minWidth: 100 }} onCommit={async (v) => editOp(r.id, { note: v })} />
          </div>
          {!r.trust && (
            <div className="row" style={{ gap: 6, flexWrap: "wrap", marginTop: 3 }} onClick={(e) => e.stopPropagation()}>
              <span className="dim2" style={{ fontSize: 11 }} title="NOW を操作の瞬間より何秒前に出すか。空にすると固定値の汎用値です">NOW の先出し</span>
              <SecInput
                value={r.nowLead}
                placeholder={`汎用 ${nowLeadText(r.nowLeadGeneric)} 秒`}
                width={96}
                onCommit={async (v) => {
                  if (v !== null) {
                    const err = nowLeadInputError(v);
                    if (err) {
                      toast(err, "error");
                      return false;
                    }
                  }
                  return editOp(r.id, { now_lead: v ?? undefined });
                }}
              />
              <button
                className="ghost"
                onClick={() => {
                  const f = nowLeadFromHere(r.tOut, useS.getState().t);
                  if (!f.ok) toast(f.error, "error");
                  else void editOp(r.id, { now_lead: f.lead });
                }}
                title="モニターの今の位置から操作の瞬間までを、この操作の先出しにします"
              >
                再生位置から
              </button>
              {(() => {
                const l = nowStartLabel(r, (t) => tc(t, 60));
                return l && <span className="ui dim" title={l.title}>{l.text}</span>;
              })()}
            </div>
          )}
          {r.problem && <div className="op-problem">⚠ {r.problem}</div>}
        </div>
      ))}
    </>
  );
}
