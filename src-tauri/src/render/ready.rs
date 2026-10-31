//! 再生の前に「まだ準備できていない区間」を調べる（演出のコマ・字幕の絵・声の音声）。
//! 画面はここで返した区間の手前で止まって待つ（`ui/src/logic/playwait.ts`）。
//! 判定はキャッシュを見るだけの純粋な関数にして、`Core::play_ready`（core/compose.rs）がまとめる。

use crate::app::dto::VoiceGap;
use crate::render::cache::{any_group_at, frame_key, OverlayCache};
use crate::render::engine::Desc;
use crate::voice::key::tts_key_text;
use crate::voice::layout::VoiceItem;
use serde_json::Value;

/// 区間を足す。直前の区間とつながる（すき間が eps 以下）なら延ばす。
fn push_span(out: &mut Vec<[f64; 2]>, a: f64, b: f64, eps: f64) {
    if let Some(last) = out.last_mut() {
        if a <= last[1] + eps {
            last[1] = last[1].max(b);
            return;
        }
    }
    out.push([a, b]);
}

/// コマ f0..f1 のうち、何か出ているのにキャッシュに無く、描くのに失敗してもいないコマの区間（秒）と、
/// [描けている（キャッシュにある）コマの数, 何か出ているコマの数]。つながるコマは 1 つの区間にまとめる。
pub fn fx_gaps(cache: &OverlayCache, desc: &Desc, ov: &Value, scale: f64, placeholder: bool, f0: i64, f1: i64)
               -> (Vec<[f64; 2]>, [u32; 2]) {
    let mut gaps: Vec<[f64; 2]> = vec![];
    let (mut done, mut total) = (0u32, 0u32);
    let eps = 0.5 / desc.fps.max(1.0);
    for f in f0.max(0)..f1 {
        let t = f as f64 / desc.fps;
        if !any_group_at(&desc.groups, ov, t) {
            continue;
        }
        total += 1;
        let key = frame_key(desc, ov, f, scale, placeholder);
        if cache.contains(key) {
            done += 1;
        } else if !cache.is_failed(key) {
            push_span(&mut gaps, t, (f + 1) as f64 / desc.fps, eps);
        }
    }
    (gaps, [done, total])
}

/// 今の手直しの音声がまだ無い行のうち、[t0, t1] にかかるもの。読まない行（読み上げる文が空）は出さない。
pub fn voice_gaps(items: &[VoiceItem], t0: f64, t1: f64) -> Vec<VoiceGap> {
    items
        .iter()
        .filter(|it| it.audio.is_none() && !tts_key_text(&it.text).is_empty())
        .filter(|it| it.start < t1 && it.start + it.dur > t0)
        .map(|it| VoiceGap { line_id: it.line_id.clone(), t0: it.start, t1: it.start + it.dur })
        .collect()
}

/// [t0, t1] にかかる行のうち、字幕の絵がまだ無い行の区間（秒、時刻順）。
/// `pending` は「描く文があり、絵がキャッシュに無く、描くのに失敗してもいない」行で true（字幕を隠しているときは呼ぶ側が呼ばない）。
pub fn sub_gaps(items: &[VoiceItem], t0: f64, t1: f64, pending: impl Fn(&VoiceItem) -> bool) -> Vec<[f64; 2]> {
    let mut v: Vec<[f64; 2]> = items
        .iter()
        .filter(|it| it.start < t1 && it.start + it.dur > t0)
        .filter(|it| pending(it))
        .map(|it| [it.start, it.start + it.dur])
        .collect();
    v.sort_by(|a, b| a[0].partial_cmp(&b[0]).unwrap_or(std::cmp::Ordering::Equal));
    let mut out: Vec<[f64; 2]> = vec![];
    for [a, b] in v {
        push_span(&mut out, a, b, 0.0);
    }
    out
}

/// 先読みの依頼を引き継げるか。同じ tag の前の依頼が、同じ倍率で、まだ生きていて、
/// 新しい始まり f0 が前の [prev_f0, prev_f1] の中なら、止めずに終わりを延ばすだけでよい。
pub fn can_extend_prefetch(prev_scale: f64, prev_f0: i64, prev_f1: i64, prev_live: usize, scale: f64, f0: i64)
                           -> bool {
    prev_live > 0 && (prev_scale - scale).abs() < 1e-9 && f0 >= prev_f0 && f0 <= prev_f1
}

/// 先読みで足りないスレッドの本数（上限 n に対して、生きている live が減っていたら足す）
pub fn missing_workers(n: usize, live: usize) -> usize {
    n.saturating_sub(live)
}

/// スレッドが抜ける前に、仕事が残っていないか見直す（取り消されておらず、次の番号が終わりより手前なら続ける）
pub fn should_continue(cancelled: bool, next: i64, f1: i64) -> bool {
    !cancelled && next < f1
}

/// 先読みが終わったときに知らせる文。失敗が無ければ None
pub fn prefetch_report(total: i64, failed: usize, first_err: Option<&str>) -> Option<String> {
    if failed == 0 {
        return None;
    }
    let head = format!("レンダリングで {failed} / {total} コマを描けませんでした");
    // 最初のエラーは 1 行にする（Python のトレースバックなら原因の書いてある最後の行）
    let line = first_err.and_then(|e| e.lines().map(str::trim).rev().find(|l| !l.is_empty()));
    Some(match line {
        Some(l) => format!("{head}: {l}"),
        None => head,
    })
}

/// 演出が読めていないときのレンダリングの知らせ（文と、エラーか）。state は EngineStatus の state。
/// 読み込み中（読み直し）はエラーにしない。started は途中まで描いたか（途中で読み直しが始まった）
pub fn not_ready_text(state: &str, error: Option<&str>, started: bool) -> (String, bool) {
    if state == "loading" {
        let what = if started { "演出を読み直しているため、レンダリングを止めました" } else { "演出を読み込み中のため、レンダリングできませんでした" };
        return (format!("{what}。読み込みが終わってから、もう一度押してください"), false);
    }
    let why = error.and_then(|e| e.lines().map(str::trim).rev().find(|l| !l.is_empty())).unwrap_or("演出が読み込まれていません");
    (format!("演出を読み込めていないため、レンダリングできませんでした: {why}"), true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::cache::CachedOverlay;
    use crate::render::engine::OverlayFrame;
    use serde_json::json;
    use std::sync::Arc;

    #[test]
    fn not_ready_text_is_info_while_loading() {
        let (t, err) = not_ready_text("loading", None, true);
        assert!(!err && t.contains("読み直している"), "{t}");
        let (t, err) = not_ready_text("loading", None, false);
        assert!(!err && t.contains("読み込み中"), "{t}");
        let (t, err) = not_ready_text("error", Some("Traceback ...\nSyntaxError: x\n"), false);
        assert!(err && t.ends_with("SyntaxError: x"), "{t}");
        let (t, err) = not_ready_text("none", Some("fx/ にタイムライン（*_fx.py）がありません"), false);
        assert!(err && t.contains("タイムライン"), "{t}");
    }

    #[test]
    fn prefetch_report_only_when_failed() {
        assert_eq!(prefetch_report(300, 0, None), None);
        let s = prefetch_report(300, 12, Some("Traceback ...
ValueError: x
")).unwrap();
        assert!(s.contains("12") && s.contains("300"), "{s}");
        assert!(s.ends_with("ValueError: x"), "{s}");
        assert_eq!(s.lines().count(), 1);
        let s = prefetch_report(300, 300, Some("演出の読み込みが終わっていないため、レンダリングできませんでした")).unwrap();
        assert!(s.contains("演出の読み込みが終わっていないため"), "{s}");
        assert!(prefetch_report(10, 1, None).unwrap().ends_with("描けませんでした"));
    }

    fn desc(groups: &[(&str, f64, f64)]) -> Desc {
        let gs: Vec<Value> = groups
            .iter()
            .map(|(k, a, b)| {
                json!({"key": k, "label": k, "line": 0, "src": "", "t0": a, "t1": b, "n": 1, "kinds": [],
                       "z": 0.0, "fp": k, "gui": false, "se": [], "bbox": null})
            })
            .collect();
        serde_json::from_value(json!({"path": "x", "end": 10.0, "fps": 10.0, "w": 1920, "h": 1080,
            "groups": gs, "cues": [], "sync": [], "sync_used": [], "meta": {}, "build_sec": 0.0,
            "stage": [], "rec_box": []}))
        .unwrap()
    }

    fn put(cache: &OverlayCache, d: &Desc, ov: &Value, f: i64) {
        let fr = OverlayFrame { w: 1, h: 1, rect: None, rgba: vec![0; 4], ms: 0.0 };
        cache.put(frame_key(d, ov, f, 0.5, false), Arc::new(CachedOverlay::from_frame(&fr)));
    }

    #[test]
    fn fx_何も出ていないコマは区間にならない() {
        let d = desc(&[("a@c1#1", 1.0, 2.0)]);
        let c = OverlayCache::new(8);
        let (g, n) = fx_gaps(&c, &d, &json!({}), 0.5, false, 0, 30);
        assert_eq!(g, vec![[1.0, 2.0]]);
        assert_eq!(n, [0, 10]);
        let (g, n) = fx_gaps(&c, &d, &json!({}), 0.5, false, 20, 30);
        assert!(g.is_empty());
        assert_eq!(n, [0, 0]);
    }

    #[test]
    fn fx_キャッシュにあるコマと失敗したコマは区間にならない() {
        let d = desc(&[("a@c1#1", 1.0, 2.0)]);
        let ov = json!({});
        let c = OverlayCache::new(8);
        put(&c, &d, &ov, 10);
        put(&c, &d, &ov, 11);
        c.mark_failed(frame_key(&d, &ov, 15, 0.5, false));
        let (g, n) = fx_gaps(&c, &d, &ov, 0.5, false, 0, 30);
        assert_eq!(n, [2, 10]);
        assert_eq!(g.len(), 2);
        assert!((g[0][0] - 1.2).abs() < 1e-9 && (g[0][1] - 1.5).abs() < 1e-9);
        assert!((g[1][0] - 1.6).abs() < 1e-9 && (g[1][1] - 2.0).abs() < 1e-9);
    }

    #[test]
    fn fx_隠したクリップは数えない() {
        let d = desc(&[("a@c1#1", 1.0, 2.0)]);
        let ov = json!({"a@c1#1": {"hidden": true}});
        let c = OverlayCache::new(8);
        let (g, n) = fx_gaps(&c, &d, &ov, 0.5, false, 0, 50);
        assert!(g.is_empty());
        assert_eq!(n, [0, 0]);
    }

    fn item(id: &str, text: &str, start: f64, dur: f64, audio: Option<&str>) -> VoiceItem {
        VoiceItem {
            line_id: id.into(),
            n: 1,
            sp: "トーク1".into(),
            text: text.into(),
            start,
            dur,
            audio: audio.map(|s| s.to_string()),
            block: String::new(),
            key: String::new(),
        }
    }

    #[test]
    fn voice_音声のある行と読まない行と区間外は出ない() {
        let items = vec![
            item("a", "こんにちは", 0.0, 2.0, Some("x")),
            item("b", "こんばんは", 2.0, 2.0, None),
            item("c", " ", 4.0, 1.0, None),
            item("d", "さようなら", 10.0, 2.0, None),
        ];
        let g = voice_gaps(&items, 1.0, 6.0);
        assert_eq!(g.len(), 1);
        assert_eq!(g[0].line_id, "b");
        assert_eq!((g[0].t0, g[0].t1), (2.0, 4.0));
        // ちょうど接するだけの行は含めない
        assert!(voice_gaps(&items, 4.0, 10.0).is_empty());
    }

    #[test]
    fn sub_かかる行だけを時刻順にまとめる() {
        let items = vec![
            item("a", "あ", 0.0, 1.0, None),
            item("b", "い", 1.0, 1.0, None),
            item("c", "う", 3.0, 1.0, None),
            item("d", "え", 9.0, 1.0, None),
        ];
        let g = sub_gaps(&items, 0.5, 5.0, |it| it.line_id != "a");
        assert_eq!(g, vec![[1.0, 2.0], [3.0, 4.0]]);
        let g = sub_gaps(&items, 0.5, 5.0, |_| true);
        assert_eq!(g, vec![[0.0, 2.0], [3.0, 4.0]]);
    }

    #[test]
    fn 先読みの引き継ぎは中なら延ばし外なら立て直す() {
        assert!(can_extend_prefetch(0.5, 10, 130, 2, 0.5, 10));
        assert!(can_extend_prefetch(0.5, 10, 130, 2, 0.5, 130));
        assert!(!can_extend_prefetch(0.5, 10, 130, 2, 0.5, 9));
        assert!(!can_extend_prefetch(0.5, 10, 130, 2, 0.5, 131));
        assert!(!can_extend_prefetch(0.5, 10, 130, 0, 0.5, 20));
        assert!(!can_extend_prefetch(0.5, 10, 130, 2, 1.0, 20));
    }

    #[test]
    fn 抜けたスレッドは足し_延ばされていたら続ける() {
        assert_eq!(missing_workers(4, 1), 3);
        assert_eq!(missing_workers(4, 4), 0);
        assert_eq!(missing_workers(2, 5), 0);
        assert!(should_continue(false, 100, 130));
        assert!(!should_continue(false, 130, 130));
        assert!(!should_continue(true, 100, 130));
    }
}
