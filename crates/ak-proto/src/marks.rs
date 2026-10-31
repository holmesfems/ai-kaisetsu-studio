//! モニターの印（Claude への場所つきの指示）の型。
//!
//! `<回>/studio/marks.json` の中身と、画面・HTTP・MCP で共通の形。保存と操作は `src-tauri/src/core/marks.rs`。

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// 印の形。座標はすべて出力の画素（1920x1080）
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, JsonSchema, TS)]
#[serde(tag = "kind", rename_all = "lowercase")]
#[ts(export)]
pub enum MarkShape {
    /// 点
    Pin { at: [i32; 2] },
    /// 矩形（x, y, 幅, 高さ）。JSON では "box"
    Rect {
        #[serde(rename = "box")]
        #[ts(rename = "box")]
        bbox: [i32; 4],
    },
    /// 矢印（始点 → 終点）
    Arrow { from: [i32; 2], to: [i32; 2] },
    /// ペン（間引いたあとの点列。最大 32 点）
    Pen { points: Vec<[i32; 2]> },
}

/// モニターの印。座標はすべて出力の画素（1920x1080）
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, JsonSchema, TS)]
#[ts(export)]
pub struct Mark {
    /// 回の中の通し番号（画面・画像・Claude への文で ①② と出る番号。消しても詰めない）
    pub id: u32,
    /// "red"（直して）| "yellow"（注目）| "green"（良い・残す）| "blue"（参考）
    pub color: String,
    pub shape: MarkShape,
    /// 一言（空でもよい）
    #[serde(default)]
    pub note: String,
    /// 描いたときの再生位置（確認画像はこのコマ）
    pub t: f64,
    /// 画面に出す区間 [t0, t1)。既定は描いたときのシーン
    pub t0: f64,
    pub t1: f64,
    /// "draft"（まだ送っていない）| "sent"（送った・Claude の作業待ち）| "done"（済）
    pub state: String,
    /// "user" | "claude"
    pub by: String,
    /// 送ったときに調べた、印の下にある演出クリップのキー（送るまでは空）
    #[serde(default)]
    pub clips: Vec<String>,
    /// 送ったときに調べた、印の下にある素材のクリップ id（送るまでは空）
    #[serde(default)]
    pub media: Vec<String>,
    /// 送ったメッセージの id
    #[serde(default)]
    #[ts(optional)]
    pub msg: Option<u64>,
    /// 済にしたときの Claude の一言
    #[serde(default)]
    pub done_note: String,
    /// 済にしたときの確認画像のパス（あれば）
    #[serde(default)]
    #[ts(optional)]
    pub done_image: Option<String>,
}

/// `<回>/studio/marks.json`。`next` は次に付ける番号（消しても戻らない）
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, JsonSchema, TS)]
#[ts(export)]
pub struct MarkFile {
    pub next: u32,
    pub marks: Vec<Mark>,
}

impl Default for MarkFile {
    fn default() -> Self {
        MarkFile { next: 1, marks: vec![] }
    }
}

/// 送るときに調べた、印の下にある演出クリップ・素材（`marks_sent` の引数）
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default, TS)]
#[ts(export)]
pub struct MarkFound {
    pub id: u32,
    /// 演出クリップのキー
    #[serde(default)]
    pub clips: Vec<String>,
    /// 素材のクリップ id
    #[serde(default)]
    pub media: Vec<String>,
}

/// 印の番号の丸数字（①〜⑳、㉑〜㉟。それより大きければ「(36)」。画面の `circled` と同じ）
pub fn circled(id: u32) -> String {
    match id {
        1..=20 => char::from_u32(0x2460 + id - 1).map(String::from).unwrap_or_default(),
        21..=35 => char::from_u32(0x3251 + id - 21).map(String::from).unwrap_or_default(),
        _ => format!("({id})"),
    }
}

/// 印の色の名前（Claude への文・画面の意味）
pub const MARK_COLORS: [(&str, &str); 4] = [("red", "赤＝直して"), ("yellow", "黄＝注目"), ("green", "緑＝良い（残す）"), ("blue", "水色＝参考")];

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn rect_uses_box_key_and_kind_tag() {
        let s = MarkShape::Rect { bbox: [1, 2, 3, 4] };
        assert_eq!(serde_json::to_value(&s).unwrap(), json!({"kind": "rect", "box": [1, 2, 3, 4]}));
        let p: MarkShape = serde_json::from_value(json!({"kind": "pin", "at": [5, 6]})).unwrap();
        assert_eq!(p, MarkShape::Pin { at: [5, 6] });
        let a: MarkShape = serde_json::from_value(json!({"kind": "arrow", "from": [0, 0], "to": [9, 9]})).unwrap();
        assert!(matches!(a, MarkShape::Arrow { .. }));
        let pen: MarkShape = serde_json::from_value(json!({"kind": "pen", "points": [[1, 1], [2, 2]]})).unwrap();
        assert!(matches!(pen, MarkShape::Pen { .. }));
    }

    #[test]
    fn circled_numbers() {
        assert_eq!(circled(1), "①");
        assert_eq!(circled(20), "⑳");
        assert_eq!(circled(21), "㉑");
        assert_eq!(circled(35), "㉟");
        assert_eq!(circled(36), "(36)");
        assert_eq!(circled(0), "(0)");
    }

    #[test]
    fn mark_file_default_starts_at_one() {
        let f = MarkFile::default();
        assert_eq!(f.next, 1);
        let back: MarkFile = serde_json::from_value(serde_json::to_value(&f).unwrap()).unwrap();
        assert_eq!(back, f);
    }
}
