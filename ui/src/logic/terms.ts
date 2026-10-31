// 画面に何か所も出る語と、値に結びつく名前の表。用語集は docs/audit/ui-strings-changes.md（ui/test/terms.test.ts で縛っている）。
// 1 か所にしか出ない説明文・ツールチップはここに移さず、コンポーネントに直書きのままにする。
import type { Delivery } from "../gen/Delivery.ts";
import type { ProposalItem } from "../gen/ProposalItem.ts";

/** Claude へのメッセージの送信方法。name は名前、label は設定の選択肢、hint は「Claude を起動」のときの補足 */
export const DELIVERY: Record<Delivery, { name: string; label: string; hint: string }> = {
  mod: {
    name: "Mod",
    label: "Mod（推奨・プロンプトの上に状況を表示）",
    hint: "Mod が回のフォルダのセッションに自動で接続します（別の場所なら /studio on）",
  },
  hook: {
    name: "フック",
    label: "フック（Mod が使えない場合）",
    hint: "回のフォルダの .claude/settings.local.json に待ち受けのフックを書いてから起動します",
  },
  channel: {
    name: "channels",
    label: "channels（組織で Channels を有効にしている場合）",
    hint: "起動時の確認画面で「I am using this for local development」を選んでください",
  },
};

/** 送信方法の値の並び（設定の選択肢の順） */
export const DELIVERY_ORDER: Delivery[] = ["mod", "hook", "channel"];

/** 推敲案の項目の状態（pending は結果の表示に使わない） */
export const PROPOSAL_STATUS: Record<Exclude<ProposalItem["status"], "pending">, string> = {
  accepted: "採用",
  rejected: "不採用",
  stale: "適用できず",
};

export type TrackKind = "fx" | "video" | "audio" | "voice" | "sub" | "se" | "guide_audio" | "cue" | "ops";

/** タイムラインの行の種類。badge は行頭の札（Premiere の V1・A1 と同じ種類の略号なので英字のまま）、name は行の名前 */
export const TRACK_KIND: Record<TrackKind, { badge: string; name: string }> = {
  fx: { badge: "FX", name: "演出" },
  video: { badge: "V", name: "映像" },
  audio: { badge: "A", name: "音声" },
  voice: { badge: "♪", name: "セリフ" },
  sub: { badge: "SUB", name: "字幕" },
  se: { badge: "SE", name: "効果音" },
  guide_audio: { badge: "A", name: "素材の音" },
  cue: { badge: "CUE", name: "cue・同期点" },
  ops: { badge: "OP", name: "操作" },
};

/** Claude の接続のボタン */
export const CLAUDE_ACTIONS = {
  register: { label: "Claude Code に登録", title: "ak-studio のツール（MCP）と Mod を入れます（最初の 1 回だけ）" },
  launch: { label: "Claude を起動", title: "回のフォルダで Claude Code を開きます" },
  hooks: { label: "フックのみ設定", title: "自分で claude を起動する場合は、先に押してください" },
} as const;
