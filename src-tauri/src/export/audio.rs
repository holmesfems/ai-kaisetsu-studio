//! 効果音・声・BGM のミックスと wav の書き出し。

use super::CANCEL_EPOCH;
use crate::core::Core;
use crate::render::engine::{parse_warnings, spawn_proc, Proc};
use crate::settings::hidden;
use anyhow::{anyhow, bail, Result};
use crossbeam_channel::{Receiver, RecvTimeoutError};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// 効果音を作る間の上限（演出の読み込み込み）。過ぎたらワーカーを止めてエラーにする
const SE_LIMIT: Duration = Duration::from_secs(600);
/// 効果音の作り直しは 1 本ずつ（同じ out/<名前>_se.wav を書くので。重なったら後のものが最後に書く）
static SE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// 作り直しの進捗の帯（上の「中止」）を、成功・失敗・中止・panic のどれでも閉じる
struct SeProgress<'a> {
    core: &'a Core,
    ok: bool,
    msg: &'static str,
}

impl Drop for SeProgress<'_> {
    fn drop(&mut self) {
        self.core.emit("progress", json!({"task": "se", "done": self.ok as i64, "total": 1, "msg": self.msg, "finished": true}));
    }
}

/// 効果音（fx/out/<名前>_se.wav ＋ se_stems/）を作る。
/// モニターと同じ演出エンジンは使わず、効果音だけの使い捨てのワーカーを 1 本立てる（作っている間もモニターが止まらず、
/// 書き出しの中止でプロセスごと止められる）。毎回ファイルから演出を読むので、いつも今の演出の効果音になる。
pub fn export_se(core: &Arc<Core>) -> Result<Value> {
    let desc = core.desc().ok_or_else(|| anyhow!(crate::msg::NO_FX))?;
    let tl = PathBuf::from(&desc.path);
    let stem = tl.file_stem().unwrap().to_string_lossy().to_string();
    let out_dir = tl.parent().unwrap().join("out");
    let epoch = CANCEL_EPOCH.load(Ordering::SeqCst);
    // ほかの作り直しが終わるのを待つ間も、中止は効く
    let _guard = loop {
        match SE_LOCK.try_lock() {
            Ok(g) => break g,
            Err(std::sync::TryLockError::Poisoned(p)) => break p.into_inner(),
            Err(std::sync::TryLockError::WouldBlock) => {
                if CANCEL_EPOCH.load(Ordering::SeqCst) != epoch {
                    return Err(crate::app::error::coded(ak_proto::ErrorCode::Canceled, "中止しました"));
                }
                std::thread::sleep(Duration::from_millis(100));
            }
        }
    };
    core.emit("progress", json!({"task": "se", "done": 0, "total": 1, "msg": "効果音を作っています"}));
    let mut fin = SeProgress { core, ok: false, msg: "効果音を作れませんでした" };
    let (ovp, clp) = {
        let st = core.st.read();
        let ps = st.as_ref().ok_or_else(crate::app::error::no_project)?;
        (ps.project.overrides_path(), ps.project.clips_path())
    };
    let python = crate::settings::resolve_python(&core.settings.get().python);
    let proc = spawn_proc(&python, &crate::settings::engine_script(), core.engine.log.clone(), "効果音 ")?;
    let killer = proc.killer();
    let pid = proc.pid();
    let load = json!({"op": "load", "timeline": desc.path, "overrides": ovp, "clips": clp,
                      "placeholder": core.placeholder()});
    // 攻略動画の操作の効果音の棚（全部の回で使う）の場所と選びも渡す
    let se = crate::sebank::with_shelf(core, json!({"op": "se", "out_dir": &out_dir, "name": stem, "profile": "full"}));
    let (tx, rx) = crossbeam_channel::bounded(1);
    let h = std::thread::spawn(move || {
        let _ = tx.send(run_se_worker(proc, &load, &se));
    });
    let res = wait_for_result(&rx, epoch, || killer.kill(), SE_LIMIT);
    let _ = h.join();
    if res.is_err() {
        // 止めたワーカーは後片付け（akfx_worker.py の se_render の finally）をしないので、作りかけの一時フォルダを消す
        // （ワーカーはもう止まっていて、SE_LOCK を持っているのでほかに書く者はいない）
        let _ = std::fs::remove_dir_all(out_dir.join(format!(".se_tmp_{pid}")));
    }
    if CANCEL_EPOCH.load(Ordering::SeqCst) != epoch {
        fin.msg = "中止しました";
    }
    let res = res?;
    fin.ok = true;
    fin.msg = "効果音を作りました";
    Ok(res)
}

/// 使い捨てのワーカーに load → se を出して、終わったら止める
fn run_se_worker(mut p: Proc, load: &Value, se: &Value) -> Result<Value> {
    let call = |p: &mut Proc, req: &Value, what: &str| -> Result<Value> {
        let (h, _) = p.call(req)?;
        if h.get("ok").and_then(|v| v.as_bool()) != Some(true) {
            bail!("{what}: {}", h.get("error").and_then(|v| v.as_str()).unwrap_or("理由が分かりません"));
        }
        Ok(h)
    };
    // 読み込みの警告（効果音の付け方を調べられなかったクリップなど）は、その分の効果音が欠けているので結果の警告に足す
    let r = call(&mut p, load, "演出を読み込めませんでした").and_then(|l| {
        let mut h = call(&mut p, se, "効果音を作れませんでした")?;
        let mut w: Vec<Value> = parse_warnings(l.get("warnings").unwrap_or(&Value::Null))
            .into_iter().map(|w| Value::from(w.message)).collect();
        // 効果音の棚の音を使えなかったとき（合成の音で鳴らした）の知らせ
        if let Some(a) = h.get("warnings").and_then(|v| v.as_array()) {
            w.extend(a.iter().cloned());
        }
        if !w.is_empty() {
            h["warnings"] = Value::Array(w);
        }
        Ok(h)
    });
    if r.is_ok() {
        let _ = p.call(&json!({"op": "quit"}));
    }
    p.kill();
    r
}

/// 結果が来るまで 100 ms ごとに待つ。epoch が変わったら（始めた後に中止が押された）、上限を過ぎたら、`kill` で止めてエラーにする
fn wait_for_result<T>(rx: &Receiver<Result<T>>, epoch: u64, kill: impl Fn(), limit: Duration) -> Result<T> {
    let t0 = Instant::now();
    loop {
        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(r) => return r,
            Err(RecvTimeoutError::Disconnected) => bail!("効果音のワーカーが突然止まりました"),
            Err(RecvTimeoutError::Timeout) => {
                let cancelled = CANCEL_EPOCH.load(Ordering::SeqCst) != epoch;
                if cancelled || t0.elapsed() > limit {
                    kill();
                    // 止まって call が抜けるのを待つ（残りのワーカーのスレッドを残さない）
                    let _ = rx.recv_timeout(Duration::from_secs(5));
                    if cancelled {
                        return Err(crate::app::error::coded(ak_proto::ErrorCode::Canceled, "中止しました"));
                    }
                    bail!("効果音の作成が {} 秒以内に終わらないので止めました", limit.as_secs());
                }
            }
        }
    }
}

// ======================================================================= 音声のミックス
pub(super) const SR: u32 = 48000;

fn read_wav_stereo(path: &Path) -> Result<Vec<f32>> {
    let mut r = hound::WavReader::open(path)?;
    let spec = r.spec();
    let ch = spec.channels as usize;
    let samples: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Int => {
            let scale = 1.0 / (1i64 << (spec.bits_per_sample - 1)) as f32;
            r.samples::<i32>().map(|s| s.unwrap_or(0) as f32 * scale).collect()
        }
        hound::SampleFormat::Float => r.samples::<f32>().map(|s| s.unwrap_or(0.0)).collect(),
    };
    let frames = samples.len() / ch.max(1);
    let mut st = Vec::with_capacity(frames * 2);
    for i in 0..frames {
        let l = samples[i * ch];
        let rr = if ch > 1 { samples[i * ch + 1] } else { l };
        st.push(l);
        st.push(rr);
    }
    if spec.sample_rate != SR {
        st = resample(&st, spec.sample_rate, SR);
    }
    Ok(st)
}

fn resample(st: &[f32], from: u32, to: u32) -> Vec<f32> {
    let n = st.len() / 2;
    let m = (n as u64 * to as u64 / from as u64) as usize;
    let mut out = Vec::with_capacity(m * 2);
    for i in 0..m {
        let x = i as f64 * from as f64 / to as f64;
        let i0 = x.floor() as usize;
        let k = (x - i0 as f64) as f32;
        let i1 = (i0 + 1).min(n.saturating_sub(1));
        for c in 0..2 {
            let a = st.get(i0 * 2 + c).copied().unwrap_or(0.0);
            let b = st.get(i1 * 2 + c).copied().unwrap_or(0.0);
            out.push(a + (b - a) * k);
        }
    }
    out
}

/// ffmpeg で 48kHz ステレオの f32 に。
pub(crate) fn decode_audio(ff: &str, path: &str) -> Result<Vec<f32>> {
    let o = hidden(Command::new(ff))
        .args(["-v", "error", "-i", path, "-vn", "-f", "f32le", "-ac", "2", "-ar", "48000", "-"])
        .output()?;
    if !o.status.success() {
        bail!("音声を読めません: {path}");
    }
    Ok(o.stdout.chunks_exact(4).map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]])).collect())
}

fn db(v: f64) -> f32 {
    10f64.powf(v / 20.0) as f32
}

/// ステレオ（インターリーブ）の mix に src を時刻 at（秒）から足す。len は使う長さ（秒。無ければ全部）、
/// fade_in・fade_out は直線のフェード（秒）。mix の外へはみ出す分は捨てる
fn mix_add(mix: &mut [f32], src: &[f32], at: f64, gain: f32, fade_in: f64, fade_out: f64, len: Option<f64>) {
    let n = mix.len() / 2;
    let start = (at * SR as f64).round() as i64;
    let frames = src.len() / 2;
    let limit = len.map(|l| (l * SR as f64) as usize).unwrap_or(frames).min(frames);
    let fi = (fade_in * SR as f64) as usize;
    let fo = (fade_out * SR as f64) as usize;
    for i in 0..limit {
        let p = start + i as i64;
        if p < 0 {
            continue;
        }
        let p = p as usize;
        if p >= n {
            break;
        }
        let mut g = gain;
        if fi > 0 && i < fi {
            g *= i as f32 / fi as f32;
        }
        if fo > 0 && i + fo > limit {
            g *= (limit - i) as f32 / fo as f32;
        }
        mix[p * 2] += src[i * 2] * g;
        mix[p * 2 + 1] += src[i * 2 + 1] * g;
    }
}

/// 素材の音をミックスに入れるときの倍率。消音なら None（入れない）
fn guide_audio_gain(doc: &crate::project::ProjectDoc) -> Option<f32> {
    (!doc.guide_audio_muted).then(|| db(doc.guide_audio_db))
}

pub fn mixdown(core: &Core, total: f64, include_se: bool) -> Result<Vec<f32>> {
    let doc = core.doc().ok_or_else(crate::app::error::no_project)?;
    let root = core.root().ok_or_else(crate::app::error::no_project)?;
    let n = (total * SR as f64).ceil() as usize;
    let mut mix = vec![0f32; n * 2];
    let add = |mix: &mut Vec<f32>, src: &[f32], at: f64, gain: f32, fade_in: f64, fade_out: f64, len: Option<f64>| {
        mix_add(mix, src, at, gain, fade_in, fade_out, len)
    };
    if !doc.voice_muted {
        for it in core.layout().iter() {
            if let Some(id) = &it.audio {
                match read_wav_stereo(&root.join("voice_cache").join(format!("{id}.wav"))) {
                    Ok(s) => add(&mut mix, &s, it.start, db(doc.voice_db), 0.0, 0.0, None),
                    Err(e) => core.note_issue_err(&format!("行 {} の声を読めないので入れていません", it.n), &e),
                }
            }
        }
    }
    // 攻略動画の素材の音（通しの素材の音を、映像と同じ並びにした wav。0 秒から）。無ければここで作り、作れなければ書き出しを止める
    if let Some(gain) = guide_audio_gain(&doc) {
        match core.guide_audio_ensure()? {
            crate::core::GuideAudio::Ready(p) => {
                let s = read_wav_stereo(&p).map_err(|e| anyhow!("素材の音（{}）を読めませんでした: {e:#}", p.display()))?;
                add(&mut mix, &s, 0.0, gain, 0.0, 0.0, None);
            }
            crate::core::GuideAudio::NoAudio => core.note_issue("素材に音がないので、素材の音は入れていません".into()),
            crate::core::GuideAudio::NotGuide => {}
        }
    }
    if include_se && !doc.se_muted {
        if let Some(desc) = core.desc() {
            let tl = PathBuf::from(&desc.path);
            let stem = tl.file_stem().unwrap().to_string_lossy().to_string();
            let p = tl.parent().unwrap().join("out").join(format!("{stem}_se.wav"));
            if p.exists() {
                match read_wav_stereo(&p) {
                    Ok(s) => add(&mut mix, &s, 0.0, db(doc.se_db), 0.0, 0.0, None),
                    Err(e) => core.note_issue_err("効果音を読めないので入れていません", &e),
                }
            }
        }
    }
    let ff = core.ffmpeg();
    for tr in doc.tracks.iter() {
        if tr.muted {
            continue;
        }
        let is_audio_track = tr.kind == "audio";
        for c in tr.clips.iter() {
            let audio_file = crate::render::media::MediaPool::is_audio(&c.src);
            if !(is_audio_track || audio_file) && tr.kind != "video" {
                continue;
            }
            if tr.kind == "video" && !audio_file {
                continue; // 動画の音は使わない（録画の音は Premiere 側で）
            }
            let s = match decode_audio(&ff, &c.src) {
                Ok(s) => s,
                Err(e) => {
                    core.note_issue_err(&format!("「{}」の音を読めないので入れていません", c.name), &e);
                    continue;
                }
            };
            let skip = (c.src_in * SR as f64) as usize * 2;
            let s = if skip < s.len() { &s[skip..] } else { &s[..0] };
            let g = db(tr.volume_db + c.volume_db);
            if c.looped && !s.is_empty() {
                let mut at = c.start;
                let seg = s.len() as f64 / 2.0 / SR as f64;
                while at < c.start + c.dur {
                    let len = (c.start + c.dur - at).min(seg);
                    add(&mut mix, s, at, g, 0.0, 0.0, Some(len));
                    at += seg;
                }
            } else {
                add(&mut mix, s, c.start, g, c.fade_in, c.fade_out, Some(c.dur));
            }
        }
    }
    // ピークを軽く抑える
    for v in mix.iter_mut() {
        let a = v.abs();
        if a > 0.9 {
            *v = v.signum() * (0.9 + 0.1 * ((a - 0.9) / 0.1).tanh());
        }
    }
    Ok(mix)
}

pub fn write_wav(path: &Path, st: &[f32]) -> Result<()> {
    let spec = hound::WavSpec { channels: 2, sample_rate: SR, bits_per_sample: 16, sample_format: hound::SampleFormat::Int };
    let mut w = hound::WavWriter::create(path, spec)?;
    for v in st {
        w.write_sample((v.clamp(-1.0, 1.0) * 32767.0) as i16)?;
    }
    w.finalize()?;
    Ok(())
}

/// 確認用: 声＋効果音＋BGM のミックスだけを wav に（画面の再生では使わないが、Claude に聞かせる・Premiere 用）。
pub fn export_mix_wav(core: &Arc<Core>) -> Result<Value> {
    let root = core.root().ok_or_else(crate::app::error::no_project)?;
    let end = core.layout_guard().iter().map(|i| i.start + i.dur).fold(0.0, f64::max) + 1.0;
    let end = end.max(core.desc().map(|d| d.end).unwrap_or(0.0));
    // 攻略動画の回は、攻略動画の終わりまで
    let (_, end) = super::export_span(core, None, end);
    let mix = mixdown(core, end, true)?;
    let p = root.join("studio").join("export").join("mix.wav");
    std::fs::create_dir_all(p.parent().unwrap())?;
    write_wav(&p, &mix)?;
    Ok(json!({"path": p}))
}

/// 攻略動画の素材の音だけの wav（`studio/export/<回>_guide_audio.wav`）。範囲があれば範囲どおりに切り出す（名前に範囲がつく）。
/// 音量・消音は掛けない（Premiere で調整するため）。素材に音が無い・作れないときは Err
pub fn export_guide_audio(core: &Arc<Core>, range: Option<(f64, f64)>) -> Result<Value> {
    let root = core.root().ok_or_else(crate::app::error::no_project)?;
    let p = match core.guide_audio_ensure()? {
        crate::core::GuideAudio::Ready(p) => p,
        crate::core::GuideAudio::NoAudio => bail!("素材に音がありません"),
        crate::core::GuideAudio::NotGuide => bail!("攻略動画の回ではありません（素材の音は攻略動画の回だけです）"),
    };
    let st = read_wav_stereo(&p).map_err(|e| anyhow!("素材の音（{}）を読めませんでした: {e:#}", p.display()))?;
    let total = st.len() as f64 / 2.0 / SR as f64;
    let (t0, t1) = range.unwrap_or((0.0, total));
    let part = crate::guide::audio::slice_range(&st, t0, t1);
    if part.is_empty() {
        bail!("書き出す範囲に素材の音がありません（素材の音は 0〜{total:.2} 秒）");
    }
    let dir = root.join("studio").join("export");
    std::fs::create_dir_all(&dir)?;
    // 範囲だけの書き出しは、全体を上書きしないよう範囲つきの名前にする
    let suffix = range.map(|(a, b)| format!("_{:.0}-{:.0}s", a, b)).unwrap_or_default();
    let out = dir.join(format!("{}_guide_audio{suffix}.wav", super::episode_stem(&root)));
    let tmp = out.with_extension("tmp.wav");
    write_wav(&tmp, part)?;
    std::fs::rename(&tmp, &out)?;
    Ok(json!({"path": out, "sec": part.len() as f64 / 2.0 / SR as f64}))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicBool;

    /// 素材の音は 0 秒から全部・音量 guide_audio_db で入り、消音なら入れない
    #[test]
    fn guide_audio_is_mixed_with_gain_and_skipped_when_muted() {
        let mut doc = crate::project::ProjectDoc::default();
        let src = vec![0.5f32; SR as usize * 2]; // 1 秒
        let mut mix = vec![0f32; SR as usize * 2 * 2]; // 2 秒
        mix_add(&mut mix, &src, 0.0, guide_audio_gain(&doc).unwrap(), 0.0, 0.0, None);
        assert_eq!(mix[0], 0.5);
        assert_eq!(mix[SR as usize * 2 - 1], 0.5);
        assert_eq!(mix[SR as usize * 2], 0.0, "素材の音の終わりより先は無音");
        // 音量 -6 dB
        doc.guide_audio_db = -6.0;
        let mut mix = vec![0f32; SR as usize * 2];
        mix_add(&mut mix, &src, 0.0, guide_audio_gain(&doc).unwrap(), 0.0, 0.0, None);
        assert!((mix[10] - 0.5 * 0.5012).abs() < 1e-3, "{}", mix[10]);
        // 消音なら入れない（倍率が無い）
        doc.guide_audio_muted = true;
        assert_eq!(guide_audio_gain(&doc), None);
        // 声などほかの音と足し合わさる
        let mut mix = vec![0.25f32; SR as usize * 2];
        mix_add(&mut mix, &src, 0.0, 1.0, 0.0, 0.0, None);
        assert_eq!(mix[100], 0.75);
        // 範囲の外（mix より長い）は捨てる
        let long = vec![1.0f32; SR as usize * 2 * 3];
        let mut mix = vec![0f32; SR as usize * 2];
        mix_add(&mut mix, &long, 0.0, 1.0, 0.0, 0.0, None);
        assert_eq!(mix.len(), SR as usize * 2);
    }

    #[test]
    fn wait_returns_result() {
        let (tx, rx) = crossbeam_channel::bounded::<Result<i32>>(1);
        tx.send(Ok(7)).unwrap();
        let epoch = CANCEL_EPOCH.load(Ordering::SeqCst);
        assert_eq!(wait_for_result(&rx, epoch, || panic!("止めない"), Duration::from_secs(5)).unwrap(), 7);
        let (tx, rx) = crossbeam_channel::bounded::<Result<i32>>(1);
        tx.send(Err(anyhow!("失敗"))).unwrap();
        assert!(wait_for_result(&rx, epoch, || panic!("止めない"), Duration::from_secs(5)).is_err());
    }

    #[test]
    fn wait_kills_on_cancel() {
        let (tx, rx) = crossbeam_channel::bounded::<Result<i32>>(1);
        let killed = Arc::new(AtomicBool::new(false));
        let k = killed.clone();
        // 古い epoch で始めたことにして、すでに中止が押された後の状態にする
        let epoch = CANCEL_EPOCH.load(Ordering::SeqCst).wrapping_sub(1);
        let tx2 = tx.clone();
        let r = wait_for_result(&rx, epoch, move || {
            k.store(true, Ordering::SeqCst);
            let _ = tx2.send(Err(anyhow!("止まった")));
        }, Duration::from_secs(30));
        assert!(killed.load(Ordering::SeqCst));
        assert!(format!("{:#}", r.unwrap_err()).contains("中止しました"));
    }

    #[test]
    fn wait_kills_on_timeout() {
        let (_tx, rx) = crossbeam_channel::bounded::<Result<i32>>(1);
        let killed = AtomicBool::new(false);
        let epoch = CANCEL_EPOCH.load(Ordering::SeqCst);
        let r = wait_for_result(&rx, epoch, || killed.store(true, Ordering::SeqCst), Duration::from_millis(250));
        assert!(killed.load(Ordering::SeqCst));
        // 同時に走る export の cancel のテストで epoch が動くと「中止しました」になるので、文言は見ない
        assert!(r.is_err());
    }
}
