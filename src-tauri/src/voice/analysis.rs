//! 声の解析（VoiSona が返す長さ・読み・音素）のキャッシュと、中継への問い合わせ。
//! 「今の手直しで作った音声か」の署名（sig）もここで持つ。

use crate::script::format::{Line, Script};
use crate::voice::key::{tts_key_text, vkey};
use crate::voice::layout::voiced_lines;
use crate::voice::Voice;
use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::time::Duration;
use ts_rs::TS;

/// 古い中継（話者の声の変更を知らない）を使っているときの知らせ
pub const OLD_SERVER: &str =
    "VoiSona の中継（voice_server.py）が古いため、話者の声の設定を使えません。古い中継（台本エディタのフォルダから起動したもの）を閉じてから、AK Studio で中継を再起動してください。";

/// 1 行ぶんの声の解析（中継の返す項目のうち、画面が使うもの）
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, TS)]
#[ts(export)]
pub struct Analysis {
    /// 音声のキャッシュの id（voice_cache/<id>.wav）
    pub id: String,
    /// 音声の長さ（秒）
    pub duration: f64,
    #[serde(default)]
    pub unread: Vec<String>,
    #[serde(default)]
    pub analyzed_text: String,
    #[serde(default)]
    pub phonemes: Option<Vec<String>>,
    #[serde(default)]
    pub phoneme_durations: Option<Vec<f64>>,
    #[serde(default)]
    #[ts(type = "Record<string, any> | null")]
    pub speaker_params: Option<Value>,
}

/// その話者の声の変更（無ければ None）
fn sp_voice<'a>(s: &'a Script, sp: &str) -> Option<&'a Value> {
    s.speakers.as_ref().and_then(|m| m.get(sp)).filter(|v| v.as_object().map_or(false, |o| !o.is_empty()))
}

/// 読み上げ辞書（`voice_dict.json` の `entries`。項目の中身は中継が決めるので Value のまま持つ）
#[derive(Default, Clone, Debug, PartialEq)]
pub struct VoiceDict {
    pub entries: Vec<Value>,
}

impl VoiceDict {
    /// 辞書を読む。ファイルが無ければ空。壊れている・`entries` が配列でないときは Err（空として扱わない）
    pub fn load(path: &std::path::Path) -> Result<VoiceDict> {
        let text = match std::fs::read_to_string(path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(VoiceDict::default()),
            Err(e) => return Err(anyhow!(e).context(format!("{} を読めません", path.display()))),
        };
        let v: Value = serde_json::from_str(text.trim_start_matches('\u{feff}'))
            .map_err(|e| anyhow!("{} が JSON として読めません（書きかけかもしれません）: {e}", path.display()))?;
        match v.get("entries") {
            Some(Value::Array(a)) => Ok(VoiceDict { entries: a.clone() }),
            _ => Err(anyhow!("{} の entries が配列ではありません", path.display())),
        }
    }

    /// その文に関わる項目（中継の `vt.relevant` と同じ。表記が文に含まれ、読みがあるもの。ファイルの順）
    pub fn relevant(&self, text: &str) -> Vec<&Value> {
        self.entries
            .iter()
            .filter(|e| {
                let surface = e.get("surface").and_then(Value::as_str).unwrap_or("");
                let reading = match e.get("reading") {
                    None | Some(Value::Null) | Some(Value::Bool(false)) => false,
                    Some(Value::String(s)) => !s.is_empty(),
                    Some(_) => true,
                };
                !surface.is_empty() && reading && text.contains(surface)
            })
            .collect()
    }
}

/// 「今の手直しで作った音声か」の署名。話者の声を変えていない行は [アクセント, 抑揚] だけ。
/// その行に関わる辞書の項目があれば、最後に {"dict": […]} を足す（辞書を変えたら作り直す対象にするため）
pub fn sig(s: &Script, l: &Line, d: &VoiceDict) -> String {
    let v = s.voice_of(l);
    let p = s.params_of(l);
    let mut a = match sp_voice(s, &l.sp) {
        Some(sv) => vec![json!(v), json!(p), sv.clone()],
        None => vec![json!(v), json!(p)],
    };
    // ルビの表記は読み上げの文に無いので、中継と同じく読み上げの文で見る
    let rel = d.relevant(&tts_key_text(&l.text));
    if !rel.is_empty() {
        a.push(json!({ "dict": rel }));
    }
    Value::Array(a).to_string()
}

/// 解析の入れ物（鍵は vkey）。解析ごとに、作ったときの sig を持つ
#[derive(Default, Clone, Debug)]
pub struct Analyses {
    map: HashMap<String, (Analysis, String)>,
    /// どの辞書で確かめた解析か（fresh・lookup の署名はこれで作る）
    dict: VoiceDict,
}

impl Analyses {
    pub fn dict(&self) -> &VoiceDict {
        &self.dict
    }

    /// 辞書を差し替える。変わったら true
    pub fn set_dict(&mut self, d: VoiceDict) -> bool {
        if self.dict == d {
            return false;
        }
        self.dict = d;
        true
    }

    pub fn get(&self, key: &str) -> Option<&Analysis> {
        self.map.get(key).map(|(a, _)| a)
    }

    pub fn insert(&mut self, key: String, a: Analysis, sig: String) {
        self.map.insert(key, (a, sig));
    }

    pub fn remove(&mut self, key: &str) -> Option<Analysis> {
        self.map.remove(key).map(|(a, _)| a)
    }

    /// other の keys にある解析（と、そのときの sig）をこちらへ写す
    pub fn merge_from(&mut self, other: &Analyses, keys: &[String]) {
        for k in keys {
            if let Some(e) = other.map.get(k) {
                self.map.insert(k.clone(), e.clone());
            }
        }
    }

    /// 解析だけを消す（辞書は残す）
    pub fn clear(&mut self) {
        self.map.clear();
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = (&String, &Analysis)> {
        self.map.iter().map(|(k, (a, _))| (k, a))
    }

    /// 今の手直しで作った音声か（鍵があり、sig が今と同じ）
    pub fn fresh(&self, s: &Script, l: &Line) -> bool {
        self.map.get(&vkey(&l.sp, &l.text)).map_or(false, |(_, g)| *g == sig(s, l, &self.dict))
    }

    /// 音声の長さ（秒）。解析が無い・有限でないときは None
    pub fn duration(&self, l: &Line) -> Option<f64> {
        self.get(&vkey(&l.sp, &l.text)).map(|a| a.duration).filter(|d| d.is_finite())
    }
}

/// 中継に渡す 1 行分（話者の声の変更も送る。送った値で作るので、保存を待たなくてよい）
pub fn req(s: &Script, l: &Line) -> Value {
    json!({
        "speaker": l.sp,
        "text": tts_key_text(&l.text),
        "analyzed_text": s.voice_of(l),
        "params": s.params_of(l),
        "speaker_params": sp_voice(s, &l.sp).cloned().unwrap_or_else(|| json!({})),
    })
}

/// lookup の結果（知らせは呼んだ側が出す）
#[derive(Default, Debug)]
pub struct LookupResult {
    /// 受け取れなかった行の数（前の解析がある行は数えない）
    pub missing: usize,
    /// 古い中継だった（話者の声を変えた行の解析は入れていない）
    pub old_server: bool,
    /// 入れた・差し替えた解析の鍵
    pub changed: Vec<String>,
}

fn is_null(v: &Option<Value>) -> bool {
    v.as_ref().map_or(true, |v| v.is_null())
}

/// 今の手直しで作った音声がまだ手元に無い行（話者の声を変えた後の行も。前に作った値へ戻したならキャッシュにある）を
/// 中継に問い合わせて入れる。問い合わせ自体の失敗は Err（解析は変えない）
pub fn lookup(voice: &Voice, s: &Script, analyses: &mut Analyses) -> Result<LookupResult> {
    let mut out = LookupResult::default();
    let lines: Vec<&Line> = voiced_lines(s).into_iter().map(|x| x.l).filter(|l| !analyses.fresh(s, l)).collect();
    if lines.is_empty() {
        return Ok(out);
    }
    let body = json!({ "items": lines.iter().map(|l| req(s, l)).collect::<Vec<_>>() });
    let res = voice.post_timeout("/api/lookup", &body, Duration::from_secs(10))?;
    let results = res["results"].as_array().ok_or_else(|| anyhow!("VoiSona の中継の返事に results がありません"))?;
    for (i, l) in lines.iter().enumerate() {
        let r = match results.get(i) {
            Some(v) if !v.is_null() => Some(serde_json::from_value::<Analysis>(v.clone())?),
            _ => None,
        };
        let key = vkey(&l.sp, &l.text);
        match r {
            // 古い voice_server は話者の声の変更を知らずに、変更なしの声を返す。それを今の声として使わない
            Some(r) if sp_voice(s, &l.sp).is_some() && is_null(&r.speaker_params) => out.old_server = true,
            Some(r) => {
                let g = sig(s, l, analyses.dict());
                analyses.insert(key.clone(), r, g);
                out.changed.push(key);
            }
            // 調整前の音声がある行は、作り直すまでその長さで並べる
            None if analyses.get(&key).is_none() => out.missing += 1,
            None => {}
        }
    }
    Ok(out)
}

/// 作った音声（呼んだ側が analyses に入れる。中継を待つあいだロックを持たなくてよいように、入れるのは別）
#[derive(Debug)]
pub struct Synthed {
    pub key: String,
    pub sig: String,
    pub analysis: Analysis,
}

/// 1 行の音声を作る（台本の中に無い行は None）。古い中継なら OLD_SERVER のエラー
pub fn synth(voice: &Voice, s: &Script, line_id: &str, d: &VoiceDict) -> Result<Option<Synthed>> {
    let Some(l) = voiced_lines(s).into_iter().map(|x| x.l).find(|l| l.id == line_id) else {
        return Ok(None);
    };
    let r = voice.post("/api/synth", &req(s, l))?;
    let a: Analysis = serde_json::from_value(r)?;
    if sp_voice(s, &l.sp).is_some() && is_null(&a.speaker_params) {
        return Err(anyhow!(OLD_SERVER));
    }
    Ok(Some(Synthed { key: vkey(&l.sp, &l.text), sig: sig(s, l, d), analysis: a }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::script::format::Block;

    fn line(text: &str) -> Line {
        Line { id: "l1".into(), sp: "トーク1".into(), text: text.into(), start: None, memo: String::new() }
    }

    fn script(l: Line) -> Script {
        Script {
            blocks: vec![Block { id: "b".into(), file: None, title: "A".into(), lines: vec![l], notes: String::new() }],
            ..Default::default()
        }
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

    #[test]
    fn sig_changes_with_adjustments() {
        let mut s = script(line("こんにちは"));
        let l = s.blocks[0].lines[0].clone();
        let mut a = Analyses::default();
        let g = sig(&s, &l, a.dict());
        a.insert(vkey(&l.sp, &l.text), an("x", 1.0), g);
        assert!(a.fresh(&s, &l));
        // アクセントの手直しを足すと fresh でなくなる
        s.voice.insert("l1".into(), json!({"sp": "トーク1", "text": "こんにちは", "analyzed_text": "<xml/>"}));
        assert!(!a.fresh(&s, &l));
        // 戻すとまた fresh
        s.voice.remove("l1");
        assert!(a.fresh(&s, &l));
        // 話者の声を変えると fresh でなくなる
        let mut sp = serde_json::Map::new();
        sp.insert("トーク1".into(), json!({"alpha": 0.5}));
        s.speakers = Some(sp);
        assert!(!a.fresh(&s, &l));
        // 空の変更は変えていないのと同じ
        let mut sp = serde_json::Map::new();
        sp.insert("トーク1".into(), json!({}));
        s.speakers = Some(sp);
        assert!(a.fresh(&s, &l));
    }

    #[test]
    fn sig_ignores_stale_adjustment() {
        // 文が合わない手直しは効かない（sig に入らない）
        let mut s = script(line("こんにちは"));
        let l = s.blocks[0].lines[0].clone();
        let d = VoiceDict::default();
        let before = sig(&s, &l, &d);
        s.params.insert("l1".into(), json!({"sp": "トーク1", "text": "別の文", "params": {"a": 1}}));
        assert_eq!(sig(&s, &l, &d), before);
        assert_eq!(before, "[null,null]");
    }

    fn dict(entries: Vec<Value>) -> VoiceDict {
        VoiceDict { entries }
    }

    /// 署名を今の辞書で作って解析に入れる
    fn put(a: &mut Analyses, s: &Script, l: &Line) {
        let g = sig(s, l, a.dict());
        a.insert(vkey(&l.sp, &l.text), an("x", 1.0), g);
    }

    #[test]
    fn sig_ignores_unrelated_dict_entries() {
        let s = script(line("｜時雨《しぐれ》だ"));
        let l = &s.blocks[0].lines[0];
        let base = sig(&s, l, &VoiceDict::default());
        assert_eq!(base, "[null,null]");
        // 文に無い語・reading の無い項目・ルビの表記にだけある語は関わらない
        let d = dict(vec![
            json!({"surface": "ほげ", "reading": "ホゲ"}),
            json!({"surface": "しぐれ"}),
            json!({"surface": "しぐれ", "reading": ""}),
            json!({"surface": "時雨", "reading": "シグレ"}),
        ]);
        assert!(d.relevant(&tts_key_text(&l.text)).is_empty());
        assert_eq!(sig(&s, l, &d), base);
    }

    #[test]
    fn fresh_follows_relevant_dict_entries() {
        let s = script(line("ブルーベリーの話"));
        let l = &s.blocks[0].lines[0];
        let mut a = Analyses::default();
        put(&mut a, &s, l);
        assert!(a.fresh(&s, l));
        // 関わらない語を足しても変わらない
        assert!(a.set_dict(dict(vec![json!({"surface": "別", "reading": "ベツ"})])));
        assert!(a.fresh(&s, l));
        // 関わる語を足すと fresh でなくなる
        let rel = json!({"surface": "ブルーベリー", "reading": "ブルーベリー", "accent": 3});
        assert!(a.set_dict(dict(vec![rel.clone()])));
        assert!(!a.fresh(&s, l));
        // その語の accent を変えると別の署名
        put(&mut a, &s, l);
        assert!(a.fresh(&s, l));
        assert!(a.set_dict(dict(vec![json!({"surface": "ブルーベリー", "reading": "ブルーベリー", "accent": 4})])));
        assert!(!a.fresh(&s, l));
        // 元の辞書に戻すと fresh（accent 3 の署名で入れ直したものに戻る）
        assert!(a.set_dict(dict(vec![rel])));
        assert!(a.fresh(&s, l));
        // 語を外すと、辞書なしの署名とは違うので fresh でなくなり、辞書なしで作ったものなら fresh
        let mut c = Analyses::default();
        put(&mut c, &s, l);
        assert!(c.set_dict(dict(vec![json!({"surface": "ブルーベリー", "reading": "ア"})])));
        assert!(!c.fresh(&s, l));
        assert!(c.set_dict(VoiceDict::default()));
        assert!(!c.set_dict(VoiceDict::default()));
        assert!(c.fresh(&s, l));
        // clear は辞書を残す
        c.set_dict(dict(vec![json!({"surface": "別", "reading": "ベツ"})]));
        c.clear();
        assert_eq!(c.dict().entries.len(), 1);
    }

    #[test]
    fn dict_load() {
        let dir = std::env::temp_dir().join(format!("ak_vdict_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("voice_dict.json");
        let _ = std::fs::remove_file(&p);
        assert_eq!(VoiceDict::load(&p).unwrap(), VoiceDict::default());
        std::fs::write(&p, r#"{"entries":[{"surface":"あ","reading":"ア"}]}"#).unwrap();
        let d = VoiceDict::load(&p).unwrap();
        assert_eq!(d.entries, vec![json!({"surface": "あ", "reading": "ア"})]);
        std::fs::write(&p, r#"{"entries": "#).unwrap();
        assert!(VoiceDict::load(&p).is_err());
        std::fs::write(&p, r#"{"entries": {}}"#).unwrap();
        assert!(VoiceDict::load(&p).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn duration_and_missing() {
        let s = script(line("あ"));
        let l = &s.blocks[0].lines[0];
        let mut a = Analyses::default();
        assert_eq!(a.duration(l), None);
        a.insert(vkey(&l.sp, &l.text), an("x", 2.5), "stale".into());
        assert_eq!(a.duration(l), Some(2.5));
        assert!(!a.fresh(&s, l));
        a.insert(vkey(&l.sp, &l.text), an("x", f64::NAN), "stale".into());
        assert_eq!(a.duration(l), None);
    }

    #[test]
    fn req_shape() {
        let mut s = script(line("｜時雨《しぐれ》だ "));
        let l = s.blocks[0].lines[0].clone();
        let r = req(&s, &l);
        assert_eq!(r["speaker"], "トーク1");
        assert_eq!(r["text"], "しぐれだ");
        assert!(r["analyzed_text"].is_null());
        assert!(r["params"].is_null());
        assert_eq!(r["speaker_params"], json!({}));
        let mut sp = serde_json::Map::new();
        sp.insert("トーク1".into(), json!({"alpha": 0.5}));
        s.speakers = Some(sp);
        assert_eq!(req(&s, &l)["speaker_params"], json!({"alpha": 0.5}));
    }
}
