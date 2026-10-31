//! 書き出し。
//!
//! - 台本 → 演出用の入力（files/script.srt と files/talk/<話者>/<番号>.wav,.txt）＋ voice_out/（フルWAV・話者別・SRT）
//! - 演出の透過 .mov（Premiere の頭に置く。微調整入り。fx/out/ に）
//! - 効果音の wav（fx/out/<名前>_se.wav ＋ se_stems/）
//! - 完成版の mp4（背景・録画・立ち絵・演出・字幕・声・効果音・BGM を全部重ねたもの）
//! - 字幕だけの透過 .mov と .srt（字幕はモニターと同じ絵。Premiere の頭に置くだけで合う）

pub mod audio;
pub mod subs;
pub mod talk;
pub mod video;

use audio::{export_guide_audio, export_mix_wav, export_se};
use subs::export_subs;
use talk::export_talk;
use video::{export_mp4, export_overlay, export_rec_track};

use crate::core::Core;
use crate::voice::layout::VoiceItem;
use anyhow::{anyhow, bail, Result};
use serde_json::{json, Value};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

/// 出力ファイル名に使う回の名前（回のフォルダ名。ドライブの直下など名前が無いときは "export"）
pub(crate) fn episode_stem(root: &std::path::Path) -> String {
    root.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "export".into())
}

pub static CANCEL: AtomicBool = AtomicBool::new(false);
/// 中止が押された回数。`run` を通らない作り直し（効果音の設定の保存）は CANCEL が前の中止のまま立っていることがあるので、
/// 始めたときの値と比べて「始めた後に中止が押されたか」を見る
pub static CANCEL_EPOCH: AtomicU64 = AtomicU64::new(0);

/// 書き出し・作り直しの中止（画面の「中止」から）
pub fn cancel() {
    CANCEL.store(true, Ordering::SeqCst);
    CANCEL_EPOCH.fetch_add(1, Ordering::SeqCst);
}
/// 実行中の書き出しの種類（同時に 1 つだけ。2 つ目は中止の印を消し、_mix.wav を取りあうので断る）
static RUNNING: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);
/// 今の書き出しで進捗を出した task（終わったときに finished を送る）
static SHOWN: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

fn progress(core: &Core, task: &str, done: i64, total: i64, msg: &str) {
    if let Ok(mut s) = SHOWN.lock() {
        if !s.iter().any(|t| t == task) {
            s.push(task.to_string());
        }
    }
    core.emit("progress", json!({"task": task, "done": done, "total": total, "msg": msg}));
}

/// 書き出しの入り口（画面のコマンドと Claude の HTTP API の両方から）。
/// kind: talk / mov / mov_clean / se / mp4 / preview_mp4 / subs_mov / subs_srt / mix / rec_track / guide_audio
pub fn run(core: &Arc<Core>, kind: &str, from: Option<f64>, to: Option<f64>) -> Result<Value> {
    let range = match (from, to) {
        (None, None) => None,
        (Some(a), Some(b)) if b > a => Some((a, b)),
        (Some(a), Some(b)) => bail!("書き出しの範囲が正しくありません（{a:.2} 〜 {b:.2} 秒）"),
        _ => bail!("書き出しの範囲は、開始と終わりの両方を指定してください"),
    };
    {
        let mut r = RUNNING.lock().map_err(|_| anyhow!("書き出しの状態が壊れています"))?;
        if let Some(k) = r.as_ref() {
            bail!("ほかの書き出し（{k}）を実行中です。終わるか中止してから始めてください");
        }
        *r = Some(kind.to_string());
    }
    if let Ok(mut s) = SHOWN.lock() {
        s.clear();
    }
    CANCEL.store(false, Ordering::SeqCst);
    // 成功・失敗・中止（panic も）のどれでも、出した進捗を閉じて実行中の印を外す
    struct Finish<'a> {
        core: &'a Core,
        ok: bool,
        msg: &'static str,
    }
    impl Drop for Finish<'_> {
        fn drop(&mut self) {
            let tasks: Vec<String> = SHOWN.lock().map(|mut s| s.drain(..).collect()).unwrap_or_default();
            for t in tasks {
                self.core.emit("progress", json!({"task": t, "done": self.ok as i64, "total": 1, "msg": self.msg, "finished": true}));
            }
            crate::core::collect_issues_here(false);
            if let Ok(mut r) = RUNNING.lock() {
                *r = None;
            }
        }
    }
    let mut fin = Finish { core, ok: false, msg: "書き出せませんでした" };
    *core.export_issues.lock() = Some(vec![]);
    crate::core::collect_issues_here(true);
    // 攻略動画の回は、効果音が今の操作のものになってから書き出す（今のものなら作り直さない。ほかの回は何もしない）
    let fresh_se = |f: &dyn Fn() -> Result<Value>| -> Result<Value> {
        core.guide_se_ensure()?;
        core.guide_se_require()?;
        f()
    };
    let se_sig = if kind == "se" { core.guide_se_sig() } else { None };
    let res = match kind {
        "talk" => export_talk(core),
        "mov" => export_overlay(core, false, range),
        "mov_clean" => export_overlay(core, true, range),
        "se" => export_se(core).inspect(|_| core.guide_se_record(se_sig)),
        "mp4" => fresh_se(&|| export_mp4(core, range, 1.0, true)),
        "preview_mp4" => fresh_se(&|| export_mp4(core, range, 2.0 / 3.0, true)),
        "subs_mov" => export_subs(core, range, true),
        "subs_srt" => export_subs(core, range, false),
        "mix" => fresh_se(&|| export_mix_wav(core)),
        "rec_track" => export_rec_track(core),
        "guide_audio" => export_guide_audio(core, range),
        k => Err(anyhow!("知らない書き出し: {k}")),
    };
    // 飛ばしたもの（読めない声・素材・字幕など）は、書き出しは続けて結果の警告に出す
    let issues = core.export_issues.lock().take().unwrap_or_default();
    let res = res.map(|v| with_issues(v, issues));
    fin.ok = res.is_ok();
    fin.msg = match &res {
        Ok(_) => "書き出しが完了しました",
        Err(_) if CANCEL.load(Ordering::SeqCst) => "中止しました",
        Err(_) => "書き出せませんでした",
    };
    res
}

/// 結果の warnings の後ろに、飛ばしたもの（メッセージ, 回数）を足す。2 回以上は「（n 回）」つき。
/// 飛ばしたものが無い・結果が object でないときは変えない
fn with_issues(mut v: Value, issues: Vec<(String, usize)>) -> Value {
    if !issues.is_empty() && v.is_object() {
        let mut w: Vec<Value> = v.get("warnings").and_then(|x| x.as_array()).cloned().unwrap_or_default();
        for (m, n) in issues {
            w.push(Value::from(if n > 1 { format!("{m}（{n} 回）") } else { m }));
        }
        v["warnings"] = Value::Array(w);
    }
    v
}

/// 書き出しの fps と終わり（秒）の決め方（純粋）。
/// 攻略動画の回（guide_fps が Some）は、演出の fps（無ければ組み立ての fps）と、タイムラインの終わり（攻略動画の終わりを含む）。
/// そうでなければ今までどおり、演出の fps（無ければ 30）と fallback_end（各書き出しが決める終わり）
pub(crate) fn span_for(guide_fps: Option<f64>, desc_fps: Option<f64>, timeline_end: f64, fallback_end: f64) -> (f64, f64) {
    match guide_fps {
        Some(g) => (desc_fps.unwrap_or(g), timeline_end.max(fallback_end)),
        None => (desc_fps.unwrap_or(30.0), fallback_end),
    }
}

/// 書き出しの fps と終わり。desc_fps は今の演出の fps、fallback_end は攻略動画でない回の終わり。
/// 攻略動画の回は `Core::fps()`・`timeline_end()`（`guide_end` を含む）で決める
pub(crate) fn export_span(core: &Core, desc_fps: Option<f64>, fallback_end: f64) -> (f64, f64) {
    let guide = core.guide_fps();
    let tl_end = if guide.is_some() { core.doc().map(|d| core.timeline_end(&d)).unwrap_or(fallback_end) } else { fallback_end };
    span_for(guide, desc_fps, tl_end, fallback_end)
}

/// 秒 → ミリ秒（script.srt と script_cues.json で同じ丸め）
fn ms(sec: f64) -> i64 {
    (sec * 1000.0).round().max(0.0) as i64
}

fn srt_time(sec: f64) -> String {
    let ms = ms(sec);
    format!("{:02}:{:02}:{:02},{:03}", ms / 3_600_000, ms / 60_000 % 60, ms / 1000 % 60, ms % 1000)
}

fn sorted_layout(core: &Core) -> Vec<VoiceItem> {
    core.sorted_layout()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn episode_stem_falls_back_without_a_folder_name() {
        assert_eq!(episode_stem(std::path::Path::new("/tmp/an/an389_x")), "an389_x");
        assert_eq!(episode_stem(std::path::Path::new("E:/")), "export");
    }

    /// 攻略動画の回は演出の fps（無ければ 60）と攻略動画の終わりまで。ほかの回は今までどおり
    #[test]
    fn span_follows_guide_end_and_fps() {
        // 攻略動画: 演出が短くても（desc の終わり 1 秒）タイムラインの終わりまで。演出が無ければ組み立ての 60 fps
        assert_eq!(span_for(Some(60.0), Some(60.0), 313.0, 1.0), (60.0, 313.0));
        assert_eq!(span_for(Some(60.0), None, 313.0, 1.0), (60.0, 313.0));
        // 演出の fps があればそれ（演出のコマと合わせる）
        assert_eq!(span_for(Some(60.0), Some(30.0), 313.0, 1.0), (30.0, 313.0));
        // 各書き出しが決めた終わりのほうが長ければ、それ
        assert_eq!(span_for(Some(60.0), Some(60.0), 10.0, 20.0), (60.0, 20.0));
        // 攻略動画でない回は今のまま（演出の fps・無ければ 30。終わりは書き出しごと）
        assert_eq!(span_for(None, Some(60.0), 313.0, 90.0), (60.0, 90.0));
        assert_eq!(span_for(None, None, 313.0, 1.0), (30.0, 1.0));
    }

    #[test]
    fn with_issues_appends_warnings() {
        // 空なら変えない
        let v = json!({"path": "a"});
        assert_eq!(with_issues(v.clone(), vec![]), v);
        // warnings が無ければ作り、1 回は回数なし・2 回以上は「（n 回）」
        let r = with_issues(json!({"path": "a"}), vec![("読めない声".into(), 1), ("素材".into(), 3)]);
        assert_eq!(r["warnings"], json!(["読めない声", "素材（3 回）"]));
        // 今の warnings の後ろに足す
        let r = with_issues(json!({"warnings": ["前"]}), vec![("後".into(), 2)]);
        assert_eq!(r["warnings"], json!(["前", "後（2 回）"]));
        // object でない値は変えない
        assert_eq!(with_issues(json!("x"), vec![("a".into(), 1)]), json!("x"));
    }

    #[test]
    fn cancel_sets_flag_and_bumps_epoch() {
        let before = CANCEL_EPOCH.load(Ordering::SeqCst);
        cancel();
        assert!(CANCEL.load(Ordering::SeqCst));
        assert!(CANCEL_EPOCH.load(Ordering::SeqCst) > before);
        CANCEL.store(false, Ordering::SeqCst);
    }
}
