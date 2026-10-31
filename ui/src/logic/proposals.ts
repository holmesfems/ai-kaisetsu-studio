// 推敲案の 1 件の見出しと中身の表示（Claude パネルの ProposalCard が使う純粋な関数）。
import type { ProposalItem } from "../gen/ProposalItem.ts";

/** 中身の表示: diff=文字単位の差分 / ins・del=足す・消す行の並び / plain=そのまま */
export type ProposalBody =
  | { t: "diff"; a: string; b: string }
  | { t: "ins"; lines: string[] }
  | { t: "del"; lines: string[] }
  | { t: "plain"; text: string };

export const NO_BLOCK = "（無いブロック）";

/**
 * 見出しの文と中身。
 * lineNo は行の id → 通し番号、blockTitle はブロックの id → 今の見出し（どちらも今の台本で引く。無ければ null / NO_BLOCK）
 */
export function proposalHead(
  it: ProposalItem,
  lineNo: (id: string | null) => number | null,
  blockTitle: (id: string | null) => string,
): { head: string; body: ProposalBody } {
  const n = (id: string | null) => lineNo(id) ?? "?";
  const lineName = (l: { sp: string; text: string }) => `[${l.sp}] ${l.text}`;
  switch (it.kind) {
    case "edit":
      return { head: `行 ${n(it.line_id)}`, body: { t: "diff", a: it.old_text, b: it.new_text } };
    case "delete":
      return { head: `行 ${n(it.line_id)} を削除`, body: { t: "del", lines: [it.old_text] } };
    case "speaker":
      return { head: `行 ${n(it.line_id)} の話者`, body: { t: "plain", text: it.old_text } };
    case "insert":
      return {
        head: it.after_id ? `行 ${n(it.after_id)} の後に追加` : `ブロック「${blockTitle(it.block_id)}」の頭に追加`,
        body: { t: "ins", lines: [it.new_text] },
      };
    case "memo":
      return {
        head: `行 ${n(it.line_id)} のメモ`,
        body: it.new_text === "" ? { t: "del", lines: [it.old_text] } : { t: "diff", a: it.old_text, b: it.new_text },
      };
    case "notes":
      return {
        head: `ブロック「${blockTitle(it.block_id)}」のメモ`,
        body: it.new_text === "" ? { t: "del", lines: [it.old_text] } : { t: "diff", a: it.old_text, b: it.new_text },
      };
    case "move":
      return {
        head: it.after_id ? `行 ${n(it.line_id)} を行 ${n(it.after_id)} の後へ移動` : `行 ${n(it.line_id)} をブロック「${blockTitle(it.block_id)}」の頭へ移動`,
        body: { t: "plain", text: it.old_text },
      };
    case "block_add": {
      const add = `ブロック「${it.new_text}」を追加（${it.lines.length} 行）`;
      return {
        head: it.after_block ? `ブロック「${blockTitle(it.after_block)}」の後に、${add}` : add,
        // ブロックのメモがあれば頭に出す
        body: { t: "ins", lines: [...(it.notes ? [`メモ: ${it.notes}`] : []), ...it.lines.map(lineName)] },
      };
    }
    case "block_delete":
      // 一緒に消えるブロックのメモ・行のメモも見せる
      return {
        head: `ブロック「${it.old_text}」を削除（${it.lines.length} 行）`,
        body: { t: "del", lines: [...(it.notes ? [`メモ: ${it.notes}`] : []), ...it.lines.map((l) => lineName(l) + (l.memo ? `（メモ: ${l.memo}）` : ""))] },
      };
    case "block_rename":
      return { head: "ブロックの見出し", body: { t: "diff", a: it.old_text, b: it.new_text } };
  }
}

/** 見出しを押したときに飛ぶ行の id。行の案はその行（insert は後ろに足す先の行）、ブロックの案はそのブロックの最初の行（無ければ null） */
export function proposalFocusLine(it: ProposalItem, firstLineOf: (blockId: string | null) => string | null): string | null {
  switch (it.kind) {
    case "notes":
    case "block_delete":
    case "block_rename":
      return firstLineOf(it.block_id);
    case "block_add":
      return firstLineOf(it.block_id) ?? firstLineOf(it.after_block);
    default:
      return it.line_id ?? it.after_id ?? firstLineOf(it.block_id);
  }
}
