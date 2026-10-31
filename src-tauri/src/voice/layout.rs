//! セリフの並び（VoiceItem）の計算。字幕・書き出し・Claude への説明の元になる。

use crate::script::format::{Block, Line, Script};
use crate::voice::analysis::Analyses;
use crate::voice::key::{sub_text, tts_key_text, tts_text, vkey};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use ts_rs::TS;

/// セリフの並び（台本の開始時刻＋音声の長さ）。字幕・話者・書き出し・Claude への説明に使う。
#[derive(Serialize, Deserialize, Clone, Debug, TS)]
#[ts(export)]
pub struct VoiceItem {
    pub line_id: String,
    /// 台本の通し番号（1 から）
    pub n: usize,
    pub sp: String,
    /// 字幕の文（ルビは表記）
    pub text: String,
    pub start: f64,
    pub dur: f64,
    /// 音声のキャッシュの id（voice_cache/<id>.wav）。今の手直しで作った音声がまだ無ければ None
    pub audio: Option<String>,
    #[serde(default)]
    pub block: String,
    /// 声の解析を引く鍵（vkey）
    #[serde(default)]
    pub key: String,
}

/// セリフの並びと、それが音声の長さで決まったものか（`layout` イベントの中身・`voice_layout_preview` の戻り値）
#[derive(Serialize, Deserialize, Clone, Debug, TS)]
#[ts(export)]
pub struct LayoutState {
    pub items: Vec<VoiceItem>,
    /// 並びの終わり（秒）
    pub total: f64,
    /// 声の解析が一度そろった（中継から長さを受け取れた）か。トラックを出してよい目安
    pub ready: bool,
}

/// 声の解析の増えた・変わった分（`analysis` イベントの中身。鍵は `VoiceItem.key`）
#[derive(Serialize, Deserialize, Clone, Debug, TS)]
#[ts(export)]
pub struct AnalysisUpdate {
    pub entries: HashMap<String, crate::voice::analysis::Analysis>,
}

fn r3(v: f64) -> f64 {
    (v * 1000.0).round() / 1000.0
}

/// 有限でない値（YAML の `.nan` など）は無いものとして扱う
fn finite(v: Option<f64>) -> Option<f64> {
    v.filter(|x| x.is_finite())
}

/// 読み上げる行（空でない行）。n は台本の通し番号
pub struct Placed<'a> {
    pub l: &'a Line,
    pub b: &'a Block,
    /// ブロックの最初の読み上げる行
    pub first: bool,
    pub n: usize,
}

pub fn voiced_lines(s: &Script) -> Vec<Placed<'_>> {
    let mut out = vec![];
    let mut n = 0;
    for b in &s.blocks {
        let mut first = true;
        for l in &b.lines {
            n += 1;
            if tts_key_text(&l.text).is_empty() {
                continue;
            }
            out.push(Placed { l, b, first, n });
            first = false;
        }
    }
    out
}

/// 音声がまだ無い行の長さの見積もり（秒）。TS の String.length（UTF-16 の長さ）と同じ数え方
pub fn est_dur(l: &Line) -> f64 {
    (tts_text(&l.text).encode_utf16().count() as f64 * 0.13).max(0.8)
}

pub struct Layout {
    pub items: Vec<VoiceItem>,
    /// 開始が変わった行の新しい開始（台本には書き戻さない。ST-8）
    #[allow(dead_code)] // テストで見る（画面・書き出しには使わない）
    pub starts: HashMap<String, f64>,
    pub total: f64,
}

/// 並べ方の間隔（秒）。有限でなければ 0
fn gap_val(g: f64) -> f64 {
    if g.is_finite() {
        g
    } else {
        0.0
    }
}

/// 台本エディタのトラックビューと同じ並べ方。
///  1. 開始が書いてある行の順番が台本の順番と食い違っていたら、その区間を詰め直す
///  2. 開始が無い行は、前の行の後ろに間隔（gap / block_gap）をあけて置く
///  3. 開始が書いてある行は動かさない。前の行と本当に重なるとき（前の行の長さが音声で分かっているとき）だけ、
///     前の行の終わりまで押し出す。間隔はここでは足さない（足すと、間隔の設定が台本を作ったときと違うだけで
///     全部の行が後ろへずれ、files/script.srt の cue＝演出の時刻と合わなくなる。issue 旧#4）
///
/// 間隔どおりに並べ直したいときは画面の「詰め直す」（relayoutFrom）。
/// 返り値の starts は、変わった行の新しい開始（台本には書き戻さない。ST-8）。
pub fn layout(s: &Script, analyses: &Analyses, gap: f64, block_gap: f64) -> Layout {
    let (gap, block_gap) = (gap_val(gap), gap_val(block_gap));
    let placed_lines = voiced_lines(s);
    // 画面の並び（arr）: 各行の開始（有限でなければ None）
    let mut start: Vec<Option<f64>> = placed_lines.iter().map(|x| finite(x.l.start)).collect();
    let dur_of = |l: &Line| analyses.duration(l).unwrap_or_else(|| est_dur(l));
    let gap_before = |first: bool, idx: usize| if idx == 0 { 0.0 } else if first { block_gap } else { gap };

    // 開始が書いてある行（arr の添字）と、それを開始の順（同じなら台本の順）に並べたもの
    let placed: Vec<usize> = (0..start.len()).filter(|&i| start[i].is_some()).collect();
    let mut by_time = placed.clone();
    by_time.sort_by(|&p, &q| start[p].unwrap().total_cmp(&start[q].unwrap()).then(p.cmp(&q)));
    let mut i = 0;
    while i < placed.len() && placed[i] == by_time[i] {
        i += 1;
    }
    if i < placed.len() {
        let mut k = placed.len() - 1;
        while placed[k] == by_time[k] {
            k -= 1;
        }
        let mut t = placed[i..=k].iter().map(|&x| start[x].unwrap()).fold(f64::INFINITY, f64::min);
        for n in i..=k {
            let x = placed[n];
            if n > i {
                t += gap_before(placed_lines[x].first, x);
            }
            start[x] = Some(r3(t.max(0.0)));
            t += dur_of(placed_lines[x].l);
        }
    }

    // 推定の長さ（音声がまだ無い行）で押し出すと、実際の音声より長めに見積もったぶんだけ後ろの行がずれて
    // 書き戻されてしまうので、押し出しは長さが分かっている行の後ろだけ
    let mut prev_end = 0.0;
    let mut prev_known = false;
    let mut out_start = vec![0.0; start.len()];
    for (idx, x) in placed_lines.iter().enumerate() {
        let v = match start[idx] {
            None => r3((if idx == 0 { 0.0 } else { prev_end + gap_before(x.first, idx) }).max(0.0)),
            Some(v) if v < 0.0 => 0.0,
            Some(v) if idx > 0 && prev_known && v < prev_end - 0.0005 => r3(prev_end),
            Some(v) => v,
        };
        out_start[idx] = v;
        prev_end = v + dur_of(x.l);
        prev_known = analyses.duration(x.l).is_some();
    }

    let mut starts = HashMap::new();
    for (idx, x) in placed_lines.iter().enumerate() {
        if finite(x.l.start) != Some(out_start[idx]) {
            starts.insert(x.l.id.clone(), out_start[idx]);
        }
    }
    let items: Vec<VoiceItem> = placed_lines
        .iter()
        .enumerate()
        .map(|(idx, x)| {
            let key = vkey(&x.l.sp, &x.l.text);
            let audio = if analyses.fresh(s, x.l) { analyses.get(&key).map(|a| a.id.clone()) } else { None };
            VoiceItem {
                line_id: x.l.id.clone(),
                n: x.n,
                sp: x.l.sp.clone(),
                text: sub_text(&x.l.text),
                start: out_start[idx],
                dur: dur_of(x.l),
                audio,
                block: x.b.title.clone(),
                key,
            }
        })
        .collect();
    let total = items.iter().fold(0.0, |m: f64, it| m.max(it.start + it.dur));
    Layout { items, starts, total }
}

/// 見積もった間隔（秒）。見本が足りないものは None（今の設定のまま）
pub struct Gaps {
    pub gap: Option<f64>,
    pub block_gap: Option<f64>,
    pub samples: usize,
}

/// 台本の開始と長さから、台本を作ったときの間隔（セリフの間隔・ブロックの間隔）を見積もる。
/// 前の行の長さが音声で分かっていて、両方に開始が書いてある隣どうしの行の「前の終わり → 次の頭」の中央値
/// （0.05 秒単位）。録画の待ちなどでわざと空けた所に引っぱられないよう中央値にする。
/// 見本が足りないときは None（今の設定のまま）。
pub fn infer_gaps(s: &Script, analyses: &Analyses) -> Gaps {
    let arr = voiced_lines(s);
    let mut in_line: Vec<f64> = vec![];
    let mut in_block: Vec<f64> = vec![];
    for i in 1..arr.len() {
        let p = arr[i - 1].l;
        let x = &arr[i];
        let (Some(ps), Some(xs), Some(d)) = (finite(p.start), finite(x.l.start), analyses.duration(p)) else {
            continue;
        };
        let g = xs - (ps + d);
        if !(-0.01..=5.0).contains(&g) {
            continue; // 重なり・長い待ちは見本にしない
        }
        (if x.first { &mut in_block } else { &mut in_line }).push(g.max(0.0));
    }
    let med = |v: &[f64]| {
        let mut a = v.to_vec();
        a.sort_by(|p, q| p.total_cmp(q));
        let m = if a.len() % 2 == 1 { a[(a.len() - 1) / 2] } else { (a[a.len() / 2 - 1] + a[a.len() / 2]) / 2.0 };
        (m / 0.05 + 0.5).floor() * 0.05
    };
    Gaps {
        gap: (in_line.len() >= 3).then(|| r3(med(&in_line))),
        block_gap: (in_block.len() >= 2).then(|| r3(med(&in_block))),
        samples: in_line.len() + in_block.len(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::voice::analysis::Analysis;

    fn l(id: &str, text: &str, start: Option<f64>) -> Line {
        Line { id: id.into(), sp: "トーク1".into(), text: text.into(), start, memo: String::new() }
    }

    fn b(title: &str, lines: Vec<Line>) -> Block {
        Block { id: format!("b-{title}"), file: None, title: title.into(), lines, notes: String::new() }
    }

    fn s(blocks: Vec<Block>) -> Script {
        Script { blocks, ..Default::default() }
    }

    fn an(id: &str, d: f64) -> Analysis {
        Analysis {
            id: id.into(),
            duration: d,
            unread: vec![],
            analyzed_text: String::new(),
            phonemes: None,
            phoneme_durations: None,
            speaker_params: None,
        }
    }

    /// 行の長さを「音声で分かっている」ことにする（今の手直しで作った音声ではない＝fresh ではない）
    fn durs(s: &Script, d: &[(&str, f64)]) -> Analyses {
        let mut a = Analyses::default();
        for x in voiced_lines(s) {
            if let Some((_, v)) = d.iter().find(|(t, _)| *t == x.l.text) {
                a.insert(vkey(&x.l.sp, &x.l.text), an(&x.l.text, *v), String::new());
            }
        }
        a
    }

    fn starts_of(r: &Layout) -> Vec<f64> {
        r.items.iter().map(|x| x.start).collect()
    }

    #[test]
    fn written_starts_stay_even_if_gaps_differ() {
        // issue 旧#4: ブロックの間も 0.3 秒で作った台本を、ブロックの間隔 1.0 で開く
        let sc = s(vec![
            b("A", vec![l("1", "a1", Some(0.0)), l("2", "a2", Some(2.3))]),
            b("B", vec![l("3", "b1", Some(4.6)), l("4", "b2", Some(6.9))]),
        ]);
        let an = durs(&sc, &[("a1", 2.0), ("a2", 2.0), ("b1", 2.0), ("b2", 2.0)]);
        let r = layout(&sc, &an, 0.3, 1.0);
        assert_eq!(r.starts.len(), 0);
        assert_eq!(starts_of(&r), vec![0.0, 2.3, 4.6, 6.9]);
    }

    #[test]
    fn pushes_only_when_overlapping() {
        let sc = s(vec![b("A", vec![l("1", "a1", Some(0.0)), l("2", "a2", Some(1.5)), l("3", "a3", Some(4.2))])]);
        let an = durs(&sc, &[("a1", 2.0), ("a2", 2.0), ("a3", 2.0)]);
        let r = layout(&sc, &an, 0.3, 1.0);
        // a2 は a1 の終わり 2.0 へ。a3 は a2 の終わり 4.0 より後なのでそのまま
        assert_eq!(starts_of(&r), vec![0.0, 2.0, 4.2]);
        assert_eq!(r.starts.len(), 1);
    }

    #[test]
    fn estimated_previous_does_not_push() {
        let sc = s(vec![b(
            "A",
            vec![l("1", "とても長いセリフとても長いセリフとても長いセリフ", Some(0.0)), l("2", "b", Some(1.0))],
        )]);
        let r = layout(&sc, &Analyses::default(), 0.3, 1.0);
        assert_eq!(r.starts.len(), 0);
    }

    #[test]
    fn lines_without_start_follow_with_gaps() {
        let sc = s(vec![b("A", vec![l("1", "a1", Some(0.0)), l("2", "a2", None)]), b("B", vec![l("3", "b1", None)])]);
        let an = durs(&sc, &[("a1", 2.0), ("a2", 1.0), ("b1", 1.0)]);
        let r = layout(&sc, &an, 0.3, 1.0);
        assert_eq!(starts_of(&r), vec![0.0, 2.3, 4.3]);
        assert_eq!(r.starts.len(), 2);
    }

    #[test]
    fn out_of_order_starts_are_repacked() {
        let sc = s(vec![b("A", vec![l("1", "a1", Some(0.0)), l("2", "a2", Some(9.0)), l("3", "a3", Some(3.0))])]);
        let an = durs(&sc, &[("a1", 2.0), ("a2", 1.0), ("a3", 1.0)]);
        let st = starts_of(&layout(&sc, &an, 0.3, 1.0));
        assert!(st[0] < st[1] && st[1] < st[2]);
    }

    #[test]
    fn estimated_duration() {
        // 短い行は 0.8 秒、長い行は 1 文字 0.13 秒（ルビは読みの長さ）
        assert_eq!(est_dur(&l("1", "あ", None)), 0.8);
        assert!((est_dur(&l("1", &"あ".repeat(10), None)) - 1.3).abs() < 1e-9);
        assert!((est_dur(&l("1", "｜時雨《しぐれ》", None)) - 0.8).abs() < 1e-9);
        let sc = s(vec![b("A", vec![l("1", "あ", None), l("2", &"い".repeat(20), None)])]);
        let r = layout(&sc, &Analyses::default(), 0.3, 1.0);
        assert_eq!(r.items[0].dur, 0.8);
        assert!((r.items[1].start - 1.1).abs() < 1e-9);
        assert!((r.total - (1.1 + 2.6)).abs() < 1e-9);
    }

    #[test]
    fn infers_gaps() {
        let sc = s(vec![
            b("A", vec![l("1", "a1", Some(0.0)), l("2", "a2", Some(2.3)), l("3", "a3", Some(4.6)), l("4", "a4", Some(6.9))]),
            b("B", vec![l("5", "b1", Some(9.2)), l("6", "b2", Some(11.5))]),
            b("C", vec![l("7", "c1", Some(13.8)), l("8", "c2", Some(40.0))]), // 長い待ちは見本にしない
        ]);
        let an = durs(&sc, &[("a1", 2.0), ("a2", 2.0), ("a3", 2.0), ("a4", 2.0), ("b1", 2.0), ("b2", 2.0), ("c1", 2.0), ("c2", 2.0)]);
        let g = infer_gaps(&sc, &an);
        assert_eq!(g.gap, Some(0.3));
        assert_eq!(g.block_gap, Some(0.3));
        assert_eq!(g.samples, 6);
        // 見本が足りなければ None
        let g = infer_gaps(&sc, &Analyses::default());
        assert_eq!((g.gap, g.block_gap, g.samples), (None, None, 0));
    }

    #[test]
    fn non_finite_starts_are_treated_as_none() {
        let sc = s(vec![b(
            "A",
            vec![l("1", "a1", Some(0.0)), l("2", "a2", Some(f64::NAN)), l("3", "a3", Some(f64::INFINITY)), l("4", "a4", Some(f64::NEG_INFINITY))],
        )]);
        let r = layout(&sc, &durs(&sc, &[("a1", 1.0), ("a2", 1.0), ("a3", 1.0), ("a4", 1.0)]), 0.3, 1.0);
        assert!(r.items.iter().all(|x| x.start.is_finite() && x.dur.is_finite()));
        assert!(r.total.is_finite());
        assert_eq!(starts_of(&r), vec![0.0, 1.3, 2.6, 3.9]);
        assert_eq!(r.starts.len(), 3);
        // 間隔が有限でなくても有限の値で並ぶ
        let r = layout(&sc, &Analyses::default(), f64::NAN, f64::INFINITY);
        assert!(r.items.iter().all(|x| x.start.is_finite()));
    }

    #[test]
    fn audio_only_when_fresh_and_key_set() {
        let sc = s(vec![b("A", vec![l("1", "こんにちは", None)])]);
        let line = &sc.blocks[0].lines[0];
        let mut a = Analyses::default();
        a.insert(vkey(&line.sp, &line.text), an("wav1", 1.0), "古い".into());
        let r = layout(&sc, &a, 0.3, 1.0);
        assert_eq!(r.items[0].audio, None);
        assert_eq!(r.items[0].dur, 1.0);
        assert_eq!(r.items[0].key, vkey("トーク1", "こんにちは"));
        let g = crate::voice::analysis::sig(&sc, line, a.dict());
        a.insert(vkey(&line.sp, &line.text), an("wav1", 1.0), g);
        assert_eq!(layout(&sc, &a, 0.3, 1.0).items[0].audio.as_deref(), Some("wav1"));
    }

    // ------------------------------------------------------------ 実際の回（読むだけ。無ければ飛ばす）

    /// script/*.md を移行用の読み込み（parse_md）で読む（_order.json の順）。ファイルは書き換えない
    fn read_episode(root: &std::path::Path) -> Script {
        use crate::script::format::parse_md;
        let dir = root.join("script");
        let mut files: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().to_string())
            .filter(|f| f.ends_with(".md") && !f.starts_with('_'))
            .collect();
        let order: Vec<String> = std::fs::read_to_string(dir.join("_order.json"))
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default();
        let pos = |f: &String| order.iter().position(|o| o == f).unwrap_or(usize::MAX);
        files.sort_by(|a, b| pos(a).cmp(&pos(b)).then(a.cmp(b)));
        let mut blocks = vec![];
        for (bi, f) in files.iter().enumerate() {
            let text = std::fs::read_to_string(dir.join(f)).unwrap();
            let p = parse_md(text.trim_start_matches('\u{feff}'));
            let mut lines = p.lines;
            for (i, ln) in lines.iter_mut().enumerate() {
                ln.id = format!("b{bi}l{i}");
            }
            if !lines.is_empty() {
                blocks.push(b(p.title.as_deref().unwrap_or(f), lines));
            }
        }
        s(blocks)
    }

    /// files/script.srt の cue（開始・終わり）
    fn read_srt(root: &std::path::Path) -> Vec<(f64, f64)> {
        let t = std::fs::read_to_string(root.join("files").join("script.srt")).unwrap().replace('\r', "");
        let sec = |x: &str| {
            let (hms, ms) = x.trim().split_once(',').unwrap();
            let p: Vec<f64> = hms.split(':').map(|v| v.parse().unwrap()).collect();
            p[0] * 3600.0 + p[1] * 60.0 + p[2] + ms.parse::<f64>().unwrap() / 1000.0
        };
        t.lines().filter_map(|ln| ln.split_once(" --> ")).map(|(a, e)| (sec(a), sec(e))).collect()
    }

    #[test]
    fn real_episode_written_starts_do_not_move() {
        // 実際の回を読むテスト。AK_TEST_EPISODE（回のフォルダ）が無ければ飛ばす（既定の回は持たない）
        let Some(root) = std::env::var_os("AK_TEST_EPISODE").filter(|v| !v.is_empty()).map(std::path::PathBuf::from) else {
            eprintln!("AK_TEST_EPISODE が無いので飛ばす");
            return;
        };
        let has_md = std::fs::read_dir(root.join("script"))
            .map(|d| d.filter_map(|e| e.ok()).any(|e| {
                let n = e.file_name().to_string_lossy().to_string();
                n.ends_with(".md") && !n.starts_with('_')
            }))
            .unwrap_or(false);
        if !has_md || !root.join("files").join("script.srt").exists() {
            eprintln!("回のフォルダが無い（または .md が無い）ので飛ばす: {}", root.display());
            return;
        }
        let sc = read_episode(&root);
        let cues = read_srt(&root);
        let all = voiced_lines(&sc);
        let placed: Vec<&Placed> = all.iter().filter(|x| x.l.start.is_some()).collect();
        assert!(placed.len() > 10, "開始の書いてある行が少なすぎる");
        // 長さは script.srt の cue（書き出したときの音声の長さ）から。開始の順に並べて対応づける
        let mut sorted = placed.clone();
        sorted.sort_by(|p, q| p.l.start.unwrap().total_cmp(&q.l.start.unwrap()));
        let mut ans = Analyses::default();
        for (i, x) in sorted.iter().enumerate() {
            if let Some((st, en)) = cues.get(i) {
                ans.insert(vkey(&x.l.sp, &x.l.text), an(&i.to_string(), en - st), String::new());
            }
        }
        for (label, a) in [("音声の長さあり", &ans), ("長さは推定", &Analyses::default())] {
            for (gap, bg) in [(0.3, 1.0), (0.3, 0.3), (0.5, 2.0)] {
                let r = layout(&sc, a, gap, bg);
                let moved = r.starts.keys().filter(|id| placed.iter().any(|x| &x.l.id == *id)).count();
                assert_eq!(moved, 0, "{label} gap={gap} block_gap={bg}: {moved} 行が動いた");
            }
        }
        // 見積もった間隔は台本エディタで使っていた 0.3 / 0.3
        let g = infer_gaps(&sc, &ans);
        assert_eq!(g.gap, Some(0.3));
        assert_eq!(g.block_gap, Some(0.3));
    }
}
