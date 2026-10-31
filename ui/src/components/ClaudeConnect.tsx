import { useState } from "react";
import { call } from "../io/api";
import { toastError } from "../state";
import type { Delivery } from "../gen/Delivery.ts";
import { failText } from "../logic/errors.ts";
import { CLAUDE_ACTIONS, DELIVERY } from "../logic/terms.ts";

/** Claude の接続の案内とボタン（登録・起動・フックのみ設定）。設定のダイアログと Claude タブで共通。
 *  delivery は選んでいる送信方法（設定のダイアログでは保存前の値）。saved は保存済みの値で、
 *  「Claude を起動」は保存済みの値で動くので、食い違うあいだは起動を止めて保存を促す */
export function ClaudeConnect({ delivery, saved = delivery }: { delivery: Delivery; saved?: Delivery }) {
  const unsaved = delivery !== saved;
  const [result, setResult] = useState("");
  return (
    <>
      <div className="dim" style={{ fontSize: 12, lineHeight: 1.8 }}>
        1. 「{CLAUDE_ACTIONS.register.label}」（最初の 1 回だけ。ak-studio のツールと Mod を入れます）<br />
        2. 「{CLAUDE_ACTIONS.launch.label}」で、この回のフォルダでターミナルが開きます<br />
        　{DELIVERY[delivery].hint}<br />
        3. Claude タブで書いたメッセージで、待っている Claude が動き出します
      </div>
      <div className="row" style={{ marginTop: 6 }}>
        <button title={CLAUDE_ACTIONS.register.title} onClick={async () => {
          try {
            const r = await call("mcp_setup");
            setResult((r.log ?? []).join(" / "));
          } catch (e) {
            setResult(failText("Claude Code に登録できませんでした", e));
          }
        }}>{CLAUDE_ACTIONS.register.label}</button>
        <button className="primary" disabled={unsaved}
          title={unsaved ? "送信方法の変更を保存してから起動してください" : CLAUDE_ACTIONS.launch.title}
          onClick={() => call("launch_claude").catch((e) => toastError("Claude を起動できませんでした", e))}>
          {CLAUDE_ACTIONS.launch.label}
        </button>
        {delivery === "hook" && (
          <button title={CLAUDE_ACTIONS.hooks.title} onClick={async () => {
            try {
              const r = await call("hooks_setup");
              setResult(`フックを設定しました: ${r.path}`);
            } catch (e) {
              setResult(failText("フックを設定できませんでした", e));
            }
          }}>{CLAUDE_ACTIONS.hooks.label}</button>
        )}
        <span className="dim" style={{ fontSize: 12 }}>{result}</span>
      </div>
    </>
  );
}
