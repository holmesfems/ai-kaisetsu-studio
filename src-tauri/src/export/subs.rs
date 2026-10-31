//! 字幕だけの透過 .mov と .srt の書き出し。

use crate::core::Core;
use crate::settings::hidden;
use super::{progress, sorted_layout, srt_time, CANCEL};
use super::video::{ffmpeg_or_err, FfPipe};
use anyhow::{bail, Result};
use serde_json::{json, Value};
use std::io::Write;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};

// ======================================================================= 字幕だけ（透過 .mov と .srt）
/// 字幕の .srt（手で入れた改行・自動の折り返し・字幕だけの書き換えをそのまま入れる）。時刻は t0 からの秒。
fn subs_srt(core: &Core, t0: f64, t1: f64) -> Result<String> {
    let doc = core.doc().ok_or_else(crate::app::error::no_project)?;
    let items = sorted_layout(core);
    let spans = crate::render::subs::spans(&items);
    let jobs: Vec<_> = items.iter().map(|it| crate::render::subs::resolve(&doc, it)).collect();
    let infos = core.subs.measure(&jobs)?;
    let mut s = String::new();
    let mut k = 0;
    for (i, (a, b)) in spans.iter().enumerate() {
        if *b <= t0 || *a >= t1 {
            continue;
        }
        let lines: Vec<&str> = infos[i].texts.iter().map(|x| x.as_str()).filter(|x| !x.trim().is_empty()).collect();
        let text = if lines.is_empty() { jobs[i].text.clone() } else { lines.join("\n") };
        if text.trim().is_empty() {
            continue;
        }
        k += 1;
        s += &format!("{}\n{} --> {}\n{}\n\n", k, srt_time(a.max(t0) - t0), srt_time(b.min(t1) - t0), text);
    }
    Ok(s)
}

/// 字幕の書き出し。mov なら 0 秒始まりの透過 qtrle（1920x1080、演出の .mov と同じ形）と .srt、そうでなければ .srt だけ。
/// 演出のタイムラインがあれば fx/out/<名前>_subs.mov、無ければ studio/export/<回>_subs.mov。
pub fn export_subs(core: &Arc<Core>, range: Option<(f64, f64)>, mov: bool) -> Result<Value> {
    CANCEL.store(false, Ordering::SeqCst);
    let root = core.root().ok_or_else(crate::app::error::no_project)?;
    let doc = core.doc().ok_or_else(crate::app::error::no_project)?;
    if crate::render::subs::hidden(&doc) {
        bail!("字幕のトラックが非表示です。タイムラインの「字幕」の 👁 で表示してください");
    }
    let items = sorted_layout(core);
    if items.is_empty() {
        bail!("書き出す字幕がありません");
    }
    let task = if mov { "subs_mov" } else { "subs_srt" };
    let desc = core.desc();
    let layout_end = items.iter().map(|i| i.start + i.dur + crate::render::subs::TAIL).fold(0.0, f64::max);
    let own_end = desc.as_ref().map(|d| d.end).unwrap_or(0.0).max(layout_end + 0.5);
    let (fps, end) = super::export_span(core, desc.as_ref().map(|d| d.fps), own_end);
    let (t0, t1) = range.unwrap_or((0.0, end));
    let (out_dir, stem) = match &desc {
        Some(d) => {
            let tl = PathBuf::from(&d.path);
            (tl.parent().unwrap().join("out"), tl.file_stem().unwrap().to_string_lossy().to_string())
        }
        None => (root.join("studio").join("export"), super::episode_stem(&root)),
    };
    std::fs::create_dir_all(&out_dir)?;
    // 区間だけの書き出しは、全体の .mov を上書きしないよう区間つきの名前にする
    let suffix = range.map(|(a, b)| format!("_{:.0}-{:.0}s", a, b)).unwrap_or_default();
    let srt_p = out_dir.join(format!("{stem}_subs{suffix}.srt"));
    progress(core, task, 0, 1, "字幕を描いています…");
    std::fs::write(&srt_p, subs_srt(core, t0, t1)?)?;
    if !mov {
        progress(core, task, 1, 1, "書き出しが完了しました");
        return Ok(json!({"path": srt_p, "srt": srt_p, "cues": items.len()}));
    }
    let ff = ffmpeg_or_err(core)?;
    let started = Instant::now();
    core.prerender_subs(t0, t1, 1.0, core.subs.workers())?;
    let out = out_dir.join(format!("{stem}_subs{suffix}.mov"));
    let tmp = out.with_extension("tmp.mov");
    let (w, h) = (1920u32, 1080u32);
    let f0 = (t0 * fps).round() as i64;
    let f1 = (t1 * fps).ceil() as i64;
    let mut cmd = hidden(Command::new(&ff));
    cmd.args(["-y", "-loglevel", "error", "-f", "rawvideo", "-pix_fmt", "rgba", "-s", &format!("{w}x{h}"), "-r",
              &format!("{fps}"), "-i", "-", "-c:v", "qtrle", "-pix_fmt", "argb", "-g", "150"])
        .arg(&tmp);
    let (ffp, mut stdin) = FfPipe::spawn(cmd)?;
    // 字幕が変わったコマだけ描き直す（同じ字幕の間は同じ絵を送る）
    let mut full = vec![0u8; (w * h * 4) as usize];
    // 今の絵を持っておく（Arc を手放すと、同じ番地に別の字幕が入ることがあるので比べられない）
    let mut cur: Option<Arc<crate::render::subs::SubBitmap>> = None;
    let mut last = Instant::now();
    let res = (|| -> Result<()> {
        for f in f0..f1 {
            if CANCEL.load(Ordering::SeqCst) {
                return Err(crate::app::error::coded(ak_proto::ErrorCode::Canceled, "中止しました"));
            }
            let t = f as f64 / fps;
            let b = core.sub_bitmap_at(t, 1.0)?.filter(|b| !b.is_empty());
            let same = match (&b, &cur) {
                (Some(a), Some(c)) => Arc::ptr_eq(a, c),
                (None, None) => true,
                _ => false,
            };
            if !same {
                if let Some(c) = cur.take() {
                    clear_rect(&mut full, w, h, c.x, c.y, c.w, c.h);
                }
                if let Some(b) = &b {
                    put_rgba(&mut full, w, h, &b.rgba(), b.x, b.y);
                }
                cur = b;
            }
            stdin.write_all(&full)?;
            if last.elapsed() > Duration::from_millis(500) {
                last = Instant::now();
                let done = f - f0 + 1;
                let eta = started.elapsed().as_secs_f64() / done as f64 * (f1 - f) as f64;
                progress(core, task, done, f1 - f0, &format!("字幕の透過 .mov を書き出しています（残り約 {:.0} 秒）", eta));
            }
        }
        Ok(())
    })();
    drop(stdin);
    if let Err(err) = ffp.finish(res) {
        let _ = std::fs::remove_file(&tmp);
        return Err(err);
    }
    let _ = std::fs::remove_file(&out);
    std::fs::rename(&tmp, &out)?;
    let mb = std::fs::metadata(&out).map(|m| m.len() as f64 / 1e6).unwrap_or(0.0);
    progress(core, task, f1 - f0, f1 - f0, "書き出しが完了しました");
    Ok(json!({"path": out, "srt": srt_p, "cues": items.len(), "mb": mb, "sec": started.elapsed().as_secs_f64()}))
}

/// 透過のコマ（RGBA、w×h）の矩形を透明に戻す。
fn clear_rect(full: &mut [u8], w: u32, h: u32, x: i64, y: i64, bw: u32, bh: u32) {
    let x0 = x.max(0) as usize;
    let x1 = (x + bw as i64).clamp(0, w as i64) as usize;
    let y0 = y.max(0) as usize;
    let y1 = (y + bh as i64).clamp(0, h as i64) as usize;
    if x0 >= x1 {
        return;
    }
    for yy in y0..y1 {
        let i = (yy * w as usize + x0) * 4;
        full[i..i + (x1 - x0) * 4].fill(0);
    }
}

/// 透明なコマに RGBA をそのまま置く（下は透明なので、重ねずに写すだけ。はみ出しは切る）。
fn put_rgba(full: &mut [u8], w: u32, h: u32, src: &crate::render::media::Rgba, x: i64, y: i64) {
    let x0 = x.max(0);
    let x1 = (x + src.w as i64).min(w as i64);
    if x0 >= x1 {
        return;
    }
    for sy in 0..src.h as i64 {
        let yy = y + sy;
        if yy < 0 || yy >= h as i64 {
            continue;
        }
        let si = ((sy * src.w as i64 + (x0 - x)) * 4) as usize;
        let di = ((yy * w as i64 + x0) * 4) as usize;
        let n = ((x1 - x0) * 4) as usize;
        full[di..di + n].copy_from_slice(&src.data[si..si + n]);
    }
}
