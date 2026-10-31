//! 演出の透過 .mov・完成版 mp4・録画トラック mp4 の書き出し（ffmpeg へコマを流す）。

use crate::render::cache::{any_group_at, CachedOverlay};
use crate::core::Core;
use crate::render::engine::{Engine, FrameReq};
use crate::settings::hidden;
use super::{progress, CANCEL};
use super::audio::{mixdown, write_wav, SR};
use anyhow::{anyhow, bail, Context, Result};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

// ======================================================================= 演出の透過 .mov
/// 書き出すコマの範囲 (f0, f1)（f1 は含まない）。range が無ければ 0〜end 秒。
/// 頭は四捨五入、終わりは切り上げ（区間の頭が半コマ前に丸まることがある）
fn frame_range(range: Option<(f64, f64)>, fps: f64, end: f64) -> (i64, i64) {
    let (t0, t1) = range.unwrap_or((0.0, end));
    ((t0 * fps).round() as i64, (t1 * fps).ceil() as i64)
}

/// 透過 .mov のファイル名。区間だけの書き出しは、本番の .mov を上書きしないよう区間つきの名前にする
fn mov_name(stem: &str, clean: bool, range: Option<(f64, f64)>) -> String {
    let suffix = range.map(|(a, b)| format!("_{:.0}-{:.0}s", a, b)).unwrap_or_default();
    if clean { format!("{stem}_clean{suffix}.mov") } else { format!("{stem}{suffix}.mov") }
}

/// 時刻 ta〜tb（秒）にかかるシーンの番号（接するだけのものは含めない）
fn scenes_in(list: &[(String, f64, f64)], ta: f64, tb: f64) -> Vec<&str> {
    list.iter().filter(|(_, a, b)| *b > ta && *a < tb).map(|(sid, _, _)| sid.as_str()).collect()
}

/// 時刻 t のシーン番号の札。"-"（どのシーンでもない）は札なし。用意できていなければ失敗（札なしで黙って進めない）。
/// 文に「（コマ n）」を足すのは呼ぶ側
fn label_for<'a, L>(list: &[(String, f64, f64)], labels: &'a BTreeMap<String, L>, t: f64) -> Result<Option<&'a L>, String> {
    let sid = crate::render::scenes::scene_at(list, t);
    match labels.get(&sid) {
        Some(l) => Ok(Some(l)),
        None if sid == "-" => Ok(None),
        None => Err(format!("シーン {sid} の札を用意できていませんでした")),
    }
}

/// 範囲 a0〜a1 秒の音声を取り出すステレオのサンプルの位置 (頭, 終わり)。ミックスの長さ len で止め、頭が後ろでも空の範囲にする
fn sample_range(a0: f64, a1: f64, len: usize) -> (usize, usize) {
    let s0 = (a0 * SR as f64).round() as usize * 2;
    let s1 = ((a1 * SR as f64).round() as usize * 2).min(len);
    (s0.min(s1), s1)
}

/// mp4 の大きさ (幅, 高さ)。倍率を掛けて、エンコーダのため偶数にする
fn mp4_size(scale: f64) -> (u32, u32) {
    (((1920.0 * scale).round() as u32) & !1, ((1080.0 * scale).round() as u32) & !1)
}

/// 書き出し用の一時エンジン。途中で失敗して早く抜けても、Drop で必ずワーカー（Python 最大 12 本）を止める
struct TempEngine(Arc<Engine>);

impl Drop for TempEngine {
    fn drop(&mut self) {
        self.0.shutdown();
    }
}

fn temp_engine(core: &Core, placeholder: bool) -> Result<(TempEngine, Arc<crate::render::engine::Desc>)> {
    let s = core.settings.get();
    let (tl, ovp, clp) = {
        let st = core.st.read();
        let ps = st.as_ref().ok_or_else(crate::app::error::no_project)?;
        (ps.project.fx_timeline().ok_or_else(|| anyhow!("演出のタイムラインがありません"))?, ps.project.overrides_path(),
         ps.project.clips_path())
    };
    let n = (std::thread::available_parallelism().map(|n| n.get()).unwrap_or(8) / 2).clamp(2, 12);
    let e = Engine::with_log(&crate::settings::resolve_python(&s.python), crate::settings::engine_script(), n,
                             core.engine.log.clone(), "書き出し ");
    e.load(&tl, &ovp, &clp, placeholder);
    // 作れなかったワーカーは返事をしないので、動いている本数だけ待つ
    let n = e.workers();
    let t0 = Instant::now();
    loop {
        std::thread::sleep(Duration::from_millis(200));
        let st = e.status.read().clone();
        if st.state == "error" {
            e.shutdown();
            bail!("演出を読み込めませんでした:\n{}", st.error.unwrap_or_default());
        }
        if st.ready_workers >= n {
            break;
        }
        // 一部のワーカーだけ読めないときも、待ち切るまで待たずに理由を出す
        if st.failed_workers > 0 && st.ready_workers + st.failed_workers >= n {
            e.shutdown();
            bail!("演出を読み込めませんでした（ワーカー {n} 本中 {} 本）:\n{}", st.failed_workers, st.worker_error.unwrap_or_default());
        }
        if t0.elapsed() > Duration::from_secs(180) {
            e.shutdown();
            bail!("演出の読み込みが終わりません");
        }
    }
    let e = TempEngine(e);
    let d = e.0.desc.read().clone().ok_or_else(|| anyhow!("演出の内容を受け取れませんでした"))?;
    Ok((e, d))
}

/// 演出のコマを順番どおりに受け取る（並列に描かせて、並べ直す）。
#[allow(clippy::too_many_arguments)]
fn ordered_frames(e: Arc<Engine>, desc: Arc<crate::render::engine::Desc>, ov: Value, f0: i64, f1: i64, scale: f64,
                  mut sink: impl FnMut(i64, Option<crate::render::engine::OverlayFrame>) -> Result<()>)
                  -> Result<()> {
    let n = e.workers().max(1) * 2;
    let next = Arc::new(AtomicI64::new(f0));
    let (tx, rx) = crossbeam_channel::bounded::<(i64, Result<Option<crate::render::engine::OverlayFrame>>)>(n * 4);
    let window = Arc::new(AtomicI64::new(f0 + (n as i64) * 6));
    let mut hs = vec![];
    for _ in 0..n {
        let (e, desc, ov, next, tx, window) = (e.clone(), desc.clone(), ov.clone(), next.clone(), tx.clone(), window.clone());
        hs.push(std::thread::spawn(move || loop {
            if CANCEL.load(Ordering::SeqCst) {
                return;
            }
            let f = next.fetch_add(1, Ordering::SeqCst);
            if f >= f1 {
                return;
            }
            while f > window.load(Ordering::SeqCst) {
                if CANCEL.load(Ordering::SeqCst) {
                    return;
                }
                std::thread::sleep(Duration::from_millis(5));
            }
            let t = f as f64 / desc.fps;
            let any = any_group_at(&desc.groups, &ov, t);
            let r = if any {
                e.frame(FrameReq { t, scale, hide: vec![], solo: None }, false, Duration::from_secs(300)).map(Some)
            } else {
                Ok(None)
            };
            if tx.send((f, r)).is_err() {
                return;
            }
        }));
    }
    drop(tx);
    let mut buf: BTreeMap<i64, Option<crate::render::engine::OverlayFrame>> = BTreeMap::new();
    let mut want = f0;
    while want < f1 {
        if CANCEL.load(Ordering::SeqCst) {
            return Err(crate::app::error::coded(ak_proto::ErrorCode::Canceled, "中止しました"));
        }
        let (f, r) = rx.recv().map_err(|_| anyhow!("ワーカーが止まりました"))?;
        buf.insert(f, r?);
        while let Some(fr) = buf.remove(&want) {
            sink(want, fr)?;
            want += 1;
            window.store(want + (n as i64) * 6, Ordering::SeqCst);
        }
    }
    for h in hs {
        let _ = h.join();
    }
    Ok(())
}

pub(super) fn ffmpeg_or_err(core: &Core) -> Result<String> {
    let f = core.ffmpeg();
    if f.is_empty() {
        return Err(crate::app::error::coded(ak_proto::ErrorCode::NeedFfmpeg, "ffmpeg が見つかりません"));
    }
    Ok(f)
}

/// 生のコマを stdin で流し込む ffmpeg。stderr は別のスレッドで読み続け（読まないとパイプが埋まって ffmpeg も
/// こちらの書き込みも止まる）、最後の 8KB を持っておく。終えるときは ffmpeg が失敗していればその理由を優先する
/// （先に死ぬと書き込み側は broken pipe になり、本当の理由が隠れていた）
pub(super) struct FfPipe {
    child: Option<std::process::Child>,
    err: Arc<std::sync::Mutex<String>>,
    reader: Option<std::thread::JoinHandle<()>>,
}

impl FfPipe {
    pub(super) fn spawn(mut cmd: Command) -> Result<(FfPipe, std::process::ChildStdin)> {
        let mut child = cmd
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .context("ffmpeg を起動できませんでした")?;
        let stdin = child.stdin.take().ok_or_else(|| anyhow!("ffmpeg の入力を開けませんでした"))?;
        let mut stderr = child.stderr.take().ok_or_else(|| anyhow!("ffmpeg の出力を開けませんでした"))?;
        let err = Arc::new(std::sync::Mutex::new(String::new()));
        let err2 = err.clone();
        let reader = std::thread::spawn(move || {
            use std::io::Read;
            let mut buf = [0u8; 4096];
            while let Ok(n) = stderr.read(&mut buf) {
                if n == 0 {
                    break;
                }
                if let Ok(mut e) = err2.lock() {
                    e.push_str(&String::from_utf8_lossy(&buf[..n]));
                    if e.len() > 8192 {
                        let cut = e.len() - 8192;
                        let cut = (cut..e.len()).find(|&i| e.is_char_boundary(i)).unwrap_or(cut);
                        e.drain(..cut);
                    }
                }
            }
        });
        Ok((FfPipe { child: Some(child), err, reader: Some(reader) }, stdin))
    }

    /// stdin を閉じてから呼ぶ。res は書き込み側の結果
    pub(super) fn finish(mut self, res: Result<()>) -> Result<()> {
        let st = self.child.take().map(|mut c| c.wait()).transpose()?;
        if let Some(r) = self.reader.take() {
            let _ = r.join();
        }
        let err = self.err.lock().map(|e| e.trim().to_string()).unwrap_or_default();
        if st.is_some_and(|s| !s.success()) {
            match res {
                Err(e) => bail!("ffmpeg がエラーで終了しました: {err}\n（書き込み側: {e:#}）"),
                Ok(()) => bail!("ffmpeg がエラーで終了しました: {err}"),
            }
        }
        res
    }
}

impl Drop for FfPipe {
    fn drop(&mut self) {
        if let Some(mut c) = self.child.take() {
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}

pub fn export_overlay(core: &Arc<Core>, clean: bool, range: Option<(f64, f64)>) -> Result<Value> {
    CANCEL.store(false, Ordering::SeqCst);
    let ff = ffmpeg_or_err(core)?;
    let task = if clean { "mov_clean" } else { "mov" };
    progress(core, task, 0, 1, "演出を読み込んでいます…");
    let (e, desc) = temp_engine(core, !clean)?;
    let ov = core.overrides();
    let tl = PathBuf::from(&desc.path);
    let stem = tl.file_stem().unwrap().to_string_lossy().to_string();
    let out_dir = tl.parent().unwrap().join("out");
    std::fs::create_dir_all(&out_dir)?;
    // 区間だけの書き出しは、本番の .mov を上書きしないよう区間つきの名前にする
    let out = out_dir.join(mov_name(&stem, clean, range));
    let tmp = out.with_extension("tmp.mov");
    let (w, h) = (desc.w, desc.h);
    // 攻略動画の回は、攻略動画の終わりまで（演出が短くても）
    let (fps, end) = super::export_span(core, Some(desc.fps), desc.end);
    let (f0, f1) = frame_range(range, fps, end);
    // 仮表示つきの版だけ、シーン番号の札を (16, 12) に重ねる。番号は Rust が決め（ずらし・隠すを入れた後の時刻で）、
    // 札の絵はワーカーに描かせる。描けなければ書き出しを失敗にする（札なしで黙って進めない）。
    // 先に（ffmpeg を起こす前に）書き出すコマの時刻（f0〜f1。t0 より半コマ前から始まりうる）にかかる札を全部もらう
    let scene_list = if clean { vec![] } else { crate::render::scenes::scenes(&desc, &ov) };
    let (ta, tb) = (f0 as f64 / fps, f1 as f64 / fps);
    let mut labels: BTreeMap<String, Arc<crate::render::scenes::Rgba>> = BTreeMap::new();
    for sid in scenes_in(&scene_list, ta, tb) {
        labels.insert(sid.to_string(), e.0.scene_label(sid)?);
    }
    let mut cmd = hidden(Command::new(&ff));
    cmd.args(["-y", "-loglevel", "error", "-f", "rawvideo", "-pix_fmt", "rgba", "-s", &format!("{w}x{h}"), "-r",
              &format!("{fps}"), "-i", "-", "-c:v", "qtrle", "-pix_fmt", "argb", "-g", "150"])
        .arg(&tmp);
    let (ffp, mut stdin) = FfPipe::spawn(cmd)?;
    let empty = vec![0u8; (w * h * 4) as usize];
    let started = Instant::now();
    let mut last = Instant::now();
    let res = ordered_frames(e.0.clone(), desc.clone(), ov, f0, f1, 1.0, |f, fr| {
        let label = if clean {
            None
        } else {
            match label_for(&scene_list, &labels, f as f64 / fps) {
                Ok(l) => l,
                Err(e) => bail!("{e}（コマ {f}）"),
            }
        };
        let base = match fr {
            Some(fr) if fr.rect.is_some() => Some(fr.to_full()),
            _ => None,
        };
        match (label, base) {
            (Some(l), base) => {
                let base = base.map(|data| crate::render::scenes::Rgba { w, h, data });
                stdin.write_all(&crate::render::scenes::overlay_label(base, l, w, h).data)?;
            }
            (None, Some(full)) => stdin.write_all(&full)?,
            (None, None) => stdin.write_all(&empty)?,
        }
        if last.elapsed() > Duration::from_millis(500) {
            last = Instant::now();
            let el = started.elapsed().as_secs_f64();
            let done = f - f0 + 1;
            let eta = el / done as f64 * (f1 - f) as f64;
            progress(core, task, done, f1 - f0, &format!("透過 .mov を書き出しています（残り約 {:.0} 秒）", eta));
        }
        Ok(())
    });
    drop(stdin);
    let res = ffp.finish(res);
    drop(e);
    if let Err(err) = res {
        let _ = std::fs::remove_file(&tmp);
        return Err(err);
    }
    let _ = std::fs::remove_file(&out);
    std::fs::rename(&tmp, &out)?;
    let mb = std::fs::metadata(&out).map(|m| m.len() as f64 / 1e6).unwrap_or(0.0);
    progress(core, task, f1 - f0, f1 - f0, "書き出しが完了しました");
    Ok(json!({"path": out, "mb": mb, "sec": started.elapsed().as_secs_f64()}))
}

// ======================================================================= 完成版 mp4
/// 書き出しの 1 コマの内訳（環境変数 AK_EXPORT_PROFILE があるときだけ測ってログに出す）。
/// mark(k) で、前の mark からの時間を k 番目に足す
struct Profile {
    on: bool,
    last: Option<Instant>,
    sum: [f64; 3],
    n: u32,
}

impl Profile {
    fn from_env() -> Profile {
        Profile { on: std::env::var_os("AK_EXPORT_PROFILE").is_some(), last: None, sum: [0.0; 3], n: 0 }
    }

    fn mark(&mut self, k: usize) {
        if !self.on {
            return;
        }
        let now = Instant::now();
        if let Some(l) = self.last {
            self.sum[k] += (now - l).as_secs_f64();
        }
        if k == 0 {
            self.n += 1;
        }
        self.last = Some(now);
    }

    fn summary(&self, names: [&str; 3]) -> Option<String> {
        let n = self.n.max(1) as f64;
        self.on.then(|| names.iter().zip(self.sum).map(|(name, s)| format!("{name} {:.1} ms", s / n * 1e3))
                         .collect::<Vec<_>>().join("・"))
    }
}

/// h264 のエンコーダの指定。NVENC は `-encoders` の一覧に載っていても GPU が無いと使えない（多くの ffmpeg は
/// GPU が無くても一覧に載せる）ので、1 コマだけ実際にエンコードして確かめ、結果を ffmpeg ごとに覚えておく
fn h264_args(ff: &str) -> Vec<&'static str> {
    static NVENC: std::sync::Mutex<Option<(String, bool)>> = std::sync::Mutex::new(None);
    let cached = NVENC.lock().ok().and_then(|g| g.as_ref().filter(|(f, _)| f == ff).map(|(_, ok)| *ok));
    let ok = cached.unwrap_or_else(|| {
        let ok = hidden(Command::new(ff))
            .args(["-hide_banner", "-loglevel", "error", "-f", "lavfi", "-i", "color=c=black:s=256x144", "-frames:v", "1",
                   "-c:v", "h264_nvenc", "-f", "null", "-"])
            .stdin(Stdio::null())
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        if let Ok(mut g) = NVENC.lock() {
            *g = Some((ff.to_string(), ok));
        }
        ok
    });
    if ok {
        vec!["-c:v", "h264_nvenc", "-preset", "p5", "-cq", "19", "-b:v", "0"]
    } else {
        vec!["-c:v", "libx264", "-crf", "18", "-preset", "medium"]
    }
}

pub fn export_mp4(core: &Arc<Core>, range: Option<(f64, f64)>, scale: f64, subs: bool) -> Result<Value> {
    CANCEL.store(false, Ordering::SeqCst);
    let ff = ffmpeg_or_err(core)?;
    let root = core.root().ok_or_else(crate::app::error::no_project)?;
    let task = "mp4";
    progress(core, task, 0, 1, "演出を読み込んでいます…");
    let have_fx = core.desc().is_some();
    let (e, desc) = if have_fx { let (e, d) = temp_engine(core, false)?; (Some(e), Some(d)) } else { (None, None) };
    let layout_end = core.layout_guard().iter().map(|i| i.start + i.dur).fold(0.0, f64::max);
    let own_end = desc.as_ref().map(|d| d.end).unwrap_or(0.0).max(layout_end + 1.0);
    // 攻略動画の回は、fps は 60、終わりは攻略動画の終わり（Core::fps()・timeline_end()）
    let (fps, end) = super::export_span(core, desc.as_ref().map(|d| d.fps), own_end);
    let (t0, t1) = range.unwrap_or((0.0, end));
    let (f0, f1) = frame_range(range, fps, end);
    let out_dir = root.join("studio").join("export");
    std::fs::create_dir_all(&out_dir)?;
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
    let out = out_dir.join(format!("{}_{stamp}.mp4", super::episode_stem(&root)));
    // 中止・失敗のときに途中までの mp4 が本番の名前で残らないよう、一時ファイルに書いてから名前を変える
    let tmp = out.with_extension("tmp.mp4");

    // 音声（範囲の頭から）。映像のコマ（f0〜f1）とちょうど同じ長さにする。
    // -shortest で揃えると、映像の方が少し長いとき最後のコマが落ちていた（旧#75）
    progress(core, task, 0, 1, "音声をミックスしています…");
    let (a0, a1) = (f0 as f64 / fps, f1 as f64 / fps);
    let mix = mixdown(core, a1 + 0.5, true)?;
    let (s0, s1) = sample_range(a0, a1, mix.len());
    let wav = out_dir.join("_mix.wav");
    write_wav(&wav, &mix[s0..s1])?;

    // 字幕（モニターと同じ絵を先にまとめて描いておく）
    if subs {
        progress(core, task, 0, 1, "字幕を描いています…");
        core.prerender_subs(t0, t1, scale, core.subs.workers())?;
    }
    let (w, h) = mp4_size(scale);
    let enc = h264_args(&ff);
    let vf = "format=yuv420p".to_string();
    let mut cmd = hidden(Command::new(&ff));
    cmd.current_dir(&out_dir)
        .args(["-y", "-loglevel", "error", "-f", "rawvideo", "-pix_fmt", "rgb24", "-s", &format!("{w}x{h}"), "-r",
               &format!("{fps}"), "-i", "-", "-i", "_mix.wav", "-vf", &vf])
        .args(&enc)
        .args(["-c:a", "aac", "-b:a", "192k", "-t", &format!("{:.6}", a1 - a0), "-movflags", "+faststart"])
        .arg(&tmp);
    let (ffp, mut stdin) = FfPipe::spawn(cmd)?;
    let started = Instant::now();
    let mut last = Instant::now();
    let mut prof = Profile::from_env();
    let mut write_frame = |f: i64, ov: Option<crate::render::engine::OverlayFrame>| -> Result<()> {
        prof.mark(0);
        let t = f as f64 / fps;
        let ovc = ov.map(|o| Arc::new(CachedOverlay::from_frame(&o)));
        let ovf = |_: f64| -> Result<Option<Arc<CachedOverlay>>> { Ok(ovc.clone()) };
        let fr = core.compose_with(t, scale, &ovf, false, subs, None)?;
        prof.mark(1);
        stdin.write_all(&fr.data)?;
        prof.mark(2);
        if last.elapsed() > Duration::from_millis(500) {
            last = Instant::now();
            let done = f - f0 + 1;
            let eta = started.elapsed().as_secs_f64() / done as f64 * (f1 - f) as f64;
            progress(core, task, done, f1 - f0, &format!("mp4 を書き出しています（残り約 {:.0} 秒）", eta));
        }
        Ok(())
    };
    let res = match (e.as_ref(), desc.clone()) {
        (Some(e), Some(d)) => ordered_frames(e.0.clone(), d, core.overrides(), f0, f1, scale, &mut write_frame),
        _ => (f0..f1).try_for_each(|f| write_frame(f, None)),
    };
    drop(stdin);
    let res = ffp.finish(res);
    if let Some(s) = prof.summary(["演出のコマ待ち", "合成", "ffmpeg への書き込み"]) {
        core.engine.push_log(format!("書き出しの内訳（{} コマ）: {s}", prof.n));
    }
    drop(e);
    let _ = std::fs::remove_file(&wav);
    if let Err(err) = res {
        let _ = std::fs::remove_file(&tmp);
        return Err(err);
    }
    std::fs::rename(&tmp, &out)?;
    progress(core, task, f1 - f0, f1 - f0, "書き出しが完了しました");
    Ok(json!({"path": out, "sec": started.elapsed().as_secs_f64()}))
}

// ======================================================================= 録画トラック mp4
/// RecWindow に差し込んだ録画（ネスト）だけを、黒い 1920x1080 の上の窓の位置に置いた mp4。0 秒始まり・演出と同じ長さ。
/// 今まで Premiere で録画トラックだけを書き出していた material_track.mp4 と同じもの（演出の下に敷く・rec_track.py に渡す）。
pub fn export_rec_track(core: &Arc<Core>) -> Result<Value> {
    CANCEL.store(false, Ordering::SeqCst);
    let ff = ffmpeg_or_err(core)?;
    let root = core.root().ok_or_else(crate::app::error::no_project)?;
    let desc = core.desc().ok_or_else(|| anyhow!("演出がまだ読み込まれていません（録画の窓の位置と長さが分かりません）"))?;
    let doc = core.doc().ok_or_else(crate::app::error::no_project)?;
    if doc.nests.values().all(|n| n.clip_count() == 0) {
        bail!("録画がまだ差し込まれていません。タイムラインの RecWindow をダブルクリックして素材を置いてください");
    }
    let task = "rec_track";
    let (fps, end) = super::export_span(core, Some(desc.fps), desc.end);
    let (_, f1) = frame_range(None, fps, end);
    let out_dir = root.join("studio").join("export");
    std::fs::create_dir_all(&out_dir)?;
    let out = out_dir.join("material_track.mp4");
    let tmp = out_dir.join("_material_track.tmp.mp4");
    let enc = h264_args(&ff);
    let mut cmd = hidden(Command::new(&ff));
    cmd.args(["-y", "-loglevel", "error", "-f", "rawvideo", "-pix_fmt", "rgb24", "-s", "1920x1080", "-r",
              &format!("{fps}"), "-i", "-"])
        .args(&enc)
        .args(["-pix_fmt", "yuv420p", "-movflags", "+faststart"])
        .arg(&tmp);
    let (ffp, mut stdin) = FfPipe::spawn(cmd)?;
    let black = vec![0u8; 1920 * 1080 * 3];
    let started = Instant::now();
    let mut last = Instant::now();
    let mut res: Result<()> = Ok(());
    for f in 0..f1 {
        if CANCEL.load(Ordering::SeqCst) {
            res = Err(crate::app::error::coded(ak_proto::ErrorCode::Canceled, "中止しました"));
            break;
        }
        let t = f as f64 / fps;
        let mut fr = crate::render::media::Rgb { w: 1920, h: 1080, data: black.clone() };
        core.draw_nests(&mut fr, &doc, t, 1.0, false);
        if let Err(e) = stdin.write_all(&fr.data) {
            res = Err(e.into());
            break;
        }
        if last.elapsed() > Duration::from_millis(500) {
            last = Instant::now();
            let eta = started.elapsed().as_secs_f64() / (f + 1) as f64 * (f1 - f) as f64;
            progress(core, task, f + 1, f1, &format!("録画トラックを書き出しています（残り約 {:.0} 秒）", eta));
        }
    }
    drop(stdin);
    if let Err(e) = ffp.finish(res) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    let _ = std::fs::remove_file(&out);
    std::fs::rename(&tmp, &out)?;
    progress(core, task, f1, f1, "書き出しが完了しました");
    Ok(json!({"path": out, "sec": started.elapsed().as_secs_f64(), "end": end}))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 書き込み先が先に死んだら、broken pipe ではなく相手の stderr（本当の理由）を返す
    #[test]
    fn ffpipe_prefers_child_error() {
        let py = crate::settings::resolve_python("python");
        let mut cmd = Command::new(&py);
        cmd.args(["-c", "import sys; sys.stderr.write('Unknown encoder boom'); sys.exit(1)"]);
        let Ok((ffp, mut stdin)) = FfPipe::spawn(cmd) else { return };
        std::thread::sleep(Duration::from_millis(800));
        let big = vec![0u8; 1 << 20];
        let res: Result<()> = (0..64).try_for_each(|_| stdin.write_all(&big).map_err(Into::into));
        drop(stdin);
        let e = format!("{:#}", ffp.finish(res).unwrap_err());
        assert!(e.contains("Unknown encoder boom"), "{e}");
    }

    #[test]
    fn frame_range_rounds_head_and_ceils_tail() {
        // 範囲なしは 0〜end
        assert_eq!(frame_range(None, 30.0, 10.0), (0, 300));
        assert_eq!(frame_range(None, 60.0, 2.5), (0, 150));
        // 終わりは切り上げ（30 fps の 1.01 秒 = 30.3 → 31）、頭は四捨五入（0.51 秒 = 15.3 → 15）
        assert_eq!(frame_range(Some((0.51, 1.01)), 30.0, 99.0), (15, 31));
        // 60 fps の端数
        assert_eq!(frame_range(Some((0.51, 1.01)), 60.0, 99.0), (31, 61));
        // 頭が半コマ前に丸まる（0.016 秒 × 30 = 0.48 → 0）
        assert_eq!(frame_range(Some((0.016, 1.0)), 30.0, 99.0), (0, 30));
    }

    #[test]
    fn mov_name_marks_range_and_clean() {
        assert_eq!(mov_name("an1", false, None), "an1.mov");
        assert_eq!(mov_name("an1", true, None), "an1_clean.mov");
        assert_eq!(mov_name("an1", false, Some((10.0, 20.0))), "an1_10-20s.mov");
        assert_eq!(mov_name("an1", true, Some((10.0, 20.0))), "an1_clean_10-20s.mov");
        // 区間の秒は {:.0} の丸め（0.4 → 0、19.6 → 20）
        assert_eq!(mov_name("an1", false, Some((0.4, 19.6))), "an1_0-20s.mov");
    }

    fn scene_list() -> Vec<(String, f64, f64)> {
        vec![("1".into(), 0.0, 5.0), ("2".into(), 5.0, 10.0)]
    }

    #[test]
    fn scenes_in_excludes_touching_edges() {
        let l = scene_list();
        assert_eq!(scenes_in(&l, 0.0, 10.0), vec!["1", "2"]);
        // ちょうど 5.0 で接するだけのものは入らない
        assert_eq!(scenes_in(&l, 5.0, 8.0), vec!["2"]);
        assert_eq!(scenes_in(&l, 0.0, 5.0), vec!["1"]);
        assert_eq!(scenes_in(&l, 4.9, 5.1), vec!["1", "2"]);
        assert!(scenes_in(&l, 10.0, 12.0).is_empty());
    }

    #[test]
    fn label_for_none_some_or_error() {
        let l = scene_list();
        let mut labels: BTreeMap<String, u32> = BTreeMap::new();
        labels.insert("1".into(), 11);
        // 札がある
        assert_eq!(label_for(&l, &labels, 1.0), Ok(Some(&11)));
        // "-"（どのシーンでもない）は札なし
        assert_eq!(label_for(&l, &labels, 12.0), Ok(None));
        // 用意できていない
        assert_eq!(label_for(&l, &labels, 6.0), Err("シーン 2 の札を用意できていませんでした".to_string()));
    }

    #[test]
    fn sample_range_clamps_and_never_panics() {
        let sr = SR as usize;
        let n = sr * 2 * 10;
        assert_eq!(sample_range(1.0, 2.0, n), (sr * 2, sr * 4));
        // ミックスより長い範囲は len で止まる
        assert_eq!(sample_range(9.0, 20.0, n), (sr * 18, n));
        // 頭がミックスの後ろでも空の範囲（s0 == s1）
        let (s0, s1) = sample_range(30.0, 31.0, n);
        assert_eq!((s0, s1), (n, n));
        assert!(vec![0i16; n][s0..s1].is_empty());
    }

    #[test]
    fn mp4_size_is_even() {
        assert_eq!(mp4_size(1.0), (1920, 1080));
        assert_eq!(mp4_size(2.0 / 3.0), (1280, 720));
        // 丸めて奇数になる倍率は 1 つ落として偶数（1920*0.4 = 768、1080*0.4 = 432 は偶数。0.3337 → 641 → 640、360）
        assert_eq!(mp4_size(0.3337), (640, 360));
        // 0.5009: 961.7 → 962、541.0 → 541 → 540
        assert_eq!(mp4_size(0.5009), (962, 540));
    }
}
