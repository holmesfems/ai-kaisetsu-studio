// 本体（Rust）とのやり取りの表。call / on（io/api.ts）はこの表で名前・引数・戻り値の型を縛る。
//
// - Commands: Tauri のコマンド名 → 引数（オブジェクト。無ければ undefined）と戻り値。src-tauri/src/app/commands.rs
//   引数の名前は Rust の引数を camelCase にしたもの（Tauri v2 の決まり。t_end → tEnd）。snake_case で書くと黙って届かない
// - Events: 本体が emit する知らせの名前 → 中身。
// - 型は ui/src/gen/（ts-rs が cargo test で書き出す）を使う。Rust が serde_json::Value で素通しするものはここに手で書く。
// - 表の漏れは Rust のテスト（src-tauri/src/app/mod.rs）が見つける:
//   generate_handler! のコマンド名 ＝ Commands のキー、emit("…" の名前 ＝ Events のキー。
//   キーは 1 行に 1 つ、行頭 2 スペースで書くこと（テストが行ごとに読む）。
import type { Analysis } from "../gen/Analysis";
import type { AnalysisUpdate } from "../gen/AnalysisUpdate";
import type { ClaudeInfo } from "../gen/ClaudeInfo";
import type { Desc } from "../gen/Desc";
import type { ErrorCode } from "../gen/ErrorCode";
import type { EngineInfo } from "../gen/EngineInfo";
import type { Guide } from "../gen/Guide";
import type { GuideBench } from "../gen/GuideBench";
import type { GuideFixed } from "../gen/GuideFixed";
import type { GuideState } from "../gen/GuideState";
import type { SrcAt } from "../gen/SrcAt";
import type { LayoutState } from "../gen/LayoutState";
import type { Mark } from "../gen/Mark";
import type { MarkFile } from "../gen/MarkFile";
import type { MarkFound } from "../gen/MarkFound";
import type { MediaProbe } from "../gen/MediaProbe";
import type { Scene } from "../gen/Scene";
import type { SettingsPatch } from "../gen/SettingsPatch";
import type { SeBankUpdate } from "../gen/SeBankUpdate";
import type { PlayReady } from "../gen/PlayReady";
import type { Snapshot } from "../gen/Snapshot";
import type { UiState } from "../gen/UiState";
import type { VoiceSnapshot } from "../gen/VoiceSnapshot";
import type { Placement } from "../logic/media";
import type { Progress } from "../state/store";
import type { ChatMsg, FontInfo, Override, ProjectDoc, Proposal, Script, SeBank, SubCheck, SubView, VoiceItem } from "../types";

/** list_projects の 1 件（回のフォルダ） */
export interface ProjectEntry {
  name: string;
  path: string;
  script: boolean;
  fx: boolean;
  /** 更新時刻（Unix 秒） */
  mtime: number;
}

/** export の戻り値（kind ごとに入る項目が違う） */
export interface ExportResult {
  path?: string;
  srt?: string;
  cues?: number;
  wav?: string;
  count?: number;
  /** 飛ばしたものの警告 */
  warnings?: string[];
  [k: string]: unknown;
}

export interface Commands {
  get_state: { args: undefined; ret: Snapshot };
  list_projects: { args: undefined; ret: { root: string; projects: ProjectEntry[]; recent: string[] } };
  open_project: { args: { path: string }; ret: Snapshot };
  save_script: { args: { script: Script }; ret: { rev: number; changed: boolean; script: Script; external: boolean } };
  script_peek: { args: { path: string }; ret: Script };
  set_doc: { args: { doc: ProjectDoc }; ret: void };
  set_overrides: { args: { ov: Record<string, Override> }; ret: void };
  patch_override: { args: { key: string; patch: Partial<Override> | null }; ret: unknown };
  set_gui_clips: { args: { clips: any[] }; ret: void };
  reload_fx: { args: undefined; ret: void };
  sync_set: { args: { cue: number; keyword: string; t: number | null }; ret: void };
  /** VoiSona の中継（voice_server）の API。戻り値は呼び出し側が型を決める */
  voice_api: { args: { path: string; body: unknown }; ret: any };
  voice_restart: { args: undefined; ret: any };
  voice_dict_path: { args: undefined; ret: string };
  /** 画面の保存前の台本で並べた結果（Rust の並べ方。Core の並びは変えない） */
  voice_layout_preview: { args: { script: Script; gap: number; blockGap: number }; ret: LayoutState };
  /** 1 行の音声を作る（台本に無い行は null）。解析は analysis・layout イベントでも届く */
  voice_synth: { args: { script: Script; lineId: string }; ret: Analysis | null };
  /** 今の並びと解析の全部（回を開いた直後の取り直し） */
  voice_state: { args: undefined; ret: VoiceSnapshot };
  /** 解析から消す（辞書を変えたとき） */
  voice_forget: { args: { keys: string[] }; ret: void };
  set_ui: { args: { ui: UiState }; ret: void };
  chat_send: { args: { text: string; ctx: Record<string, unknown>; attachments?: unknown[] }; ret: ChatMsg };
  chat_clear: { args: undefined; ret: void };
  chat_attach_image: { args: { data: string; ext: string }; ret: string };
  chat_attach_file: { args: { path: string }; ret: string };
  chat_cancel: { args: { id: number }; ret: { result: "retracted"; msg: ChatMsg } | { result: "late"; msg: ChatMsg; notice: ChatMsg } };
  chat_react: { args: { id: number; reaction: "" | "like" }; ret: ChatMsg };
  /** モニターの印（studio/marks.json）。変わるたびにイベント marks で丸ごと届く */
  marks_get: { args: undefined; ret: MarkFile };
  /** id 0 で足す（新しい印は user・draft）。既存の印は色・形・一言・t・区間だけ直る */
  mark_upsert: { args: { mark: Mark }; ret: Mark };
  mark_remove: { args: { id: number }; ret: void };
  /** 送った印を sent に（メッセージの id と、印の下にあった演出・素材を記録） */
  marks_sent: { args: { ids: number[]; msg: number; found: MarkFound[] }; ret: void };
  /** 取り消したメッセージで送った印を draft に戻す */
  marks_unsend: { args: { msg: number }; ret: void };
  mark_set_state: { args: { id: number; state: "draft" | "sent" | "done" }; ret: Mark };
  /** t のコマに印の層（1920x1080 の PNG の base64）を重ねた確認画像を保存してパスを返す */
  chat_mark_image: { args: { t: number; layer: string }; ret: string };
  proposal_resolve: { args: { id: number; idx: number | null; accept: boolean }; ret: { summary: unknown; errors: string[] } };
  proposal_report: { args: { id: number }; ret: boolean };
  prefetch: { args: { t0: number; t1: number; scale: number; tag: string }; ret: void };
  play_ready: { args: { t0: number; t1: number; scale: number; fx: boolean; subs: boolean }; ret: PlayReady };
  cached_ranges: { args: { scale: number }; ret: [number, number, number][] };
  scenes: { args: undefined; ret: Scene[] };
  export: { args: { kind: string; from?: number | null; to?: number | null }; ret: ExportResult };
  export_cancel: { args: undefined; ret: void };
  probe_media: { args: { path: string }; ret: MediaProbe };
  media_place: { args: { doc: ProjectDoc; path: string; start: number | null; track: string | null; nest: string | null }; ret: Placement };
  preview_state: { args: { doc: ProjectDoc | null; ov: Record<string, Override> | null }; ret: void };
  engine_log: { args: undefined; ret: string[] };
  claude_info: { args: undefined; ret: ClaudeInfo };
  build_info: { args: undefined; ret: string };
  engine_info: { args: undefined; ret: EngineInfo };
  settings_patch: { args: { p: SettingsPatch }; ret: void };
  open_in_editor: { args: { path: string; line?: number | null }; ret: void };
  reveal: { args: { path: string }; ret: void };
  mcp_setup: { args: undefined; ret: { ok: boolean; mod: boolean; exe: string; log: string[]; already: boolean } };
  launch_claude: { args: undefined; ret: void };
  hooks_setup: { args: undefined; ret: { ok: boolean; path: string } };
  list_fonts: { args: undefined; ret: FontInfo[] };
  subs_check: { args: undefined; ret: SubCheck };
  subs_view: { args: { doc: ProjectDoc; items: VoiceItem[] }; ret: SubView };
  /** 攻略動画（studio/guide.json）。guide が null なら攻略動画にしていない回 */
  guide_get: { args: undefined; ret: GuideState };
  /** base_rev は画面が持っている GuideState.rev。今と違えば本体は保存せず「ほかで変わりました」で断る */
  guide_set: { args: { guide: Guide; base_rev: number | null }; ret: GuideState };
  /** Claude の変更（GuideState.claude_undo）を控えに戻す。id は画面が見せていた claude_undo.id（違えば断る）。その後に直していたら断る */
  guide_undo_claude: { args: { id: number }; ret: GuideState };
  guide_create: { args: { src: string }; ret: GuideState };
  /** 攻略動画の演出の入口 fx/guide_fx.py を作って演出を読み込む（あれば Err） */
  guide_fx_create: { args: undefined; ret: void };
  /** 素材の解析を裏で始める。進み具合は progress（task guide_scan）、終わりは guide イベント */
  guide_scan: { args: undefined; ret: void };
  guide_save_fixed_default: { args: { fixed: GuideFixed }; ret: GuideState };
  /** 待機欄の大きさ・不透明度を設定の guide_bench に保存する（全部の回で使う。範囲の外は断る） */
  guide_save_bench: { args: { bench: GuideBench }; ret: GuideState };
  /** 出力の時刻 t（手動パートの中）に操作を足す。id は本体が振る */
  guide_op_add: { args: { t: number }; ret: GuideState };
  /** 出力の時刻 t に見えている素材のコマ。組み立てが無い・範囲外は null */
  guide_src_at: { args: { t: number }; ret: SrcAt | null };
  /** 素材の時刻 t_src から delta コマ進めた（戻した）コマの時刻（1 コマ送り） */
  guide_step: { args: { t_src: number; delta: number }; ret: number };
  se_bank: { args: undefined; ret: SeBank };
  se_bank_set: { args: { kinds: Record<string, unknown>; render?: boolean }; ret: SeBankUpdate };
  se_audition: { args: { kind: string; src: string }; ret: { wav: string; [k: string]: unknown } };
  se_import: { args: { kind: string; path: string }; ret: { src: string; path: string; converted: boolean } };
}

export interface Events {
  /** null は演出を捨てたとき（開いた回に fx/ のタイムラインが無い） */
  desc: Desc | null;
  engine: EngineInfo;
  overrides: Record<string, Override>;
  doc: ProjectDoc;
  fx_timeline: { fx_timeline: string | null; timelines: string[] };
  gui_clips: any[];
  script: { script: Script; external: boolean };
  chat: ChatMsg;
  "chat-update": ChatMsg;
  proposals: Proposal[];
  /** 印の全部（変わるたびに丸ごと） */
  marks: MarkFile;
  claude: ClaudeInfo;
  voice: { ok: true; info?: unknown } | { ok: false; error: string; code?: ErrorCode; detail?: string };
  /** セリフの並び（Rust が台本・間隔・解析から作ったもの） */
  layout: LayoutState;
  /** 声の解析の増えた・変わった分 */
  analysis: AnalysisUpdate;
  toast: { text: string; kind?: "info" | "error"; ms?: number; /** 元の連鎖（ツールチップに出す） */ detail?: string };
  progress: Progress;
  cache: Record<string, never>;
  se_bank: SeBankUpdate;
  /** 攻略動画の状態が変わった（読み直し・保存・解析の始まりと終わり・固定値の保存） */
  guide: GuideState;
  "ui-focus": { t?: number; scene?: string | number; clip?: string; line?: unknown; line_id?: string };
}
