//! 台本の cue 番号がずれたときの、演出クリップの鍵（`部品名@c<cue>#<順番>`）の付け替え（旧#38）。
//!
//! - `cue_map`: 前の script.srt のセリフと今のセリフ（`[話者] 文`）を突き合わせて、古い cue → 新しい cue の表を作る
//! - `plan` / `apply`: 鍵の表（studio_overrides.json・project.json の nests）の鍵を、その表で書き換える
//!
//! どちらもファイルに触らない純粋な関数（書き込みは export_talk）。

use std::collections::{BTreeMap, HashMap};

/// 古い cue → 新しい cue（どちらも 1 始まり）。消した行の cue は入らない。
pub type CueMap = BTreeMap<usize, usize>;

/// script.srt の各 cue のセリフ（`[話者] 文`。2 行以上なら空白でつなぐ）。AK Studio が書き出した script.srt の読み方（書くのは export/talk.rs。演出の fxkit は script_cues.json を読む）。
/// 付け替えに使うので、読めないところがあれば None（cue 番号が 1 から順でない・番号の行が無い・3 行に満たない cue がある）。
/// 空のファイルは cue 0 個
pub fn srt_lines(srt: &str) -> Option<Vec<String>> {
    let s = srt.trim_start_matches('\u{feff}').replace("\r\n", "\n").replace('\r', "\n");
    let s = s.trim();
    if s.is_empty() {
        return Some(vec![]);
    }
    let mut out = vec![];
    for b in s.split("\n\n") {
        let l: Vec<&str> = b.trim().split('\n').collect();
        if l.len() < 3 || l[0].trim().parse::<usize>().ok()? != out.len() + 1 {
            return None;
        }
        out.push(l[2..].join(" "));
    }
    Some(out)
}

/// 前のセリフ `old` と今のセリフ `new` を突き合わせる。
///
/// 1. 同じ文どうしを、順番を保って最長共通部分列で対応させる
/// 2. 残りのうち、前にも今にも 1 回ずつしか無い文は、場所が離れていても同じ行とみなす（並べ替え）
/// 3. 対応した行にはさまれた区間で、前と今の残りの行数が同じなら、順に同じ行とみなす（文を直しただけ。話者も文も違う組は除く）
/// 4. 行数が違う区間では、同じ話者で文が似ている行（文字の 2 つ組の一致が半分以上）を順番を保って対応させる
pub fn cue_map(old: &[String], new: &[String]) -> CueMap {
    let (n, m) = (old.len(), new.len());
    let mut o2n: Vec<Option<usize>> = vec![None; n];
    let mut taken = vec![false; m];
    // 1. 最長共通部分列
    let mut dp = vec![vec![0u32; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            dp[i][j] = if old[i] == new[j] { dp[i + 1][j + 1] + 1 } else { dp[i + 1][j].max(dp[i][j + 1]) };
        }
    }
    let mut anchors = vec![];
    let (mut i, mut j) = (0, 0);
    while i < n && j < m {
        if old[i] == new[j] {
            anchors.push((i, j));
            i += 1;
            j += 1;
        } else if dp[i + 1][j] >= dp[i][j + 1] {
            i += 1;
        } else {
            j += 1;
        }
    }
    for &(i, j) in &anchors {
        o2n[i] = Some(j);
        taken[j] = true;
    }
    // 2. 動かした行（残りの中で前にも今にも 1 回だけある文）
    let mut old_count: HashMap<&str, usize> = HashMap::new();
    let mut new_at: HashMap<&str, (usize, usize)> = HashMap::new();
    for (i, s) in old.iter().enumerate() {
        if o2n[i].is_none() {
            *old_count.entry(s).or_default() += 1;
        }
    }
    for (j, s) in new.iter().enumerate() {
        if !taken[j] {
            let e = new_at.entry(s).or_insert((j, 0));
            e.1 += 1;
        }
    }
    for i in 0..n {
        if o2n[i].is_some() || old_count.get(old[i].as_str()) != Some(&1) {
            continue;
        }
        if let Some(&(j, 1)) = new_at.get(old[i].as_str()) {
            o2n[i] = Some(j);
            taken[j] = true;
        }
    }
    // 3・4. 最長共通部分列の対応ではさまれた区間ごと
    let mut bounds = vec![(None, None)];
    bounds.extend(anchors.iter().map(|&(i, j)| (Some(i), Some(j))));
    bounds.push((Some(n), Some(m)));
    for w in bounds.windows(2) {
        let (a, b) = (w[0], w[1]);
        let oi: Vec<usize> = (a.0.map_or(0, |x| x + 1)..b.0.unwrap_or(n)).filter(|&i| o2n[i].is_none()).collect();
        let nj: Vec<usize> = (a.1.map_or(0, |x| x + 1)..b.1.unwrap_or(m)).filter(|&j| !taken[j]).collect();
        if oi.is_empty() || nj.is_empty() {
            continue;
        }
        let pairs = if oi.len() == nj.len() {
            // 話者も文も違う行は、消して別の行を足したものとみなす（録画などが無関係の行へ移らないように）
            oi.iter()
                .copied()
                .zip(nj.iter().copied())
                .filter(|&(i, j)| speaker(&old[i]) == speaker(&new[j]) || similarity(body(&old[i]), body(&new[j])) >= 0.5)
                .collect()
        } else {
            similar_pairs(old, new, &oi, &nj)
        };
        for (i, j) in pairs {
            o2n[i] = Some(j);
            taken[j] = true;
        }
    }
    o2n.iter().enumerate().filter_map(|(i, j)| j.map(|j| (i + 1, j + 1))).collect()
}

/// 行数の違う区間で、似ている行どうしを順番を保って対応させる（対応の数が最大になるように）
fn similar_pairs(old: &[String], new: &[String], oi: &[usize], nj: &[usize]) -> Vec<(usize, usize)> {
    let ok = |a: usize, b: usize| speaker(&old[oi[a]]) == speaker(&new[nj[b]]) && similarity(body(&old[oi[a]]), body(&new[nj[b]])) >= 0.5;
    let (p, q) = (oi.len(), nj.len());
    let mut dp = vec![vec![0u32; q + 1]; p + 1];
    for a in (0..p).rev() {
        for b in (0..q).rev() {
            let take = if ok(a, b) { dp[a + 1][b + 1] + 1 } else { 0 };
            dp[a][b] = take.max(dp[a + 1][b]).max(dp[a][b + 1]);
        }
    }
    let mut out = vec![];
    let (mut a, mut b) = (0, 0);
    while a < p && b < q {
        if ok(a, b) && dp[a][b] == dp[a + 1][b + 1] + 1 {
            out.push((oi[a], nj[b]));
            a += 1;
            b += 1;
        } else if dp[a + 1][b] >= dp[a][b + 1] {
            a += 1;
        } else {
            b += 1;
        }
    }
    out
}

/// `[話者] 文` の話者
fn speaker(s: &str) -> &str {
    s.strip_prefix('[').and_then(|r| r.split_once(']')).map(|(sp, _)| sp).unwrap_or("")
}

/// `[話者] 文` の文（似かたは話者の札を除いて比べる。札まで比べると短いセリフがどれも似ていることになる）
fn body(s: &str) -> &str {
    s.strip_prefix('[').and_then(|r| r.split_once(']')).map(|(_, b)| b.trim_start()).unwrap_or(s)
}

/// 文の似かた（文字の 2 つ組の Dice 係数。0〜1）
fn similarity(a: &str, b: &str) -> f64 {
    let grams = |s: &str| {
        let c: Vec<char> = s.chars().collect();
        let mut g: HashMap<(char, char), usize> = HashMap::new();
        for w in c.windows(2) {
            *g.entry((w[0], w[1])).or_default() += 1;
        }
        g
    };
    let (ga, gb) = (grams(a), grams(b));
    let total: usize = ga.values().sum::<usize>() + gb.values().sum::<usize>();
    if total == 0 {
        return if a == b { 1.0 } else { 0.0 };
    }
    let common: usize = ga.iter().map(|(k, v)| (*v).min(gb.get(k).copied().unwrap_or(0))).sum();
    2.0 * common as f64 / total as f64
}

/// 演出クリップの鍵 `部品名@c<cue>#<順番>` を分ける。`gui:` の鍵や形の違う鍵は None
pub fn parse_key(k: &str) -> Option<(&str, usize, &str)> {
    let (label, rest) = k.rsplit_once("@c")?;
    let (cue, ord) = rest.split_once('#')?;
    if label.is_empty() || cue.is_empty() || !cue.bytes().all(|b| b.is_ascii_digit()) || ord.is_empty() {
        return None;
    }
    Some((label, cue.parse().ok()?, ord))
}

/// 鍵の付け替えの予定
#[derive(Debug, Default, PartialEq)]
pub struct Plan {
    /// 古い鍵 → 新しい鍵（番号が変わるものだけ）
    pub renamed: Vec<(String, String)>,
    /// 消した行に付いていたので、付け替えずに残す鍵
    pub kept: Vec<String>,
    /// 付け替えた鍵と同じになるので、表から外す鍵（消した行に付いていたもの・前から古かったもの）
    pub dropped: Vec<String>,
}

impl Plan {
    pub fn is_empty(&self) -> bool {
        self.renamed.is_empty() && self.dropped.is_empty()
    }
}

/// 鍵の表をどう書き換えるかを決める。`old_cues` は前の script.srt の cue の数。
/// 前の script.srt に無い番号の鍵（前から古い鍵）と、形の違う鍵（`gui:` など）はそのまま
pub fn plan<'a>(keys: impl IntoIterator<Item = &'a String>, map: &CueMap, old_cues: usize) -> Plan {
    let mut p = Plan::default();
    let mut stay = vec![];
    for k in keys {
        match parse_key(k) {
            Some((label, cue, ord)) if (1..=old_cues).contains(&cue) => match map.get(&cue) {
                Some(&nc) if nc != cue => p.renamed.push((k.clone(), format!("{label}@c{nc}#{ord}"))),
                Some(_) => stay.push(k.clone()),
                None => p.kept.push(k.clone()),
            },
            _ => stay.push(k.clone()),
        }
    }
    // 番号が変わった鍵が、付け替えない鍵と同じ名前になるときは、付け替えたほうを残す
    let targets: std::collections::HashSet<&String> = p.renamed.iter().map(|(_, n)| n).collect();
    let renamed_from: std::collections::HashSet<&String> = p.renamed.iter().map(|(o, _)| o).collect();
    let mut dropped: Vec<String> = stay.iter().chain(p.kept.iter()).filter(|k| targets.contains(k) && !renamed_from.contains(k)).cloned().collect();
    dropped.sort();
    p.kept.retain(|k| !dropped.contains(k));
    p.dropped = dropped;
    p
}

/// 予定どおりに鍵の表を書き換える（付け替えは一度にまとめて行うので、15→16 と 16→17 が重なっても混ざらない）
pub fn apply<V>(m: &mut BTreeMap<String, V>, p: &Plan) {
    for k in &p.dropped {
        m.remove(k);
    }
    let moved: Vec<(String, V)> = p.renamed.iter().filter_map(|(o, n)| m.remove(o).map(|v| (n.clone(), v))).collect();
    m.extend(moved);
}

/// serde_json の表の版
pub fn apply_json(m: &mut serde_json::Map<String, serde_json::Value>, p: &Plan) {
    for k in &p.dropped {
        m.remove(k);
    }
    let moved: Vec<(String, serde_json::Value)> =
        p.renamed.iter().filter_map(|(o, n)| m.remove(o).map(|v| (n.clone(), v))).collect();
    m.extend(moved);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(xs: &[&str]) -> Vec<String> {
        xs.iter().map(|s| s.to_string()).collect()
    }

    fn pairs(m: &CueMap) -> Vec<(usize, usize)> {
        m.iter().map(|(a, b)| (*a, *b)).collect()
    }

    #[test]
    fn same_lines_map_to_themselves() {
        let a = v(&["[A] 一", "[B] 二", "[A] 三"]);
        assert_eq!(pairs(&cue_map(&a, &a)), vec![(1, 1), (2, 2), (3, 3)]);
    }

    #[test]
    fn inserted_line_shifts_later_cues() {
        let old = v(&["[A] 一", "[B] 二", "[A] 三", "[B] 四"]);
        let new = v(&["[A] 一", "[B] 二", "[A] 足した行", "[A] 三", "[B] 四"]);
        assert_eq!(pairs(&cue_map(&old, &new)), vec![(1, 1), (2, 2), (3, 4), (4, 5)]);
    }

    #[test]
    fn deleted_line_is_unmapped() {
        let old = v(&["[A] 一", "[B] 二", "[A] 三", "[B] 四"]);
        let new = v(&["[A] 一", "[A] 三", "[B] 四"]);
        assert_eq!(pairs(&cue_map(&old, &new)), vec![(1, 1), (3, 2), (4, 3)]);
    }

    #[test]
    fn edited_line_keeps_its_place() {
        let old = v(&["[A] 一", "[B] 二です", "[A] 三"]);
        let new = v(&["[A] 一", "[B] まったく別の文", "[A] 三"]);
        assert_eq!(pairs(&cue_map(&old, &new)), vec![(1, 1), (2, 2), (3, 3)]);
        // 先頭・末尾の行を直しても同じ
        let new2 = v(&["[A] いち", "[B] 二です", "[A] さん"]);
        assert_eq!(pairs(&cue_map(&old, &new2)), vec![(1, 1), (2, 2), (3, 3)]);
    }

    #[test]
    fn edited_and_inserted_in_one_gap_uses_similarity() {
        let old = v(&["[A] 一", "[B] 燃費がとても良い車です", "[A] 三"]);
        let new = v(&["[A] 一", "[B] 燃費がかなり良い車です", "[A] 足した行", "[A] 三"]);
        assert_eq!(pairs(&cue_map(&old, &new)), vec![(1, 1), (2, 2), (3, 4)]);
        // 足した行が前に来ても
        let new2 = v(&["[A] 一", "[A] 足した行", "[B] 燃費がかなり良い車です", "[A] 三"]);
        assert_eq!(pairs(&cue_map(&old, &new2)), vec![(1, 1), (2, 3), (3, 4)]);
    }

    #[test]
    fn reordered_lines_follow_their_text() {
        let old = v(&["[A] 一", "[B] 二", "[A] 三", "[B] 四", "[A] 五"]);
        let new = v(&["[A] 一", "[A] 三", "[B] 四", "[B] 二", "[A] 五"]);
        assert_eq!(pairs(&cue_map(&old, &new)), vec![(1, 1), (2, 4), (3, 2), (4, 3), (5, 5)]);
    }

    #[test]
    fn repeated_text_is_not_matched_out_of_order() {
        // 同じ文が 2 回あると、どれを動かしたか分からないので並べ替えとしては対応させない
        let old = v(&["[A] はい", "[B] 二", "[A] はい"]);
        let new = v(&["[B] 二", "[A] はい", "[A] はい", "[A] はい"]);
        let m = cue_map(&old, &new);
        assert_eq!(m.get(&2), Some(&1));
        assert_eq!(m.get(&3), Some(&2));
    }

    #[test]
    fn short_different_lines_are_not_similar() {
        let old = v(&["[トーク1] 一", "[トーク1] そうですね", "[トーク1] 三"]);
        let new = v(&["[トーク1] 一", "[トーク1] なるほど", "[トーク1] 確かに", "[トーク1] 三"]);
        assert_eq!(pairs(&cue_map(&old, &new)), vec![(1, 1), (3, 4)]);
    }

    #[test]
    fn replaced_line_with_other_speaker_is_not_paired() {
        let old = v(&["[A] 一", "[B] 二", "[A] 三"]);
        let new = v(&["[A] 一", "[A] 足した行", "[A] 三"]);
        assert_eq!(pairs(&cue_map(&old, &new)), vec![(1, 1), (3, 3)]);
    }

    #[test]
    fn srt_lines_reads_exported_srt() {
        let s = "\u{feff}1\n00:00:00,000 --> 00:00:01,000\n[A] 一\n\n2\r\n00:00:01,000 --> 00:00:02,000\r\n[B] 二\r\nつづき\r\n\r\n";
        assert_eq!(srt_lines(s), Some(v(&["[A] 一", "[B] 二 つづき"])));
        // 番号が飛んでいる・番号の行が無いときは対応させない
        assert_eq!(srt_lines("1\n0 --> 1\n[A] 一\n\n3\n1 --> 2\n[B] 二\n"), None);
        assert_eq!(srt_lines("00:00 --> 00:01\n[A] 一\nつづき\n"), None);
        assert_eq!(srt_lines(""), Some(vec![]));
        // 改行が \r だけ（古い Mac）でも読める。3 行に満たない cue があれば読まない
        assert_eq!(srt_lines("1\r0 --> 1\r[A] 一\r\r2\r1 --> 2\r[B] 二\r"), Some(v(&["[A] 一", "[B] 二"])));
        assert_eq!(srt_lines("1\n0 --> 1\n[A] 一\n\n2\n1 --> 2\n"), None);
    }

    /// 見本の script_cues.srt（export/talk.rs のテストが同じ内容を縛る）を読むと、見本の表の `[speaker] text` と同じ並びになる
    #[test]
    fn srt_lines_matches_cue_table_fixture() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
        let srt = std::fs::read_to_string(dir.join("script_cues.srt")).unwrap();
        let table: ak_proto::CueTable = serde_json::from_str(&std::fs::read_to_string(dir.join("script_cues.json")).unwrap()).unwrap();
        let want: Vec<String> = table.cues.iter().map(|c| format!("[{}] {}", c.speaker, c.text)).collect();
        assert_eq!(srt_lines(&srt), Some(want));
    }

    #[test]
    fn parse_key_accepts_only_cue_keys() {
        assert_eq!(parse_key("telop@c15#1"), Some(("telop", 15, "1")));
        assert_eq!(parse_key("a@b@c3#2"), Some(("a@b", 3, "2")));
        assert_eq!(parse_key("gui:abc"), None);
        assert_eq!(parse_key("telop@cx#1"), None);
        assert_eq!(parse_key("telop@c15"), None);
    }

    fn ks(xs: &[&str]) -> Vec<String> {
        v(xs)
    }

    #[test]
    fn plan_renames_after_insert() {
        let map: CueMap = [(1, 1), (2, 2), (3, 4), (4, 5)].into_iter().collect();
        let keys = ks(&["telop@c2#1", "telop@c3#1", "RecWindow@c4#1", "gui:x", "telop@c99#1"]);
        let p = plan(&keys, &map, 4);
        assert_eq!(p.renamed, vec![("telop@c3#1".into(), "telop@c4#1".into()), ("RecWindow@c4#1".into(), "RecWindow@c5#1".into())]);
        assert!(p.kept.is_empty() && p.dropped.is_empty());
        let mut m: BTreeMap<String, i32> = keys.iter().cloned().zip(1..).collect();
        apply(&mut m, &p);
        let want: BTreeMap<String, i32> =
            [("telop@c2#1", 1), ("telop@c4#1", 2), ("RecWindow@c5#1", 3), ("gui:x", 4), ("telop@c99#1", 5)]
                .into_iter()
                .map(|(k, v)| (k.to_string(), v))
                .collect();
        assert_eq!(m, want);
    }

    #[test]
    fn plan_after_delete_drops_colliding_key_of_deleted_line() {
        // cue 2 を消した: 3→2, 4→3。cue 2 の telop は付け替えられず、3 の telop が c2 に来るので外す
        let map: CueMap = [(1, 1), (3, 2), (4, 3)].into_iter().collect();
        let keys = ks(&["telop@c2#1", "stamp@c2#1", "telop@c3#1", "telop@c4#2"]);
        let p = plan(&keys, &map, 4);
        assert_eq!(p.renamed, vec![("telop@c3#1".into(), "telop@c2#1".into()), ("telop@c4#2".into(), "telop@c3#2".into())]);
        assert_eq!(p.dropped, vec!["telop@c2#1".to_string()]);
        assert_eq!(p.kept, vec!["stamp@c2#1".to_string()]);
        let mut m: BTreeMap<String, &str> = [("telop@c2#1", "消した行"), ("stamp@c2#1", "消した行のハンコ"), ("telop@c3#1", "三"), ("telop@c4#2", "四")]
            .into_iter()
            .map(|(k, v)| (k.to_string(), v))
            .collect();
        apply(&mut m, &p);
        assert_eq!(m.get("telop@c2#1"), Some(&"三"));
        assert_eq!(m.get("telop@c3#2"), Some(&"四"));
        assert_eq!(m.get("stamp@c2#1"), Some(&"消した行のハンコ"));
        assert_eq!(m.len(), 3);
    }

    #[test]
    fn plan_after_reorder_swaps_without_mixing() {
        let map: CueMap = [(1, 2), (2, 1)].into_iter().collect();
        let keys = ks(&["telop@c1#1", "telop@c2#1"]);
        let p = plan(&keys, &map, 2);
        assert!(p.dropped.is_empty());
        let mut m = serde_json::Map::new();
        m.insert("telop@c1#1".into(), serde_json::json!({"dx": 1}));
        m.insert("telop@c2#1".into(), serde_json::json!({"dx": 2}));
        apply_json(&mut m, &p);
        assert_eq!(m["telop@c2#1"]["dx"], 1);
        assert_eq!(m["telop@c1#1"]["dx"], 2);
    }

    #[test]
    fn plan_is_empty_when_numbers_stay() {
        let map: CueMap = [(1, 1), (2, 2)].into_iter().collect();
        let keys = ks(&["telop@c1#1", "telop@c2#1"]);
        assert!(plan(&keys, &map, 2).is_empty());
    }
}
