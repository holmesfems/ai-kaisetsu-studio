import React, { useEffect, useState } from "react";
import { toast } from "../../state";
import type { GuideBench } from "../../gen/GuideBench";
import { benchFromPercent, benchPercentText, fmtSeconds, parseSeconds } from "../../logic/guide";

/** 攻略タブの表の見た目 */
export const th: React.CSSProperties = { textAlign: "left", fontWeight: "normal", color: "var(--dim2)", fontSize: 11, padding: "2px 6px" };
export const td: React.CSSProperties = { padding: "3px 6px", borderTop: "1px solid var(--line)", verticalAlign: "middle" };

/** 秒の入力欄。Enter か欄を出たときに確定し、空にすると null（値を消す）。読めない入力・保存できなかった入力は元に戻す */
export function SecInput({ value, placeholder, width = 84, onCommit }: {
  value: number | null;
  placeholder?: string;
  width?: number;
  onCommit: (v: number | null) => Promise<boolean>;
}) {
  const [text, setText] = useState(fmtSeconds(value));
  useEffect(() => setText(fmtSeconds(value)), [value]);
  const commit = () => {
    const v = parseSeconds(text);
    if (v === undefined) {
      toast("秒で入力してください（例: 12.4、1:23.5）", "error");
      setText(fmtSeconds(value));
      return;
    }
    // 保存を断られたら（知らせは saveGuide が出す）、保存した値に戻す。value は変わらないので useEffect では戻らない
    // 見えている桁で比べる（1/60 秒の値などを、フォーカスを外しただけで丸めて保存し直さない）
    if (fmtSeconds(v) !== fmtSeconds(value)) void onCommit(v).then((ok) => { if (!ok) setText(fmtSeconds(value)); });
  };
  return (
    <input
      className="ui"
      style={{ width }}
      value={text}
      placeholder={placeholder}
      spellCheck={false}
      onChange={(e) => setText(e.target.value)}
      onBlur={commit}
      onKeyDown={(e) => {
        if (e.key === "Enter") (e.target as HTMLInputElement).blur();
        if (e.key === "Escape") setText(fmtSeconds(value));
      }}
    />
  );
}

/** 文字の入力欄。Enter か欄を出たときに確定する。保存できなかったら元に戻す */
export function TextInput({ value, placeholder, title, style, onCommit }: {
  value: string;
  placeholder?: string;
  title?: string;
  style?: React.CSSProperties;
  onCommit: (v: string) => Promise<boolean>;
}) {
  const [text, setText] = useState(value);
  useEffect(() => setText(value), [value]);
  const commit = () => {
    if (text !== value) void onCommit(text).then((ok) => { if (!ok) setText(value); });
  };
  return (
    <input
      value={text}
      placeholder={placeholder}
      title={title}
      style={style}
      spellCheck={false}
      onChange={(e) => setText(e.target.value)}
      onBlur={commit}
      onKeyDown={(e) => {
        if (e.key === "Enter") (e.target as HTMLInputElement).blur();
        if (e.key === "Escape") setText(value);
      }}
    />
  );
}

/** 待機欄の % の入力欄。Enter か欄を出たときに確定してすぐ保存する。読めない・範囲の外・保存できなかった入力は元に戻す */
export function BenchInput({ bench, field, onSave }: {
  bench: GuideBench;
  field: keyof GuideBench;
  onSave: (b: GuideBench) => Promise<boolean>;
}) {
  const shown = benchPercentText(bench[field]);
  const [text, setText] = useState(shown);
  useEffect(() => setText(shown), [shown]);
  const commit = () => {
    if (text.trim() === shown) return;
    const r = benchFromPercent(bench, field, text);
    if (!r.ok) {
      toast(r.error, "error");
      setText(shown);
      return;
    }
    void onSave(r.bench).then((ok) => { if (!ok) setText(shown); });
  };
  return (
    <input
      className="ui"
      style={{ width: 56 }}
      value={text}
      spellCheck={false}
      onChange={(e) => setText(e.target.value)}
      onBlur={commit}
      onKeyDown={(e) => {
        if (e.key === "Enter") (e.target as HTMLInputElement).blur();
        if (e.key === "Escape") setText(shown);
      }}
    />
  );
}
