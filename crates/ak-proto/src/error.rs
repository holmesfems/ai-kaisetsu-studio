//! 画面・HTTP に返す失敗の型。

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// 画面・HTTP に返す失敗の種類。画面はこれで説明・次の操作の案内を出し分ける
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum ErrorCode {
    /// 既定（分類できない失敗）
    Failed,
    /// ファイル・行・クリップが無い（io の NotFound もここ）
    NotFound,
    /// 使用中・アクセスできない（io の PermissionDenied）
    Denied,
    /// ファイルの書式が読めない（serde_json・serde_yaml）
    BadFormat,
    /// 渡した値が正しくない（引数の検査）
    BadInput,
    /// 回を開いていない
    NoProject,
    /// 中止した
    Canceled,
    /// ffmpeg が見つからない
    NeedFfmpeg,
    /// 声の中継に接続できない
    VoiceDown,
    /// 演出ワーカーが止まっている
    EngineDown,
    /// 保存はできたが、そのあとの作り直し・取り直しだけ失敗した
    PartlySaved,
}

/// 失敗の中身（コマンドの reject と HTTP の応答で同じ形）
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, JsonSchema, TS)]
#[ts(export)]
pub struct ApiError {
    pub code: ErrorCode,
    /// 画面に出す和文（「〜できませんでした: 」の後ろに付く理由）
    pub message: String,
    /// 元の連鎖そのまま（英語の原因も含む）。ログ・「詳細」用
    pub detail: String,
    /// `partly_saved` のとき、あとの失敗の元の種類（案内を出し分けるため）。無ければ省く
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub cause: Option<ErrorCode>,
}
