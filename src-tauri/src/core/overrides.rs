//! 微調整（overrides）のマージ・掃除・ネストの印。

use crate::app::error::{bad_input};
use crate::render::cache::group_span;
use crate::render::ease;
use super::Core;
use anyhow::{bail, Result};
use serde_json::{json, Value};
use std::path::Path;

impl Core {
    // ------------------------------------------------------------------ 演出の微調整
    pub fn overrides(&self) -> Value {
        self.st.read().as_ref().map(|p| p.overrides.clone()).unwrap_or_else(|| json!({}))
    }

    pub fn set_overrides(&self, ov: Value) -> Result<()> {
        let _one = self.ov_lock.lock();
        self.set_overrides_locked(ov)
    }

    fn set_overrides_locked(&self, ov: Value) -> Result<()> {
        let ov = clean_overrides(ov);
        // 録画を差し込んだ RecWindow の仮表示を消す印は、いつも project.json のネストから決める
        let ov = match self.doc() {
            Some(doc) => nest_flags(ov, &doc),
            None => ov,
        };
        {
            let mut st = self.st.write();
            let ps = st.as_mut().ok_or_else(crate::app::error::no_project)?;
            let p = ps.project.overrides_path();
            self.mark_self_write(&p);
            ps.project.save_overrides(&ov)?;
            ps.overrides = ov.clone();
        }
        // 見比べの間は、ワーカーに変更前の微調整を持たせたまま（終えるときに今のものを渡す）
        if self.preview.read().is_none() {
            self.engine.set_overrides(&ov);
        }
        self.emit("overrides", ov);
        Ok(())
    }

    pub fn patch_override(&self, key: &str, patch: &Value) -> Result<Value> {
        let _one = self.ov_lock.lock();
        let ov = merge_override(&self.overrides(), key, patch);
        self.set_overrides_locked(ov)?;
        Ok(self.overrides().get(key).cloned().unwrap_or(Value::Null))
    }

    /// project.json の nests と微調整を、同じ鍵の中で読んで書き換える（cue 番号の付け替え。旧#38）。
    /// 微調整は nest_flags を 1 回だけ通して保存するので、途中の状態をファイル・画面に出さない。
    /// 保存は project.json → studio_overrides.json の順。どこで失敗したかはエラーの文に書く。f が Err なら何も変えない
    pub fn update_doc_and_overrides<T>(
        &self, root: &Path,
        f: impl FnOnce(&crate::project::Project, &mut crate::project::ProjectDoc, &mut serde_json::Map<String, Value>) -> Result<T>,
    ) -> Result<T> {
        let _one = self.ov_lock.lock();
        let mut ov = self.overrides().as_object().cloned().unwrap_or_default();
        let ov0 = ov.clone();
        // doc は読むところから保存まで書き込みの鍵を持つ（画面の set_doc と入れ違わないように）。f が Err なら何も変えない
        let (r, doc_v) = {
            let mut st = self.st.write();
            let ps = st.as_mut().ok_or_else(crate::app::error::no_project)?;
            if ps.project.root != root {
                bail!("別の回が開かれました");
            }
            let mut doc = ps.project.doc.clone();
            let doc0 = serde_json::to_value(&doc)?;
            let r = f(&ps.project, &mut doc, &mut ov)?;
            let doc_v = serde_json::to_value(&doc)?;
            if doc_v == doc0 {
                (r, None)
            } else {
                let prev = std::mem::replace(&mut ps.project.doc, doc);
                match ps.project.save() {
                    Ok(n) => self.notice_external_save(n),
                    Err(e) => {
                        ps.project.doc = prev;
                        return Err(e.context("差し込んだ録画（project.json）を保存できませんでした（調整と録画のファイルは変えていません）"));
                    }
                }
                (r, Some(doc_v))
            }
        };
        let doc_changed = doc_v.is_some();
        if let Some(v) = doc_v {
            self.emit("doc", v);
        }
        if ov != ov0 || doc_changed {
            self.set_overrides_locked(Value::Object(ov)).map_err(|e| {
                let done = if doc_changed { "差し込んだ録画は付け替えましたが、" } else { "" };
                e.context(format!("{done}演出の調整（studio_overrides.json）を保存できませんでした"))
            })?;
        }
        Ok(r)
    }

    pub fn set_gui_clips(&self, v: Value) -> Result<()> {
        check_gui_clips(&v)?;
        {
            let mut st = self.st.write();
            let ps = st.as_mut().ok_or_else(crate::app::error::no_project)?;
            let p = ps.project.clips_path();
            self.mark_self_write(&p);
            ps.project.save_gui_clips(&v)?;
            ps.gui_clips = v.clone();
        }
        self.emit("gui_clips", v);
        self.reload_fx();
        Ok(())
    }

    pub fn set_doc(&self, doc: crate::project::ProjectDoc) -> Result<()> {
        let gaps_changed = self.doc().is_some_and(|d| d.gap != doc.gap || d.block_gap != doc.block_gap);
        self.set_doc_inner(doc)?;
        // 間隔が変わったら並びが変わる
        if gaps_changed {
            self.refresh_voice(false);
        }
        Ok(())
    }

    fn set_doc_inner(&self, doc: crate::project::ProjectDoc) -> Result<()> {
        let placeholder_changed;
        let timeline_changed;
        {
            let mut st = self.st.write();
            let ps = st.as_mut().ok_or_else(crate::app::error::no_project)?;
            placeholder_changed = ps.project.doc.placeholder != doc.placeholder;
            timeline_changed = ps.project.doc.fx_timeline != doc.fx_timeline;
            ps.project.doc = doc;
            let n = ps.project.save()?;
            self.notice_external_save(n);
        }
        if placeholder_changed || timeline_changed {
            self.cache.clear();
            self.reload_fx();
        }
        self.sync_nest_flags()?;
        Ok(())
    }

    /// 外（Claude）から変えたとき: 保存して、画面にも新しい doc を送る。
    pub fn set_doc_and_notify(&self, doc: crate::project::ProjectDoc) -> Result<()> {
        let v = serde_json::to_value(&doc)?;
        self.set_doc(doc)?;
        self.emit("doc", v);
        Ok(())
    }

    /// ネストに素材が入っている区間を、RecWindow の微調整（no_placeholder）に書く（変わったときだけ）。
    pub fn sync_nest_flags(&self) -> Result<()> {
        let Some(doc) = self.doc() else { return Ok(()) };
        let ov = self.overrides();
        if nest_flags(ov.clone(), &doc) != ov {
            self.set_overrides(ov)?;
        }
        Ok(())
    }

}

/// 微調整 ov の key に patch を重ねる（項目が null なら外す。patch が null なら key ごと外す）。clean_overrides の前の形。
/// 画面の mergeOverride と見本 ui_rules.json で縛っている（app/rules_fixture.rs）。オブジェクトでない patch は何もしない
pub fn merge_override(ov: &Value, key: &str, patch: &Value) -> Value {
    let mut all = ov.as_object().cloned().unwrap_or_default();
    let mut merged = all.get(key).and_then(Value::as_object).cloned().unwrap_or_default();
    if patch.is_null() {
        merged.clear();
    } else if let Some(o) = patch.as_object() {
        for (k, v) in o {
            if v.is_null() {
                merged.remove(k);
            } else {
                merged.insert(k.clone(), v.clone());
            }
        }
    }
    all.insert(key.to_string(), Value::Object(merged));
    Value::Object(all)
}

/// 空の微調整（全部が既定値）を消す。画面の cleanOverrides と見本 ui_rules.json で縛っている（app/rules_fixture.rs）。
pub fn clean_overrides(ov: Value) -> Value {
    let mut out = serde_json::Map::new();
    if let Some(o) = ov.as_object() {
        for (k, v) in o {
            let Some(m) = v.as_object() else { continue };
            let mut m2 = serde_json::Map::new();
            for (kk, vv) in m {
                let default = match kk.as_str() {
                    "shift" | "end" | "dx" | "dy" => vv.as_f64().map(|x| x.abs() < 1e-6).unwrap_or(false),
                    "scale" | "opacity" => vv.as_f64().map(|x| (x - 1.0).abs() < 1e-6).unwrap_or(false),
                    "hidden" => vv.as_bool() == Some(false),
                    _ => vv.is_null(),
                };
                if !default {
                    m2.insert(kk.clone(), vv.clone());
                }
            }
            // pivot だけ残っても意味がない
            let meaningful = m2.keys().any(|k| k != "pivot" && k != "note");
            if meaningful || m2.contains_key("note") {
                out.insert(k.clone(), Value::Object(m2));
            }
        }
    }
    Value::Object(out)
}

/// 元の窓 base に微調整 o（倍率 scale・中心 pivot・移動 dx dy）を入れた窓。画面の windowBoxOf と見本 ui_rules.json で縛っている。
/// pivot は 2 つとも数のときだけ使い、そうでなければ元の窓の中心。数でない scale・dx・dy は既定（1・0・0）
pub fn window_box(base: [f64; 4], o: Option<&Value>) -> [f64; 4] {
    let num = |k: &str, d: f64| o.and_then(|o| o.get(k)).and_then(|v| v.as_f64()).unwrap_or(d);
    let s = num("scale", 1.0);
    let (dx, dy) = (num("dx", 0.0), num("dy", 0.0));
    let pv = o
        .and_then(|o| o.get("pivot"))
        .and_then(|p| p.as_array())
        .and_then(|p| Some((p.first()?.as_f64()?, p.get(1)?.as_f64()?)))
        .unwrap_or(((base[0] + base[2]) / 2.0, (base[1] + base[3]) / 2.0));
    [
        pv.0 + (base[0] - pv.0) * s + dx,
        pv.1 + (base[1] - pv.1) * s + dy,
        pv.0 + (base[2] - pv.0) * s + dx,
        pv.1 + (base[3] - pv.1) * s + dy,
    ]
}

/// RecWindow の窓（微調整のずらし・移動・拡大を入れた後、1920x1080 の座標）と、入り・抜けの濃さ。見えていなければ None。
/// 入り・抜けの長さと緩急は RecWindow が決め、ワーカーが desc の rec.fade で返す（古い fxparts の分もワーカーが補う）。
/// ここは数値を持たず、緩急は render/ease.rs の表で引く（その表は fxkit.EASES と見本 ease.json で縛っている）。
pub fn rec_window(g: &crate::render::engine::Group, nest: &crate::project::Nest, ov: &Value, t: f64) -> Option<([f64; 4], f64)> {
    let (a, b, hid) = group_span(g, ov);
    if hid || t < a || t >= b {
        return None;
    }
    let o = ov.get(&g.key);
    let base = g.rec.first().map(|r| r.bx).unwrap_or(nest.bx);
    let bx = window_box(base, o);
    // 入り・抜けは RecWindow が持つ値（ワーカーが desc の rec.fade で返す）。無ければ入り・抜けなし。
    // 緩急の名前が分からないときは、その側だけ ease_out（入り）・ease_in_out（抜け）で代える（rec_fade_warnings が警告に出す）
    let mut k = 1.0;
    if let Some(f) = g.rec.first().and_then(|r| r.fade.as_ref()) {
        let ein = ease::by_name(&f.in_ease).unwrap_or(ease::ease_out);
        let eout = ease::by_name(&f.out_ease).unwrap_or(ease::ease_in_out);
        if f.in_ > 0.0 {
            k = ein((t - a) / f.in_);
        }
        if f.out > 0.0 && t >= b - f.out {
            k *= 1.0 - eout((t - (b - f.out)) / f.out);
        }
    }
    let opacity = o.and_then(|o| o.get("opacity")).and_then(|v| v.as_f64()).unwrap_or(1.0);
    Some((bx, k * opacity.clamp(0.0, 1.0)))
}

/// RecWindow の入り・抜けの緩急の名前が render/ease.rs の表に無い窓の警告（回の fx/ で緩急を足したときなど。
/// 窓の枠は Python の緩急で出るが、差し込んだ録画は代わりの緩急になる）
pub fn rec_fade_warnings(desc: &crate::render::engine::Desc) -> Vec<crate::render::engine::Warning> {
    let mut out = vec![];
    for g in &desc.groups {
        let Some(f) = g.rec.first().and_then(|r| r.fade.as_ref()) else { continue };
        let bad: Vec<&str> = [f.in_ease.as_str(), f.out_ease.as_str()].into_iter().filter(|n| ease::by_name(n).is_none()).collect();
        if !bad.is_empty() {
            out.push(crate::render::engine::Warning {
                key: format!("rec-fade-ease:{}", g.key),
                message: format!("{} の入り・抜けの緩急（{}）を AK Studio が知らないので、差し込んだ録画は ease_out・ease_in_out で出します",
                                 g.key, bad.join("・")),
                level: None,
            });
        }
    }
    out
}

/// 微調整に「録画が入っている区間では仮表示を出さない」印（no_placeholder: [[開始, 終わり], …]）を、ネストの中身から付け直す。
/// 中身が同じなら触らない（コマのキャッシュの鍵が変わらないように）。
pub fn nest_flags(ov: Value, doc: &crate::project::ProjectDoc) -> Value {
    let mut m = match ov {
        Value::Object(m) => m,
        _ => serde_json::Map::new(),
    };
    let mut keys: Vec<String> = m
        .iter()
        .filter(|(_, v)| v.get("no_placeholder").is_some())
        .map(|(k, _)| k.clone())
        .collect();
    keys.extend(doc.nests.keys().cloned());
    keys.sort();
    keys.dedup();
    for k in keys {
        let want: Option<Value> = doc.nests.get(&k).map(|n| n.covered()).filter(|r| !r.is_empty()).map(|r| {
            Value::Array(
                r.iter()
                    .map(|x| json!([(x[0] * 1000.0).round() / 1000.0, (x[1] * 1000.0).round() / 1000.0]))
                    .collect(),
            )
        });
        let have = m.get(&k).and_then(|o| o.get("no_placeholder")).cloned();
        if have == want {
            continue;
        }
        match want {
            Some(w) => {
                let e = m.entry(k.clone()).or_insert_with(|| json!({}));
                if !e.is_object() {
                    *e = json!({});
                }
                e.as_object_mut().unwrap().insert("no_placeholder".into(), w);
            }
            None => {
                if let Some(o) = m.get_mut(&k).and_then(|o| o.as_object_mut()) {
                    o.remove("no_placeholder");
                    if o.is_empty() {
                        m.remove(&k);
                    }
                }
            }
        }
    }
    Value::Object(m)
}


/// 画面で追加した演出（studio_clips.json）の形: オブジェクトの配列。違うものは保存も通知もしない
fn check_gui_clips(v: &Value) -> Result<()> {
    match v.as_array() {
        Some(a) if a.iter().all(|x| x.is_object()) => Ok(()),
        _ => return Err(bad_input(format!("画面で追加した演出は配列で渡してください"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::{MediaClip, Nest, ProjectDoc, Track};

    #[test]
    fn gui_clips_must_be_array_of_objects() {
        assert!(check_gui_clips(&json!([])).is_ok());
        assert!(check_gui_clips(&json!([{"a": 1}, {}])).is_ok());
        assert!(check_gui_clips(&json!({})).is_err());
        assert!(check_gui_clips(&json!([1])).is_err());
        assert!(check_gui_clips(&json!([{"a": 1}, null])).is_err());
        assert!(check_gui_clips(&Value::Null).is_err());
    }

    #[test]
    fn nest_flags_follow_nest_clips() {
        let mut doc = ProjectDoc::default();
        let mut n = Nest::default();
        n.tracks.push(Track {
            clips: vec![MediaClip { src: "r.mp4".into(), start: 10.0, dur: 4.0, ..Default::default() }],
            ..Default::default()
        });
        doc.nests.insert("RecWindow@c70#1".into(), n);
        let ov = nest_flags(json!({"RecWindow@c70#1": {"dx": 5}, "RecWindow@c18#1": {"no_placeholder": [[1, 2]]}}), &doc);
        assert_eq!(ov["RecWindow@c70#1"]["no_placeholder"], json!([[10.0, 14.0]]));
        assert_eq!(ov["RecWindow@c70#1"]["dx"], json!(5));
        // ネストが無くなった窓の印は消える（ほかの微調整が無ければキーごと）
        assert!(ov.get("RecWindow@c18#1").is_none());
        // 2 回目は変わらない
        assert_eq!(nest_flags(ov.clone(), &doc), ov);
        doc.nests.get_mut("RecWindow@c70#1").unwrap().tracks[0].clips.clear();
        let ov2 = nest_flags(ov, &doc);
        assert_eq!(ov2, json!({"RecWindow@c70#1": {"dx": 5}}));
    }

    fn rec_group(fade: Option<crate::render::engine::RecFade>) -> crate::render::engine::Group {
        use crate::render::engine::{Group, RecInfo};
        Group {
            key: "RecWindow@c1#1".into(),
            label: "RecWindow".into(),
            func: None,
            line: 1,
            src: String::new(),
            t0: 10.0,
            t1: 14.0,
            n: 1,
            kinds: vec![],
            z: 0.0,
            fp: String::new(),
            gui: false,
            se: vec![],
            bbox: None,
            rec: vec![RecInfo { bx: [100.0, 100.0, 500.0, 400.0], segs: vec![], t1: Some(14.0), fade }],
        }
    }

    #[test]
    fn rec_window_follows_fade_from_worker() {
        use crate::render::engine::RecFade;
        let fade = RecFade { in_: 0.35, in_ease: "ease_out".into(), out: 0.25, out_ease: "ease_in_out".into() };
        let g = rec_group(Some(fade));
        let nest = Nest::default();
        let ov = json!({});
        let k = |t: f64| rec_window(&g, &nest, &ov, t).map(|r| r.1);
        // 入りの途中（窓の頭 + 0.1 秒）と、出きった後
        assert!((k(10.1).unwrap() - ease::ease_out(0.1 / 0.35)).abs() < 1e-12);
        assert_eq!(k(10.5).unwrap(), 1.0);
        // 抜けの途中（窓の終わりは t1 + 0.25 秒ではなく group の t1。その 0.1 秒手前）
        assert!((k(13.9).unwrap() - (1.0 - ease::ease_in_out(0.15 / 0.25))).abs() < 1e-12);
        assert!(k(9.9).is_none() && k(14.0).is_none());
    }

    #[test]
    fn unknown_ease_is_warned() {
        use crate::render::engine::{Desc, RecFade};
        let ok = RecFade { in_: 0.35, in_ease: "ease_out".into(), out: 0.25, out_ease: "ease_in_out".into() };
        let bad = RecFade { in_: 0.35, in_ease: "ease_bounce".into(), out: 0.25, out_ease: "ease_in_out".into() };
        let d: Desc = serde_json::from_value(json!({"path": "a.py", "end": 1.0, "fps": 60.0, "w": 1920, "h": 1080,
            "groups": [], "cues": [], "sync": [], "sync_used": [], "meta": {}, "build_sec": 0.0, "stage": [], "rec_box": []})).unwrap();
        let mut d = d;
        d.groups = vec![rec_group(Some(ok)), { let mut g = rec_group(Some(bad)); g.key = "RecWindow@c2#1".into(); g }, rec_group(None)];
        let w = rec_fade_warnings(&d);
        assert_eq!(w.len(), 1);
        assert_eq!(w[0].key, "rec-fade-ease:RecWindow@c2#1");
        assert!(w[0].message.contains("ease_bounce"));
    }

    #[test]
    fn rec_window_without_fade_is_always_opaque() {
        use crate::render::engine::RecFade;
        let nest = Nest::default();
        let ov = json!({});
        // fade が無い（古いワーカー）
        let g = rec_group(None);
        assert_eq!(rec_window(&g, &nest, &ov, 10.0).unwrap().1, 1.0);
        assert_eq!(rec_window(&g, &nest, &ov, 13.95).unwrap().1, 1.0);
        // 緩急の名前が分からない側だけ代わりの緩急（入りは ease_out）。抜けはそのまま効く
        let bad = RecFade { in_: 0.35, in_ease: "ease_bounce".into(), out: 0.25, out_ease: "ease_in_out".into() };
        let gb = rec_group(Some(bad));
        assert!((rec_window(&gb, &nest, &ov, 10.1).unwrap().1 - ease::ease_out(0.1 / 0.35)).abs() < 1e-12);
        assert!(rec_window(&gb, &nest, &ov, 13.95).unwrap().1 < 0.5);
        // 微調整の opacity は掛かる
        assert_eq!(rec_window(&g, &nest, &json!({"RecWindow@c1#1": {"opacity": 0.5}}), 11.0).unwrap().1, 0.5);
    }
}
