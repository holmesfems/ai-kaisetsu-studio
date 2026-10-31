//! 画面に返す・知らせる形が決まっているもの（ts-rs で `ui/src/gen/` の TypeScript の型になる）。
//! 画面と HTTP をまたぐ型・HTTP の要求は `crates/ak-proto` に置く。

use crate::agent::{ChatMsg, Proposal};
use crate::project::ProjectDoc;
use crate::render::engine::{Desc, EngineStatus, Warning};
use crate::script::Script;
use crate::settings::{Delivery, Settings};
use crate::voice::analysis::Analysis;
use crate::voice::layout::LayoutState;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use ts_rs::TS;

/// ⚙ 設定のダイアログで変えた項目（変えた項目だけ入れる。None は変えない）。
/// 開いている間に本体が変えた項目（last_project など）を古い値で戻さないよう、設定は丸ごとでなく項目ごとに送る
#[derive(Deserialize, Clone, Debug, Default, TS)]
#[serde(default)]
#[ts(export, optional_fields)]
pub struct SettingsPatch {
    pub an_root: Option<String>,
    pub template_dir: Option<String>,
    pub python: Option<String>,
    pub ffmpeg: Option<String>,
    pub voisona_user: Option<String>,
    /// Some("") で消す。None は変えない
    pub voisona_pass: Option<String>,
    /// 声のエンジン（voisona・voicevox、または .py の絶対パス。正しくなければ保存しない）
    pub voice_engine: Option<String>,
    /// VOICEVOX ENGINE の URL（空で既定の http://127.0.0.1:50021）
    pub voicevox_url: Option<String>,
    /// 演出ワーカーの数（1〜32 に丸める）
    pub workers: Option<f64>,
    /// 演出キャッシュの上限 MB（100〜65536 に丸める）
    pub cache_mb: Option<f64>,
    /// 取り消しの猶予の秒（0〜30 に丸める）
    pub cancel_grace: Option<f64>,
    pub delivery: Option<Delivery>,
}

/// 画面が 250ms ごとに本体へ送る状況（再生位置・選択など）。Claude への「いまの状況」の帯と `/api/status` の `ui` になる
#[derive(Serialize, Deserialize, Clone, Debug, Default, TS)]
#[serde(default)]
#[ts(export)]
pub struct UiState {
    /// 再生位置（秒）
    pub t: f64,
    pub playing: bool,
    /// 画面の選択（ui/src/types.ts の Selection）。中身は見ない所が多いので Value のまま
    #[ts(type = "import(\"../types.ts\").Selection | null")]
    pub selection: Option<Value>,
    /// 選んでいる台本の行（通し番号）
    pub lines: Vec<u64>,
    #[serde(rename = "in")]
    pub in_t: Option<f64>,
    #[serde(rename = "out")]
    pub out_t: Option<f64>,
}

/// 今の手直しの音声がまだ無い行（再生で待つ対象）
#[derive(Serialize, Clone, Debug, TS)]
#[ts(export)]
pub struct VoiceGap {
    /// 台本の行の id
    pub line_id: String,
    /// 行の始まり（秒）
    pub t0: f64,
    /// 行の終わり（秒）
    pub t1: f64,
}

/// 再生の前に調べた「まだ準備できていない区間」（`play_ready` の戻り値）
#[derive(Serialize, Clone, Debug, TS)]
#[ts(export)]
pub struct PlayReady {
    /// 演出のコマがまだ無い区間（秒、時刻順）
    pub fx: Vec<[f64; 2]>,
    /// 調べた範囲の演出のコマ: [描けている数, 何か出ている数]
    pub fx_count: [u32; 2],
    /// 字幕の絵がまだ無い区間（秒、時刻順）
    pub subs: Vec<[f64; 2]>,
    /// 今の手直しの音声がまだ無い行
    pub voice: Vec<VoiceGap>,
}

/// 開いている回
#[derive(Serialize, Clone, Debug, TS)]
#[ts(export)]
pub struct ProjectInfo {
    /// 回のフォルダ（/ 区切り）
    pub root: String,
    pub name: String,
    pub doc: ProjectDoc,
    /// fx/ の中のタイムライン（相対パス）
    pub timelines: Vec<String>,
}

/// 画面の初期表示に要るもの一式（`get_state`・`open_project` の戻り値）。
/// 回を開いていないときは project: null と settings だけ。
/// script・desc は無ければ（台本が無い・演出がまだ）省く
#[derive(Serialize, Clone, Debug, TS)]
#[ts(export)]
pub struct Snapshot {
    pub project: Option<ProjectInfo>,
    /// voisona_pass は空にして送る（中身は画面に出さない）。設定済みかは voisona_pass_set
    pub settings: Settings,
    pub voisona_pass_set: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub script: Option<Script>,
    /// 演出クリップの微調整（キーは演出クリップのキー）
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(type = "Record<string, import(\"../types.ts\").Override>", optional)]
    pub overrides: Option<Value>,
    /// 画面から足した簡単な演出
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(type = "any[]", optional)]
    pub gui_clips: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub desc: Option<Desc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub engine: Option<EngineInfo>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub chat: Option<Vec<ChatMsg>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub proposals: Option<Vec<Proposal>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub claude: Option<ClaudeInfo>,
    /// セリフの並びと声の解析（全部）
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub voice: Option<VoiceSnapshot>,
}

/// セリフの並びと声の解析の全部（`Snapshot.voice`）。以降の変わった分は `layout`・`analysis` イベント
#[derive(Serialize, Clone, Debug, TS)]
#[ts(export)]
pub struct VoiceSnapshot {
    pub layout: LayoutState,
    /// 鍵は `VoiceItem.key`
    pub analysis: HashMap<String, Analysis>,
}

/// 演出エンジンの状態（`engine_info`・`engine` イベント）
#[derive(Serialize, Clone, Debug, TS)]
#[ts(export)]
pub struct EngineInfo {
    pub status: EngineStatus,
    pub cache_frames: usize,
    pub cache_mb: f64,
    pub warnings: Vec<Warning>,
    pub fxkit: Option<String>,
    /// 描くのに使った演出の kit（`kit: private (API 1)` のような表示）
    pub kit: Option<String>,
}

/// Claude の作業状況（Mod の知らせ）
#[derive(Serialize, Clone, Debug, TS)]
#[ts(export)]
pub struct ClaudeActivity {
    pub working: bool,
    pub text: String,
    /// 作業を始めた時刻（Unix ms）
    pub since_ms: u64,
    /// 最後に知らせが来た時刻（Unix ms）
    pub updated_ms: u64,
}

/// Claude とのつながり（`claude_info`・`claude` イベント）
#[derive(Serialize, Clone, Debug, TS)]
#[ts(export)]
pub struct ClaudeInfo {
    pub activity: ClaudeActivity,
    pub connected: bool,
    /// まだ Claude に渡していないメッセージの数
    pub pending: usize,
    pub watching: bool,
    pub delivery: Delivery,
    /// Claude が reply の status で知らせた「今やっていること」
    pub status: String,
    pub port: u16,
    pub api_error: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(v: &Value) -> Vec<&str> {
        let mut k: Vec<&str> = v.as_object().unwrap().keys().map(|k| k.as_str()).collect();
        k.sort();
        k
    }

    /// 回を開いていないときの形は { project: null, settings } だけ（画面は無い項目を ?? で読む）
    /// 画面に送る設定からはパスワードの中身を除き、設定済みかだけを付ける
    /// 画面が送る形を読み、同じ形で書く。欠けた項目は既定
    #[test]
    fn ui_state_round_trip() {
        let raw = r#"{"t":1.5,"playing":false,"selection":null,"lines":[3],"in":null,"out":2.5}"#;
        let u: UiState = serde_json::from_str(raw).unwrap();
        assert_eq!((u.t, u.lines.clone(), u.in_t, u.out_t), (1.5, vec![3], None, Some(2.5)));
        let v = serde_json::to_value(&u).unwrap();
        assert_eq!(v, serde_json::from_str::<Value>(raw).unwrap());
        let d: UiState = serde_json::from_str("{}").unwrap();
        assert_eq!((d.t, d.playing, d.lines.len()), (0.0, false, 0));
    }

    #[test]
    fn ui_settings_hide_voisona_pass() {
        let s = Settings { voisona_pass: "secret".into(), voisona_user: "u".into(), ..Settings::default() };
        let u = s.for_ui();
        assert_eq!(u.voisona_pass, "");
        assert_eq!(u.voisona_user, "u");
        assert_eq!(s.voisona_pass, "secret", "本体の値は変わらない");
    }

    #[test]
    fn snapshot_without_project_has_only_project_and_settings() {
        let s = Snapshot {
            project: None,
            settings: Settings::default(),
            voisona_pass_set: false,
            script: None,
            overrides: None,
            gui_clips: None,
            desc: None,
            engine: None,
            chat: None,
            proposals: None,
            claude: None,
            voice: None,
        };
        let v = serde_json::to_value(&s).unwrap();
        assert_eq!(keys(&v), ["project", "settings", "voisona_pass_set"]);
        assert!(v["project"].is_null());
    }

    #[test]
    fn claude_info_keeps_wire_names() {
        let c = ClaudeInfo {
            activity: ClaudeActivity { working: true, text: "x".into(), since_ms: 1, updated_ms: 2 },
            connected: false,
            pending: 0,
            watching: false,
            delivery: Delivery::Mod,
            status: String::new(),
            port: 47815,
            api_error: None,
        };
        let v = serde_json::to_value(&c).unwrap();
        assert_eq!(keys(&v), ["activity", "api_error", "connected", "delivery", "pending", "port", "status", "watching"]);
        assert!(v["api_error"].is_null());
        assert_eq!(v["activity"]["since_ms"], 1);
    }
}
