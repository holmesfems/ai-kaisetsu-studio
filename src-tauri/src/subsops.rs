//! セリフごとの字幕（改行・大きさ・フォント）を、外（Claude）から読む・直す。
//! 書く先は画面の `patchSubLine` と同じ `project.json` の `subtitle_lines[鍵]`。
//! Python を使う測り（`core.subs.measure`）と、使わない部分（`apply`・`line_json`）を分けてある。

use crate::app::error::{bad_input, no_project, not_found};
use crate::core::Core;
use crate::project::ProjectDoc;
use crate::render::subs::{line_key, resolve, SubStyle};
use crate::voice::layout::VoiceItem;
use ak_proto::http::SubsSetItem;
use anyhow::Result;
use serde_json::{json, Value};

/// 字幕の大きさの範囲
pub const SIZE_MIN: f64 = 20.0;
pub const SIZE_MAX: f64 = 120.0;

/// 画面の normSubText（ui/src/logic/subs.ts）と同じ: `\r\n` → `\n`、行末の空白を削り、空の行は捨てる
pub fn norm_text(v: &str) -> String {
    v.replace("\r\n", "\n").split('\n').map(str::trim_end).filter(|l| !l.trim().is_empty()).collect::<Vec<_>>().join("\n")
}

/// t が、台本の字幕の文 text に改行を入れただけか。改行の前後の空白（台本の文の空白の所で改行したとき）だけは消えてよい
pub fn only_breaks(t: &str, text: &str) -> bool {
    let parts: Vec<&str> = t.split('\n').collect();
    let mut rest = text;
    for (i, p) in parts.iter().enumerate() {
        let last = i + 1 == parts.len();
        let p = if i == 0 { *p } else { p.trim_start() };
        let p = if last { p } else { p.trim_end() };
        let Some(r) = rest.strip_prefix(p) else { return false };
        rest = if last { r } else { r.trim_start() };
    }
    rest.is_empty()
}

/// セリフの指し方（行の id → "#12" → 通し番号）で並びの中の位置を引く。
/// `VoiceItem.n` は script_read の通し番号と同じ数え方（`voiced_lines` が全行を数える）
pub fn find_item(items: &[VoiceItem], v: &Value) -> Option<usize> {
    if let Some(id) = v.as_str() {
        if let Some(i) = items.iter().position(|it| it.line_id == id) {
            return Some(i);
        }
        let t = id.trim();
        if let Ok(n) = t.strip_prefix('#').unwrap_or(t).parse::<usize>() {
            return items.iter().position(|it| it.n == n);
        }
        return None;
    }
    let n = v.as_u64()? as usize;
    items.iter().position(|it| it.n == n)
}

/// "#番号" を除いたフォントのパス
fn font_file(spec: &str) -> &str {
    match spec.rsplit_once('#') {
        Some((p, n)) if !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()) => p,
        _ => spec,
    }
}

/// items を doc.subtitle_lines に当てる。1 件でも断れば doc は変えない。変えたセリフの line_id（items の順・重複なし）を返す。
/// font_exists はフォントのファイルがあるかの判定（テストで差し替える）
pub fn apply(doc: &mut ProjectDoc, items: &[SubsSetItem], layout: &[VoiceItem], font_exists: &dyn Fn(&str) -> bool) -> Result<Vec<String>> {
    let mut all = doc.subtitle_lines.as_object().cloned().unwrap_or_default();
    let mut changed: Vec<String> = vec![];
    for (i, item) in items.iter().enumerate() {
        let at = |m: String| bad_input(format!("items[{i}]: {m}"));
        let k = find_item(layout, &item.line)
            .ok_or_else(|| not_found(format!("items[{i}]: セリフ {} が無い（script_read の通し番号か id。声の無い行は指せない）", item.line)))?;
        let it = &layout[k];
        let key = line_key(&it.sp, &it.text);
        let mut o = all.get(&key).and_then(Value::as_object).cloned().unwrap_or_default();
        if item.text.is_none() && item.size.is_none() && item.font.is_none() {
            return Err(at("text・size・font のどれかを渡してください".into()));
        }
        if let Some(v) = &item.text {
            match v {
                Value::Null => {
                    o.remove("text");
                }
                Value::String(s) => {
                    let t = norm_text(s);
                    if !only_breaks(&t, &it.text) {
                        return Err(at(format!("字幕の文を変えずに改行だけ入れてください（今: {}）", it.text)));
                    }
                    // 台本の文と同じ（改行なし）なら、字幕だけの設定は要らない
                    if t == it.text {
                        o.remove("text");
                    } else {
                        o.insert("text".into(), json!(t));
                    }
                }
                _ => return Err(at("text は文字列か null".into())),
            }
        }
        if let Some(v) = &item.size {
            match v {
                Value::Null => {
                    o.remove("size");
                }
                _ => {
                    let n = v.as_f64().filter(|n| (SIZE_MIN..=SIZE_MAX).contains(n)).ok_or_else(|| {
                        at(format!("size は {SIZE_MIN}〜{SIZE_MAX} の数か null（今の値: {v}）"))
                    })?;
                    o.insert("size".into(), json!(n));
                }
            }
        }
        if let Some(v) = &item.font {
            match v {
                Value::Null => {
                    o.remove("font");
                }
                Value::String(s) if s.trim().is_empty() => {
                    o.remove("font");
                }
                Value::String(s) => {
                    if !font_exists(font_file(s)) {
                        return Err(at(format!("フォントのファイルが無い: {}", font_file(s))));
                    }
                    o.insert("font".into(), json!(s));
                }
                _ => return Err(at("font は文字列か null".into())),
            }
        }
        if o.is_empty() {
            all.remove(&key);
        } else {
            all.insert(key, Value::Object(o));
        }
        if !changed.contains(&it.line_id) {
            changed.push(it.line_id.clone());
        }
    }
    doc.subtitle_lines = Value::Object(all);
    Ok(changed)
}

/// 測った結果（Python の subtitle_measure の texts と overflow）
pub struct Measured {
    pub rows: Vec<String>,
    pub overflow: Vec<bool>,
}

/// 1 セリフの返り。measured が None なら rows・overflow は付けない
pub fn line_json(doc: &ProjectDoc, it: &VoiceItem, measured: Option<&Measured>) -> Value {
    let job = resolve(doc, it);
    let mut o = json!({
        "n": it.n, "id": it.line_id, "sp": it.sp, "text": it.text,
        "sub_text": job.text, "manual": job.manual, "custom": job.custom,
        "size": job.style.size, "font": job.style.font,
    });
    if let Some(own) = doc.subtitle_lines.get(line_key(&it.sp, &it.text)) {
        o["own"] = own.clone();
    }
    if let Some(m) = measured {
        o["rows"] = json!(m.rows);
        o["row_count"] = json!(m.rows.len());
        o["overflow"] = json!(m.overflow);
    }
    o
}

fn style_json(s: &SubStyle) -> Value {
    json!({"font": s.font, "size": s.size, "line_h": s.line_h, "margin_l": s.margin_l, "margin_r": s.margin_r, "wrap": s.wrap})
}

/// subs_read の返りを組み立てる。items は開始の順の並び、want は返すセリフの位置（並びの順）。
/// measured は want と同じ並びの測り結果。測れなかったら Err(理由) を渡す（rows・overflow を省いて measure_error に入れる）
pub fn build(doc: &ProjectDoc, items: &[VoiceItem], want: &[usize], measured: Result<Vec<Measured>, String>) -> Value {
    let view = crate::render::subs::view(doc, items);
    let width = 1920.0 - view.track.margin_l - view.track.margin_r;
    let speakers: serde_json::Map<String, Value> = view.speakers.iter().map(|(k, s)| (k.clone(), style_json(s))).collect();
    let (ms, err) = match measured {
        Ok(m) => (m, None),
        Err(e) => (vec![], Some(e)),
    };
    let lines: Vec<Value> = want.iter().enumerate().map(|(j, &k)| line_json(doc, &items[k], ms.get(j))).collect();
    let mut out = json!({"track": style_json(&view.track), "speakers": speakers, "width": width, "lines": lines});
    if let Some(e) = err {
        out["measure_error"] = json!(e);
    }
    out
}

fn measure(core: &Core, doc: &ProjectDoc, items: &[VoiceItem], want: &[usize]) -> Result<Vec<Measured>, String> {
    if want.is_empty() {
        return Ok(vec![]);
    }
    let jobs: Vec<_> = want.iter().map(|&k| resolve(doc, &items[k])).collect();
    let infos = core.subs.measure(&jobs).map_err(|e| format!("字幕の行の分け方を測れませんでした: {e:#}"))?;
    Ok(infos.iter().map(|i| Measured { rows: i.texts.clone(), overflow: i.overflow.clone() }).collect())
}

/// subs_read: lines は指し方（省くと全部）
pub fn read(core: &Core, lines: Option<&[Value]>) -> Result<Value> {
    let doc = core.doc().ok_or_else(no_project)?;
    let items = core.sorted_layout();
    let want: Vec<usize> = match lines {
        None => (0..items.len()).collect(),
        Some(ls) => {
            let mut w = vec![];
            for v in ls {
                let k = find_item(&items, v).ok_or_else(|| not_found(format!("セリフ {v} が無い（script_read の通し番号か id。声の無い行は指せない）")))?;
                if !w.contains(&k) {
                    w.push(k);
                }
            }
            w.sort_unstable();
            w
        }
    };
    let m = measure(core, &doc, &items, &want);
    Ok(build(&doc, &items, &want, m))
}

/// subs_set: 書いて画面に知らせ（reason があれば知らせの文に足す）、直したセリフをもう一度測って返す
pub fn set(core: &Core, items: &[SubsSetItem], reason: &str) -> Result<Value> {
    if items.is_empty() {
        return Err(bad_input("items が空です"));
    }
    let mut doc = core.doc().ok_or_else(no_project)?;
    let layout = core.sorted_layout();
    let changed = apply(&mut doc, items, &layout, &|p| std::path::Path::new(p).is_file())?;
    core.set_doc_and_notify(doc.clone())?;
    let why = if reason.trim().is_empty() { String::new() } else { format!("（{}）", reason.trim()) };
    core.emit("toast", json!({"text": format!("Claude がセリフの字幕を直しました（{} 件）{why}", changed.len())}));
    let want: Vec<usize> = layout.iter().enumerate().filter(|(_, it)| changed.contains(&it.line_id)).map(|(k, _)| k).collect();
    let m = measure(core, &doc, &layout, &want);
    let out = build(&doc, &layout, &want, m);
    let mut r = json!({"ok": true, "lines": out["lines"]});
    if let Some(e) = out.get("measure_error") {
        r["measure_error"] = e.clone();
    }
    Ok(r)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(n: usize, sp: &str, text: &str) -> VoiceItem {
        VoiceItem { line_id: format!("l{n}"), n, sp: sp.into(), text: text.into(), start: n as f64, dur: 1.0, audio: None, block: String::new(), key: String::new() }
    }

    fn layout() -> Vec<VoiceItem> {
        vec![item(1, "トーク1", "こんにちは世界"), item(3, "トーク2", "長い長いセリフです")]
    }

    fn it(line: Value, text: Option<Value>, size: Option<Value>, font: Option<Value>) -> SubsSetItem {
        SubsSetItem { line, text, size, font }
    }

    fn run(doc: &mut ProjectDoc, items: &[SubsSetItem]) -> Result<Vec<String>> {
        apply(doc, items, &layout(), &|p| p == "C:/ok.ttf")
    }

    fn own(doc: &ProjectDoc, sp: &str, text: &str) -> Option<Value> {
        doc.subtitle_lines.get(line_key(sp, text)).cloned()
    }

    #[test]
    fn writes_text_and_size_then_removes_with_null() {
        let mut doc = ProjectDoc::default();
        let r = run(&mut doc, &[it(json!("l1"), Some(json!("こんにちは\n世界")), Some(json!(48)), None)]).unwrap();
        assert_eq!(r, ["l1"]);
        assert_eq!(own(&doc, "トーク1", "こんにちは世界"), Some(json!({"text": "こんにちは\n世界", "size": 48.0})));
        // size だけ外す
        run(&mut doc, &[it(json!("l1"), None, Some(Value::Null), None)]).unwrap();
        assert_eq!(own(&doc, "トーク1", "こんにちは世界"), Some(json!({"text": "こんにちは\n世界"})));
        // 全部外れたら鍵ごと消える
        run(&mut doc, &[it(json!("l1"), Some(Value::Null), None, None)]).unwrap();
        assert_eq!(own(&doc, "トーク1", "こんにちは世界"), None);
        assert!(doc.subtitle_lines.as_object().unwrap().is_empty());
    }

    #[test]
    fn same_text_as_script_is_dropped_and_whitespace_is_normalized() {
        let mut doc = ProjectDoc::default();
        run(&mut doc, &[it(json!("l1"), Some(json!("こんにちは\r\n世界")), None, None)]).unwrap();
        assert_eq!(own(&doc, "トーク1", "こんにちは世界").unwrap()["text"], "こんにちは\n世界");
        run(&mut doc, &[it(json!("l1"), Some(json!("こんにちは世界")), None, None)]).unwrap();
        assert_eq!(own(&doc, "トーク1", "こんにちは世界"), None);
        // 行末の空白は削る
        run(&mut doc, &[it(json!("l1"), Some(json!("こんにちは  \n世界 ")), None, None)]).unwrap();
        assert_eq!(own(&doc, "トーク1", "こんにちは世界").unwrap()["text"], "こんにちは\n世界");
        assert_eq!(norm_text("a \r\n\r\n b  \n"), "a\n b");
    }

    #[test]
    fn rejects_changed_text_and_writes_nothing() {
        let mut doc = ProjectDoc::default();
        let before = doc.subtitle_lines.clone();
        // 2 件目が文を変えている → 1 件目も書かない
        let r = run(
            &mut doc,
            &[it(json!("l1"), Some(json!("こんにちは\n世界")), None, None), it(json!("l3"), Some(json!("違う\n文")), None, None)],
        );
        let e = format!("{:#}", r.unwrap_err());
        assert!(e.contains("改行だけ") && e.contains("長い長いセリフです"), "{e}");
        assert_eq!(doc.subtitle_lines, before);
        // 空白を足す・消すのは、改行の前後でなければ文を変えたことになる
        assert!(run(&mut doc, &[it(json!("l1"), Some(json!("こんにちは 世\n界")), None, None)]).is_err());
        assert!(run(&mut doc, &[it(json!("l1"), Some(json!("こんにちは 世界")), None, None)]).is_err());
        assert_eq!(doc.subtitle_lines, before);
    }

    #[test]
    fn only_breaks_allows_newlines_and_spaces_around_them() {
        assert!(only_breaks("こんにちは\n世界", "こんにちは世界"));
        assert!(only_breaks("こんにちは世界", "こんにちは世界"));
        // 台本の文の空白の所で改行するときは、空白が消えてよい
        assert!(only_breaks("Hello\nWorld", "Hello World"));
        assert!(only_breaks("Hello \n World", "Hello World"));
        assert!(!only_breaks("Hel lo\nWorld", "Hello World"));
        assert!(!only_breaks("こんにちは 世界", "こんにちは世界"));
        assert!(!only_breaks("こんにちは\n世", "こんにちは世界"));
        assert!(!only_breaks("こんにちは\n世界！", "こんにちは世界"));
    }

    #[test]
    fn rejects_bad_size_font_unknown_line_and_empty() {
        let mut doc = ProjectDoc::default();
        let before = doc.subtitle_lines.clone();
        assert!(run(&mut doc, &[it(json!("l1"), None, Some(json!(10)), None)]).is_err());
        assert!(run(&mut doc, &[it(json!("l1"), None, Some(json!(121)), None)]).is_err());
        assert_eq!(doc.subtitle_lines, before);
        assert!(run(&mut doc, &[it(json!("l1"), None, Some(json!("大")), None)]).is_err());
        assert!(run(&mut doc, &[it(json!("l1"), None, Some(json!(20)), None)]).is_ok());
        assert!(run(&mut doc, &[it(json!("l1"), None, Some(json!(120)), None)]).is_ok());
        assert!(run(&mut doc, &[it(json!("l1"), None, None, Some(json!("C:/none.ttf")))]).is_err());
        assert!(run(&mut doc, &[it(json!("l1"), None, None, Some(json!("C:/ok.ttf#2")))]).is_ok());
        assert_eq!(own(&doc, "トーク1", "こんにちは世界").unwrap()["font"], "C:/ok.ttf#2");
        assert!(run(&mut doc, &[it(json!("l1"), None, None, Some(Value::Null))]).is_ok());
        // 知らない行・声の並びに無い行（通し番号 2）・何も渡していない項目
        assert!(run(&mut doc, &[it(json!("nope"), None, Some(json!(48)), None)]).is_err());
        assert!(run(&mut doc, &[it(json!(2), None, Some(json!(48)), None)]).is_err());
        assert!(run(&mut doc, &[it(json!("l1"), None, None, None)]).is_err());
        // 外した後は空に戻っている
        assert!(doc.subtitle_lines.as_object().unwrap().len() == 1);
        run(&mut doc, &[it(json!("l1"), None, Some(Value::Null), None)]).unwrap();
        assert!(doc.subtitle_lines.as_object().unwrap().is_empty());
    }

    #[test]
    fn lines_are_found_by_id_number_or_hash() {
        let l = layout();
        assert_eq!(find_item(&l, &json!("l3")), Some(1));
        assert_eq!(find_item(&l, &json!(3)), Some(1));
        assert_eq!(find_item(&l, &json!("#3")), Some(1));
        assert_eq!(find_item(&l, &json!(" 1 ")), Some(0));
        assert_eq!(find_item(&l, &json!("##3")), None);
        assert_eq!(find_item(&l, &json!(2)), None);
        assert_eq!(find_item(&l, &json!(null)), None);
    }

    #[test]
    fn build_reads_style_own_and_measured() {
        let mut doc = ProjectDoc::default();
        let l = layout();
        run(&mut doc, &[it(json!(1), Some(json!("こんにちは\n世界")), Some(json!(48)), None)]).unwrap();
        let m = Measured { rows: vec!["こんにちは".into(), "世界".into()], overflow: vec![false, false] };
        let v = build(&doc, &l, &[0], Ok(vec![m]));
        assert_eq!(v["track"]["size"], 54.0);
        assert_eq!(v["width"], 1500.0);
        assert!(v["speakers"]["トーク2"]["font"].is_string());
        let x = &v["lines"][0];
        assert_eq!((x["n"].clone(), x["id"].clone()), (json!(1), json!("l1")));
        assert_eq!(x["sub_text"], "こんにちは\n世界");
        assert_eq!((x["manual"].clone(), x["custom"].clone()), (json!(true), json!(false)));
        assert_eq!(x["size"], 48.0);
        assert_eq!(x["own"], json!({"text": "こんにちは\n世界", "size": 48.0}));
        assert_eq!(x["row_count"], 2);
        assert_eq!(x["overflow"], json!([false, false]));
        assert!(v.get("measure_error").is_none());
        // 触っていないセリフには own が無い
        let v = build(&doc, &l, &[1], Ok(vec![Measured { rows: vec!["長い長いセリフです".into()], overflow: vec![true] }]));
        assert!(v["lines"][0].get("own").is_none());
        assert_eq!(v["lines"][0]["overflow"], json!([true]));
    }

    #[test]
    fn build_without_measure_keeps_lines_and_says_why() {
        let doc = ProjectDoc::default();
        let v = build(&doc, &layout(), &[0, 1], Err("Python が無い".into()));
        assert_eq!(v["lines"].as_array().unwrap().len(), 2);
        assert!(v["lines"][0].get("rows").is_none() && v["lines"][0].get("overflow").is_none());
        assert_eq!(v["measure_error"], "Python が無い");
        // セリフが無い回
        let v = build(&doc, &[], &[], Ok(vec![]));
        assert_eq!(v["lines"], json!([]));
    }
}
