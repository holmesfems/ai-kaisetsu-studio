//! 台本の推敲案（Claude の提案）の解決と適用。
//!
//! Claude は行を「通し番号」（台本の頭から 1, 2, 3 …）か行の id で指す。提案を受け取った時点で id に直し、
//! そのときの文（old_text）も覚えておく。採用するときに文が変わっていたら「古い提案」として止める。
//! 直しても行の id は変わらない（声の手直しは id に付いているが、文が違えば自動で使われなくなる）。

use crate::app::error::{bad_input, not_found};
use crate::agent::{ProposalItem, ProposalLine};
use crate::script::{Block, Line, Script};
use anyhow::{anyhow, bail, Result};
use serde_json::{json, Value};

/// 通し番号つきの一覧（Claude に見せる形）。
pub fn numbered(s: &Script, layout: &[crate::voice::layout::VoiceItem]) -> Value {
    let mut n = 0;
    let mut blocks = vec![];
    for b in &s.blocks {
        let mut lines = vec![];
        for l in &b.lines {
            n += 1;
            let lay = layout.iter().find(|it| it.line_id == l.id);
            let mut o = json!({
                "n": n, "id": l.id, "sp": l.sp, "text": l.text,
                "start": lay.map(|x| (x.start * 1000.0).round() / 1000.0).or(l.start),
                "dur": lay.map(|x| (x.dur * 1000.0).round() / 1000.0),
                "voiced": lay.map(|x| x.audio.is_some()).unwrap_or(false),
            });
            if !l.memo.is_empty() {
                o["memo"] = json!(l.memo);
            }
            // アクセント・抑揚の手直しが今の文で効いているか（文を直すと効かなくなる）
            let (v, p) = (s.voice_of(l).is_some(), s.params_of(l).is_some());
            if v || p {
                o["adjusted"] = json!({"accent": v, "params": p});
            }
            lines.push(o);
        }
        blocks.push(json!({"id": b.id, "file": b.file, "title": b.title, "notes": b.notes, "lines": lines}));
    }
    json!({"rev": s.rev, "blocks": blocks, "memo": s.memo, "total_lines": n})
}

/// 行の id からメモを引く（無ければ None）
pub fn memo_of(s: &Script, line_id: &str) -> Option<String> {
    s.blocks.iter().flat_map(|b| b.lines.iter()).find(|l| l.id == line_id).map(|l| l.memo.clone())
}

fn find_by_n(s: &Script, n: usize) -> Option<(usize, usize)> {
    let mut k = 0;
    for (bi, b) in s.blocks.iter().enumerate() {
        for li in 0..b.lines.len() {
            k += 1;
            if k == n {
                return Some((bi, li));
            }
        }
    }
    None
}

fn find_by_id(s: &Script, id: &str) -> Option<(usize, usize)> {
    for (bi, b) in s.blocks.iter().enumerate() {
        if let Some(li) = b.lines.iter().position(|l| l.id == id) {
            return Some((bi, li));
        }
    }
    None
}

fn line_ref(s: &Script, v: &Value) -> Option<(usize, usize)> {
    if let Some(id) = v.as_str() {
        if let Some(r) = find_by_id(s, id) {
            return Some(r);
        }
        // 空白は削り、"#" は 1 つだけ外す（"##12" は無し）
        let t = id.trim();
        if let Ok(n) = t.strip_prefix('#').unwrap_or(t).parse::<usize>() {
            return find_by_n(s, n);
        }
    }
    v.as_u64().and_then(|n| find_by_n(s, n as usize))
}

/// セリフの指し方（行の id → "#12" → 通し番号）を、行の id にする。画面は自分では引かず、これで引いた結果を使う。
pub fn line_id(s: &Script, v: &Value) -> Option<String> {
    line_ref(s, v).map(|(bi, li)| s.blocks[bi].lines[li].id.clone())
}

/// ブロックの指し方（id・見出し・ファイル名）。文字列でなければ None
fn block_ref(s: &Script, v: &Value) -> Option<usize> {
    let k = v.as_str()?;
    s.blocks.iter().position(|b| b.id == k || b.title == k || b.file.as_deref() == Some(k))
}

/// insert・move の置き場所（after の行の後ろ、無ければ block の頭）。(after_id, block_id)
fn anchor(s: &Script, it: &Value, kind: &str) -> Result<(Option<String>, Option<String>)> {
    if it["after"].is_null() {
        let bi = block_ref(s, &it["block"]).ok_or_else(|| anyhow!("{kind} には after（行）か block が要る"))?;
        Ok((None, Some(s.blocks[bi].id.clone())))
    } else {
        let (bi, li) = line_ref(s, &it["after"]).ok_or_else(|| not_found(format!("行が見つからない: {}", it["after"])))?;
        Ok((Some(s.blocks[bi].lines[li].id.clone()), Some(s.blocks[bi].id.clone())))
    }
}

fn block_of(s: &Script, it: &Value, kind: &str) -> Result<usize> {
    if it["block"].is_null() {
        return Err(bad_input(format!("{kind} には block が要る")));
    }
    block_ref(s, &it["block"]).ok_or_else(|| not_found(format!("ブロックが見つからない: {}", it["block"])))
}

fn lines_of(v: &Value) -> Result<Vec<ProposalLine>> {
    let Some(arr) = v.as_array() else { return Ok(vec![]) };
    arr.iter()
        .map(|l| {
            let text = l["text"].as_str().unwrap_or("").to_string();
            if text.trim().is_empty() {
                return Err(bad_input("lines の text が空の行があります"));
            }
            let sp = crate::script::norm_sp(l["sp"].as_str().or(l["speaker"].as_str()).unwrap_or(""));
            Ok(ProposalLine {
                sp: if sp.is_empty() { "トーク1".into() } else { sp },
                text,
                memo: l["memo"].as_str().unwrap_or("").to_string(),
            })
        })
        .collect()
}

/// Claude から来た案（line / after は通し番号か id）を、保存できる形に直す。
pub fn resolve_items(s: &Script, items: &[Value]) -> Result<Vec<ProposalItem>> {
    let mut out = vec![];
    for it in items {
        let kind = it["kind"].as_str().unwrap_or("edit").to_string();
        let reason = it["reason"].as_str().unwrap_or("").to_string();
        let new_text = it["text"].as_str().or(it["new_text"].as_str()).unwrap_or("").to_string();
        // 話者は md を読むときと同じく揃える（talk2・2 → トーク2）。揃えないと声のキャッシュの鍵・話者ごとの字幕・立ち絵と合わない
        let sp = crate::script::norm_sp(it["sp"].as_str().or(it["speaker"].as_str()).unwrap_or(""));
        let base = ProposalItem { kind: kind.clone(), reason, status: "pending".into(), ..Default::default() };
        match kind.as_str() {
            "edit" | "delete" | "speaker" | "memo" => {
                let (bi, li) = line_ref(s, &it["line"]).ok_or_else(|| not_found(format!("行が見つからない: {}", it["line"])))?;
                let l = &s.blocks[bi].lines[li];
                if let Some(exp) = it["expect"].as_str() {
                    if exp.trim() != l.text.trim() {
                        return Err(bad_input(format!("行 {} の文が想定と違う（今: {}）。台本を読み直してから提案して", it["line"], l.text)));
                    }
                }
                if kind == "edit" && new_text.is_empty() {
                    return Err(bad_input(format!("edit には text が要る（行 {}）", it["line"])));
                }
                out.push(ProposalItem {
                    line_id: Some(l.id.clone()),
                    block_id: Some(s.blocks[bi].id.clone()),
                    // memo は「今のメモ」を old_text に覚える（文ではなく）
                    old_text: if kind == "memo" { l.memo.clone() } else { l.text.clone() },
                    new_text,
                    sp: if sp.is_empty() { l.sp.clone() } else { sp },
                    ..base
                });
            }
            "insert" => {
                let (after_id, block_id) = anchor(s, it, "insert")?;
                if new_text.is_empty() {
                    return Err(bad_input("insert には text が要る"));
                }
                out.push(ProposalItem {
                    after_id,
                    block_id,
                    new_text,
                    sp: if sp.is_empty() { "トーク1".into() } else { sp },
                    ..base
                });
            }
            "move" => {
                let (bi, li) = line_ref(s, &it["line"]).ok_or_else(|| not_found(format!("行が見つからない: {}", it["line"])))?;
                let l = &s.blocks[bi].lines[li];
                let (after_id, block_id) = anchor(s, it, "move")?;
                if after_id.as_deref() == Some(l.id.as_str()) {
                    return Err(bad_input(format!("行 {} を自分自身の後ろへは動かせません", it["line"])));
                }
                out.push(ProposalItem {
                    line_id: Some(l.id.clone()),
                    after_id,
                    block_id,
                    old_text: l.text.clone(),
                    sp: l.sp.clone(),
                    ..base
                });
            }
            "notes" => {
                let bi = block_of(s, it, "notes")?;
                let b = &s.blocks[bi];
                out.push(ProposalItem { block_id: Some(b.id.clone()), old_text: b.notes.clone(), new_text, ..base });
            }
            "block_rename" => {
                let bi = block_of(s, it, "block_rename")?;
                let title = it["title"].as_str().unwrap_or("").trim().to_string();
                if title.is_empty() {
                    return Err(bad_input("block_rename には title が要る"));
                }
                let b = &s.blocks[bi];
                out.push(ProposalItem { block_id: Some(b.id.clone()), old_text: b.title.clone(), new_text: title, ..base });
            }
            "block_delete" => {
                let bi = block_of(s, it, "block_delete")?;
                let b = &s.blocks[bi];
                let lines = b.lines.iter().map(|l| ProposalLine { sp: l.sp.clone(), text: l.text.clone(), memo: l.memo.clone() }).collect();
                out.push(ProposalItem { block_id: Some(b.id.clone()), old_text: b.title.clone(), notes: b.notes.clone(), lines, ..base });
            }
            "block_add" => {
                let title = it["title"].as_str().unwrap_or("").trim().to_string();
                if title.is_empty() {
                    return Err(bad_input("block_add には title が要る"));
                }
                let after_block = if it["after_block"].is_null() {
                    None
                } else {
                    let bi = block_ref(s, &it["after_block"]).ok_or_else(|| not_found(format!("ブロックが見つからない: {}", it["after_block"])))?;
                    Some(s.blocks[bi].id.clone())
                };
                out.push(ProposalItem {
                    block_id: Some(crate::script::new_id("b")),
                    after_block,
                    new_text: title,
                    notes: new_text,
                    lines: lines_of(&it["lines"])?,
                    ..base
                });
            }
            k => {
                return Err(bad_input(format!(
                    "知らない kind: {k}（edit / insert / delete / speaker / memo / notes / move / block_add / block_delete / block_rename）"
                )))
            }
        }
    }
    // 消すブロックを、同じ推敲案のほかの項目が触っていたら断る（採用の順で、行が消えるか削除が古い案になる）
    let deleted: Vec<&str> = out.iter().filter(|i| i.kind == "block_delete").filter_map(|i| i.block_id.as_deref()).collect();
    for (k, i) in out.iter().enumerate() {
        let line_block = |id: &Option<String>| id.as_deref().and_then(|id| find_by_id(s, id)).map(|(bi, _)| s.blocks[bi].id.as_str());
        let touched = [i.block_id.as_deref(), i.after_block.as_deref(), line_block(&i.line_id), line_block(&i.after_id)];
        let hit = touched.iter().flatten().any(|b| deleted.contains(b));
        if hit && !(i.kind == "block_delete" && deleted.iter().filter(|d| Some(**d) == i.block_id.as_deref()).count() == 1) {
            return Err(bad_input(format!("items[{k}] が、同じ推敲案で消すブロックを触っています。ブロックの削除は別の推敲案に分けてください")));
        }
    }
    // ブロックの id は回を開き直すと変わるので、ファイル名も覚えておく（改名してもファイル名は変わらない）
    let file_of = |id: &Option<String>| id.as_deref().and_then(|id| s.blocks.iter().find(|b| b.id == id)).and_then(|b| b.file.clone());
    for i in out.iter_mut() {
        i.block_file = file_of(&i.block_id);
        i.after_block_file = file_of(&i.after_block);
    }
    Ok(out)
}

/// ブロックの位置を id で、無ければ（回を開き直して id が変わった）ファイル名で引く
fn block_pos(s: &Script, id: Option<&str>, file: Option<&str>) -> Option<usize> {
    s.blocks
        .iter()
        .position(|b| Some(b.id.as_str()) == id)
        .or_else(|| file.and_then(|f| s.blocks.iter().position(|b| b.file.as_deref() == Some(f))))
}

/// insert・move を入れる場所 (ブロックの番号, 行の位置)。アンカー（after の行の直後、無ければブロックの頭）から、
/// 前の項目のうち採用済みで同じアンカーに置いた行が続く間は飛ばす（どの順で採用しても items の順に並ぶ）
fn place_at(s: &Script, it: &ProposalItem, earlier: &[ProposalItem]) -> Result<(usize, usize)> {
    let (bi, mut pos) = if let Some(a) = it.after_id.as_deref() {
        let (bi, li) = find_by_id(s, a).ok_or_else(|| anyhow!("置き先の行はもうありません"))?;
        (bi, li + 1)
    } else {
        let bi = block_pos(s, it.block_id.as_deref(), it.block_file.as_deref()).ok_or_else(|| anyhow!("そのブロックはもうありません"))?;
        (bi, 0)
    };
    let placed: Vec<&str> = earlier
        .iter()
        .filter(|e| e.status == "accepted" && e.after_id == it.after_id && (it.after_id.is_some() || e.block_id == it.block_id))
        .filter_map(|e| match e.kind.as_str() {
            "insert" => e.new_id.as_deref(),
            "move" => e.line_id.as_deref(),
            _ => None,
        })
        .collect();
    let lines = &s.blocks[bi].lines;
    while pos < lines.len() && placed.contains(&lines[pos].id.as_str()) {
        pos += 1;
    }
    Ok((bi, pos))
}

/// 1 つの案を台本に当てる。当てられなければ Err（古い提案など）。
/// `earlier` は同じ推敲案の自分より前の項目（置き場所の並びに使う）。insert を当てたら `it.new_id` に置いた行の id を入れる。
pub fn apply_item(s: &mut Script, it: &mut ProposalItem, earlier: &[ProposalItem]) -> Result<()> {
    match it.kind.as_str() {
        "edit" | "delete" | "speaker" => {
            let id = it.line_id.as_deref().ok_or_else(|| anyhow!("行の指定がありません"))?;
            let (bi, li) = find_by_id(s, id).ok_or_else(|| anyhow!("その行はもうありません"))?;
            let l = &mut s.blocks[bi].lines[li];
            if l.text.trim() != it.old_text.trim() {
                return Err(bad_input(format!("推敲案を出したあとに行が書き換わっています（今: {}）", l.text)));
            }
            match it.kind.as_str() {
                "edit" => {
                    l.text = it.new_text.clone();
                    if !it.sp.is_empty() {
                        l.sp = it.sp.clone();
                    }
                }
                "speaker" => l.sp = it.sp.clone(),
                _ => {
                    s.blocks[bi].lines.remove(li);
                }
            }
        }
        "memo" => {
            let id = it.line_id.as_deref().ok_or_else(|| anyhow!("行の指定がありません"))?;
            let (bi, li) = find_by_id(s, id).ok_or_else(|| anyhow!("その行はもうありません"))?;
            let l = &mut s.blocks[bi].lines[li];
            if l.memo != it.old_text {
                return Err(bad_input(format!("推敲案を出したあとに行のメモが書き換わっています（今: {}）", l.memo)));
            }
            l.memo = it.new_text.clone();
        }
        "notes" | "block_rename" | "block_delete" => {
            let bid = it.block_id.as_deref().ok_or_else(|| anyhow!("ブロックの指定がありません"))?;
            let bi = block_pos(s, Some(bid), it.block_file.as_deref()).ok_or_else(|| anyhow!("そのブロックはもうありません"))?;
            let b = &mut s.blocks[bi];
            match it.kind.as_str() {
                "notes" => {
                    if b.notes != it.old_text {
                        return Err(bad_input(format!("推敲案を出したあとにブロックのメモが書き換わっています（今: {}）", b.notes)));
                    }
                    b.notes = it.new_text.clone();
                }
                "block_rename" => {
                    if b.title != it.old_text {
                        return Err(bad_input(format!("推敲案を出したあとに見出しが書き換わっています（今: {}）", b.title)));
                    }
                    b.title = it.new_text.clone();
                }
                _ => {
                    // 出したあとに書いた行のメモ・ブロックの notes も、黙って消さない
                    let same = b.title == it.old_text
                        && b.notes == it.notes
                        && b.lines.len() == it.lines.len()
                        && b.lines.iter().zip(&it.lines).all(|(l, p)| l.sp == p.sp && l.text == p.text && l.memo == p.memo);
                    if !same {
                        return Err(bad_input("推敲案を出したあとにブロックの中身が書き換わっています"));
                    }
                    s.blocks.remove(bi);
                }
            }
        }
        "insert" => {
            let (bi, pos) = place_at(s, it, earlier)?;
            let id = s.fresh_id("p");
            it.new_id = Some(id.clone());
            let nl = Line { id, sp: it.sp.clone(), text: it.new_text.clone(), start: None, memo: String::new() };
            s.blocks[bi].lines.insert(pos, nl);
        }
        "move" => {
            let id = it.line_id.as_deref().ok_or_else(|| anyhow!("行の指定がありません"))?;
            let (bi, li) = find_by_id(s, id).ok_or_else(|| anyhow!("その行はもうありません"))?;
            if s.blocks[bi].lines[li].text.trim() != it.old_text.trim() {
                return Err(bad_input(format!("推敲案を出したあとに行が書き換わっています（今: {}）", s.blocks[bi].lines[li].text)));
            }
            if it.after_id.as_deref() == Some(id) {
                bail!("行を自分自身の後ろへは動かせません");
            }
            // 置き先が無いまま行だけ外さないよう、先に確かめる
            place_at(s, it, earlier)?;
            let line = s.blocks[bi].lines.remove(li);
            let (bi2, pos) = place_at(s, it, earlier)?;
            s.blocks[bi2].lines.insert(pos, line);
        }
        "block_add" => {
            let bid = it.block_id.clone().ok_or_else(|| anyhow!("ブロックの id がありません"))?;
            if s.blocks.iter().any(|b| b.id == bid) {
                bail!("そのブロックはもうあります");
            }
            let mut pos = s.blocks.len();
            if let Some(a) = it.after_block.as_deref() {
                let ai = block_pos(s, Some(a), it.after_block_file.as_deref()).ok_or_else(|| anyhow!("置き先のブロックはもうありません"))?;
                pos = ai + 1;
                let placed: Vec<&str> = earlier
                    .iter()
                    .filter(|e| e.status == "accepted" && e.kind == "block_add" && e.after_block == it.after_block)
                    .filter_map(|e| e.block_id.as_deref())
                    .collect();
                while pos < s.blocks.len() && placed.contains(&s.blocks[pos].id.as_str()) {
                    pos += 1;
                }
            }
            s.blocks.insert(pos, Block { id: bid, file: None, title: it.new_text.clone(), lines: vec![], notes: it.notes.clone() });
            for pl in &it.lines {
                let l = Line { id: s.fresh_id("p"), sp: pl.sp.clone(), text: pl.text.clone(), start: None, memo: pl.memo.clone() };
                s.blocks[pos].lines.push(l);
            }
        }
        k => bail!("不明な kind です: {k}"),
    }
    Ok(())
}

/// Claude の返事の添付のうち `line` があるものに、今の台本で引いた `line_id` を足す。引けないときは付けない
/// （画面は番号で飛ばず、押せない表示にする）。台本が無いときも付けない。
pub fn resolve_line_attachments(s: Option<&Script>, attachments: Vec<Value>) -> Vec<Value> {
    attachments
        .into_iter()
        .map(|mut a| {
            if let (Some(s), Some(obj)) = (s, a.as_object_mut()) {
                if let Some(id) = obj.get("line").and_then(|l| line_id(s, l)) {
                    obj.insert("line_id".into(), Value::String(id));
                }
            }
            a
        })
        .collect()
}

#[cfg(test)]
mod line_tests {
    use super::*;
    use crate::script::Block;

    fn script() -> Script {
        let mk = |id: &str| Line { id: id.into(), sp: "トーク1".into(), text: id.into(), start: None, memo: String::new() };
        let b = |id: &str, lines: Vec<Line>| Block { id: id.into(), file: None, title: id.into(), lines, notes: String::new() };
        Script { blocks: vec![b("b1", vec![mk("a"), mk("b")]), b("b2", vec![mk("c")])], ..Default::default() }
    }

    #[test]
    fn セリフの指し方() {
        let s = script();
        assert_eq!(line_id(&s, &json!("c")).as_deref(), Some("c"));
        assert_eq!(line_id(&s, &json!("#2")).as_deref(), Some("b"));
        assert_eq!(line_id(&s, &json!("3")).as_deref(), Some("c"));
        assert_eq!(line_id(&s, &json!(1)).as_deref(), Some("a"));
        assert_eq!(line_id(&s, &json!(" 2 ")).as_deref(), Some("b"));
        assert_eq!(line_id(&s, &json!("##2")), None);
        assert_eq!(line_id(&s, &json!(9999)), None);
        assert_eq!(line_id(&s, &json!(1.5)), None);
        assert_eq!(line_id(&s, &json!(0)), None);
    }

    #[test]
    fn 添付に行の_id_を足す() {
        let s = script();
        let out = resolve_line_attachments(Some(&s), vec![json!({"line": "#3", "label": "x"}), json!({"line": 9999}), json!({"t": 1.0})]);
        assert_eq!(out[0]["line_id"], "c");
        assert_eq!(out[0]["label"], "x");
        assert!(out[1].get("line_id").is_none());
        assert!(out[2].get("line_id").is_none());
        let none = resolve_line_attachments(None, vec![json!({"line": 1})]);
        assert!(none[0].get("line_id").is_none());
    }
}

#[cfg(test)]
mod propose_tests {
    use super::*;

    fn mk(id: &str, memo: &str) -> Line {
        Line { id: id.into(), sp: "トーク1".into(), text: id.into(), start: None, memo: memo.into() }
    }

    fn script() -> Script {
        let b = |id: &str, lines: Vec<Line>| Block { id: id.into(), file: Some(format!("{id}.yaml")), title: format!("T{id}"), lines, notes: format!("n{id}") };
        Script { blocks: vec![b("b1", vec![mk("a", "ma"), mk("b", "")]), b("b2", vec![mk("c", "")])], ..Default::default() }
    }

    fn texts(s: &Script, bi: usize) -> Vec<String> {
        s.blocks[bi].lines.iter().map(|l| l.text.clone()).collect()
    }

    fn titles(s: &Script) -> Vec<String> {
        s.blocks.iter().map(|b| b.title.clone()).collect()
    }

    /// 案を番号の順に採用する（前の項目を渡しながら）
    fn accept(s: &mut Script, items: &mut [ProposalItem], order: &[usize]) {
        for &i in order {
            let earlier = items[..i].to_vec();
            apply_item(s, &mut items[i], &earlier).unwrap();
            items[i].status = "accepted".into();
        }
    }

    #[test]
    fn メモを直す_消す_古い案() {
        let mut s = script();
        let mut it = resolve_items(&s, &[json!({"kind": "memo", "line": "a", "text": "新"}), json!({"kind": "memo", "line": 1, "text": ""})]).unwrap();
        assert_eq!(it[0].old_text, "ma");
        apply_item(&mut s, &mut it[0], &[]).unwrap();
        assert_eq!(s.blocks[0].lines[0].memo, "新");
        // 2 件目は古い（メモが ma ではなくなった）
        assert!(apply_item(&mut s, &mut it[1], &[]).is_err());
        let mut it = resolve_items(&s, &[json!({"kind": "memo", "line": "a", "text": ""})]).unwrap();
        apply_item(&mut s, &mut it[0], &[]).unwrap();
        assert_eq!(s.blocks[0].lines[0].memo, "");
    }

    #[test]
    fn ブロックのメモを直す() {
        let mut s = script();
        let mut it = resolve_items(&s, &[json!({"kind": "notes", "block": "Tb2", "text": "新"}), json!({"kind": "notes", "block": "b2", "text": ""})]).unwrap();
        apply_item(&mut s, &mut it[0], &[]).unwrap();
        assert_eq!(s.blocks[1].notes, "新");
        assert!(apply_item(&mut s, &mut it[1], &[]).is_err());
        assert!(resolve_items(&s, &[json!({"kind": "notes", "block": "無い", "text": "x"})]).is_err());
        assert!(resolve_items(&s, &[json!({"kind": "notes", "text": "x"})]).is_err());
    }

    fn inserts(after: Option<&str>, n: usize) -> Vec<Value> {
        (1..=n)
            .map(|i| match after {
                Some(a) => json!({"kind": "insert", "after": a, "text": i.to_string()}),
                None => json!({"kind": "insert", "block": "b2", "text": i.to_string()}),
            })
            .collect()
    }

    #[test]
    fn 同じ行の後ろへの_insert_は_items_の順() {
        for order in [[0, 1, 2], [1, 0, 2], [2, 1, 0], [1, 2, 0]] {
            let mut s = script();
            let mut it = resolve_items(&s, &inserts(Some("a"), 3)).unwrap();
            accept(&mut s, &mut it, &order);
            assert_eq!(texts(&s, 0), ["a", "1", "2", "3", "b"], "{order:?}");
        }
    }

    #[test]
    fn ブロックの頭への_insert_も_items_の順() {
        for order in [[0, 1], [1, 0]] {
            let mut s = script();
            let mut it = resolve_items(&s, &inserts(None, 2)).unwrap();
            accept(&mut s, &mut it, &order);
            assert_eq!(texts(&s, 1), ["1", "2", "c"], "{order:?}");
        }
    }

    #[test]
    fn 行を別のブロックへ動かす() {
        let mut s = script();
        let mut it = resolve_items(&s, &[json!({"kind": "move", "line": "a", "after": "c"})]).unwrap();
        accept(&mut s, &mut it, &[0]);
        assert_eq!(texts(&s, 0), ["b"]);
        assert_eq!(texts(&s, 1), ["c", "a"]);
        assert_eq!(s.blocks[1].lines[1].memo, "ma");
    }

    #[test]
    fn 同じ置き場所への_move_は_items_の順() {
        for order in [[0, 1], [1, 0]] {
            let mut s = script();
            let mut it = resolve_items(&s, &[json!({"kind": "move", "line": "a", "after": "c"}), json!({"kind": "move", "line": "b", "after": "c"})]).unwrap();
            accept(&mut s, &mut it, &order);
            assert_eq!(texts(&s, 1), ["c", "a", "b"], "{order:?}");
            assert!(s.blocks[0].lines.is_empty());
        }
        // ブロックの頭へ
        let mut s = script();
        let mut it = resolve_items(&s, &[json!({"kind": "move", "line": "a", "block": "b2"}), json!({"kind": "move", "line": "b", "block": "b2"})]).unwrap();
        accept(&mut s, &mut it, &[1, 0]);
        assert_eq!(texts(&s, 1), ["a", "b", "c"]);
    }

    #[test]
    fn move_は自分の後ろなら断る_文が変わったら古い() {
        let mut s = script();
        assert!(resolve_items(&s, &[json!({"kind": "move", "line": "a", "after": "a"})]).is_err());
        assert!(resolve_items(&s, &[json!({"kind": "move", "line": "a"})]).is_err());
        let mut it = resolve_items(&s, &[json!({"kind": "move", "line": "a", "after": "c"})]).unwrap();
        s.blocks[0].lines[0].text = "変".into();
        assert!(apply_item(&mut s, &mut it[0], &[]).is_err());
        assert_eq!(texts(&s, 0), ["変", "b"]);
    }

    #[test]
    fn ブロックを足す() {
        let mut s = script();
        let mut it = resolve_items(
            &s,
            &[json!({"kind": "block_add", "title": "新", "after_block": "Tb1", "text": "メモ", "lines": [{"sp": "talk2", "text": "あ", "memo": "m"}, {"text": "い"}]})],
        )
        .unwrap();
        assert_eq!(it[0].lines.len(), 2);
        assert_eq!(it[0].lines[0].sp, "トーク2");
        accept(&mut s, &mut it, &[0]);
        assert_eq!(titles(&s), ["Tb1", "新", "Tb2"]);
        let b = &s.blocks[1];
        assert_eq!((b.notes.as_str(), b.file.as_deref()), ("メモ", None));
        assert_eq!(b.lines[0].memo, "m");
        // 行の id は付いていて、重ならない
        assert_eq!(s.line_ids().len(), 5);
        assert!(resolve_items(&s, &[json!({"kind": "block_add", "text": "x"})]).is_err());
        assert!(resolve_items(&s, &[json!({"kind": "block_add", "title": "x", "lines": [{"sp": "トーク1", "text": " "}]})]).is_err());
        assert!(resolve_items(&s, &[json!({"kind": "block_add", "title": "x", "after_block": "無い"})]).is_err());
    }

    #[test]
    fn 同じブロックの後ろへの_block_add_は_items_の順_省くと最後() {
        for order in [[0, 1], [1, 0]] {
            let mut s = script();
            let mut it = resolve_items(&s, &[json!({"kind": "block_add", "title": "X", "after_block": "b1"}), json!({"kind": "block_add", "title": "Y", "after_block": "b1"})]).unwrap();
            accept(&mut s, &mut it, &order);
            assert_eq!(titles(&s), ["Tb1", "X", "Y", "Tb2"], "{order:?}");
        }
        let mut s = script();
        let mut it = resolve_items(&s, &[json!({"kind": "block_add", "title": "末"})]).unwrap();
        accept(&mut s, &mut it, &[0]);
        assert_eq!(titles(&s).last().unwrap(), "末");
    }

    #[test]
    fn ブロックを消す_中身が変わったら古い() {
        let mut s = script();
        let mut it = resolve_items(&s, &[json!({"kind": "block_delete", "block": "b1"})]).unwrap();
        assert_eq!(it[0].lines.len(), 2);
        let mut stale = s.clone();
        stale.blocks[0].lines[1].text = "変".into();
        assert!(apply_item(&mut stale, &mut it[0], &[]).is_err());
        assert_eq!(stale.blocks.len(), 2);
        apply_item(&mut s, &mut it[0], &[]).unwrap();
        assert_eq!(s.blocks.len(), 1);
        assert_eq!(s.blocks[0].id, "b2");
        // 出したあとに書いた行のメモ・ブロックの notes も古い案にする（黙って消さない）
        let s = script();
        let mut it = resolve_items(&s, &[json!({"kind": "block_delete", "block": "b1"})]).unwrap();
        let mut memo = s.clone();
        memo.blocks[0].lines[1].memo = "書いた".into();
        assert!(apply_item(&mut memo, &mut it[0], &[]).is_err());
        let mut notes = s.clone();
        notes.blocks[0].notes = "書いた".into();
        assert!(apply_item(&mut notes, &mut it[0], &[]).is_err());
    }

    #[test]
    fn 消すブロックを同じ推敲案のほかの項目が触っていたら断る() {
        let s = script();
        for other in [
            json!({"kind": "move", "line": "a", "after": "c"}),
            json!({"kind": "move", "line": "c", "after": "a"}),
            json!({"kind": "edit", "line": "b", "text": "x"}),
            json!({"kind": "memo", "line": "a", "text": "x"}),
            json!({"kind": "insert", "block": "b1", "text": "x"}),
            json!({"kind": "notes", "block": "b1", "text": "x"}),
            json!({"kind": "block_add", "title": "x", "after_block": "b1"}),
            json!({"kind": "block_delete", "block": "b1"}),
        ] {
            assert!(resolve_items(&s, &[other.clone(), json!({"kind": "block_delete", "block": "b1"})]).is_err(), "{other}");
            assert!(resolve_items(&s, &[json!({"kind": "block_delete", "block": "b1"}), other.clone()]).is_err(), "{other}");
        }
        // ほかのブロックを触るのはよい
        assert!(resolve_items(&s, &[json!({"kind": "edit", "line": "c", "text": "x"}), json!({"kind": "block_delete", "block": "b1"})]).is_ok());
    }

    #[test]
    fn 回を開き直してブロックの_id_が変わってもファイル名で引ける() {
        let s = script();
        let mut it = resolve_items(
            &s,
            &[
                json!({"kind": "notes", "block": "b2", "text": "新"}),
                json!({"kind": "insert", "block": "b2", "text": "頭"}),
                json!({"kind": "block_add", "title": "X", "after_block": "b1"}),
            ],
        )
        .unwrap();
        assert_eq!(it[0].block_file.as_deref(), Some("b2.yaml"));
        assert_eq!(it[2].after_block_file.as_deref(), Some("b1.yaml"));
        // 読み直すとブロックの id が作り直される
        let mut s2 = s.clone();
        for (i, b) in s2.blocks.iter_mut().enumerate() {
            b.id = format!("new{i}");
        }
        accept(&mut s2, &mut it, &[0, 1, 2]);
        assert_eq!(s2.blocks[2].notes, "新");
        assert_eq!(texts(&s2, 2), ["頭", "c"]);
        assert_eq!(titles(&s2), ["Tb1", "X", "Tb2"]);
    }

    #[test]
    fn ブロックの改名はファイル名を変えない() {
        let mut s = script();
        let mut it = resolve_items(&s, &[json!({"kind": "block_rename", "block": "b1", "title": "新題"})]).unwrap();
        apply_item(&mut s, &mut it[0], &[]).unwrap();
        assert_eq!(s.blocks[0].title, "新題");
        assert_eq!(s.blocks[0].file.as_deref(), Some("b1.yaml"));
        assert!(apply_item(&mut s, &mut it[0], &[]).is_err());
        assert!(resolve_items(&s, &[json!({"kind": "block_rename", "block": "b1"})]).is_err());
    }

    #[test]
    fn 知らない_kind_は断る() {
        assert!(resolve_items(&script(), &[json!({"kind": "zzz"})]).is_err());
    }

    /// 保存まで: block_add で新しい .yaml と _order.json、block_delete でファイルが消えて控えが残る
    #[test]
    fn 保存まで通る() {
        let dir = std::env::temp_dir().join(format!("aks-propose-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("1.yaml"), "title: A\nlines:\n  - id: l1\n    sp: トーク1\n    text: あ\n").unwrap();
        std::fs::write(dir.join("2.yaml"), "title: B\nlines:\n  - id: l2\n    sp: トーク1\n    text: い\n").unwrap();
        let mut st = crate::script::ScriptStore::open(&dir).unwrap();
        let mut s = st.script.clone();
        let mut it = resolve_items(&s, &[json!({"kind": "block_add", "title": "新ブロック", "after_block": "A", "lines": [{"sp": "トーク2", "text": "う"}]})]).unwrap();
        accept(&mut s, &mut it, &[0]);
        st.save(s).unwrap();
        let order: Vec<String> = serde_json::from_str(&std::fs::read_to_string(dir.join("_order.json")).unwrap()).unwrap();
        assert_eq!(order, ["1.yaml", "新ブロック.yaml", "2.yaml"]);
        assert!(std::fs::read_to_string(dir.join("新ブロック.yaml")).unwrap().contains("text: う"));

        let mut s = st.script.clone();
        let mut it = resolve_items(&s, &[json!({"kind": "block_delete", "block": "B"})]).unwrap();
        accept(&mut s, &mut it, &[0]);
        st.save(s).unwrap();
        assert!(!dir.join("2.yaml").exists());
        let found = std::fs::read_dir(dir.join("_backup")).unwrap().flatten().any(|e| e.path().join("2.yaml").exists());
        assert!(found, "控えが残っていない");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
