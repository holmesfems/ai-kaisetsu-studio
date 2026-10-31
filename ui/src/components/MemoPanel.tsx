import React from "react";
import { editScript, sendChat, useS } from "../state";

export default function MemoPanel() {
  const memo = useS((s) => s.script?.memo ?? "");
  const has = useS((s) => !!s.script);
  if (!has) return <div className="empty">台本がありません。台本タブで作成してください</div>;
  return (
    <div className="col" style={{ flex: 1, padding: 10, minHeight: 0 }}>
      <div className="row">
        <span className="tag">// _memo.md</span>
        <span className="grow" />
        <button onClick={() => sendChat("構成メモを読んで、台本の流れ（尺・順番・抜けている話題）を見てください", { kind: "memo_review" })}>Claude に構成を見てもらう</button>
      </div>
      <textarea
        style={{ flex: 1, resize: "none", lineHeight: 1.7, fontSize: 13 }}
        value={memo}
        spellCheck={false}
        onChange={(e) => editScript((s) => { s.memo = e.target.value; }, false)}
        placeholder="構成メモ（コンセプト・構成・尺の目安など。Markdown）"
      />
    </div>
  );
}
