//! 推定の同期点の一覧（`sync_report`）。演出が `Timeline.s()` で引いた同期点を、重ねずにまとめる。
//! method は whisper（音声から）・manual（手で動かした）・ratio（文字位置からの推定）など。ratio は音と合っていないかもしれない。

use anyhow::{anyhow, Result};
use serde::Serialize;
use serde_json::Value;
use std::collections::HashSet;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SyncRow {
    pub cue: i64,
    pub keyword: String,
    /// true なら「言い終わり」の時刻
    pub end: bool,
    pub t: f64,
    pub method: String,
    /// 文字位置からの推定（method が ratio）。音声に合わせた同期点が無い
    pub estimated: bool,
}

/// `desc.sync_used`（引いた回数ぶん並ぶ）を (cue, keyword, end) ごとに 1 件にする。最初に出てきた順
pub fn report(used: &[Value]) -> Result<Vec<SyncRow>> {
    let mut seen = HashSet::new();
    let mut out = vec![];
    for u in used {
        let bad = || anyhow!("同期点の記録を読めません: {u}");
        let cue = u["cue"].as_i64().ok_or_else(bad)?;
        let keyword = u["keyword"].as_str().ok_or_else(bad)?.to_string();
        let end = u["end"].as_bool().unwrap_or(false);
        let t = u["t"].as_f64().ok_or_else(bad)?;
        let method = u["method"].as_str().unwrap_or("").to_string();
        if !seen.insert((cue, keyword.clone(), end)) {
            continue;
        }
        let estimated = method == "ratio";
        out.push(SyncRow { cue, keyword, end, t, method, estimated });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn dedups_by_cue_keyword_end_and_marks_ratio() {
        let used = [
            json!({"cue": 3, "keyword": "A", "end": false, "t": 1.0, "method": "whisper"}),
            json!({"cue": 3, "keyword": "A", "end": false, "t": 1.0, "method": "whisper"}),
            json!({"cue": 3, "keyword": "A", "end": true, "t": 1.5, "method": "whisper"}),
            json!({"cue": 4, "keyword": "B", "end": false, "t": 2.0, "method": "ratio"}),
            json!({"cue": 4, "keyword": "C", "end": false, "t": 2.5, "method": "manual"}),
        ];
        let r = report(&used).unwrap();
        assert_eq!(r.len(), 4);
        assert_eq!(
            r.iter().map(|x| (x.cue, x.keyword.as_str(), x.end)).collect::<Vec<_>>(),
            [(3, "A", false), (3, "A", true), (4, "B", false), (4, "C", false)]
        );
        assert_eq!(r.iter().map(|x| x.estimated).collect::<Vec<_>>(), [false, false, true, false]);
        assert_eq!(r[3].method, "manual");
    }

    #[test]
    fn broken_record_is_an_error() {
        assert!(report(&[json!({"cue": 1})]).is_err());
        assert!(report(&[]).unwrap().is_empty());
    }
}
