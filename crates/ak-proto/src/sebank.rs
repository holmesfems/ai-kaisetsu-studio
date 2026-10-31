//! 効果音の設定の形。ワーカー（`fxkit/fxse.py` の `bank_info`）が作り、Rust（`sebank.rs`）が読み、画面・MCP に渡す。
//! 見本の JSON は `src-tauri/tests/fixtures/se_bank.json`（Python と Rust の両方のテストが読む）。

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// 効果音の候補 1 つ
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, JsonSchema, TS)]
#[ts(export)]
pub struct SeCand {
    /// "synth"（合成の音）か fx/se/ の下の .wav の相対パス（棚の種類は棚の下の `<種類>/<名前>.wav`）
    pub src: String,
    /// 標準の大きさからの音量（dB）
    pub db: f64,
    pub exists: bool,
    /// 使えない .wav の理由（16bit PCM 以外など）。使えるなら null
    #[serde(default)]
    pub bad: Option<String>,
    /// 一覧に足した候補（se/<種類>/ にも鳴らす音にも無いもの）
    pub extra: bool,
    /// se/<種類>/ に置いてあって自動で出ている候補
    pub folder: bool,
    /// 素材の出どころのメモ（bank.json の _sources）。無ければ空
    pub source: String,
}

/// 効果音の種類（シチュエーション）ごとの設定
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, JsonSchema, TS)]
#[ts(export)]
pub struct SeKind {
    pub kind: String,
    /// 画面の表示名（kit が付ける。表に無い種類は種類の名前）
    pub label: String,
    /// 標準の大きさ（ピーク dBFS）
    pub level: f64,
    pub prio: u32,
    /// 素材の音に替えられるか（数え上げのカチカチは合成の音だけ）
    pub bankable: bool,
    pub lite: bool,
    pub stem: String,
    /// 鳴らす候補（複数なら出てくる順に順番に使う）
    #[serde(rename = "use")]
    #[ts(rename = "use")]
    pub use_: Vec<String>,
    pub explicit: bool,
    /// 音を回ではなくアプリの効果音の棚（全部の回で使う）から選ぶ種類（攻略動画の操作の 4 つのグループ）。選ぶのは 1 つだけ
    pub shelf: bool,
    /// この回で鳴る時刻（秒）
    pub times: Vec<f64>,
    pub cands: Vec<SeCand>,
}

/// 効果音の設定（種類ごとの候補・鳴らす音・音量）
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, JsonSchema, TS)]
#[ts(export)]
pub struct SeBank {
    /// 保存先の bank.json
    pub path: String,
    /// bank.json が（回に、無ければ kit の既定の棚に）あるか
    pub exists: bool,
    /// 回の se/ の下の .wav（kit の既定の棚の分も含む。相対パス）
    pub files: Vec<String>,
    /// 効果音の棚の場所（%LOCALAPPDATA%\ak-studio\se_library）。分からなければ null
    #[serde(default)]
    pub shelf_dir: Option<String>,
    /// 音量の範囲（dB）
    pub db_range: [f64; 2],
    pub kinds: Vec<SeKind>,
}
