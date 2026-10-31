//! 台本の形（Line・Block・Script）と、md の読み込み・YAML の書き出し・読み込み。

use anyhow::Context;
use anyhow::{anyhow, Result};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashSet;
use ts_rs::TS;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, TS)]
#[ts(export)]
pub struct Line {
    pub id: String,
    pub sp: String,
    pub text: String,
    pub start: Option<f64>,
    /// Claude 向けのメモ。読み上げ・字幕には使わない
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub memo: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, TS)]
#[ts(export)]
pub struct Block {
    pub id: String,
    /// ファイル名（新しいブロックは None。保存時に見出しから決める）
    pub file: Option<String>,
    pub title: String,
    pub lines: Vec<Line>,
    pub notes: String,
}

/// `_voice.json` の 1 行分（アクセントの手直し）。TypeScript の型を作るためだけの形（Script では Value で持つ）
#[derive(Serialize, Deserialize, Clone, Debug, TS)]
#[ts(export)]
pub struct VoiceAdj {
    pub sp: String,
    /// 作ったときの読み上げの文（tts_key_text(text)）
    pub text: String,
    /// VoiSona の解析結果の XML
    pub analyzed_text: String,
}

/// `_voice_params.json` の 1 行分（抑揚・音素の長さなど）。TypeScript の型を作るためだけの形
#[derive(Serialize, Deserialize, Clone, Debug, TS)]
#[ts(export)]
pub struct ParamsAdj {
    pub sp: String,
    pub text: String,
    #[ts(type = "Record<string, any>")]
    pub params: Value,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, TS)]
#[ts(export)]
pub struct Script {
    pub blocks: Vec<Block>,
    pub memo: String,
    /// `_voice.json`（キー: 行の id。値: {sp, text, analyzed_text}。sp と text が今の行と同じときだけ効く）
    #[ts(as = "std::collections::HashMap<String, VoiceAdj>")]
    pub voice: serde_json::Map<String, Value>,
    /// `_voice_params.json`（キー: 行の id。値: {sp, text, params}）
    #[ts(as = "std::collections::HashMap<String, ParamsAdj>")]
    pub params: serde_json::Map<String, Value>,
    /// `_voice_speakers.json`（キー: 話者。値: VoiSona の global_parameters のうち、共通のプリセットから変えた項目）。
    /// 保存のときに無ければ（None）、ディスクの中身をそのまま残す
    #[serde(default)]
    #[ts(type = "Record<string, import(\"../logic/speakervoice.ts\").SpeakerVoice>", optional = nullable)]
    pub speakers: Option<serde_json::Map<String, Value>>,
    /// 保存のたびに増える（画面と Claude の食い違いを見つけるため）
    #[ts(type = "number")]
    pub rev: u64,
}

impl Script {
    /// 台本の全部の行の id
    pub fn line_ids(&self) -> HashSet<String> {
        self.blocks.iter().flat_map(|b| b.lines.iter()).map(|l| l.id.clone()).collect()
    }

    /// 台本のどの行とも重ならない新しい id
    pub fn fresh_id(&self, prefix: &str) -> String {
        let used = self.line_ids();
        loop {
            let id = new_id(prefix);
            if !used.contains(&id) {
                return id;
            }
        }
    }

    /// 行の手直しのうち、今の話者・読み上げの文と合うものだけ（合わなければ使われない）
    fn adj<'a>(m: &'a serde_json::Map<String, Value>, l: &Line) -> Option<&'a Value> {
        let e = m.get(&l.id)?;
        (e["sp"].as_str() == Some(l.sp.as_str()) && e["text"].as_str() == Some(tts_key_text(&l.text).as_str())).then_some(e)
    }

    /// アクセントの手直し（VoiSona の解析結果の XML）
    pub fn voice_of(&self, l: &Line) -> Option<&str> {
        Self::adj(&self.voice, l).and_then(|e| e["analyzed_text"].as_str())
    }

    /// 抑揚・音素の長さなど
    pub fn params_of(&self, l: &Line) -> Option<&Value> {
        Self::adj(&self.params, l).map(|e| &e["params"]).filter(|p| !p.is_null())
    }
}

pub use crate::voice::key::{sub_text, tts_key_text, tts_text, vkey};

fn nfkc_lite(s: &str) -> String {
    // 全角英数を半角に（NFKC の必要な分だけ）。空白は消す
    s.chars()
        .filter(|c| !c.is_whitespace())
        .map(|c| match c {
            '！'..='～' => char::from_u32(c as u32 - 0xFEE0).unwrap_or(c),
            _ => c,
        })
        .collect()
}

pub fn norm_sp(s: &str) -> String {
    let n = nfkc_lite(s);
    let low = n.to_lowercase();
    if ["2", "トーク2", "talk2"].contains(&low.as_str()) {
        return "トーク2".into();
    }
    if ["1", "トーク1", "talk1"].contains(&low.as_str()) {
        return "トーク1".into();
    }
    n
}

static NEXT_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

fn base36(mut n: u64) -> String {
    if n == 0 {
        return "0".into();
    }
    let mut v = vec![];
    while n > 0 {
        v.push(b"0123456789abcdefghijklmnopqrstuvwxyz"[(n % 36) as usize]);
        n /= 36;
    }
    v.reverse();
    String::from_utf8(v).unwrap()
}

/// 回をまたいでも重ならない id（英小文字と数字。先頭は prefix）。ui/src/voice.ts の nid と同じ形
pub fn new_id(prefix: &str) -> String {
    let ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let n = NEXT_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    format!("{}{}{}", prefix, base36(ms), base36(n))
}

pub struct Parsed {
    pub title: Option<String>,
    pub lines: Vec<Line>,
    pub notes: String,
}

pub fn parse_md(text: &str) -> Parsed {
    let mut lines = vec![];
    let mut notes = vec![];
    let mut title: Option<String> = None;
    let mut cols: Option<(i64, i64, i64, i64)> = None; // セリフ・話者・開始・メモ
    let head = Regex::new(r"^#{1,6}\s").unwrap();
    for raw in text.replace('\r', "").split('\n') {
        let t = raw.trim();
        if title.is_none() && head.is_match(t) {
            title = Some(t.trim_start_matches('#').trim().to_string());
            continue;
        }
        if t.starts_with('|') {
            let inner = t.trim_start_matches('|');
            let inner = inner.strip_suffix('|').unwrap_or(inner);
            let cells: Vec<String> = inner.split('|').map(|c| c.trim().to_string()).collect();
            let joined: String = cells.concat();
            if cells.len() < 2 || joined.chars().all(|c| c == '-' || c == ':' || c.is_whitespace()) {
                continue;
            }
            let names: Vec<String> = cells.iter().map(|c| nfkc_lite(c).to_lowercase()).collect();
            if names.iter().any(|n| n == "話者" || n == "speaker") {
                let idx = |k: &str| names.iter().position(|n| n == k).map(|i| i as i64).unwrap_or(-1);
                let mut sp = idx("話者");
                if sp < 0 {
                    sp = idx("speaker");
                }
                let mut memo = idx("メモ");
                if memo < 0 {
                    memo = idx("memo");
                }
                cols = Some((idx("セリフ"), sp, idx("開始"), memo));
                continue;
            }
            let n = cells.len();
            let (mut sp, mut tx, mut start, mut memo) = (cells[n - 1].clone(), cells[n - 2].clone(), None, String::new());
            if let Some((ct, cs, cst, cm)) = cols {
                if cs >= 0 && ct >= 0 {
                    sp = cells.get(cs as usize).cloned().unwrap_or_default();
                    tx = cells.get(ct as usize).cloned().unwrap_or_default();
                    if cst >= 0 {
                        if let Some(v) = cells.get(cst as usize) {
                            if !v.is_empty() {
                                start = v.parse::<f64>().ok();
                            }
                        }
                    }
                    if cm >= 0 {
                        if let Some(v) = cells.get(cm as usize) {
                            memo = v.replace("<br>", "\n");
                        }
                    }
                }
            }
            lines.push(Line { id: new_id("l"), sp: norm_sp(&sp), text: tx, start, memo });
            continue;
        }
        if raw.contains('\t') {
            let mut it = raw.split('\t');
            let sp = it.next().unwrap_or("");
            let rest: Vec<&str> = it.collect();
            lines.push(Line { id: new_id("l"), sp: norm_sp(sp), text: rest.join(" ").trim().to_string(), start: None, memo: String::new() });
            continue;
        }
        notes.push(raw.to_string());
    }
    Parsed { title, lines, notes: notes.join("\n").trim().to_string() }
}

// ---------------------------------------------------------------------------------------------
// YAML の書き出し（docs/script-format.md §1）。自前で決まった形に書く

/// 引用符なしでは危ない（読み方が変わる・行が割れる）文字
fn is_odd(c: char) -> bool {
    c < ' ' || ('\u{7f}'..='\u{9f}').contains(&c) || matches!(c, '\u{2028}' | '\u{2029}' | '\u{feff}' | '\u{fffe}' | '\u{ffff}')
}

pub(super) fn is_id_shape(s: &str) -> bool {
    let mut it = s.chars();
    matches!(it.next(), Some('a'..='z')) && it.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
}

/// 引用符なしで書いてよい 1 行の文字列か（PyYAML・Rust のどちらで読んでも同じ文字列に戻るものだけ）
fn plain_ok(s: &str) -> bool {
    let Some(f) = s.chars().next() else { return false };
    if s.chars().any(is_odd) || s.trim() != s || s.contains(": ") || s.contains(" #") || s.ends_with(':') {
        return false;
    }
    if !f.is_ascii() {
        return true;
    }
    is_id_shape(s) && !["y", "n", "yes", "no", "on", "off", "true", "false", "null"].contains(&s)
}

/// 二重引用符（JSON のエスケープ。YAML が行の区切りや読めない文字として扱うものは \uXXXX に）
fn dq(s: &str) -> String {
    let j = serde_json::to_string(s).unwrap_or_else(|_| "\"\"".into());
    let mut o = String::with_capacity(j.len());
    for c in j.chars() {
        if is_odd(c) {
            o += &format!("\\u{:04X}", c as u32);
        } else {
            o.push(c);
        }
    }
    o
}

/// 改行を含む文字列を `|-` / `|` のブロックにできるときの（見出し行の記号, 本文）。できなければ None（二重引用符にする）
fn block_of(s: &str) -> Option<(&'static str, &str)> {
    let (body, mark) = match s.strip_suffix('\n') {
        Some(b) => (b, "|"),
        None => (s, "|-"),
    };
    if body.is_empty() || body.ends_with('\n') || body.starts_with('\n') {
        return None;
    }
    let ok = body.split('\n').all(|l| {
        !l.chars().any(is_odd) && !l.starts_with([' ', '\t']) && !l.ends_with([' ', '\t'])
    });
    ok.then_some((mark, body))
}

/// `<lead><key>: <値>` を 1 つ書く。indent は key の桁（ブロックの本文はその 2 つ右）
fn put(o: &mut String, lead: &str, indent: usize, key: &str, v: &str) {
    o.push_str(lead);
    o.push_str(key);
    o.push_str(": ");
    if v.contains('\n') {
        if let Some((mark, body)) = block_of(v) {
            o.push_str(mark);
            o.push('\n');
            for l in body.split('\n') {
                if !l.is_empty() {
                    o.push_str(&" ".repeat(indent + 2));
                    o.push_str(l);
                }
                o.push('\n');
            }
            return;
        }
        o.push_str(&dq(v));
    } else if plain_ok(v) {
        o.push_str(v);
    } else {
        o.push_str(&dq(v));
    }
    o.push('\n');
}

/// 開始秒（小数 3 桁まで・末尾の 0 は落とす。整数でも `3.0` と書いて数として読ませる）
fn fmt_start(v: f64) -> Option<String> {
    if !v.is_finite() {
        return None;
    }
    let mut s = format!("{:.3}", v);
    while s.ends_with('0') {
        s.pop();
    }
    if s.ends_with('.') {
        s.push('0');
    }
    Some(s)
}

pub fn to_yaml(b: &Block) -> String {
    let mut o = String::new();
    put(&mut o, "", 0, "title", &b.title);
    if !b.notes.is_empty() {
        put(&mut o, "", 0, "notes", &b.notes);
    }
    if b.lines.is_empty() {
        o.push_str("lines: []\n");
        return o;
    }
    o.push_str("lines:\n");
    for l in &b.lines {
        put(&mut o, "  - ", 4, "id", &l.id);
        put(&mut o, "    ", 4, "sp", &l.sp);
        if let Some(s) = l.start.and_then(fmt_start) {
            o += &format!("    start: {s}\n");
        }
        put(&mut o, "    ", 4, "text", &l.text);
        if !l.memo.is_empty() {
            put(&mut o, "    ", 4, "memo", &l.memo);
        }
    }
    o
}

// ---------------------------------------------------------------------------------------------
// YAML の読み込み（正しい YAML なら何でも読む。足りない項目は既定値）

pub(super) struct RawLine {
    pub(super) id: Option<String>,
    pub(super) sp: String,
    pub(super) text: String,
    pub(super) start: Option<f64>,
    pub(super) memo: String,
}

pub(super) struct RawBlock {
    pub(super) title: Option<String>,
    pub(super) notes: String,
    pub(super) lines: Vec<RawLine>,
}

fn yscalar(v: Option<&serde_yaml_ng::Value>) -> Option<String> {
    use serde_yaml_ng::Value as Y;
    match v? {
        Y::String(s) => Some(s.clone()),
        Y::Number(n) => Some(n.to_string()),
        Y::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

pub(super) fn parse_yaml(text: &str, file: &str) -> Result<RawBlock> {
    use serde_yaml_ng::Value as Y;
    let empty = RawBlock { title: None, notes: String::new(), lines: vec![] };
    if text.trim().is_empty() {
        return Ok(empty);
    }
    let v: Y = serde_yaml_ng::from_str(text)
        .with_context(|| format!("台本 {file} を読めません（YAML として正しくありません。書きかけかもしれません）"))?;
    let m = match &v {
        Y::Null => return Ok(empty),
        Y::Mapping(m) => m,
        _ => return Err(anyhow!("台本 {file} を読めません（先頭が「title: …」のような「キー: 値」の形ではありません）")),
    };
    let mut lines = vec![];
    match m.get("lines") {
        None | Some(Y::Null) => {}
        Some(Y::Sequence(seq)) => {
            for (i, it) in seq.iter().enumerate() {
                let Y::Mapping(lm) = it else {
                    return Err(anyhow!("台本 {file} を読めません（lines の {} つ目が「キー: 値」の形ではありません）", i + 1));
                };
                let start = match lm.get("start") {
                    Some(Y::Number(n)) => n.as_f64(),
                    Some(Y::String(s)) => s.trim().parse::<f64>().ok(),
                    _ => None,
                };
                lines.push(RawLine {
                    id: yscalar(lm.get("id")).filter(|s| !s.is_empty()),
                    sp: norm_sp(&yscalar(lm.get("sp")).unwrap_or_default()),
                    text: yscalar(lm.get("text")).unwrap_or_default(),
                    start,
                    memo: yscalar(lm.get("memo")).unwrap_or_default(),
                });
            }
        }
        Some(_) => return Err(anyhow!("台本 {file} を読めません（lines が一覧の形ではありません）")),
    }
    Ok(RawBlock { title: yscalar(m.get("title")), notes: yscalar(m.get("notes")).unwrap_or_default(), lines })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn line(id: &str, sp: &str, text: &str) -> Line {
        Line { id: id.into(), sp: sp.into(), text: text.into(), start: None, memo: String::new() }
    }

    fn block(title: &str, lines: Vec<Line>, notes: &str) -> Block {
        Block { id: "b".into(), file: None, title: title.into(), lines, notes: notes.into() }
    }

    // ---- md（移行用の読み込み）

    #[test]
    fn parse_md_basic() {
        let md = "## 序論\n\n| # | セリフ | 話者 | 開始 |\n|---|---|---|---|\n| 1 | どうも｜A《えー》 | トーク1 | 0.000 |\n| 2 | はい | トーク2 |  |\n\nメモ\n";
        let p = parse_md(md);
        assert_eq!(p.title.as_deref(), Some("序論"));
        assert_eq!(p.lines.len(), 2);
        assert_eq!(p.lines[1].sp, "トーク2");
        assert_eq!(p.lines[0].start, Some(0.0));
        assert_eq!(p.notes, "メモ");
    }

    #[test]
    fn parse_md_memo() {
        let md = "## 序論\n\n| # | セリフ | 話者 | 開始 | メモ |\n|---|---|---|---|---|\n| 1 | 交換対象です | トーク1 | 0.000 | ここで出す:A、B<br>2行目 |\n| 2 | はい | トーク2 |  |  |\n| 3 | う | トーク1 |  |\n";
        let p = parse_md(md);
        assert_eq!(p.lines.len(), 3);
        assert_eq!(p.lines[0].memo, "ここで出す:A、B\n2行目");
        assert_eq!(p.lines[1].memo, "");
        assert_eq!(p.lines[2].sp, "トーク1");
    }

    // ---- YAML の書き出し・読み込み

    #[test]
    fn emit_layout() {
        let mut l1 = line("l51x9k", "トーク2", "山田、W。どれも限定ですね");
        l1.start = Some(209.45);
        l1.memo = "ここで立ち絵を並べる\n名前は｜で区切る（a|b）".into();
        let mut l2 = line("l52x9k", "トーク1", "yes");
        l2.start = Some(3.0);
        let b = block("200枠と75枠", vec![l1, l2], "画面に出すもの：#1〜2\n#5〜8");
        let want = "title: \"200枠と75枠\"\nnotes: |-\n  画面に出すもの：#1〜2\n  #5〜8\nlines:\n  - id: l51x9k\n    sp: トーク2\n    start: 209.45\n    text: 山田、W。どれも限定ですね\n    memo: |-\n      ここで立ち絵を並べる\n      名前は｜で区切る（a|b）\n  - id: l52x9k\n    sp: トーク1\n    start: 3.0\n    text: \"yes\"\n";
        assert_eq!(to_yaml(&b), want);
        // 空の台本・notes 無し
        assert_eq!(to_yaml(&block("題", vec![], "")), "title: 題
lines: []
");
    }

    #[test]
    fn start_format() {
        assert_eq!(fmt_start(3.0).unwrap(), "3.0");
        assert_eq!(fmt_start(209.45).unwrap(), "209.45");
        assert_eq!(fmt_start(0.1234).unwrap(), "0.123");
        assert_eq!(fmt_start(12.5).unwrap(), "12.5");
        assert_eq!(fmt_start(f64::NAN), None);
    }

    fn tricky() -> Vec<&'static str> {
        vec![
            "yes", "no", "on", "off", "true", "null", "~", "1:20", "0x10", "1e3", "-", "- a", "a: b", "a #b", "#x", "\"q\"", "'s'",
            " lead", "trail ", "  both  ", "tab\there", "\u{1}", "a\u{7f}b", "", "｜時雨《しぐれ》", "時雨: a", "あ #b", "あ:", "あ:い",
            "line1\nline2", "line1\nline2\n", "a  \nb", "  a\nb", "a\n\nb", "a\n\n", "\n", "\nabc", "a\r\nb", "a\u{85}b", "a\u{2028}b",
            "=", "<<", "?", "!x", "&a", "*a", "@a", "`a", "%a", "[a]", "{a}", "a,b", "ab", "l51x9k", "n", "y", "1", "1.5", "2024-01-02", "０１", "|x", ">x",
            "memo with | and ｜ here", "x\n| a\n｜ b", "emoji 😀 text", "a\u{3000}", "\u{3000}a", "---", "...", "a\nb\n  c",
        ]
    }

    #[test]
    fn roundtrip_tricky() {
        for t in tricky() {
            let mut l = line("l1", "トーク1", t);
            l.memo = t.to_string();
            l.start = Some(1.5);
            let b = block(t, vec![l.clone(), line("l2", t, "x")], t);
            let out = to_yaml(&b);
            let r = parse_yaml(&out, "x.yaml").unwrap_or_else(|e| panic!("{t:?} が読めない: {e}\n{out}"));
            assert_eq!(r.title.as_deref(), Some(t), "title {t:?}\n{out}");
            assert_eq!(r.notes, t, "notes {t:?}\n{out}");
            assert_eq!(r.lines.len(), 2, "{t:?}");
            assert_eq!(r.lines[0].text, t, "text {t:?}\n{out}");
            assert_eq!(r.lines[0].memo, t, "memo {t:?}\n{out}");
            assert_eq!(r.lines[0].start, Some(1.5));
            // 話者は norm_sp を通る（トーク1 / トーク2 以外は空白を消すだけ）
            assert_eq!(r.lines[1].sp, norm_sp(t), "sp {t:?}\n{out}");
        }
    }

    /// AK_PYYAML_DIR を指定すると、tricky の文字列を書いた tricky.yaml と期待値 tricky.json を出す（PyYAML で読み比べる用）
    #[test]
    fn dump_for_pyyaml() {
        let Ok(dir) = std::env::var("AK_PYYAML_DIR") else { return };
        let ts = tricky();
        let lines: Vec<Line> = ts
            .iter()
            .enumerate()
            .map(|(i, t)| {
                let mut l = line(&format!("l{i}"), "トーク1", t);
                l.memo = t.to_string();
                l
            })
            .collect();
        let b = block("題", lines, "");
        std::fs::write(Path::new(&dir).join("tricky.yaml"), to_yaml(&b)).unwrap();
        std::fs::write(Path::new(&dir).join("tricky.json"), serde_json::to_string(&ts).unwrap()).unwrap();
    }

    #[test]
    fn quoting_rules() {
        let q = |s: &str| {
            let mut o = String::new();
            put(&mut o, "", 0, "k", s);
            o
        };
        assert_eq!(q("l51x9k"), "k: l51x9k\n");
        assert_eq!(q("日本語"), "k: 日本語\n");
        assert_eq!(q("yes"), "k: \"yes\"\n");
        assert_eq!(q("W は強い"), "k: \"W は強い\"\n");
        assert_eq!(q(""), "k: \"\"\n");
        assert_eq!(q("あ: い"), "k: \"あ: い\"\n");
        assert_eq!(q("a\nb\n"), "k: |\n  a\n  b\n");
        assert_eq!(q("a\nb"), "k: |-\n  a\n  b\n");
        assert_eq!(q("a\n\nb"), "k: |-\n  a\n\n  b\n");
        assert_eq!(q("a \nb"), "k: \"a \\nb\"\n");
        assert_eq!(q("\u{1}"), "k: \"\\u0001\"\n");
    }

    #[test]
    fn parse_lenient() {
        // id 無し・重なる id・足りない項目・知らないキー・整数の開始・手書きの形
        let t = "title: T\nextra: 1\nlines:\n  - id: a\n    sp: talk2\n    text: ひとつめ\n    start: 3\n  - id: a\n    text: ふたつめ\n  - sp: トーク1\n    text: みっつめ\n    color: red\n  - id: 7\n    text: 数の id\n";
        let r = parse_yaml(t, "x.yaml").unwrap();
        assert_eq!(r.lines.len(), 4);
        assert_eq!(r.lines[0].sp, "トーク2");
        assert_eq!(r.lines[0].start, Some(3.0));
        assert_eq!(r.lines[1].sp, "");
        assert_eq!(r.lines[2].id, None);
        assert_eq!(r.lines[3].id.as_deref(), Some("7"));
        assert!(parse_yaml("", "x.yaml").unwrap().lines.is_empty());
        assert!(parse_yaml("lines:\n", "x.yaml").unwrap().lines.is_empty());
        let e = parse_yaml("title: [\nlines", "broken.yaml").err().unwrap().to_string();
        assert!(e.contains("broken.yaml"), "{e}");
    }

    #[test]
    fn fixture_is_canonical() {
        let text = include_str!("../../tests/fixtures/script_v2_sample.yaml");
        let r = parse_yaml(text, "script_v2_sample.yaml").unwrap();
        let b = Block {
            id: "b".into(),
            file: None,
            title: r.title.unwrap(),
            notes: r.notes,
            lines: r
                .lines
                .into_iter()
                .map(|l| Line { id: l.id.unwrap(), sp: l.sp, text: l.text, start: l.start, memo: l.memo })
                .collect(),
        };
        assert_eq!(to_yaml(&b), text.replace("\r\n", "\n"));
    }

    // ---- id

    #[test]
    fn ids_are_unique_and_shaped() {
        let mut set = HashSet::new();
        for _ in 0..2000 {
            let id = new_id("l");
            assert!(is_id_shape(&id), "{id}");
            assert!(set.insert(id));
        }
        let mut s = Script::default();
        s.blocks.push(block("A", vec![line("l1", "トーク1", "あ")], ""));
        assert!(!s.line_ids().contains(&s.fresh_id("l")));
    }

    #[test]
    fn voice_helpers() {
        let mut s = Script::default();
        let l = line("l1", "トーク1", "どうも｜A《えー》");
        s.voice.insert("l1".into(), serde_json::json!({"sp": "トーク1", "text": "どうもえー", "analyzed_text": "<x/>"}));
        s.params.insert("l1".into(), serde_json::json!({"sp": "トーク1", "text": "どうもえー", "params": {"intonation": 1.2}}));
        assert_eq!(s.voice_of(&l), Some("<x/>"));
        assert_eq!(s.params_of(&l).unwrap()["intonation"], 1.2);
        let mut l2 = l.clone();
        l2.text = "別の文".into();
        assert_eq!(s.voice_of(&l2), None);
        assert!(s.params_of(&l2).is_none());
        l2 = l.clone();
        l2.sp = "トーク2".into();
        assert_eq!(s.voice_of(&l2), None);
    }
}
