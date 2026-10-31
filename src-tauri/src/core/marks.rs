//! モニターの印（Claude への場所つきの指示）。`<回>/studio/marks.json` の読み書きと操作。
//!
//! 正は Rust（`Core::marks`）。変えるたびに原子的に保存し、イベント "marks"（`MarkFile` 丸ごと）で画面に知らせる。
//! 外のエディタの変更は見張らない（Claude は MCP で触る）。型は `ak-proto`（`marks.rs`）。

use super::Core;
use ak_proto::http::MarkAddReq;
use ak_proto::{circled, ErrorCode, Mark, MarkFile, MarkFound, MarkShape, MARK_COLORS};
use anyhow::{anyhow, Context, Result};
use serde_json::json;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

const FILE: &str = "marks.json";
/// 確認画像（印の層を重ねたコマ）の大きさ
const W: u32 = 1920;
const H: u32 = 1080;
/// ペンの点の最大数（画面が間引いたあと）
const PEN_MAX: usize = 32;

/// 回の印の持ち物。`path` が None のあいだは保存しない（読めないファイルを残したまま空で上書きしないため）
#[derive(Debug, Default)]
pub struct MarkStore {
    file: MarkFile,
    path: Option<PathBuf>,
}

fn coded(code: ErrorCode, msg: impl Into<String>) -> anyhow::Error {
    crate::app::error::coded(code, msg)
}

fn check(m: &Mark) -> Result<()> {
    if !MARK_COLORS.iter().any(|(c, _)| *c == m.color) {
        return Err(coded(ErrorCode::BadInput, format!("印の色が正しくありません: {}（red・yellow・green・blue）", m.color)));
    }
    if !["draft", "sent", "done"].contains(&m.state.as_str()) {
        return Err(coded(ErrorCode::BadInput, format!("印の状態が正しくありません: {}（draft・sent・done）", m.state)));
    }
    if !["user", "claude"].contains(&m.by.as_str()) {
        return Err(coded(ErrorCode::BadInput, format!("印の作り手が正しくありません: {}（user・claude）", m.by)));
    }
    if let MarkShape::Pen { points } = &m.shape {
        if points.is_empty() || points.len() > PEN_MAX {
            return Err(coded(ErrorCode::BadInput, format!("ペンの点は 1〜{PEN_MAX} 個にしてください（{} 個）", points.len())));
        }
    }
    if ![m.t, m.t0, m.t1].iter().all(|x| x.is_finite()) || m.t1 <= m.t0 {
        return Err(coded(ErrorCode::BadInput, "印の区間が正しくありません（t0 < t1 の秒で）"));
    }
    Ok(())
}

/// 印を出す区間の既定: t を含むシーン。無ければ t の前後 2 秒
pub fn default_range(scenes: &[(String, f64, f64)], t: f64) -> (f64, f64) {
    match scenes.iter().find(|(_, a, b)| *a <= t && t < *b) {
        Some((_, a, b)) => (*a, *b),
        None => ((t - 2.0).max(0.0), t + 2.0),
    }
}

/// 層（透明を含む RGBA）を不透明のコマの上にアルファで重ねる。大きさが違えばエラー
pub fn overlay_layer(base: &mut image::RgbImage, layer: &image::RgbaImage) -> Result<()> {
    if base.dimensions() != layer.dimensions() {
        let ((bw, bh), (lw, lh)) = (base.dimensions(), layer.dimensions());
        return Err(coded(ErrorCode::BadInput, format!("印の層の大きさ（{lw}x{lh}）がコマ（{bw}x{bh}）と違います")));
    }
    for (b, l) in base.pixels_mut().zip(layer.pixels()) {
        let a = l[3] as u32;
        if a == 0 {
            continue;
        }
        for i in 0..3 {
            b[i] = ((l[i] as u32 * a + b[i] as u32 * (255 - a) + 127) / 255) as u8;
        }
    }
    Ok(())
}

impl MarkStore {
    /// 回の studio フォルダから読む。壊れていれば退避して空で始め、知らせる文を返す。
    /// 退避できずファイルが残るときは、空で上書きしないよう保存しない
    pub fn open(studio: &Path) -> (MarkStore, Option<String>) {
        let p = studio.join(FILE);
        let (v, warn) = crate::project::load_json_or_keep(&p, "印", json!({"next": 1, "marks": []}));
        let mut keep_off = warn.is_some() && p.exists();
        let mut warn = warn.map(|w| if keep_off { format!("{w}このあいだは印を保存しません") } else { w });
        let mut file = match serde_json::from_value::<MarkFile>(v) {
            Ok(f) => f,
            Err(e) => {
                // オブジェクトだが中身の形が違う。空で始めて上書きする前に、元のファイルを残す
                let e = crate::app::error::reason(e);
                warn = Some(match crate::agent::copy_broken(&p) {
                    Ok(name) => format!("印（{FILE}）の形が違うため読み込めませんでした: {e}。元のファイルは {name} に残しました"),
                    Err(e2) => {
                        keep_off = true;
                        let e2 = crate::app::error::reason(e2);
                        format!("印（{FILE}）の形が違うため読み込めず、元のファイルも残せませんでした（{e}／{e2}）。このあいだは印を保存しません")
                    }
                });
                MarkFile::default()
            }
        };
        let top = file.marks.iter().map(|m| m.id).max().unwrap_or(0);
        file.next = file.next.max(top + 1).max(1);
        (MarkStore { file, path: if keep_off { None } else { Some(p) } }, warn)
    }

    pub fn file(&self) -> &MarkFile {
        &self.file
    }

    pub fn get(&self, id: u32) -> Option<&Mark> {
        self.file.marks.iter().find(|m| m.id == id)
    }

    fn get_mut(&mut self, id: u32) -> Result<&mut Mark> {
        self.file.marks.iter_mut().find(|m| m.id == id).ok_or_else(|| coded(ErrorCode::NotFound, format!("印 {} がありません", circled(id))))
    }

    /// 足す（id 0 なら新しい番号）か、直す。直すのは色・形・一言・t・区間だけ
    /// （状態・送った記録・済の記録は変えない。画面が古い持ち物で上書きして Claude の「済」を消さないため）。
    /// 新しい印はいつも user の draft
    pub fn upsert(&mut self, mut m: Mark) -> Result<Mark> {
        if let Some(old) = self.get(m.id).filter(|_| m.id != 0).cloned() {
            let merged = Mark { color: m.color, shape: m.shape, note: m.note, t: m.t, t0: m.t0, t1: m.t1, ..old };
            check(&merged)?;
            *self.get_mut(m.id)? = merged.clone();
            return Ok(merged);
        }
        if m.id == 0 {
            m.id = self.file.next;
        }
        m.by = "user".into();
        m.state = "draft".into();
        m.clips.clear();
        m.media.clear();
        m.msg = None;
        m.done_note.clear();
        m.done_image = None;
        check(&m)?;
        self.file.next = self.file.next.max(m.id + 1);
        self.file.marks.push(m.clone());
        Ok(m)
    }

    /// 消す（番号は詰めない）。無ければ false
    pub fn remove(&mut self, id: u32) -> bool {
        let n = self.file.marks.len();
        self.file.marks.retain(|m| m.id != id);
        self.file.marks.len() != n
    }

    /// 送った: 済でない印を sent にして、メッセージの id と下にあったクリップ・素材を記録する
    pub fn sent(&mut self, ids: &[u32], msg: u64, found: &[MarkFound]) {
        for m in self.file.marks.iter_mut().filter(|m| ids.contains(&m.id) && m.state != "done") {
            m.state = "sent".into();
            m.msg = Some(msg);
            let f = found.iter().find(|f| f.id == m.id);
            m.clips = f.map(|f| f.clips.clone()).unwrap_or_default();
            m.media = f.map(|f| f.media.clone()).unwrap_or_default();
        }
    }

    /// 取り消し: そのメッセージで送った印（まだ済でないもの）を draft に戻す。戻したものがあれば true
    pub fn unsend(&mut self, msg: u64) -> bool {
        let mut any = false;
        for m in self.file.marks.iter_mut().filter(|m| m.msg == Some(msg) && m.state == "sent") {
            m.state = "draft".into();
            m.msg = None;
            m.clips.clear();
            m.media.clear();
            any = true;
        }
        any
    }

    /// 状態を変える（画面の「済にする」「戻す」）。済から出るときは済の記録を消し、draft にするときは送った記録も消す
    pub fn set_state(&mut self, id: u32, state: &str) -> Result<Mark> {
        if !["draft", "sent", "done"].contains(&state) {
            return Err(coded(ErrorCode::BadInput, format!("印の状態が正しくありません: {state}（draft・sent・done）")));
        }
        let m = self.get_mut(id)?;
        // 送っていない印（user の印でメッセージの記録が無い）を sent にはしない（二度と送れなくなる）
        let state = if state == "sent" && m.by == "user" && m.msg.is_none() { "draft" } else { state };
        if state != "done" {
            m.done_note.clear();
            m.done_image = None;
        }
        if state == "draft" {
            m.msg = None;
            m.clips.clear();
            m.media.clear();
        }
        m.state = state.into();
        Ok(m.clone())
    }

    /// Claude が済にする
    pub fn resolve(&mut self, id: u32, note: Option<String>, image: Option<String>) -> Result<Mark> {
        let m = self.get_mut(id)?;
        m.state = "done".into();
        m.done_note = note.unwrap_or_default().trim().to_string();
        m.done_image = image.map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
        Ok(m.clone())
    }

    /// Claude が印を足す（by: claude、state: sent）
    pub fn add_by_claude(&mut self, req: MarkAddReq, range: (f64, f64)) -> Result<Mark> {
        let m = Mark {
            id: self.file.next,
            color: req.color,
            shape: req.shape,
            note: req.note,
            t: req.t,
            t0: req.t0.unwrap_or(range.0),
            t1: req.t1.unwrap_or(range.1),
            state: "sent".into(),
            by: "claude".into(),
            clips: vec![],
            media: vec![],
            msg: None,
            done_note: String::new(),
            done_image: None,
        };
        check(&m)?;
        self.file.next += 1;
        self.file.marks.push(m.clone());
        Ok(m)
    }

    fn save(&self) -> Result<()> {
        let Some(p) = &self.path else { return Ok(()) };
        let text = serde_json::to_string_pretty(&self.file)?;
        crate::settings::write_atomic(p, text).with_context(|| format!("印を保存できませんでした: {}", p.display()))
    }
}

impl Core {
    /// 今の印の全部
    pub fn marks_file(&self) -> MarkFile {
        self.marks.read().file().clone()
    }

    /// 回を開いたとき: 印を読み直す（壊れていれば知らせる）
    pub(super) fn marks_open(&self, studio: &Path) {
        let (store, warn) = MarkStore::open(studio);
        let file = store.file().clone();
        *self.marks.write() = store;
        if let Some(w) = warn {
            self.notice_error(w);
        }
        self.emit("marks", file);
    }

    /// 印を変える。`f` が `Ok((変えたか, 戻り値))` なら、変えていれば保存して画面に知らせる。
    /// 回が無ければ no_project。保存に失敗したら画面にも知らせたうえで Err を返す（メモリの中身は変わったまま）
    fn marks_mutate<T>(&self, f: impl FnOnce(&mut MarkStore) -> Result<(bool, T)>) -> Result<T> {
        self.root().ok_or_else(crate::app::error::no_project)?;
        let (file, saved, out) = {
            let mut s = self.marks.write();
            let (changed, out) = f(&mut s)?;
            if !changed {
                return Ok(out);
            }
            (s.file().clone(), s.save(), out)
        };
        self.emit("marks", file);
        if let Err(e) = saved {
            self.notice_err("印を保存できませんでした", &e);
            return Err(e);
        }
        Ok(out)
    }

    pub fn mark_upsert(&self, m: Mark) -> Result<Mark> {
        self.marks_mutate(|s| s.upsert(m).map(|m| (true, m)))
    }

    pub fn mark_remove(&self, id: u32) -> Result<()> {
        self.marks_mutate(|s| Ok((s.remove(id), ())))
    }

    pub fn marks_sent(&self, ids: &[u32], msg: u64, found: &[MarkFound]) -> Result<()> {
        self.marks_mutate(|s| {
            s.sent(ids, msg, found);
            Ok((true, ()))
        })
    }

    pub fn marks_unsend(&self, msg: u64) -> Result<()> {
        self.marks_mutate(|s| Ok((s.unsend(msg), ())))
    }

    pub fn mark_set_state(&self, id: u32, state: &str) -> Result<Mark> {
        self.marks_mutate(|s| s.set_state(id, state).map(|m| (true, m)))
    }

    /// Claude の `mark_resolve`: 済にして、画面にトーストを出す
    pub fn mark_resolve(&self, id: u32, note: Option<String>, image: Option<String>) -> Result<Mark> {
        self.root().ok_or_else(crate::app::error::no_project)?;
        let m = self.marks_mutate(|s| s.resolve(id, note, image).map(|m| (true, m)))?;
        let tail = if m.done_note.is_empty() { String::new() } else { format!("（{}）", m.done_note) };
        self.emit("toast", json!({"text": format!("Claude が印{}を済にしました{tail}", circled(m.id)), "kind": "info", "ms": 8000}));
        Ok(m)
    }

    /// Claude の `mark_add`
    pub fn mark_add_by_claude(&self, req: MarkAddReq) -> Result<Mark> {
        self.root().ok_or_else(crate::app::error::no_project)?;
        // シーンを調べるのは印の持ち物を取る前（scenes は演出・攻略動画の状態を読む）
        let range = default_range(&self.scenes(), req.t);
        self.marks_mutate(|s| s.add_by_claude(req, range).map(|m| (true, m)))
    }

    /// 印の層（PNG）を t のコマ（字幕つき）に重ねて、`<回>/studio/chat/mark-<日時>-<連番>.jpg` に保存し、パスを返す
    pub fn mark_image(&self, t: f64, layer_png: &[u8]) -> Result<PathBuf> {
        static SEQ: AtomicU32 = AtomicU32::new(0);
        let layer = image::load_from_memory(layer_png).context("印の層の画像を読めませんでした")?.to_rgba8();
        if layer.dimensions() != (W, H) {
            let (w, h) = layer.dimensions();
            return Err(coded(ErrorCode::BadInput, format!("印の層は {W}x{H} の画像にしてください（受け取ったのは {w}x{h}）")));
        }
        let dir = self.root().ok_or_else(crate::app::error::no_project)?.join("studio").join("chat");
        let fr = self.compose(t, 1.0, &[], None, true, true)?;
        let mut img = image::RgbImage::from_raw(fr.w, fr.h, fr.data).ok_or_else(|| anyhow!("コマの画像を作れませんでした"))?;
        overlay_layer(&mut img, &layer)?;
        let mut jpg = vec![];
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpg, 90).encode_image(&img)?;
        std::fs::create_dir_all(&dir)?;
        let name = format!("mark-{}-{:02}.jpg", chrono::Local::now().format("%Y%m%d-%H%M%S"), SEQ.fetch_add(1, Ordering::SeqCst) % 100);
        let p = dir.join(name);
        crate::settings::write_atomic(&p, &jpg).with_context(|| format!("確認画像を保存できませんでした（{}）", p.display()))?;
        Ok(p)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        let n = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
        let d = std::env::temp_dir().join(format!("ak-studio-test-marks-{tag}-{}-{n}", std::process::id()));
        let _ = std::fs::create_dir_all(&d);
        d
    }

    fn mark(id: u32) -> Mark {
        Mark {
            id, color: "red".into(), shape: MarkShape::Pin { at: [10, 20] }, note: "一言".into(), t: 5.0, t0: 4.0, t1: 8.0,
            state: "draft".into(), by: "user".into(), clips: vec![], media: vec![], msg: None, done_note: String::new(), done_image: None,
        }
    }

    fn broken(d: &Path) -> Vec<PathBuf> {
        std::fs::read_dir(d).unwrap().flatten().map(|e| e.path())
            .filter(|p| p.file_name().unwrap().to_string_lossy().starts_with("marks.json.broken-")).collect()
    }

    #[test]
    fn 足すと番号が増え_消しても詰めない() {
        let mut s = MarkStore::default();
        assert_eq!(s.file().next, 1);
        let a = s.upsert(mark(0)).unwrap();
        let b = s.upsert(mark(0)).unwrap();
        assert_eq!((a.id, b.id), (1, 2));
        assert!(s.remove(1));
        assert!(!s.remove(1));
        let c = s.upsert(mark(0)).unwrap();
        assert_eq!(c.id, 3, "消した番号は使い回さない");
        assert_eq!(s.file().marks.iter().map(|m| m.id).collect::<Vec<_>>(), [2, 3]);
    }

    #[test]
    fn nextを保存して開き直しても続く() {
        let d = tmp("next");
        let (mut s, w) = MarkStore::open(&d);
        assert!(w.is_none());
        s.upsert(mark(0)).unwrap();
        s.upsert(mark(0)).unwrap();
        s.remove(2);
        s.save().unwrap();
        let (mut s2, w) = MarkStore::open(&d);
        assert!(w.is_none());
        assert_eq!(s2.file().next, 3);
        assert_eq!(s2.upsert(mark(0)).unwrap().id, 3);
        // next が小さく書かれていても、ある印のいちばん大きい番号の次から
        std::fs::write(d.join("marks.json"), serde_json::to_string(&MarkFile { next: 1, marks: vec![mark(5)] }).unwrap()).unwrap();
        assert_eq!(MarkStore::open(&d).0.file().next, 6);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn 壊れたmarks_jsonは退避して知らせ_空で上書きしない() {
        let d = tmp("broken");
        std::fs::write(d.join("marks.json"), "{").unwrap();
        let (mut s, w) = MarkStore::open(&d);
        assert!(w.unwrap().contains("印"));
        assert!(s.file().marks.is_empty());
        let kept = broken(&d);
        assert_eq!(kept.len(), 1);
        assert_eq!(std::fs::read_to_string(&kept[0]).unwrap(), "{");
        s.upsert(mark(0)).unwrap();
        s.save().unwrap();
        assert_eq!(std::fs::read_to_string(&kept[0]).unwrap(), "{", "退避したファイルは残る");
        assert!(std::fs::read_to_string(d.join("marks.json")).unwrap().contains("一言"));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn 形の違うmarks_jsonも退避する() {
        let d = tmp("shape");
        std::fs::write(d.join("marks.json"), r#"{"next": 2, "marks": [{"id": 1}]}"#).unwrap();
        let (s, w) = MarkStore::open(&d);
        assert!(w.unwrap().contains("形が違う"));
        assert!(s.file().marks.is_empty());
        assert_eq!(broken(&d).len(), 1);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn 保存先の無いストアは保存しない() {
        // 読めず退避もできなかったとき（path が None）は、空のまま上書きしない
        let mut s = MarkStore::default();
        s.upsert(mark(0)).unwrap();
        s.save().unwrap();
        // 読めないもの（フォルダ）は退避されて知らされ、そのあとは普通に保存できる
        let d = tmp("keepoff");
        std::fs::create_dir_all(d.join("marks.json")).unwrap();
        let (mut s, w) = MarkStore::open(&d);
        assert!(w.is_some());
        s.upsert(mark(0)).unwrap();
        s.save().unwrap();
        assert!(d.join("marks.json").is_file());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn 送る_取り消す_済にする() {
        let mut s = MarkStore::default();
        s.upsert(mark(0)).unwrap();
        s.upsert(mark(0)).unwrap();
        s.sent(&[1, 2, 9], 42, &[MarkFound { id: 1, clips: vec!["telop@c15#1".into()], media: vec!["img-3".into()] }]);
        let m1 = s.get(1).unwrap();
        assert_eq!((m1.state.as_str(), m1.msg), ("sent", Some(42)));
        assert_eq!(m1.clips, ["telop@c15#1"]);
        assert_eq!(m1.media, ["img-3"]);
        assert!(s.get(2).unwrap().clips.is_empty());
        // 取り消し: draft に戻って記録が消える。別のメッセージでは動かない
        assert!(!s.unsend(7));
        assert!(s.unsend(42));
        let m1 = s.get(1).unwrap();
        assert_eq!((m1.state.as_str(), m1.msg), ("draft", None));
        assert!(m1.clips.is_empty() && m1.media.is_empty());
        assert!(!s.unsend(42));
        // 済にする: 一言と確認画像
        s.sent(&[1], 43, &[]);
        let d = s.resolve(1, Some(" 小さくした ".into()), Some("E:/x/still.png".into())).unwrap();
        assert_eq!((d.state.as_str(), d.done_note.as_str(), d.done_image.as_deref()), ("done", "小さくした", Some("E:/x/still.png")));
        // 済は取り消しで draft に戻らず、送り直しでも sent に戻らない
        assert!(!s.unsend(43));
        s.sent(&[1], 44, &[]);
        assert_eq!(s.get(1).unwrap().state, "done");
        // 画面の「戻す」で sent に。済の記録は消える
        let back = s.set_state(1, "sent").unwrap();
        assert_eq!((back.state.as_str(), back.done_note.as_str(), back.done_image.clone()), ("sent", "", None));
        // 送っていない user の印は、sent を頼まれても draft（送れなくなるため）
        s.upsert(mark(0)).unwrap();
        assert_eq!(s.set_state(2, "sent").unwrap().state, "draft");
        s.sent(&[2], 50, &[]);
        assert_eq!(s.set_state(2, "sent").unwrap().state, "sent");
        assert!(s.set_state(1, "xx").is_err());
        assert!(s.set_state(99, "done").is_err());
        assert!(s.resolve(99, None, None).is_err());
    }

    #[test]
    fn 直しは色_形_一言_区間だけで_済の記録を消さない() {
        let mut s = MarkStore::default();
        s.upsert(mark(0)).unwrap();
        s.sent(&[1], 5, &[]);
        s.resolve(1, Some("直した".into()), None).unwrap();
        // 画面が古い持ち物（draft）のまま区間だけ直して送ってきても、済は消えない
        let mut stale = mark(1);
        stale.t1 = 9.0;
        stale.note = "直し".into();
        let got = s.upsert(stale).unwrap();
        assert_eq!((got.state.as_str(), got.done_note.as_str(), got.msg, got.t1, got.note.as_str()), ("done", "直した", Some(5), 9.0, "直し"));
        // 新しい印は、何を渡してもユーザーの draft
        let mut n = mark(0);
        n.state = "done".into();
        n.by = "claude".into();
        let n = s.upsert(n).unwrap();
        assert_eq!((n.state.as_str(), n.by.as_str()), ("draft", "user"));
    }

    #[test]
    fn 正しくない印は断る() {
        let mut s = MarkStore::default();
        let mut m = mark(0);
        m.color = "pink".into();
        assert!(s.upsert(m).is_err());
        let mut m = mark(0);
        m.t1 = m.t0;
        assert!(s.upsert(m).is_err());
        let mut m = mark(0);
        m.shape = MarkShape::Pen { points: vec![[0, 0]; 33] };
        assert!(s.upsert(m).is_err());
        assert!(s.file().marks.is_empty());
        assert_eq!(s.file().next, 1);
    }

    #[test]
    fn claudeの印は区間の既定を使う() {
        let scenes = vec![("1".to_string(), 0.0, 10.0), ("2".to_string(), 10.0, 20.0)];
        assert_eq!(default_range(&scenes, 10.0), (10.0, 20.0));
        assert_eq!(default_range(&scenes, 3.0), (0.0, 10.0));
        assert_eq!(default_range(&scenes, 25.0), (23.0, 27.0));
        assert_eq!(default_range(&[], 1.0), (0.0, 3.0));
        let mut s = MarkStore::default();
        let req = |t0, t1| MarkAddReq { color: "yellow".into(), shape: MarkShape::Pin { at: [1, 2] }, note: "ここ".into(), t: 12.0, t0, t1 };
        let m = s.add_by_claude(req(None, None), default_range(&scenes, 12.0)).unwrap();
        assert_eq!((m.id, m.by.as_str(), m.state.as_str(), m.t0, m.t1), (1, "claude", "sent", 10.0, 20.0));
        let m = s.add_by_claude(req(Some(11.0), None), (10.0, 20.0)).unwrap();
        assert_eq!((m.id, m.t0, m.t1), (2, 11.0, 20.0));
        assert!(s.add_by_claude(req(Some(30.0), Some(20.0)), (10.0, 20.0)).is_err());
        assert_eq!(s.file().next, 3, "断った印は番号を使わない");
    }

    #[test]
    fn 層の重ね_透明は元のまま_不透明は層の色_大きさ違いはエラー() {
        let mut base = image::RgbImage::from_pixel(3, 1, image::Rgb([10, 20, 30]));
        let mut layer = image::RgbaImage::from_pixel(3, 1, image::Rgba([0, 0, 0, 0]));
        layer.put_pixel(1, 0, image::Rgba([200, 100, 50, 255]));
        layer.put_pixel(2, 0, image::Rgba([210, 120, 70, 128]));
        overlay_layer(&mut base, &layer).unwrap();
        assert_eq!(base.get_pixel(0, 0).0, [10, 20, 30]);
        assert_eq!(base.get_pixel(1, 0).0, [200, 100, 50]);
        let h = base.get_pixel(2, 0).0;
        assert!((h[0] as i32 - 110).abs() <= 1 && (h[1] as i32 - 70).abs() <= 1 && (h[2] as i32 - 50).abs() <= 1, "{h:?}");
        let mut base = image::RgbImage::new(2, 2);
        let layer = image::RgbaImage::new(3, 2);
        assert!(overlay_layer(&mut base, &layer).is_err());
    }
}
