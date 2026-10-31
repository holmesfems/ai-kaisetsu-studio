//! 素材トラック（背景・録画・立ち絵・BGM）と、RecWindow のネストの中の素材を、外（Claude）から操作する。
//! 置き方の既定は mediaops::plan の 1 か所で決め、画面の「素材を追加」（commands::media_place 経由）も同じものを使う。

use crate::app::error::{bad_input, not_found};
use crate::core::Core;
use crate::render::media::MediaPool;
use crate::project::{MediaClip, Nest, ProjectDoc, Track, Trans};
use anyhow::Result;
use serde_json::{json, Value};

/// project.json に残る id。起動し直しても前に作ったものと重ならないよう、画面（ui/src/voice.ts の nid）と同じく
/// 時刻（36 進）＋連番で作り、doc の中で使われていればもう一度作る
fn new_id(p: &str, doc: &ProjectDoc) -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let used: std::collections::HashSet<&str> = doc
        .tracks
        .iter()
        .chain(doc.nests.values().flat_map(|n| n.tracks.iter()))
        .flat_map(|t| std::iter::once(t.id.as_str()).chain(t.clips.iter().map(|c| c.id.as_str())))
        .collect();
    loop {
        let ms = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
        let id = format!("{p}{}{}", radix36(ms as u64), radix36(SEQ.fetch_add(1, Ordering::Relaxed) + 1));
        if !used.contains(id.as_str()) {
            return id;
        }
    }
}

fn radix36(mut n: u64) -> String {
    const D: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let mut v = vec![];
    loop {
        v.push(D[(n % 36) as usize]);
        n /= 36;
        if n == 0 {
            break;
        }
    }
    v.reverse();
    String::from_utf8(v).unwrap_or_default()
}

fn kind_of(path: &str) -> &'static str {
    if MediaPool::is_image(path) {
        "image"
    } else if MediaPool::is_audio(path) {
        "audio"
    } else {
        "video"
    }
}

fn clip_json(c: &MediaClip) -> Value {
    json!({"id": c.id, "name": c.name, "src": c.src, "start": c.start, "dur": c.dur, "src_in": c.src_in, "speed": c.speed,
           "loop": c.looped, "x": c.x, "y": c.y, "scale": c.scale, "opacity": c.opacity, "fade_in": c.fade_in,
           "fade_out": c.fade_out, "volume_db": c.volume_db, "speaker": c.speaker, "flip": c.flip,
           "crop": [c.crop_l, c.crop_t, c.crop_r, c.crop_b], "feather": c.feather,
           "trans_in": trans_json(&c.trans_in), "trans_out": trans_json(&c.trans_out)})
}

fn trans_json(t: &Trans) -> Value {
    if t.is_none() {
        Value::Null
    } else {
        json!({"kind": t.kind, "dur": t.dur, "dir": t.dir})
    }
}

/// トランジションの種類
pub const TRANS_KINDS: [&str; 6] = ["fade", "slide", "pop", "slam", "wipe", "glitch"];

/// トランジションの指定を読む。"wipe" のような種類だけの文字列か {kind, dur, dir}（足りない値は今の値のまま）。
/// null・"" ・"none" で外す。
fn patch_trans(cur: &mut Trans, v: &Value) -> Result<()> {
    // 途中でエラーになったら何も変えない
    let mut t = cur.clone();
    patch_trans_in(&mut t, v)?;
    *cur = t;
    Ok(())
}

fn patch_trans_in(cur: &mut Trans, v: &Value) -> Result<()> {
    let kind_ok = |k: &str| -> Result<()> {
        if k.is_empty() || k == "none" || TRANS_KINDS.contains(&k) {
            Ok(())
        } else {
            return Err(bad_input(format!("トランジションの種類 {k} は無い（{} のどれか）", TRANS_KINDS.join(" / "))))
        }
    };
    match v {
        Value::Null => *cur = Trans::default(),
        Value::String(k) => {
            kind_ok(k)?;
            if k.is_empty() || k == "none" {
                *cur = Trans::default();
            } else {
                cur.kind = k.clone();
            }
        }
        Value::Object(_) => {
            if let Some(k) = v.get("kind").and_then(|x| x.as_str()) {
                kind_ok(k)?;
                if k.is_empty() || k == "none" {
                    *cur = Trans::default();
                    return Ok(());
                }
                cur.kind = k.into();
            }
            if let Some(d) = num(v, "dur") {
                cur.dur = d.clamp(0.0, 10.0);
            }
            if let Some(d) = v.get("dir").and_then(|x| x.as_str()) {
                if !["", "left", "right", "up", "down"].contains(&d) {
                    return Err(bad_input(format!("dir は left / right / up / down")));
                }
                cur.dir = d.into();
            }
            if cur.kind.is_empty() {
                return Err(bad_input(format!("トランジションの kind（{}）が要る", TRANS_KINDS.join(" / "))));
            }
        }
        _ => return Err(bad_input(format!("トランジションは種類の文字列か {{kind, dur, dir}}"))),
    }
    Ok(())
}

fn track_json(t: &Track) -> Value {
    json!({"id": t.id, "kind": t.kind, "name": t.name, "hidden": t.hidden, "muted": t.muted, "locked": t.locked,
           "volume_db": t.volume_db, "clips": t.clips.iter().map(clip_json).collect::<Vec<_>>()})
}

/// 素材トラックとネストの一覧（Claude に見せる形）。
pub fn summary(doc: &ProjectDoc) -> Value {
    let tracks: Vec<Value> = doc.tracks.iter().filter(|t| t.kind != "fx").map(track_json).collect();
    let nests: Vec<Value> = doc
        .nests
        .iter()
        .map(|(k, n)| json!({"key": k, "canvas": [n.w, n.h], "fit": n.fit,
                             "tracks": n.tracks.iter().map(track_json).collect::<Vec<_>>()}))
        .collect();
    json!({
        "note": "x, y は 1920x1080 の画面での中心の位置（Premiere の「位置」と同じ）、scale は素材の大きさに対する倍率。\
                 ネストの中は、そのネストのキャンバス（canvas）の座標。トラックは下から上の順（後ろのものほど手前）。",
        "tracks": tracks,
        "nests": nests,
    })
}

fn num(v: &Value, k: &str) -> Option<f64> {
    v.get(k).and_then(|x| x.as_f64())
}

/// 素材を置く計画。画面の「素材を追加」（ui/src/media.ts・nest.ts）と Claude の media_add が同じ既定で置くよう、
/// 置き方（位置・大きさ・長さ・入れるトラック）はここだけで決める（DU-1）
pub struct Plan {
    pub clip: MediaClip,
    pub kind: &'static str,
    /// RecWindow のネストのキー（無ければ本体のトラック）
    pub nest: Option<String>,
    pub track: TrackChoice,
}

pub enum TrackChoice {
    Existing(String),
    /// 新しいトラック。本体なら音声は一番上、ほかは演出のトラックの下に、ネストなら一番上に足す
    New(Track),
}

/// 新しいトラックの名前と音量（画面の「＋」で足すトラックも同じ）
pub fn new_track(kind: &str, id: String, n: usize) -> Track {
    let base = match kind { "audio" => "BGM", "image" => "立ち絵", _ => "録画" };
    Track {
        id,
        kind: kind.into(),
        name: if n > 1 { format!("{base} {n}") } else { base.into() },
        volume_db: if kind == "audio" { -18.0 } else { 0.0 },
        ..Default::default()
    }
}

/// ネストのトラック（キャンバスの上に重ねる映像・画像）
pub fn new_nest_track(id: String, n: usize) -> Track {
    Track { id, kind: "video".into(), name: format!("V{n}"), ..Default::default() }
}

/// 区間 [a, b) が空いているか（1e-6 秒の余裕）。画面の free と見本 ui_rules.json で縛っている
pub fn free(t: &Track, a: f64, b: f64) -> bool {
    !t.clips.iter().any(|c| c.start < b - 1e-6 && c.start + c.dur > a + 1e-6)
}

/// RecWindow の窓（演出が読み込まれていればその box、無ければ録画の窓の既定）
fn rec_box(core: &Core, key: &str) -> [f64; 4] {
    let d = core.desc();
    let from_group = d.as_ref().and_then(|d| d.groups.iter().find(|g| g.key == key)).and_then(|g| g.rec.first()).map(|r| r.bx);
    let from_desc = d.as_ref().map(|d| d.rec_box.clone()).filter(|r| r.len() == 4).map(|r| [r[0], r[1], r[2], r[3]]);
    from_group.or(from_desc).unwrap_or(Nest::default().bx)
}

/// 素材（w x h）をキャンバス（cw x ch）に収める倍率（縦横比を保つ）。画面の fitScale の contain と見本 ui_rules.json で縛っている
pub fn fit_scale(cw: f64, ch: f64, w: f64, h: f64) -> f64 {
    if w > 0.0 && h > 0.0 { ((cw / w).min(ch / h) * 100000.0).round() / 100000.0 } else { 1.0 }
}

/// 新しいトラックを足す位置（top は「ネストか音声」で一番上、ほかは演出のトラックの下）に入れ、その位置を返す。
/// 画面の insertTrack と見本 ui_rules.json で縛っている
pub fn insert_track(tracks: &mut Vec<Track>, t: Track, top: bool) -> usize {
    if top {
        tracks.push(t);
        tracks.len() - 1
    } else {
        let at = tracks.iter().position(|x| x.kind == "fx").unwrap_or(tracks.len());
        tracks.insert(at, t);
        at
    }
}

pub fn plan(core: &Core, doc: &ProjectDoc, path: &str, start: Option<f64>, want_track: Option<&str>, nest: Option<&str>)
            -> Result<Plan> {
    let path = path.replace('\\', "/");
    if !std::path::Path::new(&path).exists() {
        return Err(not_found(format!("ファイルがありません: {path}")));
    }
    let kind = kind_of(&path);
    // 素材の大きさ・長さ
    let (w, h, dur) = if kind == "image" {
        let im = core.media.image(&path)?;
        (im.full.w as f64, im.full.h as f64, 0.0)
    } else {
        let v = crate::render::media::probe(&core.ffmpeg(), &path)?;
        (v.w as f64, v.h as f64, v.dur)
    };
    let start = start.unwrap_or_else(|| core.ui.read().t).max(0.0);
    let name = path.rsplit('/').next().unwrap_or(&path).to_string();
    let mut c = MediaClip { id: new_id(if nest.is_some() { "n" } else { "m" }, doc), name, src: path.clone(), start,
                            ..Default::default() };
    let r3 = |v: f64| (v * 1000.0).round() / 1000.0;
    if let Some(key) = nest {
        if kind == "audio" {
            return Err(bad_input(format!("音声はネストに置けません。本体のタイムラインに置いてください")));
        }
        // RecWindow のネストの中: 窓が見えている区間に収め、キャンバスに収まる大きさで真ん中に
        let (a, b) = core
            .desc()
            .and_then(|d| d.groups.iter().find(|g| g.key == key).map(|g| crate::render::cache::group_span(g, &core.overrides())))
            .map(|(a, b, _)| (a, b))
            .unwrap_or((start, start + 5.0));
        let (cw, ch) = doc.nests.get(key).map(|n| (n.w, n.h)).unwrap_or((1920.0, 1080.0));
        c.start = r3(start.max(a).min(b - 0.1));
        let room = (b - c.start).max(0.1);
        c.dur = r3(if kind == "image" || dur <= 0.0 { room } else { dur.min(room) }.max(0.1));
        c.x = cw / 2.0;
        c.y = ch / 2.0;
        c.scale = fit_scale(cw, ch, w, h);
        let tracks = doc.nests.get(key).map(|n| n.tracks.as_slice()).unwrap_or(&[]);
        let (a, b) = (c.start, c.start + c.dur);
        let chosen = want_track
            .and_then(|t| tracks.iter().find(|x| (x.id == t || x.name == t) && free(x, a, b)))
            .or_else(|| tracks.iter().find(|x| free(x, a, b)));
        let track = match chosen {
            Some(t) => TrackChoice::Existing(t.id.clone()),
            None => TrackChoice::New(new_nest_track(new_id("nt", doc), tracks.len() + 1)),
        };
        return Ok(Plan { clip: c, kind, nest: Some(key.to_string()), track });
    }
    match kind {
        "image" => {
            // 立ち絵の既定: 左の領域（x < 600）に、高さ 1000px ほどで、終わりまで（最低 5 秒）
            c.scale = if h > 0.0 { ((1000.0 / h).min(1.0) * 1000.0).round() / 1000.0 } else { 1.0 };
            c.x = 300.0;
            c.y = 1080.0 - h * c.scale / 2.0 + 20.0;
            c.dur = r3((core.timeline_end(doc) - start).max(5.0));
        }
        "audio" => c.dur = if dur > 0.0 { dur } else { 10.0 },
        _ => {
            c.dur = if dur > 0.0 { dur } else { 10.0 };
            if w >= 1900.0 {
                // 録画: 録画の窓（REC_BOX）にはめる（位置 (1240, 460)・66.667%）
                let rb = core.desc().map(|d| d.rec_box.clone()).filter(|r| r.len() == 4)
                    .unwrap_or_else(|| Nest::default().bx.to_vec());
                c.x = (rb[0] + rb[2]) / 2.0;
                c.y = (rb[1] + rb[3]) / 2.0;
                c.scale = ((rb[2] - rb[0]) / w * 100000.0).round() / 100000.0;
            }
        }
    }
    let ok = |x: &Track| if kind == "audio" { x.kind == "audio" } else { x.kind == kind };
    let chosen = match want_track {
        Some(t) => {
            let tr = doc.tracks.iter().find(|x| x.id == t || x.name == t);
            if let Some(tr) = tr {
                if tr.kind == "fx" || (tr.kind == "audio") != (kind == "audio") {
                    // 種類の合わないトラックに落としたときは、種類に合うトラックへ
                    None
                } else {
                    Some(tr)
                }
            } else {
                return Err(bad_input(format!("トラック {t} がありません")));
            }
        }
        None => None,
    }
    // 背景（v1）には勝手に入れない
    .or_else(|| doc.tracks.iter().find(|x| ok(x) && x.id != "v1"))
    .or_else(|| doc.tracks.iter().find(|x| ok(x)));
    let track = match chosen {
        Some(t) => TrackChoice::Existing(t.id.clone()),
        None => TrackChoice::New(new_track(kind, new_id("t", doc), 1)),
    };
    Ok(Plan { clip: c, kind, nest: None, track })
}

/// 計画どおり doc に入れる。入れたトラックの id を返す
pub fn apply_plan(core: &Core, doc: &mut ProjectDoc, plan: Plan) -> String {
    let Plan { clip, nest, track, kind } = plan;
    // 選んだトラックが（計画を立ててから入れるまでの間に）無くなっていたときの代わり
    let fallback = match &nest {
        Some(k) => new_nest_track(new_id("nt", doc), doc.nests.get(k).map(|n| n.tracks.len()).unwrap_or(0) + 1),
        None => new_track(kind, new_id("t", doc), 1),
    };
    let tracks: &mut Vec<Track> = match &nest {
        Some(k) => {
            let bx = rec_box(core, k);
            let n = doc.nests.entry(k.clone()).or_insert_with(|| Nest { bx, ..Default::default() });
            &mut n.tracks
        }
        None => &mut doc.tracks,
    };
    let track = match track {
        TrackChoice::Existing(id) if !tracks.iter().any(|t| t.id == id) => TrackChoice::New(fallback),
        t => t,
    };
    let idx = match track {
        TrackChoice::Existing(id) => tracks.iter().position(|t| t.id == id),
        TrackChoice::New(t) => {
            let top = nest.is_some() || t.kind == "audio";
            Some(insert_track(tracks, t, top))
        }
    };
    let idx = idx.unwrap_or(0);
    tracks[idx].clips.push(clip);
    tracks[idx].id.clone()
}

/// 画面に返す形（画面はこれで doc に入れる。元に戻すの履歴に入るよう、画面の editDoc で入れる）
pub fn plan_json(p: &Plan) -> Value {
    let (track_id, new_track) = match &p.track {
        TrackChoice::Existing(id) => (Some(id.clone()), None),
        TrackChoice::New(t) => (None, Some(t.clone())),
    };
    json!({"clip": p.clip, "kind": p.kind, "nest": p.nest, "track_id": track_id, "new_track": new_track})
}

/// 素材を置く（Claude の media_add）。req: {path, start?, track?, nest?, と patch の項目}
pub fn add(core: &Core, req: &Value) -> Result<Value> {
    check_keys(req, &["path", "track", "nest"])?;
    let path = req["path"].as_str().ok_or_else(|| bad_input(format!("path（素材のファイル）が要る")))?;
    if core.root().is_none() {
        return Err(crate::app::error::no_project());
    }
    // 素材を調べ終えてから doc を読み直して入れる（調べている間の画面の変更を消さないように）
    let probe_doc = core.doc().ok_or_else(crate::app::error::no_project)?;
    let mut p = plan(core, &probe_doc, path, num(req, "start"), req["track"].as_str(), req["nest"].as_str())?;
    if let Some(n) = req["name"].as_str() {
        p.clip.name = n.into();
    }
    apply_patch(&mut p.clip, req)?;
    let id = p.clip.id.clone();
    let nest_key = p.nest.clone();
    let mut doc = core.doc().ok_or_else(crate::app::error::no_project)?;
    let track_id = apply_plan(core, &mut doc, p);
    core.set_doc_and_notify(doc)?;
    Ok(json!({"ok": true, "clip_id": id, "track": track_id, "nest": nest_key}))
}

/// 素材の patch で受ける鍵（media_list の crop も受ける）。知らない鍵（打ち間違いも）は黙って無視せずエラーにする
const PATCH_KEYS: &[&str] = &[
    "start", "dur", "src_in", "speed", "x", "y", "scale", "opacity", "fade_in", "fade_out", "volume_db", "crop", "crop_l", "crop_t",
    "crop_r", "crop_b", "feather", "loop", "flip", "speaker", "name", "trans_in", "trans_out",
];

fn check_keys(p: &Value, extra: &[&str]) -> Result<()> {
    let Some(o) = p.as_object() else { return Err(bad_input(format!("patch はオブジェクトで渡してください"))) };
    let bad: Vec<&str> = o.keys().map(|k| k.as_str()).filter(|k| !PATCH_KEYS.contains(k) && !extra.contains(k)).collect();
    if !bad.is_empty() {
        return Err(bad_input(format!("知らない項目: {}（使える項目: {}）", bad.join(", "), PATCH_KEYS.join(", "))));
    }
    Ok(())
}

fn apply_patch(c: &mut MediaClip, p: &Value) -> Result<()> {
    if let Some(a) = p.get("crop").and_then(|v| v.as_array()) {
        let v: Vec<f64> = a.iter().filter_map(|x| x.as_f64()).collect();
        if v.len() != 4 {
            return Err(bad_input(format!("crop は [左, 上, 右, 下]（%）の 4 つで渡してください")));
        }
        c.crop_l = v[0].clamp(0.0, 100.0);
        c.crop_t = v[1].clamp(0.0, 100.0);
        c.crop_r = v[2].clamp(0.0, 100.0);
        c.crop_b = v[3].clamp(0.0, 100.0);
    }
    if let Some(v) = num(p, "start") { c.start = v.max(0.0) }
    if let Some(v) = num(p, "dur") { c.dur = v.max(0.05) }
    if let Some(v) = num(p, "src_in") { c.src_in = v.max(0.0) }
    if let Some(v) = num(p, "speed") { c.speed = v.clamp(0.1, 10.0) }
    if let Some(v) = num(p, "x") { c.x = v }
    if let Some(v) = num(p, "y") { c.y = v }
    if let Some(v) = num(p, "scale") { c.scale = v.max(0.001) }
    if let Some(v) = num(p, "opacity") { c.opacity = v.clamp(0.0, 1.0) }
    if let Some(v) = num(p, "fade_in") { c.fade_in = v.max(0.0) }
    if let Some(v) = num(p, "fade_out") { c.fade_out = v.max(0.0) }
    if let Some(v) = num(p, "volume_db") { c.volume_db = v }
    if let Some(v) = num(p, "crop_l") { c.crop_l = v.clamp(0.0, 100.0) }
    if let Some(v) = num(p, "crop_t") { c.crop_t = v.clamp(0.0, 100.0) }
    if let Some(v) = num(p, "crop_r") { c.crop_r = v.clamp(0.0, 100.0) }
    if let Some(v) = num(p, "crop_b") { c.crop_b = v.clamp(0.0, 100.0) }
    if let Some(v) = num(p, "feather") { c.feather = v.max(0.0) }
    if let Some(v) = p.get("loop").and_then(|x| x.as_bool()) { c.looped = v }
    if let Some(v) = p.get("flip").and_then(|x| x.as_bool()) { c.flip = v }
    if let Some(v) = p.get("speaker").and_then(|x| x.as_str()) { c.speaker = crate::script::norm_sp(v) }
    if let Some(v) = p.get("name").and_then(|x| x.as_str()) { c.name = v.into() }
    if let Some(v) = p.get("trans_in") { patch_trans(&mut c.trans_in, v)? }
    if let Some(v) = p.get("trans_out") { patch_trans(&mut c.trans_out, v)? }
    Ok(())
}

fn find<'a>(doc: &'a mut ProjectDoc, id: &str) -> Option<(&'a mut Vec<MediaClip>, usize)> {
    for t in doc.tracks.iter_mut().chain(doc.nests.values_mut().flat_map(|n| n.tracks.iter_mut())) {
        if let Some(i) = t.clips.iter().position(|c| c.id == id) {
            return Some((&mut t.clips, i));
        }
    }
    None
}

pub fn update(core: &Core, id: &str, patch: &Value) -> Result<Value> {
    let mut doc = core.doc().ok_or_else(crate::app::error::no_project)?;
    check_keys(patch, &[])?;
    let (clips, i) = find(&mut doc, id).ok_or_else(|| not_found(format!("素材のクリップ {id} が無い（media_list で確かめて）")))?;
    apply_patch(&mut clips[i], patch)?;
    let out = clip_json(&clips[i]);
    core.set_doc_and_notify(doc)?;
    Ok(json!({"ok": true, "clip": out}))
}

pub fn remove(core: &Core, id: &str) -> Result<Value> {
    let mut doc = core.doc().ok_or_else(crate::app::error::no_project)?;
    let (clips, i) = find(&mut doc, id).ok_or_else(|| not_found(format!("素材のクリップ {id} が無い")))?;
    let c = clips.remove(i);
    core.set_doc_and_notify(doc)?;
    Ok(json!({"ok": true, "removed": c.name}))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn patch_checks_keys_and_reads_crop() {
        assert!(check_keys(&json!({"opacty": 0.5}), &[]).is_err());
        assert!(check_keys(&json!({"opacity": 0.5, "crop": [1, 2, 3, 4]}), &[]).is_ok());
        assert!(check_keys(&json!({"path": "a.png"}), &["path"]).is_ok());
        let mut c = MediaClip::default();
        apply_patch(&mut c, &json!({"crop": [1, 2, 3, 4], "speaker": "talk2"})).unwrap();
        assert_eq!((c.crop_l, c.crop_t, c.crop_r, c.crop_b), (1.0, 2.0, 3.0, 4.0));
        assert_eq!(c.speaker, "トーク2");
        apply_patch(&mut c, &json!({"speed": 50})).unwrap();
        assert_eq!(c.speed, 10.0);
        apply_patch(&mut c, &json!({"speed": 0.01})).unwrap();
        assert_eq!(c.speed, 0.1);
    }

    #[test]
    fn ids_do_not_collide_with_saved_ones() {
        let mut doc = ProjectDoc::default();
        let a = new_id("m", &doc);
        doc.tracks.push(Track { id: "t1".into(), clips: vec![MediaClip { id: a.clone(), ..Default::default() }], ..Default::default() });
        let b = new_id("m", &doc);
        assert_ne!(a, b);
        assert!(a.starts_with('m') && a.len() > 8);
    }

    #[test]
    fn patch_transitions() {
        let mut c = MediaClip::default();
        apply_patch(&mut c, &json!({"trans_in": "wipe", "trans_out": {"kind": "slide", "dur": 0.5, "dir": "right"}})).unwrap();
        assert_eq!(c.trans_in.kind, "wipe");
        assert_eq!(c.trans_out, Trans { kind: "slide".into(), dur: 0.5, dir: "right".into() });
        // 足りない値は今のまま
        apply_patch(&mut c, &json!({"trans_out": {"dur": 0.2}})).unwrap();
        assert_eq!(c.trans_out.kind, "slide");
        assert_eq!(c.trans_out.dur, 0.2);
        // 外す
        apply_patch(&mut c, &json!({"trans_in": null, "trans_out": "none"})).unwrap();
        assert!(c.trans_in.is_none() && c.trans_out.is_none());
        // 知らない種類・向きはエラー
        assert!(apply_patch(&mut c, &json!({"trans_in": "spin"})).is_err());
        assert!(apply_patch(&mut c, &json!({"trans_in": {"kind": "slide", "dir": "diagonal"}})).is_err());
        assert!(apply_patch(&mut c, &json!({"trans_in": {"dur": 0.3}})).is_err());
    }
}
