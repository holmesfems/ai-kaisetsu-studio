//! 手直しの履歴（`<回>/studio/edit_log.jsonl`）。台本の保存の前と後を行ごとに比べ、
//! セリフ・話者・アクセント（`_voice.json`）・抑揚（`_voice_params.json`）の変わった所を 1 行ずつ追記する。
//!
//! 1 行の形（JSON）:
//! `{"ts", "kind": text|speaker|accent|params, "line_id", "block_file", "before", "after", "source": user|claude_proposal|voice_review_apply, "note"?, "proposal_id"?}`
//! - text・speaker の before/after は文字列。accent・params は `_voice.json`・`_voice_params.json` のその行の値（無ければ null）
//! - 追記だけで、書いた行は書き換えない。画面の保存（打っている間は 0.7 秒ごと）は、同じ行・同じ種類の変更が
//!   `WINDOW` のあいだ続く限り 1 件にまとめ（before は最初の、after は最後の中身）、手が止まってから書く。
//!   まとめた結果が元に戻っていたら（before == after）書かない
//! - Claude の推敲案の採用（source=claude_proposal）はまとめずにすぐ書く。案の中身は `studio/proposals.json` にあるので、
//!   ここには前後と `proposal_id` だけ
//! - `note` は理由欄（今は画面から付ける所は無い。あとで付けられるように形だけ用意する）
//! - `voice_review.py apply --write` は Python（`voice/akscript.py` の `append_edit_log`）が同じ形で書く

use super::format::Script;
use chrono::{DateTime, Local};
use serde::Serialize;
use serde_json::Value;
use std::collections::{BTreeSet, HashMap};
use std::io::Write;
use std::path::{Path, PathBuf};

/// 同じ行の画面の変更を 1 件にまとめる長さ。最後の変更からこれだけ手が止まったら書く
pub const WINDOW: chrono::Duration = chrono::Duration::seconds(20);

/// 回のフォルダの中の履歴のファイル（`studio/edit_log.jsonl`）
pub fn log_path(studio_dir: &Path) -> PathBuf {
    studio_dir.join("edit_log.jsonl")
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Text,
    Speaker,
    Accent,
    Params,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    User,
    ClaudeProposal,
    VoiceReviewApply,
}

/// 誰の保存か（`Core::save_script_from` に渡す）
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Origin {
    pub source: Source,
    pub proposal_id: Option<u64>,
}

impl Origin {
    pub const USER: Origin = Origin { source: Source::User, proposal_id: None };
    pub fn proposal(id: u64) -> Origin {
        Origin { source: Source::ClaudeProposal, proposal_id: Some(id) }
    }
}

/// 1 行ぶんの変更（保存の前後の比較の結果）
#[derive(Clone, Debug, PartialEq)]
pub struct Change {
    pub kind: Kind,
    pub line_id: String,
    pub block_file: Option<String>,
    pub before: Value,
    pub after: Value,
}

/// edit_log.jsonl の 1 行
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Entry {
    pub ts: String,
    pub kind: Kind,
    pub line_id: String,
    pub block_file: Option<String>,
    pub before: Value,
    pub after: Value,
    pub source: Source,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub proposal_id: Option<u64>,
}

/// 保存の前（old）と後（new）の台本を比べる。両方にある行のセリフ・話者と、`_voice.json`・`_voice_params.json` の
/// 値が変わった行の id。行の追加・削除そのものは記録しない（その行の手直しの値が増えた・消えたことは accent・params に出る）
pub fn diff(old: &Script, new: &Script) -> Vec<Change> {
    let old_lines: HashMap<&str, &super::format::Line> =
        old.blocks.iter().flat_map(|b| b.lines.iter()).map(|l| (l.id.as_str(), l)).collect();
    let file_of = |s: &Script, id: &str| -> Option<String> {
        s.blocks.iter().find(|b| b.lines.iter().any(|l| l.id == id)).and_then(|b| b.file.clone())
    };
    let mut out = vec![];
    for b in &new.blocks {
        for l in &b.lines {
            let Some(o) = old_lines.get(l.id.as_str()) else { continue };
            let file = b.file.clone().or_else(|| file_of(old, &l.id));
            if o.sp != l.sp {
                out.push(Change {
                    kind: Kind::Speaker,
                    line_id: l.id.clone(),
                    block_file: file.clone(),
                    before: Value::String(o.sp.clone()),
                    after: Value::String(l.sp.clone()),
                });
            }
            if o.text != l.text {
                out.push(Change {
                    kind: Kind::Text,
                    line_id: l.id.clone(),
                    block_file: file,
                    before: Value::String(o.text.clone()),
                    after: Value::String(l.text.clone()),
                });
            }
        }
    }
    for (kind, om, nm) in [(Kind::Accent, &old.voice, &new.voice), (Kind::Params, &old.params, &new.params)] {
        let keys: BTreeSet<&String> = om.keys().chain(nm.keys()).collect();
        for k in keys {
            let (b, a) = (om.get(k), nm.get(k));
            if b == a {
                continue;
            }
            out.push(Change {
                kind,
                line_id: k.clone(),
                block_file: file_of(new, k).or_else(|| file_of(old, k)),
                before: b.cloned().unwrap_or(Value::Null),
                after: a.cloned().unwrap_or(Value::Null),
            });
        }
    }
    out
}

/// 保存 1 回で記録する変更。before = 保存の前の台本、ui = 保存に送られた台本、after = 保存のあとの台本。
/// 外（Claude・voice_review）の変更も一緒に読み直したとき（external）は、送られた台本と同じになった所だけ残す
/// （外が書いた変更を、この保存のせいにしない）
pub fn save_changes(before: &Script, ui: &Script, after: &Script, external: bool) -> Vec<Change> {
    let mut changes = diff(before, after);
    if external {
        let ui_side = diff(before, ui);
        changes.retain(|c| ui_side.iter().any(|u| u.kind == c.kind && u.line_id == c.line_id && u.after == c.after));
    }
    changes
}

struct Pending {
    path: PathBuf,
    entry: Entry,
    last: DateTime<Local>,
}

/// 画面の変更をまとめておく所（書くのは呼び出し元。返ってきた (ファイル, 行) を `append` で書く）
#[derive(Default)]
pub struct EditLog {
    pending: Vec<Pending>,
}

fn ts(t: DateTime<Local>) -> String {
    t.to_rfc3339_opts(chrono::SecondsFormat::Secs, false)
}

impl EditLog {
    /// 保存 1 回ぶんの変更を受け取る。今すぐ書くもの（まとめ終わったもの・Claude の採用）を返す
    pub fn record(&mut self, path: &Path, changes: Vec<Change>, origin: Origin, now: DateTime<Local>) -> Vec<(PathBuf, Entry)> {
        let mut out = vec![];
        for c in changes {
            let at = self.pending.iter().position(|p| p.path == path && p.entry.kind == c.kind && p.entry.line_id == c.line_id);
            if let Some(i) = at {
                let p = &mut self.pending[i];
                if origin.source == Source::User && now - p.last < WINDOW {
                    p.entry.after = c.after;
                    p.entry.block_file = c.block_file.or(p.entry.block_file.take());
                    p.entry.ts = ts(now);
                    p.last = now;
                    continue;
                }
                // 間が空いた・別の人の変更が来た: 前の分を先に書く（順番を保つ）
                let p = self.pending.remove(i);
                out.extend(Self::finish(p));
            }
            let entry = Entry {
                ts: ts(now),
                kind: c.kind,
                line_id: c.line_id,
                block_file: c.block_file,
                before: c.before,
                after: c.after,
                source: origin.source,
                note: None,
                proposal_id: origin.proposal_id,
            };
            if origin.source == Source::User {
                self.pending.push(Pending { path: path.to_path_buf(), entry, last: now });
            } else {
                out.push((path.to_path_buf(), entry));
            }
        }
        out
    }

    /// 手が止まってから WINDOW たったもの（all なら全部）を取り出す
    pub fn take_idle(&mut self, now: DateTime<Local>, all: bool) -> Vec<(PathBuf, Entry)> {
        let (done, keep): (Vec<Pending>, Vec<Pending>) =
            std::mem::take(&mut self.pending).into_iter().partition(|p| all || now - p.last >= WINDOW);
        self.pending = keep;
        done.into_iter().flat_map(Self::finish).collect()
    }

    pub fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }

    fn finish(p: Pending) -> Option<(PathBuf, Entry)> {
        (p.entry.before != p.entry.after).then_some((p.path, p.entry))
    }
}

/// 行をファイルの末尾に足す（ファイルが無ければ作る）。1 行ずつ JSON
pub fn append(path: &Path, entries: &[Entry]) -> anyhow::Result<()> {
    if entries.is_empty() {
        return Ok(());
    }
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d)?;
    }
    let mut buf = String::new();
    for e in entries {
        buf.push_str(&serde_json::to_string(e)?);
        buf.push('\n');
    }
    let mut f = std::fs::OpenOptions::new().create(true).append(true).open(path)?;
    f.write_all(buf.as_bytes())?;
    Ok(())
}

/// (ファイル, 行) をファイルごとにまとめて書く。書けなかったファイルの理由を返す
pub fn append_all(items: Vec<(PathBuf, Entry)>) -> Vec<String> {
    let mut by: Vec<(PathBuf, Vec<Entry>)> = vec![];
    for (p, e) in items {
        match by.iter_mut().find(|(q, _)| *q == p) {
            Some((_, v)) => v.push(e),
            None => by.push((p, vec![e])),
        }
    }
    by.into_iter()
        .filter_map(|(p, v)| append(&p, &v).err().map(|e| format!("手直しの履歴（{}）に書けませんでした: {e}", p.display())))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::script::format::{Block, Line};
    use serde_json::json;

    fn script(lines: &[(&str, &str, &str)]) -> Script {
        Script {
            blocks: vec![Block {
                id: "b".into(),
                file: Some("01.yaml".into()),
                title: "t".into(),
                lines: lines
                    .iter()
                    .map(|(id, sp, text)| Line { id: id.to_string(), sp: sp.to_string(), text: text.to_string(), start: None, memo: String::new() })
                    .collect(),
                notes: String::new(),
            }],
            ..Default::default()
        }
    }

    #[test]
    fn diff_text_speaker_accent_params() {
        let old = script(&[("l1", "トーク1", "プラマニS2"), ("l2", "トーク2", "そう")]);
        let mut new = script(&[("l1", "トーク1", "プラマニのS2"), ("l2", "トーク1", "そう"), ("l3", "トーク1", "新しい")]);
        new.voice.insert("l1".into(), json!({"sp": "トーク1", "text": "プラマニのS2", "analyzed_text": "<x/>"}));
        new.params.insert("l2".into(), json!({"sp": "トーク1", "text": "そう", "params": {"speed": 1.1}}));
        let d = diff(&old, &new);
        let kinds: Vec<(Kind, &str)> = d.iter().map(|c| (c.kind, c.line_id.as_str())).collect();
        assert_eq!(kinds, vec![(Kind::Text, "l1"), (Kind::Speaker, "l2"), (Kind::Accent, "l1"), (Kind::Params, "l2")]);
        assert_eq!(d[0].before, json!("プラマニS2"));
        assert_eq!(d[0].after, json!("プラマニのS2"));
        assert_eq!(d[0].block_file.as_deref(), Some("01.yaml"));
        assert_eq!(d[2].before, Value::Null);
        // 変わっていなければ何も出ない
        assert!(diff(&new, &new).is_empty());
    }

    #[test]
    fn user_edits_coalesce_until_idle() {
        let p = Path::new("x/edit_log.jsonl");
        let t0 = Local::now();
        let mut log = EditLog::default();
        let ch = |b: &str, a: &str| vec![Change { kind: Kind::Text, line_id: "l1".into(), block_file: Some("01.yaml".into()), before: json!(b), after: json!(a) }];
        // 1 文字ずつ打つ（0.7 秒ごとの保存）
        assert!(log.record(p, ch("プラマニS2", "プラマニのS2"), Origin::USER, t0).is_empty());
        assert!(log.record(p, ch("プラマニのS2", "プラマニのS"), Origin::USER, t0 + chrono::Duration::seconds(1)).is_empty());
        assert!(log.record(p, ch("プラマニのS", "プラマニのS3"), Origin::USER, t0 + chrono::Duration::seconds(2)).is_empty());
        assert!(log.take_idle(t0 + chrono::Duration::seconds(10), false).is_empty());
        let out = log.take_idle(t0 + chrono::Duration::seconds(30), false);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].1.before, json!("プラマニS2"));
        assert_eq!(out[0].1.after, json!("プラマニのS3"));
        assert_eq!(out[0].1.source, Source::User);
        assert!(log.is_empty());
    }

    #[test]
    fn reverted_edit_is_dropped_and_proposal_flushes_first() {
        let p = Path::new("x/edit_log.jsonl");
        let t0 = Local::now();
        let mut log = EditLog::default();
        let ch = |b: &str, a: &str| vec![Change { kind: Kind::Text, line_id: "l1".into(), block_file: None, before: json!(b), after: json!(a) }];
        log.record(p, ch("A", "AB"), Origin::USER, t0);
        log.record(p, ch("AB", "A"), Origin::USER, t0 + chrono::Duration::seconds(1));
        assert!(log.take_idle(t0, true).is_empty(), "元に戻したものは書かない");

        log.record(p, ch("A", "B"), Origin::USER, t0);
        // 同じ行に Claude の案が採用された: 画面の分を先に、案の分をすぐ書く
        let out = log.record(p, ch("B", "C"), Origin::proposal(7), t0 + chrono::Duration::seconds(1));
        assert_eq!(out.len(), 2);
        assert_eq!((out[0].1.source, out[0].1.after.clone()), (Source::User, json!("B")));
        assert_eq!((out[1].1.source, out[1].1.proposal_id), (Source::ClaudeProposal, Some(7)));
        assert!(log.is_empty());
    }

    /// 回の台本フォルダで、セリフを 1 つ・アクセントを 1 つ直して保存すると、それぞれ前後の入った行が追記される。
    /// 外（voice_review）が書いた _voice.json は、そのあとの画面の保存のせいにしない
    #[test]
    fn store_saves_write_rows() {
        use crate::script::ScriptStore;
        let d = std::env::temp_dir().join(format!("ak-editlog-store-{}", crate::script::new_id("t")));
        let sd = d.join("script");
        std::fs::create_dir_all(&sd).unwrap();
        std::fs::write(sd.join("01.yaml"), "title: 一\nlines:\n  - id: l1\n    sp: トーク1\n    text: プラマニS2\n  - id: l2\n    sp: トーク2\n    text: そう\n").unwrap();
        let path = log_path(&d.join("studio"));
        let mut st = ScriptStore::open(&sd).unwrap();
        let mut log = EditLog::default();
        let t0 = Local::now();
        let save = |st: &mut ScriptStore, s: Script, t: DateTime<Local>, log: &mut EditLog| {
            let before = st.script.clone();
            st.save(s.clone()).unwrap();
            let ch = save_changes(&before, &s, &st.script, st.merged_external);
            append_all(log.record(&path, ch, Origin::USER, t));
        };
        // セリフを直す（2 回の保存に分けて打つ）
        let mut s = st.script.clone();
        s.blocks[0].lines[0].text = "プラマニの".into();
        save(&mut st, s, t0, &mut log);
        let mut s = st.script.clone();
        s.blocks[0].lines[0].text = "プラマニのS2".into();
        save(&mut st, s, t0 + chrono::Duration::seconds(1), &mut log);
        // アクセントを直す
        let mut s = st.script.clone();
        s.voice.insert("l1".into(), json!({"sp": "トーク1", "text": "プラマニのS2", "analyzed_text": "<a/>"}));
        save(&mut st, s, t0 + chrono::Duration::seconds(2), &mut log);
        assert!(!path.exists(), "手が止まるまでは書かない");
        append_all(log.take_idle(t0 + chrono::Duration::seconds(60), false));
        // 外が _voice.json を書いた（見張りが読み直す前に、画面が古い rev で別の行を保存した）
        let stale = st.script.clone();
        std::fs::write(sd.join("_voice.json"), serde_json::to_string(&json!({
            "l1": {"sp": "トーク1", "text": "プラマニのS2", "analyzed_text": "<a/>"},
            "l2": {"sp": "トーク2", "text": "そう", "analyzed_text": "<b/>"}})).unwrap()).unwrap();
        let mut s = stale;
        s.blocks[0].lines[1].sp = "トーク1".into();
        save(&mut st, s, t0 + chrono::Duration::seconds(70), &mut log);
        assert_eq!(st.script.voice["l2"]["analyzed_text"], "<b/>", "外の変更は残る");
        append_all(log.take_idle(t0, true));

        let rows: Vec<Value> = std::fs::read_to_string(&path).unwrap().lines().map(|l| serde_json::from_str(l).unwrap()).collect();
        let got: Vec<(String, String)> = rows.iter().map(|r| (r["kind"].as_str().unwrap().into(), r["line_id"].as_str().unwrap().into())).collect();
        assert_eq!(got, vec![("text".into(), "l1".into()), ("accent".into(), "l1".into()), ("speaker".into(), "l2".into())], "{rows:#?}");
        assert_eq!((rows[0]["before"].clone(), rows[0]["after"].clone()), (json!("プラマニS2"), json!("プラマニのS2")));
        assert_eq!(rows[0]["block_file"], "01.yaml");
        assert_eq!(rows[1]["before"], Value::Null);
        assert_eq!(rows[1]["after"]["analyzed_text"], "<a/>");
        assert!(rows.iter().all(|r| r["source"] == "user"));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn append_only_and_shape() {
        let d = std::env::temp_dir().join(format!("ak-editlog-{}", crate::script::new_id("t")));
        let p = log_path(&d.join("studio"));
        let e = Entry {
            ts: "2026-11-28T10:00:00+09:00".into(),
            kind: Kind::Accent,
            line_id: "l1".into(),
            block_file: Some("01.yaml".into()),
            before: Value::Null,
            after: json!({"analyzed_text": "<x/>"}),
            source: Source::User,
            note: None,
            proposal_id: None,
        };
        append(&p, std::slice::from_ref(&e)).unwrap();
        append(&p, &[Entry { kind: Kind::Text, note: Some("読みを直すため".into()), ..e.clone() }]).unwrap();
        let t = std::fs::read_to_string(&p).unwrap();
        let rows: Vec<Value> = t.lines().map(|l| serde_json::from_str(l).unwrap()).collect();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0]["kind"], "accent");
        assert_eq!(rows[0]["source"], "user");
        assert!(rows[0].get("note").is_none());
        assert_eq!(rows[1]["note"], "読みを直すため");
        let _ = std::fs::remove_dir_all(&d);
    }
}
