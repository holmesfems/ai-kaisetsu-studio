//! 演出の cue 同期点を手で動かす（タイムラインの CUE 行の◆をドラッグ）。
//!
//! 演出の fxkit.Timeline は、同期点のファイル（sync_points.json）と同じフォルダの `sync_manual.json`
//! を優先して読む（形は ak-proto の `SyncManual`・`SyncPoint`）。ここはそのファイルを書き換えて、演出を読み直す。

use anyhow::Context;
use crate::core::Core;
use ak_proto::SyncPoint;
use anyhow::{anyhow, bail, Result};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

fn round3(x: f64) -> f64 {
    (x * 1000.0).round() / 1000.0
}

/// 手で動かした同期点の置き場所。演出が同期点を読んだフォルダ（desc.sync_path の隣）、分からなければ `<回>/files/`。
pub fn manual_path(sync_path: Option<&str>, root: &Path) -> PathBuf {
    match sync_path.map(Path::new).and_then(|p| p.parent()) {
        Some(d) if !d.as_os_str().is_empty() => d.join("sync_manual.json"),
        _ => root.join("files").join("sync_manual.json"),
    }
}

/// sync_manual.json の中身を、(cue, keyword) の 1 件ぶん差し替える純粋な関数。
/// t があれば置き換え（無ければ追加）、None ならそのエントリを消す。他のエントリ・知らない項目・ほかのキーは残す。
/// 書くエントリは `SyncPoint` の形にする。ほかのエントリは手で書いたもの（cue が文字列など）でも、型に通さずそのまま残す
pub fn replace_entry(mut doc: Value, cue: i64, keyword: &str, t: Option<f64>, t_end: Option<f64>) -> Result<Value> {
    let root = doc.as_object_mut().ok_or_else(|| anyhow!("sync_manual.json の形が違います（オブジェクトではありません）"))?;
    let list = root.entry("sync").or_insert_with(|| json!([]));
    let list = list.as_array_mut().ok_or_else(|| anyhow!("sync_manual.json の sync が配列ではありません"))?;
    let same = |e: &Value| e.get("cue").and_then(|c| c.as_i64()) == Some(cue) && e.get("keyword").and_then(|k| k.as_str()) == Some(keyword);
    let Some(t) = t else {
        list.retain(|e| !same(e));
        return Ok(doc);
    };
    let t = round3(t);
    let end = round3(t_end.unwrap_or(t));
    // 前のエントリの知らない項目は extra に入れて残す
    let extra = match list.iter().find(|e| same(e)).and_then(|e| e.as_object()) {
        Some(o) => o.iter().filter(|(k, _)| !matches!(k.as_str(), "cue" | "keyword" | "t" | "t_end" | "method")).map(|(k, v)| (k.clone(), v.clone())).collect(),
        None => Default::default(),
    };
    let point = serde_json::to_value(SyncPoint { cue, keyword: keyword.into(), t, t_end: Some(end), method: Some("manual".into()), extra })?;
    match list.iter_mut().find(|e| same(e)) {
        Some(e) => *e = point,
        None => list.push(point),
    }
    Ok(doc)
}

/// 同期点を新しい時刻 `t` に動かしたときの (t, t_end)。0 未満にはしない。t_end は t と同じだけずらす（無ければ t と同じ）。
/// 小数 3 桁に丸める。
pub fn moved(t0: f64, t_end0: Option<f64>, t: f64) -> (f64, f64) {
    let nt = t.max(0.0);
    let end = t_end0.map(|e| e + (nt - t0)).unwrap_or(nt);
    (round3(nt), round3(end.max(0.0)))
}

/// 同期点を手で動かす（t が None なら自動の位置に戻す）。t_end は今の演出の同期点から Rust が決める。書いたら演出を読み直す。
pub fn set(core: &Core, cue: i64, keyword: &str, t: Option<f64>) -> Result<()> {
    if t.map(|x| !x.is_finite() || x < 0.0).unwrap_or(false) {
        bail!("同期点の時刻が正しくありません");
    }
    let t_end = t.map(|t| {
        let cur = core.desc().and_then(|d| {
            d.sync.iter().find(|e| e.get("cue").and_then(|c| c.as_i64()) == Some(cue) && e.get("keyword").and_then(|k| k.as_str()) == Some(keyword)).cloned()
        });
        match cur.as_ref().and_then(|e| e.get("t").and_then(|x| x.as_f64())) {
            Some(t0) => moved(t0, cur.as_ref().and_then(|e| e.get("t_end")).and_then(|x| x.as_f64()), t).1,
            None => t,
        }
    });
    let root = core.root().ok_or_else(crate::app::error::no_project)?;
    let sync_path = core.desc().and_then(|d| d.sync_path.clone());
    let p = manual_path(sync_path.as_deref(), &root);
    let doc = match std::fs::read_to_string(&p) {
        Ok(s) => serde_json::from_str::<Value>(s.trim_start_matches('\u{feff}'))
            .with_context(|| format!("{} が読めないので書き換えませんでした", p.display()))?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => json!({"sync": []}),
        Err(e) => return Err(anyhow::Error::new(e).context(format!("{} を読めません", p.display()))),
    };
    let doc = replace_entry(doc, cue, keyword, t, t_end).map_err(|e| anyhow!("{}: {e}", p.display()))?;
    if let Some(d) = p.parent() {
        std::fs::create_dir_all(d)?;
    }
    core.mark_self_write(&p);
    crate::settings::write_atomic(&p, serde_json::to_string_pretty(&doc)?)?;
    core.reload_fx();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ak_proto::SyncManual;

    #[test]
    fn 動かしたとき_t_end_は同じだけずれる() {
        assert_eq!(moved(10.0, Some(10.5), 11.0), (11.0, 11.5));
        assert_eq!(moved(10.0, Some(10.5), 9.25), (9.25, 9.75));
    }

    #[test]
    fn t_end_が無ければ_t_と同じ() {
        assert_eq!(moved(10.0, None, 12.0), (12.0, 12.0));
    }

    #[test]
    fn 動かしたとき0未満にはしない() {
        assert_eq!(moved(1.0, Some(1.5), -3.0), (0.0, 0.5));
        assert_eq!(moved(1.0, None, -3.0), (0.0, 0.0));
    }

    #[test]
    fn 動かしたとき3桁に丸める() {
        assert_eq!(moved(1.0, Some(1.2), 1.23456), (1.235, 1.435));
    }

    fn kws(d: &Value) -> Vec<(i64, String, f64)> {
        d["sync"].as_array().unwrap().iter()
            .map(|e| (e["cue"].as_i64().unwrap(), e["keyword"].as_str().unwrap().to_string(), e["t"].as_f64().unwrap()))
            .collect()
    }

    #[test]
    fn add_to_empty() {
        let d = replace_entry(json!({"sync": []}), 3, "あ", Some(1.23456), Some(2.0)).unwrap();
        assert_eq!(d["sync"][0], json!({"cue": 3, "keyword": "あ", "t": 1.235, "t_end": 2.0, "method": "manual"}));
    }

    #[test]
    fn missing_sync_key_is_made() {
        let d = replace_entry(json!({}), 1, "k", Some(0.5), None).unwrap();
        assert_eq!(d["sync"][0]["t_end"], json!(0.5));
    }

    #[test]
    fn replace_keeps_others() {
        let d0 = json!({"note": "x", "sync": [
            {"cue": 1, "keyword": "a", "t": 1.0, "t_end": 1.5, "extra": 9},
            {"cue": 2, "keyword": "b", "t": 2.0, "t_end": 2.5, "method": "manual"}]});
        let d = replace_entry(d0, 1, "a", Some(1.5), Some(2.0)).unwrap();
        assert_eq!(d["note"], json!("x"));
        assert_eq!(d["sync"][0]["extra"], json!(9));
        assert_eq!(d["sync"][0]["t"], json!(1.5));
        assert_eq!(d["sync"][0]["method"], json!("manual"));
        assert_eq!(kws(&d), vec![(1, "a".into(), 1.5), (2, "b".into(), 2.0)]);
    }

    #[test]
    fn same_cue_other_keyword_is_separate() {
        let d0 = json!({"sync": [{"cue": 1, "keyword": "a", "t": 1.0, "t_end": 1.0}]});
        let d = replace_entry(d0, 1, "b", Some(2.0), Some(2.0)).unwrap();
        assert_eq!(d["sync"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn delete_entry() {
        let d0 = json!({"note": 1, "sync": [
            {"cue": 1, "keyword": "a", "t": 1.0, "t_end": 1.0},
            {"cue": 2, "keyword": "b", "t": 2.0, "t_end": 2.0}]});
        let d = replace_entry(d0.clone(), 1, "a", None, None).unwrap();
        assert_eq!(kws(&d), vec![(2, "b".into(), 2.0)]);
        assert_eq!(d["note"], json!(1));
        // 無いものを消しても何も起きない
        assert_eq!(replace_entry(d0.clone(), 9, "z", None, None).unwrap(), d0);
        // 最後の 1 件を消すと空の配列
        let e = replace_entry(json!({"sync": [{"cue": 1, "keyword": "a", "t": 1.0}]}), 1, "a", None, None).unwrap();
        assert_eq!(e, json!({"sync": []}));
    }

    #[test]
    fn bad_shape_is_error() {
        assert!(replace_entry(json!([1]), 1, "a", Some(1.0), None).is_err());
        assert!(replace_entry(json!({"sync": 3}), 1, "a", Some(1.0), None).is_err());
    }

    fn fixture() -> Value {
        let p = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/sync_manual.json");
        serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap()
    }

    /// 見本は SyncManual として読め、書き戻すと同じ JSON になる（fxkit/test_timeline.py も同じ見本を読む）
    #[test]
    fn fixture_round_trips() {
        let doc = fixture();
        let m: SyncManual = serde_json::from_value(doc.clone()).unwrap();
        assert_eq!(m.sync.len(), 3);
        assert!(m.sync.iter().any(|p| p.method.is_none()));
        assert!(m.sync.iter().any(|p| p.extra.contains_key("whisper_t")));
        assert_eq!(serde_json::to_value(&m).unwrap(), doc);
    }

    /// 手で書いた形の違うエントリがあっても、ほかのエントリは動かせる（形の違うものはそのまま残す）
    #[test]
    fn odd_entries_are_kept_as_is() {
        let odd = json!(["コメント", {"cue": "5", "keyword": "x", "t": 1}, {"cue": 6, "keyword": "y", "t": null, "t_end": null}]);
        let d0 = json!({"sync": [odd[0], odd[1], odd[2], {"cue": 7, "keyword": "z", "t": 140, "note": "n"}]});
        let d = replace_entry(d0, 7, "z", Some(2.0), None).unwrap();
        assert_eq!(d["sync"][0], odd[0]);
        assert_eq!(d["sync"][1], odd[1]);
        assert_eq!(d["sync"][2], odd[2]);
        assert_eq!(d["sync"][3], json!({"cue": 7, "keyword": "z", "t": 2.0, "t_end": 2.0, "method": "manual", "note": "n"}));
    }

    #[test]
    fn fixture_move_one_entry() {
        let doc = fixture();
        let first = doc["sync"][0].clone();
        let d = replace_entry(doc.clone(), first["cue"].as_i64().unwrap(), first["keyword"].as_str().unwrap(), Some(9.87654), None).unwrap();
        assert_eq!(d["sync"][0]["t"], json!(9.877));
        assert_eq!(d["sync"][0]["t_end"], json!(9.877));
        assert_eq!(d["sync"][0]["method"], json!("manual"));
        assert_eq!(d["sync"][1], doc["sync"][1]);
        assert_eq!(d["sync"][2], doc["sync"][2]);
    }

    #[test]
    fn manual_path_rules() {
        let root = Path::new("R");
        assert_eq!(manual_path(Some("D/files/sync_points.json"), root), Path::new("D/files/sync_manual.json"));
        assert_eq!(manual_path(None, root), Path::new("R/files/sync_manual.json"));
    }
}
