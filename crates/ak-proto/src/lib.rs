//! 入口をまたぐ型（画面と HTTP で共通の DTO、HTTP の要求・応答、MCP の引数）。
//!
//! - TypeScript の型は ts-rs（`#[derive(TS)] #[ts(export)]`）で `ui/src/gen/` に書き出す（`cargo test` で作り直す）
//! - MCP の引数の JSON Schema は schemars（`JsonSchema`）で作る
//!
//! 依存は軽く保つ（MCP サーバーからも使うため）。

pub mod error;
pub mod guide;
pub mod http;
pub mod marks;
pub mod schema;
pub mod sebank;

pub use error::{ApiError, ErrorCode};
pub use sebank::{SeBank, SeCand, SeKind};
pub use marks::{circled, Mark, MarkFile, MarkFound, MarkShape, MARK_COLORS};
pub use guide::{Anchor, Cut, Guide, GuideBench, GuideFixed, GuideFixedOpt, GuideFormation, GuideLayout, GuideOp, GuideSe, GuideSub, HudCrop, HudZoom};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use ts_rs::TS;

/// シーン（SCENE 57-2 など）の範囲。画面（Tauri の `scenes`）と HTTP（`/api/scenes`）で同じ形
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, JsonSchema, TS)]
#[ts(export)]
pub struct Scene {
    /// シーン番号（"57-2" など。"s" は付かない）
    pub scene: String,
    pub t0: f64,
    pub t1: f64,
}

/// 素材（画像・動画・音声）を調べた結果（`probe_media`）。画像は dur 0・fps 0・image: true
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, JsonSchema, TS)]
#[ts(export)]
pub struct MediaProbe {
    pub w: u32,
    pub h: u32,
    pub fps: f64,
    /// 長さ（秒）。画像は 0
    pub dur: f64,
    pub image: bool,
    pub has_audio: bool,
    pub has_video: bool,
}

/// 効果音の設定を変えた結果（`se_bank_set` の戻り値と `se_bank` イベントで同じ形）
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, TS)]
#[ts(export)]
pub struct SeBankUpdate {
    /// 変えたあとの設定（fxkit/fxse.py の bank_info）
    pub bank: SeBank,
    /// 効果音を作り直せたか。true のとき wav・count が付く
    pub rendered: bool,
    /// 作り直した印（画面の再生はこの値で効果音の wav を読み直す）
    pub rev: u64,
    /// 作り直した効果音の wav
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub wav: Option<String>,
    /// 効果音の数
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub count: Option<u32>,
    /// 作り直しの警告（棚の音を読めず合成の音で鳴らした、など）。無ければ付かない
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[ts(type = "Array<string> | undefined")]
    pub warnings: Vec<String>,
}

/// 台本の 1 セリフ（cue）。番号は 1 から。時刻は動画全体の秒（script.srt と同じミリ秒の丸め）
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, TS)]
#[ts(export)]
pub struct Cue {
    pub n: i64,
    pub start: f64,
    pub end: f64,
    pub text: String,
    pub speaker: String,
}

/// `files/script_cues.json` の中身（`{"cues": [...]}`）。Rust が書き（`export/talk.rs`）、fxkit の Timeline と align_sync が読む
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
pub struct CueTable {
    pub cues: Vec<Cue>,
}

/// `sync_manual.json` の `sync` の 1 件（手で動かした同期点）。知らない項目は `extra` に残す
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct SyncPoint {
    pub cue: i64,
    pub keyword: String,
    pub t: f64,
    /// 言い終わり。古いファイルには無いことがある（書き戻しでも足さない）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub t_end: Option<f64>,
    /// 無ければ fxkit が "manual" とみなす
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub method: Option<String>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, Value>,
}

/// `sync_manual.json` の全体。`sync` 以外のキーも `extra` に残す
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
pub struct SyncManual {
    #[serde(default)]
    pub sync: Vec<SyncPoint>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, Value>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// 効果音の設定の見本（Python の `engine/test_sebank.py` も同じファイルを読む）
    fn sample_bank_json() -> Value {
        serde_json::from_str(include_str!("../../../src-tauri/tests/fixtures/se_bank.json")).unwrap()
    }

    fn sample_bank() -> SeBank {
        serde_json::from_value(sample_bank_json()).unwrap()
    }

    #[test]
    fn se_bank_fixture_roundtrips() {
        // 見本をそのまま型で読み、書き戻して同じになる（項目の名前・型がずれたらここで分かる）
        assert_eq!(serde_json::to_value(sample_bank()).unwrap(), sample_bank_json());
    }

    #[test]
    fn se_bank_rejects_broken_shape() {
        let mut v = sample_bank_json();
        v["kinds"][0]["level"] = json!("loud");
        assert!(serde_json::from_value::<SeBank>(v).is_err());
        let mut v = sample_bank_json();
        v.as_object_mut().unwrap().remove("db_range");
        assert!(serde_json::from_value::<SeBank>(v).is_err());
    }

    #[test]
    fn scene_keeps_http_shape() {
        let v = serde_json::to_value(Scene { scene: "57-2".into(), t0: 1.5, t1: 9.0 }).unwrap();
        assert_eq!(v, json!({"scene": "57-2", "t0": 1.5, "t1": 9.0}));
    }

    #[test]
    fn se_bank_update_omits_wav_and_count_when_not_rendered() {
        let u = SeBankUpdate { bank: sample_bank(), rendered: false, rev: 7, wav: None, count: None, warnings: vec![] };
        let v = serde_json::to_value(&u).unwrap();
        assert!(v.get("wav").is_none() && v.get("count").is_none() && v["rev"] == 7);
        let u = SeBankUpdate { bank: sample_bank(), rendered: true, rev: 7, wav: Some("a.wav".into()), count: Some(3), warnings: vec![] };
        assert_eq!(serde_json::to_value(&u).unwrap()["count"], 3);
    }

    #[test]
    fn cue_table_shape() {
        let t = CueTable { cues: vec![Cue { n: 1, start: 0.0, end: 1.5, text: "あ".into(), speaker: "トーク1".into() }] };
        let v = serde_json::to_value(&t).unwrap();
        assert_eq!(v, json!({"cues": [{"n": 1, "start": 0.0, "end": 1.5, "text": "あ", "speaker": "トーク1"}]}));
        assert_eq!(serde_json::from_value::<CueTable>(v).unwrap(), t);
    }

    #[test]
    fn sync_point_keeps_unknown_fields_and_omits_missing_method() {
        let v = json!({"cue": 3, "keyword": "あ", "t": 1.5, "t_end": 2.0, "extra": 9});
        let p: SyncPoint = serde_json::from_value(v.clone()).unwrap();
        assert_eq!(p.method, None);
        assert_eq!(p.extra["extra"], json!(9));
        assert_eq!(serde_json::to_value(&p).unwrap(), v);
        let m: SyncManual = serde_json::from_value(json!({"note": "x", "sync": [v.clone()]})).unwrap();
        assert_eq!(serde_json::to_value(&m).unwrap(), json!({"sync": [v], "note": "x"}));
    }

    #[test]
    fn guide_defaults_and_unknown_fields() {
        // 空の guide.json でも読めて、既定の重ね方が入る
        let g: Guide = serde_json::from_str("{}").unwrap();
        assert_eq!(g.version, 1);
        assert_eq!(g.layout.scale, 0.7);
        assert_eq!(g.layout.crops.len(), 3);
        // layout に crops が無ければ既定の切り抜きが入る
        let g: Guide = serde_json::from_value(json!({"layout": {"scale": 0.6}})).unwrap();
        assert_eq!(g.layout.scale, 0.6);
        assert_eq!(g.layout.manual_scale, 0.75);
        assert_eq!(g.layout.crops[0].name, "top");
        // 知らない項目は保存し直しても残る
        let g: Guide = serde_json::from_value(json!({"fps": 60, "anchors": {"manual_end": {"t": 1.5}}})).unwrap();
        let v = serde_json::to_value(&g).unwrap();
        assert_eq!(v["fps"], 60);
        assert_eq!(v["anchors"]["manual_end"], json!({"t": 1.5}));
    }

    #[test]
    fn guide_bench_range_is_checked() {
        use crate::guide::GuideBench;
        assert!(GuideBench::default().check().is_ok());
        assert_eq!(GuideBench::default(), GuideBench { scale: 0.5, opacity: 0.5 });
        // 手で直した settings.json の外れた値は、描くときに範囲へ収める（数でなければ既定）
        assert_eq!(GuideBench { scale: 10.0, opacity: 0.0 }.clamped(), GuideBench { scale: 1.0, opacity: 0.1 });
        assert_eq!(GuideBench { scale: f64::NAN, opacity: 0.3 }.clamped(), GuideBench { scale: 0.5, opacity: 0.3 });
        let ok = |scale, opacity| GuideBench { scale, opacity }.check().is_ok();
        // 端は含む
        assert!(ok(0.2, 0.1) && ok(1.0, 1.0));
        // 外・数でない値は断る（和文）
        assert!(!ok(0.19, 0.5) && !ok(1.01, 0.5) && !ok(0.5, 0.09) && !ok(0.5, 1.01));
        assert!(!ok(f64::NAN, 0.5) && !ok(0.5, f64::INFINITY));
        let e = GuideBench { scale: 2.0, opacity: 0.5 }.check().unwrap_err();
        assert!(e.contains("大きさ") && e.contains("20") && e.contains("100"), "{e}");
        assert!(GuideBench { scale: 0.5, opacity: 0.0 }.check().unwrap_err().contains("不透明度"));
        // 設定の古い形（guide_bench が無い）は既定で読める
        let b: GuideBench = serde_json::from_value(json!({"scale": 0.7})).unwrap();
        assert_eq!(b, GuideBench { scale: 0.7, opacity: 0.5 });
    }

    #[test]
    fn guide_se_defaults_to_synth() {
        // src だけ・db だけでも読める。無ければ合成の音で 0 dB
        let a: GuideSe = serde_json::from_value(json!({"src": "guide_done/a.wav"})).unwrap();
        assert_eq!(a, GuideSe { src: "guide_done/a.wav".into(), db: 0.0 });
        let b: GuideSe = serde_json::from_value(json!({"db": -3.5})).unwrap();
        assert_eq!(b, GuideSe { src: "synth".into(), db: -3.5 });
    }

    #[test]
    fn old_guide_json_gets_zoom_and_sub_defaults() {
        // 段3 までの guide.json（zooms・sub_box・subs が無い）は既定で読める
        let g: Guide = serde_json::from_value(json!({"src": "a.mp4", "layout": {"scale": 0.6, "crops": []}})).unwrap();
        assert!(g.subs.is_empty());
        assert_eq!(g.layout.sub_box, [1060.0, 150.0, 360.0, 240.0]);
        assert_eq!(g.layout.zooms.len(), 1);
        let z = &g.layout.zooms[0];
        assert_eq!((z.name.as_str(), z.src, z.to, z.scale), ("cost", [2280.0, 1560.0, 2752.0, 1765.0], [1456.0, 600.0], 0.95));
        assert_eq!(z.parts, vec!["manual".to_string()]);
        // 書いてあれば（空でも）そのまま。subs は項目が足りなくても読め、知らない項目は残る
        let g: Guide = serde_json::from_value(json!({
            "layout": {"zooms": [], "sub_box": [1.0, 2.0, 3.0, 4.0]},
            "subs": [{"src": "OPVF9634.MP4", "src_in": 1.0, "src_out": 4.0, "at_op": "op1", "z": 1}]
        })).unwrap();
        assert!(g.layout.zooms.is_empty());
        assert_eq!(g.layout.sub_box, [1.0, 2.0, 3.0, 4.0]);
        assert_eq!((g.subs[0].offset, g.subs[0].label.as_str()), (0.0, ""));
        assert_eq!(serde_json::to_value(&g).unwrap()["subs"][0]["z"], 1);
    }

    #[test]
    fn guide_ops_and_formation_are_typed_and_keep_unknown_fields() {
        let g: Guide = serde_json::from_value(json!({"ops": [], "formation": {}})).unwrap();
        assert!(g.ops.is_empty());
        // 操作: where は予約語なので r#where。知らない項目（cost_at・frame）は残る
        let v = json!({"id": "op3", "t": 48.2, "kind": "deploy", "op": "ウタゲ", "where": "R3C5 ↑", "info": "S2 は溜めてから", "precise": true,
                       "cue_text": "Kill15", "note": "早いと危ない", "cost_at": 27, "frame": 2905});
        let g: Guide = serde_json::from_value(json!({"ops": [v.clone()]})).unwrap();
        assert_eq!(g.ops[0].r#where, "R3C5 ↑");
        assert_eq!(g.ops[0].info, "S2 は溜めてから");
        assert!(g.ops[0].precise);
        let out = serde_json::to_value(&g).unwrap();
        assert_eq!(out["ops"][0], v);
        // 項目が足りない操作も読める（古い guide.json は info 無し）
        let g: Guide = serde_json::from_value(json!({"ops": [{"id": "op1", "t": 1.0}]})).unwrap();
        assert_eq!(g.ops[0].info, "");
        assert_eq!((g.ops[0].kind.as_str(), g.ops[0].precise), ("", false));
        // NOW の先出し: 古い guide.json は None（書き出しにも出ない）。値があれば読み書きできる
        assert_eq!(g.ops[0].now_lead, None);
        assert!(serde_json::to_value(&g).unwrap()["ops"][0].get("now_lead").is_none());
        let g: Guide = serde_json::from_value(json!({"ops": [{"id": "op1", "t": 1.0, "now_lead": 2.5}]})).unwrap();
        assert_eq!(g.ops[0].now_lead, Some(2.5));
        assert_eq!(serde_json::to_value(&g).unwrap()["ops"][0]["now_lead"], 2.5);
        // 編成: 既定の 12 枠、used が空なら全部使う
        let f = GuideFormation::default();
        let s = f.effective_slots();
        assert_eq!(s.len(), 12);
        assert_eq!(s[0], [230.0, 426.0, 500.0, 1010.0]);
        assert_eq!(s[5], [1895.0, 426.0, 2165.0, 1010.0]);
        assert_eq!(s[6], [230.0, 1046.0, 500.0, 1630.0]);
        assert!(f.is_used(3));
        let f = GuideFormation { used: vec![0, 2], ..Default::default() };
        assert!(f.is_used(2) && !f.is_used(1));
        let f: GuideFormation = serde_json::from_value(json!({"used": [1], "notes": {"1": "Lv90"}, "x": 1})).unwrap();
        assert_eq!(serde_json::to_value(&f).unwrap()["x"], 1);
    }

    #[test]
    fn guide_fixed_layers() {
        let base = GuideFixed { manual_lead: 5.0, ..Default::default() };
        let o = GuideFixedOpt { result_len: Some(10.0), ..Default::default() };
        let f = o.over(&base);
        assert_eq!((f.manual_lead, f.result_len, f.formation_end), (5.0, 10.0, 8.0));
        // 汎用の NOW の先出し: 既定 1.0 秒。設定に項目が無い古い settings.json も読める。回の値が上に重なる
        assert_eq!(GuideFixed::default().now_lead, 1.0);
        let old: GuideFixed = serde_json::from_value(json!({"manual_lead": 3.0})).unwrap();
        assert_eq!((old.manual_lead, old.now_lead), (3.0, 1.0));
        let o = GuideFixedOpt { now_lead: Some(2.0), ..Default::default() };
        assert_eq!(o.over(&base).now_lead, 2.0);
        assert_eq!(GuideFixedOpt::default().over(&base).now_lead, 1.0);
        // 回の値が無ければ、書き出しにも出ない
        assert_eq!(serde_json::to_value(GuideFixedOpt::default()).unwrap(), json!({}));
    }
}
