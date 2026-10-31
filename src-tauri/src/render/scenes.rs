//! シーン番号（「57」「57-2」）を決める。番号を決めるのは Rust だけで、Python のワーカーは札の絵を描くだけ（旧#97）。
//! モニターの SCENE N・Claude に渡す「シーン N」・仮表示つきの演出 .mov の札は、どれもここの結果から作る。
//! 決め方は fxkit の Timeline.scenes と同じだが、微調整のずらし・隠すを入れた後の時刻で数える。

use crate::render::cache::group_span;
use crate::render::engine::Desc;
use serde_json::Value;

/// 同じシーンの中で、この秒数より近い出だしはまとめる（fxkit の Timeline.scenes の merge）
const MERGE: f64 = 1.0;

/// 札を置く位置（fxparts.SceneLabel の xy）
pub const LABEL_XY: (i64, i64) = (16, 12);

/// シーンの一覧 (番号, 始まり, 終わり)。CUE_A..CUE_B の cue ごとに、cue の中に別のクリップの出だしがあれば「57-1」「57-2」と分ける
pub fn scenes(desc: &Desc, ov: &Value) -> Vec<(String, f64, f64)> {
    let cue_a = desc.meta.get("CUE_A").and_then(|v| v.as_i64());
    let cue_b = desc.meta.get("CUE_B").and_then(|v| v.as_i64());
    let cues: Vec<&ak_proto::Cue> = desc
        .cues
        .iter()
        .filter(|c| cue_a.map(|a| c.n >= a).unwrap_or(true) && cue_b.map(|b| c.n <= b).unwrap_or(true))
        .collect();
    let mut starts: Vec<f64> = desc
        .groups
        .iter()
        .filter_map(|g| {
            let (a, _, hid) = group_span(g, ov);
            (!hid).then_some(a)
        })
        .collect();
    starts.sort_by(|a, b| a.total_cmp(b));
    let mut out = vec![];
    for (i, c) in cues.iter().enumerate() {
        let a = c.start;
        let b = cues.get(i + 1).map(|n| n.start).unwrap_or(desc.end);
        let mut ts = vec![a];
        for &t in &starts {
            if a + MERGE <= t && t < b - MERGE && t - ts.last().unwrap() >= MERGE {
                ts.push(t);
            }
        }
        for (k, &t) in ts.iter().enumerate() {
            let sid = if ts.len() == 1 { c.n.to_string() } else { format!("{}-{}", c.n, k + 1) };
            out.push((sid, t, *ts.get(k + 1).unwrap_or(&b)));
        }
    }
    out
}

/// 時刻 t のシーン番号。どのシーンにも入らなければ "-"
pub fn scene_at(scenes: &[(String, f64, f64)], t: f64) -> String {
    scenes.iter().find(|(_, a, b)| *a <= t && t < *b).map(|s| s.0.clone()).unwrap_or_else(|| "-".into())
}

/// シーンを見せる時刻（頭から 0.8 秒。短いシーンは真ん中）。"s57-2" のような頭の s は外す。無ければ None。
/// MCP の render_still と ui_focus の scene が同じ時刻になるように、画面ではなくここで決める
pub fn scene_focus_t(scenes: &[(String, f64, f64)], scene: &str) -> Option<f64> {
    let sc = scene.trim_start_matches('s');
    scenes.iter().find(|s| s.0 == sc).map(|s| (s.1 + 0.8).min((s.1 + s.2) / 2.0))
}

/// 全体の絵（RGBA。ストレートアルファ）
#[derive(Clone, Debug, PartialEq)]
pub struct Rgba {
    pub w: u32,
    pub h: u32,
    pub data: Vec<u8>,
}

impl Rgba {
    pub fn transparent(w: u32, h: u32) -> Rgba {
        Rgba { w, h, data: vec![0; w as usize * h as usize * 4] }
    }
}

/// コマ（無ければ透明）の (16, 12) に札を重ねる。コマの外にはみ出す分は切る
pub fn overlay_label(frame: Option<Rgba>, label: &Rgba, w: u32, h: u32) -> Rgba {
    let mut out = frame.unwrap_or_else(|| Rgba::transparent(w, h));
    for ly in 0..label.h as i64 {
        let y = LABEL_XY.1 + ly;
        if y < 0 || y >= out.h as i64 {
            continue;
        }
        for lx in 0..label.w as i64 {
            let x = LABEL_XY.0 + lx;
            if x < 0 || x >= out.w as i64 {
                continue;
            }
            let si = (ly as usize * label.w as usize + lx as usize) * 4;
            let di = (y as usize * out.w as usize + x as usize) * 4;
            let la = label.data[si + 3] as f32 / 255.0;
            if la <= 0.0 {
                continue;
            }
            let fa = out.data[di + 3] as f32 / 255.0;
            let oa = la + fa * (1.0 - la);
            for c in 0..3 {
                let v = (label.data[si + c] as f32 * la + out.data[di + c] as f32 * fa * (1.0 - la)) / oa;
                out.data[di + c] = v.round().clamp(0.0, 255.0) as u8;
            }
            out.data[di + 3] = (oa * 255.0).round().clamp(0.0, 255.0) as u8;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// groups: (key, t0, t1)、cues: (n, start)。fps 10・終わり 100 秒
    fn desc(groups: &[(&str, f64, f64)], cues: &[(i64, f64)], meta: Value) -> Desc {
        let gs: Vec<Value> = groups
            .iter()
            .map(|(k, a, b)| {
                json!({"key": k, "label": k, "line": 0, "src": "", "t0": a, "t1": b, "n": 1, "kinds": [],
                       "z": 0.0, "fp": k, "gui": false, "se": [], "bbox": null})
            })
            .collect();
        let cs: Vec<Value> =
            cues.iter().map(|(n, s)| json!({"n": n, "start": s, "end": s + 1.0, "text": "", "speaker": ""})).collect();
        serde_json::from_value(json!({"path": "x", "end": 100.0, "fps": 10.0, "w": 1920, "h": 1080,
            "groups": gs, "cues": cs, "sync": [], "sync_used": [], "meta": meta, "build_sec": 0.0,
            "stage": [], "rec_box": []}))
        .unwrap()
    }

    #[test]
    fn scene_focus_is_head_plus_08_or_middle() {
        let sc = vec![("3".to_string(), 10.0, 20.0), ("4".to_string(), 10.0, 11.0), ("57-2".to_string(), 30.0, 40.0)];
        assert_eq!(scene_focus_t(&sc, "3"), Some(10.8));
        assert_eq!(scene_focus_t(&sc, "4"), Some(10.5));
        assert_eq!(scene_focus_t(&sc, "s3"), scene_focus_t(&sc, "3"));
        assert_eq!(scene_focus_t(&sc, "s57-2"), Some(30.8));
        assert_eq!(scene_focus_t(&sc, "999"), None);
        assert_eq!(scene_focus_t(&[], "3"), None);
    }

    fn ids(s: &[(String, f64, f64)]) -> Vec<&str> {
        s.iter().map(|x| x.0.as_str()).collect()
    }

    #[test]
    fn cue_1つにシーン1つなら番号だけ() {
        let d = desc(&[("a", 10.0, 12.0)], &[(56, 0.0), (57, 10.0)], json!({}));
        let s = scenes(&d, &json!({}));
        assert_eq!(ids(&s), ["56", "57"]);
        assert_eq!((s[0].1, s[0].2), (0.0, 10.0));
        assert_eq!((s[1].1, s[1].2), (10.0, 100.0));
    }

    #[test]
    fn cueの中に別の出だしがあれば分ける() {
        let d = desc(&[("a", 10.0, 12.0), ("b", 14.0, 15.0), ("c", 14.5, 16.0)], &[(57, 10.0), (58, 20.0)], json!({}));
        let s = scenes(&d, &json!({}));
        // 14.5 は 14.0 から 1 秒未満なのでまとめる
        assert_eq!(ids(&s), ["57-1", "57-2", "58"]);
        assert_eq!((s[0].1, s[0].2), (10.0, 14.0));
        assert_eq!((s[1].1, s[1].2), (14.0, 20.0));
    }

    #[test]
    fn cueの端から1秒以内は分けない() {
        let d = desc(&[("a", 10.5, 12.0)], &[(57, 10.0), (58, 20.0)], json!({}));
        assert_eq!(ids(&scenes(&d, &json!({}))), ["57", "58"]);
        let d = desc(&[("a", 19.5, 21.0)], &[(57, 10.0), (58, 20.0)], json!({}));
        assert_eq!(ids(&scenes(&d, &json!({}))), ["57", "58"]);
    }

    #[test]
    fn cue_a_cue_bの範囲だけ() {
        let d = desc(&[], &[(1, 0.0), (2, 10.0), (3, 20.0), (4, 30.0)], json!({"CUE_A": 2, "CUE_B": 3}));
        let s = scenes(&d, &json!({}));
        assert_eq!(ids(&s), ["2", "3"]);
        assert_eq!(s[1].2, 100.0);
    }

    #[test]
    fn ずらしを入れた時刻で数える() {
        let d = desc(&[("a", 10.5, 11.5)], &[(57, 10.0), (58, 20.0)], json!({}));
        assert_eq!(ids(&scenes(&d, &json!({}))), ["57", "58"]);
        // 10.5 秒の出だしを 4.5 秒後ろへずらすと 15 秒で次のシーンが切れる
        let s = scenes(&d, &json!({"a": {"shift": 4.5}}));
        assert_eq!(ids(&s), ["57-1", "57-2", "58"]);
        assert_eq!((s[0].2, s[1].1), (15.0, 15.0));
    }

    #[test]
    fn 隠したクリップは数えない() {
        let d = desc(&[("a", 15.0, 16.0)], &[(57, 10.0), (58, 20.0)], json!({}));
        assert_eq!(ids(&scenes(&d, &json!({}))), ["57-1", "57-2", "58"]);
        assert_eq!(ids(&scenes(&d, &json!({"a": {"hidden": true}}))), ["57", "58"]);
    }

    #[test]
    fn scene_atは区間の外で横棒() {
        let s = vec![("5".to_string(), 10.0, 20.0), ("6".to_string(), 20.0, 30.0)];
        assert_eq!(scene_at(&s, 9.9), "-");
        assert_eq!(scene_at(&s, 10.0), "5");
        assert_eq!(scene_at(&s, 19.99), "5");
        assert_eq!(scene_at(&s, 20.0), "6");
        assert_eq!(scene_at(&s, 30.0), "-");
        assert_eq!(scene_at(&[], 1.0), "-");
    }

    fn solid(w: u32, h: u32, px: [u8; 4]) -> Rgba {
        Rgba { w, h, data: px.iter().copied().cycle().take(w as usize * h as usize * 4).collect() }
    }

    fn px(r: &Rgba, x: u32, y: u32) -> [u8; 4] {
        let i = (y as usize * r.w as usize + x as usize) * 4;
        [r.data[i], r.data[i + 1], r.data[i + 2], r.data[i + 3]]
    }

    #[test]
    fn 演出が無いコマは透明なコマに札だけを描く() {
        let label = solid(4, 3, [10, 20, 30, 255]);
        let r = overlay_label(None, &label, 100, 50);
        assert_eq!((r.w, r.h), (100, 50));
        assert_eq!(px(&r, 16, 12), [10, 20, 30, 255]);
        assert_eq!(px(&r, 19, 14), [10, 20, 30, 255]);
        assert_eq!(px(&r, 20, 12), [0, 0, 0, 0]);
        assert_eq!(px(&r, 15, 12), [0, 0, 0, 0]);
        assert_eq!(px(&r, 16, 15), [0, 0, 0, 0]);
    }

    #[test]
    fn 演出のコマには札を上に重ねる() {
        let base = solid(100, 50, [200, 0, 0, 255]);
        let label = solid(2, 2, [0, 0, 100, 255]);
        let r = overlay_label(Some(base.clone()), &label, 100, 50);
        assert_eq!(px(&r, 16, 12), [0, 0, 100, 255]);
        assert_eq!(px(&r, 50, 30), [200, 0, 0, 255]);
        // 半透明の札は下の絵と混ぜる
        let half = solid(1, 1, [0, 0, 0, 128]);
        let r = overlay_label(Some(base), &half, 100, 50);
        let p = px(&r, 16, 12);
        assert!((p[0] as i32 - 100).abs() <= 1 && p[3] == 255, "{p:?}");
    }

    #[test]
    fn 札がコマからはみ出しても落ちない() {
        let label = solid(200, 100, [1, 2, 3, 255]);
        let r = overlay_label(None, &label, 50, 20);
        assert_eq!(r.data.len(), 50 * 20 * 4);
        assert_eq!(px(&r, 49, 19), [1, 2, 3, 255]);
        assert_eq!(px(&r, 15, 19), [0, 0, 0, 0]);
    }
}
