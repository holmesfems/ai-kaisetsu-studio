//! HTTP API（`src-tauri/src/app/api.rs`）の要求と、MCP のツールの引数。
//!
//! 同じ型から HTTP の読み取り（serde）と MCP の `inputSchema`（schemars → `schema::input_schema`）を作る。
//! 項目の `///` の doc コメントは、そのまま MCP の引数の `description` になる（Claude に見える文なので、日本語で、1 行で書く）。
//! Claude に見せない開発者向けの注意は `//` で書く。

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

fn yes() -> bool {
    true
}

/// スキーマの `required` に項目を足す。`#[serde(default)]` の項目は schemars が必須にしないので、MCP に必須として見せるのに使う
fn require(schema: &mut schemars::Schema, name: &str) {
    let obj = schema.ensure_object();
    let req = obj.entry("required").or_insert_with(|| Value::Array(vec![]));
    if let Some(a) = req.as_array_mut() {
        if !a.iter().any(|x| x == name) {
            a.push(Value::String(name.into()));
        }
    }
}

fn require_patch(schema: &mut schemars::Schema) {
    require(schema, "patch");
}

fn require_kinds(schema: &mut schemars::Schema) {
    require(schema, "kinds");
}

// 推敲案の 1 件の種類
// 候補を `enum` で出すため、各項目には doc コメントを付けない（付けると oneOf になる）
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ProposeKind {
    Edit,
    Insert,
    Delete,
    Speaker,
    Memo,
    Notes,
    Move,
    BlockAdd,
    BlockDelete,
    BlockRename,
}

// 推敲案の block_add の行
#[derive(Serialize, Deserialize, Clone, Debug, JsonSchema)]
pub struct ProposeLine {
    /// 話者（トーク1 / トーク2）
    pub sp: String,
    /// 文（ルビは ｜漢字《よみ》）
    pub text: String,
    /// 行のメモ（省略可）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memo: Option<String>,
}

// 推敲案の 1 件（`script_propose`・`script_apply` の items の要素）
// HTTP は寛容に読む（script::ops::resolve_items が Value から読む。text の代わりに new_text も受ける）。
// ここは MCP に見せる形
#[derive(Serialize, Deserialize, Clone, Debug, JsonSchema)]
pub struct ProposeItem {
    /// edit=文を直す / insert=行を足す / delete=行を消す / speaker=話者だけ変える / memo=行のメモ（line と text。空で消す） / notes=ブロックのメモ（block と text。空で消す） / move=行を after の行の後ろ（または block の頭）へ動かす（line と after か block） / block_add=ブロックを足す（title・after_block・text=ブロックのメモ・lines） / block_delete=ブロックを消す（block。中の行も消える） / block_rename=ブロックの見出しを変える（block と title）
    pub kind: ProposeKind,
    /// 対象の行（script_read の通し番号 n か、行の id）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<Value>,
    /// insert のとき、この行の後ろに足す（通し番号か id）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<Value>,
    /// insert・move でブロックの頭に置くとき、notes・block_delete・block_rename で対象にするブロック（見出し・ファイル名か id）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub block: Option<String>,
    /// 新しい文（ルビは ｜漢字《よみ》）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// 話者（トーク1 / トーク2）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sp: Option<String>,
    /// 今の文（食い違いの検出用。省略可）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expect: Option<String>,
    /// 直す理由（画面に出る）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// block_add・block_rename の見出し
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// block_add で、このブロックの後ろに足す（見出しか id。省くと最後）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after_block: Option<String>,
    /// block_add で足す行（省略可）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lines: Option<Vec<ProposeLine>>,
}

/// 台本の推敲案を出す（`script_propose`）
#[derive(Deserialize, Serialize, JsonSchema)]
pub struct ProposeReq {
    #[serde(default)]
    pub title: String,
    /// 案全体の説明
    #[serde(default)]
    pub note: String,
    #[schemars(with = "Vec<ProposeItem>")]
    pub items: Vec<Value>,
}

/// 台本を直接書き換える（`script_apply`）
#[derive(Deserialize, Serialize, JsonSchema)]
pub struct ApplyReq {
    #[serde(default)]
    pub title: String,
    #[schemars(with = "Vec<ProposeItem>")]
    pub items: Vec<Value>,
    /// 前から受けていた案の説明（MCP の script_apply の引数には出さない）
    #[serde(default)]
    #[schemars(skip)]
    pub note: String,
}

/// 台本の構成メモを書き換える（`memo_write`）
#[derive(Deserialize, Serialize, JsonSchema)]
pub struct MemoReq {
    pub memo: String,
}

/// 演出クリップの一覧（`clips_list`。HTTP ではクエリ）
#[derive(Deserialize, Serialize, JsonSchema)]
pub struct ClipsQ {
    /// この秒以降に見えるもの
    pub from: Option<f64>,
    pub to: Option<f64>,
    pub key: Option<String>,
}

/// 演出クリップの微調整（HTTP の `/api/clips/override`。MCP の `clip_adjust` はこの patch に直して送る）
#[derive(Deserialize, Serialize)]
pub struct OvReq {
    pub key: String,
    pub patch: Value,
}

/// 確認画像を描く（`render_still`）
#[derive(Deserialize, Serialize, JsonSchema)]
pub struct StillReq {
    pub t: Option<f64>,
    pub scene: Option<String>,
    pub cue: Option<i64>,
    /// このクリップだけを描く
    pub solo: Option<String>,
    /// 字幕を入れる（既定 true）
    #[serde(default = "yes")]
    pub subs: bool,
}

/// cue ごとのコマを並べた確認画像を描く（`render_sheet`）
#[derive(Deserialize, Serialize, JsonSchema)]
pub struct SheetReq {
    /// cue の範囲 [最初, 最後]。各 cue の見どころ（cue の頭から、長さの 60% か 2.2 秒の早い方）のコマを並べる。ts とどちらか一方
    pub cues: Option<Vec<i64>>,
    /// 並べるコマの時刻（秒）の並び。cues とどちらか一方
    pub ts: Option<Vec<f64>>,
    /// 字幕を入れる（既定 false）
    #[serde(default)]
    pub subs: bool,
}

/// 置いた素材を変える（`media_update`）
#[derive(Deserialize, Serialize, JsonSchema)]
#[schemars(transform = require_patch)]
pub struct MediaPatch {
    pub clip_id: String,
    // 省くと空の patch になる（HTTP は寛容）が、MCP には必須として見せる
    #[serde(default)]
    #[schemars(with = "Map<String, Value>")]
    pub patch: Value,
}

/// 置いた素材を外す（`media_remove`）
#[derive(Deserialize, Serialize, JsonSchema)]
pub struct ClipIdReq {
    pub clip_id: String,
}

/// 効果音の設定を変える（`se_set`）
#[derive(Deserialize, Serialize, JsonSchema)]
#[schemars(transform = require_kinds)]
pub struct SeSet {
    /// {"slam": {"use": ["H9.wav"], "db": {"H9.wav": -3}}, "glitch": {"add": ["glitch/G5.wav"]}}
    /// 攻略動画の操作の効果音（guide_pre・guide_done・guide_precise_pre・guide_precise_done）はアプリの効果音の棚から 1 つ選ぶ:
    /// {"guide_done": {"use": ["guide_done/決定ボタンを押す51.wav"], "db": {"guide_done/決定ボタンを押す51.wav": -2}}}
    // 省くと null になる（HTTP は寛容。set が形を確かめて断る）が、MCP には必須として見せる
    #[serde(default)]
    #[schemars(with = "Map<String, Value>")]
    pub kinds: Value,
    /// 効果音を作り直す（既定 true）
    #[serde(default = "yes")]
    pub render: bool,
}

/// チャットへの返事（`reply`）
#[derive(Deserialize, Serialize, JsonSchema)]
pub struct ReplyReq {
    #[serde(default)]
    pub text: String,
    /// {"image": パス} / {"t": 秒, "label": …} / {"clip": キー} / {"line": 通し番号}
    #[serde(default)]
    #[schemars(with = "Vec<Map<String, Value>>")]
    pub attachments: Vec<Value>,
    /// 「台本を読んでいます」など。空文字で消す
    pub status: Option<String>,
}

/// 画面を動かす（`ui_focus`）。画面の `ui-focus` イベントの中身と同じ
#[derive(Deserialize, Serialize, Clone, Debug, Default, JsonSchema)]
pub struct UiFocus {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub t: Option<f64>,
    // 数で来ても受ける（画面は文字にして比べる）が、MCP には文字として見せる
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<String>")]
    pub scene: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clip: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<Value>,
    /// `line` を Rust が今の台本で引いた行の id（本体が入れる。MCP には見せない）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(skip)]
    pub line_id: Option<String>,
}

/// 書き出しの種類
pub const EXPORT_KINDS: [&str; 11] =
    ["talk", "mov", "mov_clean", "se", "mp4", "preview_mp4", "rec_track", "subs_mov", "subs_srt", "mix", "guide_audio"];

/// 書き出し（`export`）
#[derive(Deserialize, Serialize, JsonSchema)]
pub struct ExportReq {
    // 知らない種類は export::run が断る（HTTP は文字列で受ける）。候補は MCP に見せる（EXPORT_KINDS と同じ。テストで確かめる）
    #[schemars(extend("enum" = ["talk", "mov", "mov_clean", "se", "mp4", "preview_mp4", "rec_track", "subs_mov", "subs_srt", "mix", "guide_audio"]))]
    pub kind: String,
    pub from: Option<f64>,
    pub to: Option<f64>,
}

// ---------------------------------------------------------------- 攻略動画（guide_read / guide_update / guide_still / guide_coords）

// 「無い」と「null」を分ける（null は「全部使う」などの意味を持つ）。項目が無ければ None、null なら Some(None)
fn double_option<'de, D, T>(d: D) -> Result<Option<Option<T>>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Ok(Some(Option::deserialize(d)?))
}

/// 操作を 1 つ足す。時刻は t・t_out・frame のどれか 1 つ（必須）
#[derive(Deserialize, Serialize, JsonSchema, Clone, Debug, Default)]
#[serde(deny_unknown_fields)]
pub struct OpAdd {
    /// 素材の秒
    #[serde(default)]
    pub t: Option<f64>,
    /// 出力の秒（手動パートの中。素材の時刻に直して置く）
    #[serde(default)]
    pub t_out: Option<f64>,
    /// 素材のコマ番号
    #[serde(default)]
    pub frame: Option<u32>,
    /// deploy（配置）/ skill（スキル）/ retreat（撤退）/ trust（信頼枠の配置）
    pub kind: String,
    /// キャラ名
    #[serde(default)]
    pub op: String,
    /// 置き場所・向き（例 "R3C5 ↑"）
    #[serde(default)]
    pub r#where: String,
    /// 補足（自由文。動画の NOW/NEXT・操作の帯に小さく出る）
    #[serde(default)]
    pub info: String,
    /// フレーム単位で合わせる操作
    #[serde(default)]
    pub precise: bool,
    // 数の最小・最大は schemars の属性にせず、本体（normalize_guide）が断る
    /// NOW を操作の瞬間より何秒前に出すか（0〜10 秒。省くと固定値の汎用値。信頼枠は常に 0）
    #[serde(default)]
    pub now_lead: Option<f64>,
    /// コストでのタイミングの指定（例 "Kill15 の敵が出た数字 +12"）
    #[serde(default)]
    pub cue_text: String,
    /// 注意書き
    #[serde(default)]
    pub note: String,
}

/// 操作を直す。渡した項目だけ変える（時刻は t・t_out・frame のどれか 1 つ）
#[derive(Deserialize, Serialize, JsonSchema, Clone, Debug, Default)]
#[serde(deny_unknown_fields)]
pub struct OpUpdate {
    /// 直す操作の id（guide_read の ops[].id）
    pub id: String,
    /// 素材の秒
    #[serde(default)]
    pub t: Option<f64>,
    /// 出力の秒（手動パートの中）
    #[serde(default)]
    pub t_out: Option<f64>,
    /// 素材のコマ番号
    #[serde(default)]
    pub frame: Option<u32>,
    #[serde(default)]
    pub kind: Option<String>,
    /// キャラ名
    #[serde(default)]
    pub op: Option<String>,
    #[serde(default)]
    pub r#where: Option<String>,
    /// 補足（自由文）
    #[serde(default)]
    pub info: Option<String>,
    #[serde(default)]
    pub precise: Option<bool>,
    /// NOW の先出し（秒、0〜10）。省くと今のまま、null で固定値の汎用値に戻す
    #[serde(default, deserialize_with = "double_option", skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<f64>")]
    pub now_lead: Option<Option<f64>>,
    #[serde(default)]
    pub cue_text: Option<String>,
    #[serde(default)]
    pub note: Option<String>,
}

/// 操作の足す・直す・消す。全部か無しか（1 つでもエラーなら何も保存しない）
#[derive(Deserialize, Serialize, JsonSchema, Clone, Debug, Default)]
#[serde(deny_unknown_fields)]
pub struct OpsPatch {
    /// 足す操作（id は本体が振る）
    #[serde(default)]
    pub add: Vec<OpAdd>,
    /// 直す操作
    #[serde(default)]
    pub update: Vec<OpUpdate>,
    /// 消す操作の id
    #[serde(default)]
    pub remove: Vec<String>,
}

/// 編成の直し（使う枠・補足）
#[derive(Deserialize, Serialize, JsonSchema, Clone, Debug, Default)]
#[serde(deny_unknown_fields)]
pub struct FormationPatch {
    /// 使う枠の番号の並び（0〜11）。null で全部使う。省くと今のまま
    #[serde(default, deserialize_with = "double_option")]
    #[schemars(with = "Option<Vec<usize>>")]
    pub used: Option<Option<Vec<usize>>>,
    /// 枠の番号 → 補足（レベル・凸など）。渡した枠だけ変える。null で消す
    #[serde(default)]
    #[schemars(with = "std::collections::BTreeMap<String, Option<String>>")]
    pub notes: std::collections::BTreeMap<String, Option<String>>,
}

/// 攻略動画の直し（`guide_update`）。操作・編成・補足素材の小窓だけ直せる（アンカー・カット・固定値・重ね方は画面で）
#[derive(Deserialize, Serialize, JsonSchema, Clone, Debug, Default)]
#[serde(deny_unknown_fields)]
pub struct GuidePatch {
    /// 直す理由（必須。控えと、画面の「Claude の変更を戻す」に出る）
    pub reason: String,
    /// 操作の足す・直す・消す
    #[serde(default)]
    pub ops: Option<OpsPatch>,
    /// 編成（使う枠・補足）
    #[serde(default)]
    pub formation: Option<FormationPatch>,
    /// 補足素材の小窓の並び（渡したら丸ごと置き換える。[] で全部外す）
    #[serde(default)]
    pub subs: Option<Vec<crate::guide::GuideSub>>,
}

/// 素材そのままの 1 コマの確認画像（`guide_still`）。op・t・frame・t_out のどれか 1 つ（省略は画面の再生位置）
#[derive(Deserialize, Serialize, JsonSchema, Clone, Debug, Default)]
#[serde(deny_unknown_fields)]
pub struct GuideStillReq {
    /// その操作のコマ（操作の id）
    #[serde(default)]
    pub op: Option<String>,
    /// 素材の秒
    #[serde(default)]
    pub t: Option<f64>,
    /// 素材のコマ番号
    #[serde(default)]
    pub frame: Option<u32>,
    /// 出力の秒（その時刻に見せている素材のコマ）
    #[serde(default)]
    pub t_out: Option<f64>,
    /// 切り抜き [x0, y0, x1, y1]（素材の画素。2752x2064 での値）。省くと全体
    #[serde(default)]
    pub crop: Option<[f64; 4]>,
    /// true で目盛り（100px ごとに線、200px ごとに素材の画素の数字）を重ねる
    #[serde(default)]
    pub grid: bool,
    /// 素材の 1 画素を何画素で出すか（既定: 長い辺が 1600px に収まる倍率。切り抜きは 1.0 まで。最大 2.0）
    #[serde(default)]
    pub scale: Option<f64>,
}

/// 素材の画素と出力の画素の変換（`guide_coords`）
#[derive(Deserialize, Serialize, JsonSchema, Clone, Debug, Default)]
#[serde(deny_unknown_fields)]
pub struct GuideCoordsReq {
    /// パート（formation / manual / auto / result）
    pub part: String,
    /// 変換する点 [x, y] の並び
    pub points: Vec<[f64; 2]>,
    /// out=素材の画素から出力（1920x1080）の画素へ（既定）/ src=出力から素材へ
    #[serde(default)]
    #[schemars(extend("enum" = ["out", "src"]))]
    pub to: Option<String>,
}

// ---------------------------------------------------------------- モニターの印（marks_list / mark_add / mark_resolve）

/// 印の一覧（`marks_list`。HTTP ではクエリ）
#[derive(Deserialize, Serialize, JsonSchema)]
pub struct MarksQ {
    /// true で全部（済・まだ送っていない下書きも）。既定は、送られていて済でないもの
    pub all: Option<bool>,
}

/// 印を済にする（`mark_resolve`）
#[derive(Deserialize, Serialize, JsonSchema)]
pub struct MarkResolveReq {
    /// 印の番号（メッセージの「印①」の 1）
    pub id: u32,
    /// 直した内容の一言
    pub note: Option<String>,
    /// 直したあとの確認画像（render_still のパス）
    pub image: Option<String>,
}

/// Claude の印を足す（`mark_add`）。迷ったところ・確認してほしい所を示す
#[derive(Deserialize, Serialize, JsonSchema)]
pub struct MarkAddReq {
    /// 色: red（直して）/ yellow（注目）/ green（良い）/ blue（参考）
    #[schemars(extend("enum" = ["red", "yellow", "green", "blue"]))]
    pub color: String,
    /// 形。座標は出力の画素（1920x1080）。{"kind":"pin","at":[x,y]} / {"kind":"rect","box":[x,y,幅,高さ]} / {"kind":"arrow","from":[x,y],"to":[x,y]} / {"kind":"pen","points":[[x,y],…]}
    pub shape: crate::MarkShape,
    /// 一言
    #[serde(default)]
    pub note: String,
    /// 見てほしい再生位置（秒）
    pub t: f64,
    /// 印を出す区間の始め（秒）。省くと t を含むシーン（無ければ t の 2 秒前）
    pub t0: Option<f64>,
    /// 印を出す区間の終わり（秒）。省くと t を含むシーン（無ければ t の 2 秒後）
    pub t1: Option<f64>,
}

/// セリフの字幕を読む（`subs_read`。HTTP ではクエリ）。MCP の引数は配列で受け、`,` で繋いで送る（schema は手書き）
#[derive(Deserialize, Serialize)]
pub struct SubsQ {
    /// 読むセリフの指し方（行の id・"#12"・通し番号）を `,` で区切ったもの。省くと全部のセリフ
    pub lines: Option<String>,
}

/// 渡したか（null も「渡した」）を分けて読む。null は「その項目を外す」
fn given<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<Value>, D::Error> {
    Value::deserialize(d).map(Some)
}

/// セリフの字幕の直し 1 件（`subs_set`）
#[derive(Deserialize, Serialize, JsonSchema)]
pub struct SubsSetItem {
    /// セリフの指し方（行の id か通し番号。"#12" でもよい）
    pub line: Value,
    /// 字幕の文。改行は \n。文字は台本の字幕の文のまま、改行だけを入れる（台本の文の空白の所で改行するときだけ、その空白は消えてよい）。null で外す（台本の文に戻る）
    #[serde(default, deserialize_with = "given", skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<String>")]
    pub text: Option<Value>,
    /// 字幕の大きさ（20〜120）。null で外す（話者・共通の大きさに戻る）
    #[serde(default, deserialize_with = "given", skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<f64>")]
    pub size: Option<Value>,
    /// フォントのファイル（.ttc は「パス#番号」）。null で外す
    #[serde(default, deserialize_with = "given", skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<String>")]
    pub font: Option<Value>,
}

/// セリフの字幕を直す（`subs_set`）。1 件でも断れば何も書かない
#[derive(Deserialize, Serialize, JsonSchema)]
pub struct SubsSetReq {
    pub items: Vec<SubsSetItem>,
    /// 直した理由（画面の知らせに出す）
    #[serde(default)]
    pub reason: String,
}
