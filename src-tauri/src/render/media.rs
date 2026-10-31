//! 素材（動画・画像）の読み込みと、モニター用の合成。
//!
//! 動画は ffmpeg で欲しい大きさに縮めながら連続で読み、読んだコマは JPEG にして覚えておく
//! （背景ループ 30 秒なら 1 周で全部キャッシュに入り、以後は読み直さない）。

use crate::settings::hidden;
use anyhow::{anyhow, Context, Result};
use parking_lot::Mutex;
use std::collections::HashMap;
use std::io::Read;
use std::process::{Child, ChildStdout, Command, Stdio};
use std::sync::Arc;

/// 動画・音声を調べた結果（画像は image: true で、ここでは作らない）。画面の probe_media と同じ形
pub type VideoInfo = ak_proto::MediaProbe;

/// `ffmpeg -i` の表示から大きさ・フレームレート・長さを読む（ffprobe が無くても動くように）。
pub fn probe(ffmpeg: &str, path: &str) -> Result<VideoInfo> {
    let o = hidden(Command::new(ffmpeg)).args(["-hide_banner", "-i", path]).output()?;
    let s = String::from_utf8_lossy(&o.stderr).to_string();
    let dur = regex::Regex::new(r"Duration: (\d+):(\d+):(\d+(?:\.\d+)?)")
        .unwrap()
        .captures(&s)
        .map(|c| c[1].parse::<f64>().unwrap() * 3600.0 + c[2].parse::<f64>().unwrap() * 60.0 + c[3].parse::<f64>().unwrap())
        .unwrap_or(0.0);
    let vline = s.lines().find(|l| l.contains("Video:"));
    let (mut w, mut h, mut fps) = (0, 0, 30.0);
    if let Some(l) = vline {
        if let Some(c) = regex::Regex::new(r", (\d{2,5})x(\d{2,5})").unwrap().captures(l) {
            w = c[1].parse().unwrap_or(0);
            h = c[2].parse().unwrap_or(0);
        }
        if let Some(c) = regex::Regex::new(r"([\d.]+) fps").unwrap().captures(l) {
            fps = c[1].parse().unwrap_or(30.0);
        } else if let Some(c) = regex::Regex::new(r"([\d.]+) tbr").unwrap().captures(l) {
            fps = c[1].parse().unwrap_or(30.0);
        }
    }
    if dur <= 0.0 && vline.is_none() && !s.contains("Audio:") {
        return Err(anyhow!("読めない素材です: {path}"));
    }
    Ok(VideoInfo { w, h, fps, dur, image: false, has_audio: s.contains("Audio:"), has_video: vline.is_some() })
}

/// 動画の各コマの時刻（秒、昇順）。デコードせずにパケットの pts を `framemd5` で並べる。
/// B フレームでパケットの順と表示の順が違うので、並べ直す。pts が無い・負のパケットは捨てる
pub fn scan_pts(ffmpeg: &str, path: &str) -> Result<Vec<f64>> {
    let o = hidden(Command::new(ffmpeg))
        .args(["-hide_banner", "-nostdin", "-i", path, "-map", "0:v:0", "-c", "copy", "-f", "framemd5", "-"])
        .stdin(Stdio::null())
        .output()
        .context("ffmpeg を起動できません")?;
    if !o.status.success() {
        return Err(anyhow!("コマの時刻を読めませんでした: {path}: {}", String::from_utf8_lossy(&o.stderr).trim()));
    }
    let text = String::from_utf8_lossy(&o.stdout);
    let mut tb = 1.0;
    let mut pts = vec![];
    for l in text.lines() {
        if let Some(r) = l.strip_prefix("#tb 0:") {
            if let Some((n, d)) = r.trim().split_once('/') {
                if let (Ok(n), Ok(d)) = (n.trim().parse::<f64>(), d.trim().parse::<f64>()) {
                    if d > 0.0 {
                        tb = n / d;
                    }
                }
            }
        } else if !l.starts_with('#') {
            // stream, dts, pts, duration, size, hash
            // pts が負のパケット（編集リストで捨てる、切り出した頭の分）はデコードしても出ないので数えない
            if let Some(p) = l.split(',').nth(2).and_then(|x| x.trim().parse::<i64>().ok()).filter(|&p| p >= 0) {
                pts.push(p as f64 * tb);
            }
        }
    }
    pts.sort_by(|a, b| a.total_cmp(b));
    Ok(pts)
}

/// コマの時刻の表が、fps の目盛り（先頭から 1/fps 秒ごと）に乗っているか（固定 fps か）。空は乗っている扱い
pub fn is_regular(pts: &[f64], fps: f64) -> bool {
    let Some(&p0) = pts.first() else { return true };
    let tol = 0.25 / fps;
    p0 < tol && pts.iter().enumerate().all(|(i, &p)| (p - p0 - i as f64 / fps).abs() < tol)
}

/// RGB（不透明）のフレーム。
#[derive(Clone)]
pub struct Rgb {
    pub w: u32,
    pub h: u32,
    pub data: Vec<u8>,
}

/// RGBA（ストレートアルファ）の画像。
#[derive(Clone)]
pub struct Rgba {
    pub w: u32,
    pub h: u32,
    pub data: Vec<u8>,
}

struct Decoder {
    child: Child,
    out: ChildStdout,
    /// 次に読めるコマ番号
    next: i64,
    w: u32,
    h: u32,
    /// 最後に返したコマ（番号と絵）。同じコマをもう一度引かれたらこれを返す
    last: Option<(i64, Rgb)>,
    /// 最後に使った順番（作り直す読み手を選ぶ）
    used: u64,
}

/// 1 本の動画に同時に持つ読み手（ffmpeg）の数。カットのクロスフェードの A・B で 2 本
const MAX_DECODERS: usize = 2;
/// 書き出し（cache=false）で、読み飛ばして先へ進める最大のコマ数。これより先は読み手を作り直す
/// （作り直しは 1 回 0.3 秒ほど、読み飛ばしは 1 コマ 3 ms ほど）
const READ_AHEAD: i64 = 90;

#[derive(Default)]
struct Decoders {
    list: Vec<Decoder>,
    tick: u64,
}

/// idx を読むのに使える読み手（同じ大きさで、前に返したコマか、次〜ahead 先のコマ）。いちばん近いもの
fn pick_decoder(list: &[Decoder], idx: i64, w: u32, h: u32, ahead: i64) -> Option<usize> {
    list.iter()
        .enumerate()
        .filter(|(_, d)| d.w == w && d.h == h)
        .filter_map(|(k, d)| {
            if d.last.as_ref().is_some_and(|(li, _)| *li == idx) {
                Some((0, k))
            } else {
                (idx >= d.next && idx <= d.next + ahead).then_some((idx - d.next + 1, k))
            }
        })
        .min()
        .map(|(_, k)| k)
}

impl Drop for Decoder {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// 動画 1 本（＋出力の大きさ）ぶんの読み手とキャッシュ。
/// 読んだコマの JPEG（幅・高さ・コマ番号ごと）
type FrameJpegs = HashMap<(u32, u32, i64), Arc<Vec<u8>>>;

pub struct VideoSource {
    pub path: String,
    pub info: VideoInfo,
    ffmpeg: String,
    dec: Mutex<Decoders>,
    /// 読み手（ffmpeg）を起こした回数（テストで、作り直していないことを確かめる）
    starts: std::sync::atomic::AtomicUsize,
    cache: Mutex<FrameJpegs>,
    cache_bytes: Mutex<usize>,
    budget: usize,
    /// 「コマ番号で読む」モードのコマごとの時刻（秒）。攻略動画の本体は解析の表（`with_pts`）。ほかの動画は初めて使うときに
    /// `scan_pts` で作り、fps の目盛りに乗っていれば None（今までどおり fps で割った時刻で引く。旧#155）。作れなければそのエラー
    pts: std::sync::OnceLock<std::result::Result<Option<Arc<Vec<f64>>>, String>>,
    /// fps で割って読む素材の、読める最後のコマ番号（最後のコマを読めなかったら 1 つ前にする。旧#153）
    last_readable: std::sync::atomic::AtomicI64,
}

impl VideoSource {
    pub fn new(ffmpeg: &str, path: &str) -> Result<Self> {
        let info = probe(ffmpeg, path)?;
        Ok(VideoSource {
            path: path.to_string(),
            info,
            ffmpeg: ffmpeg.to_string(),
            dec: Mutex::new(Decoders::default()),
            starts: Default::default(),
            cache: Mutex::new(HashMap::new()),
            cache_bytes: Mutex::new(0),
            budget: 400 << 20,
            pts: std::sync::OnceLock::new(),
            last_readable: std::sync::atomic::AtomicI64::new(i64::MAX),
        })
    }

    /// コマ番号で読む版。idx は pts の添字（解析で数えたコマ番号）で、コマの数は pts の長さ。
    /// 読み始めは `-ss <pts[idx] − 0.001>`（入力の前）＋ `-fps_mode passthrough` で、fps の変換は入れない
    pub fn with_pts(ffmpeg: &str, path: &str, pts: Arc<Vec<f64>>) -> Result<Self> {
        if pts.is_empty() {
            return Err(anyhow!("コマの時刻の表が空です: {path}"));
        }
        let mut v = Self::new(ffmpeg, path)?;
        v.pts = std::sync::OnceLock::from(Ok(Some(pts)));
        Ok(v)
    }

    /// コマの時刻の表を作る（初めて呼んだときだけ。同時に呼んでも作るのは 1 回）。
    /// 可変 fps の素材は、fps で割った時刻では読み始めの位置でコマが変わるので、表で読む（旧#155）
    fn pts_table(&self) -> &std::result::Result<Option<Arc<Vec<f64>>>, String> {
        self.pts.get_or_init(|| {
            if !self.info.has_video {
                return Ok(None);
            }
            match scan_pts(&self.ffmpeg, &self.path) {
                Ok(p) if !p.is_empty() && !is_regular(&p, self.info.fps) => Ok(Some(Arc::new(p))),
                Ok(_) => Ok(None),
                Err(e) => Err(format!("{e:#}")),
            }
        })
    }

    /// コマの時刻の表（無ければ、または作れなければ None = fps で割って読む）
    pub fn pts(&self) -> Option<&Arc<Vec<f64>>> {
        self.pts_table().as_ref().ok().and_then(|p| p.as_ref())
    }

    /// 素材の秒 st に映すコマの番号。pts の表があれば、st 以前でいちばん後のコマ（無ければ 0）。
    /// 表が無ければ floor(st × fps)
    pub fn index_at(&self, st: f64) -> i64 {
        match self.pts() {
            Some(p) => (p.partition_point(|&t| t <= st + 1e-6) as i64 - 1).max(0),
            None => (st * self.info.fps).floor() as i64,
        }
    }

    pub fn frame_count(&self) -> i64 {
        match self.pts() {
            Some(p) => p.len() as i64,
            None => ((self.info.dur * self.info.fps).floor() as i64).max(1),
        }
    }

    /// 素材の idx コマ目を w×h で。
    pub fn frame(&self, idx: i64, w: u32, h: u32) -> Result<Rgb> {
        self.frame_opt(idx, w, h, true)
    }

    /// cache=false は書き出し用（順に読むだけで、JPEG にして覚えない）。
    /// 読み手は `MAX_DECODERS` 本まで持ち、近くを読んでいる読み手で続きを読む（カットのクロスフェードで
    /// 2 か所を交互に読んでも、それぞれ順読みのまま）。同じコマをもう一度引いたら（可変 fps の素材を 60 fps で出すと
    /// 同じコマが続く）、前に返した絵をそのまま返す（読み手を作り直さない）
    pub fn frame_opt(&self, idx: i64, w: u32, h: u32, cache: bool) -> Result<Rgb> {
        // 表を作れなかった素材は、読む順でコマが変わりうるので黙って fps で割って読まない
        if let Err(e) = self.pts_table() {
            return Err(anyhow!("{e}"));
        }
        let last = (self.frame_count() - 1).min(self.last_readable.load(std::sync::atomic::Ordering::Relaxed));
        let idx = idx.clamp(0, last);
        if cache {
            if let Some(j) = self.cache.lock().get(&(w, h, idx)).cloned() {
                return decode_jpeg(&j);
            }
        }
        let ahead = if cache { 45 } else { READ_AHEAD };
        let mut g = self.dec.lock();
        g.tick += 1;
        let tick = g.tick;
        let k = match pick_decoder(&g.list, idx, w, h, ahead) {
            Some(k) => k,
            None => {
                self.starts.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                let d = self.start(idx, w, h)?;
                if g.list.len() < MAX_DECODERS {
                    g.list.push(d);
                    g.list.len() - 1
                } else {
                    // いちばん長く使っていない読み手を作り直す
                    let k = (0..g.list.len()).min_by_key(|&k| g.list[k].used).unwrap_or(0);
                    g.list[k] = d;
                    k
                }
            }
        };
        let d = &mut g.list[k];
        d.used = tick;
        if let Some((li, im)) = &d.last {
            if *li == idx {
                // 書き出しの読み方が残した生の絵。モニターには覚えた JPEG の絵で返す（読む順で絵を変えない。旧#153）
                return Ok(if cache { self.remember_and_view(idx, im) } else { im.clone() });
            }
        }
        let n = (w * h * 3) as usize;
        // この呼び出しで最後に読んだコマ（読み手が尽きたときに使う）
        let mut prev: Option<(i64, Rgb)> = None;
        loop {
            let mut buf = vec![0u8; n];
            if let Err(e) = d.out.read_exact(&mut buf) {
                g.list.remove(k);
                // fps で割って読む素材の最後のコマは ffmpeg の fps フィルタが出さないので、以後は 1 つ前のコマで代える（旧#153）。
                // コマ番号で読む素材（passthrough）は最後のコマも出るので、読めなければ本当の失敗としてそのまま返す
                if self.pts().is_none() && idx > 0 && idx == self.frame_count() - 1 {
                    self.last_readable.store(idx - 1, std::sync::atomic::Ordering::Relaxed);
                    // モニターは 1 つ前のコマを覚えたばかりなので、作り直しでキャッシュの絵を返す
                    if let Some((p, f)) = prev.filter(|_| !cache) {
                        if p == idx - 1 {
                            return Ok(f);
                        }
                    }
                    drop(g);
                    return self.frame_opt(idx - 1, w, h, cache);
                }
                return Err(anyhow!("動画を読み込めませんでした: {} ({e})", self.path));
            }
            let cur = d.next;
            d.next += 1;
            let mut frame = Rgb { w, h, data: buf };
            // 読んだコマは全部覚える（行き過ぎない限り）。返す絵は覚えた JPEG から戻す
            if cache {
                if cur >= idx {
                    frame = self.remember_and_view(cur, &frame);
                } else {
                    self.remember(cur, &frame);
                }
            }
            if cur >= idx {
                // モニター（cache=true）は JPEG のキャッシュに当たるので、書き出しの読み方だけ覚える
                d.last = if cache { None } else { Some((cur, frame.clone())) };
                return Ok(frame);
            }
            prev = Some((cur, frame));
        }
    }

    /// コマを JPEG にしてキャッシュに覚える。覚えた JPEG を返す（作れなければ None）
    fn remember(&self, idx: i64, f: &Rgb) -> Option<Arc<Vec<u8>>> {
        let j = Arc::new(encode_jpeg(f, 90).ok()?);
        let mut c = self.cache.lock();
        let mut cb = self.cache_bytes.lock();
        *cb += j.len();
        if let Some(old) = c.insert((f.w, f.h, idx), j.clone()) {
            *cb -= old.len();
        }
        if *cb > self.budget {
            // 半分捨てる（素朴でよい）
            let keys: Vec<_> = c.keys().cloned().take(c.len() / 2).collect();
            for k in keys {
                if let Some(v) = c.remove(&k) {
                    *cb -= v.len();
                }
            }
        }
        Some(j)
    }

    /// remember して、覚えた JPEG を戻した絵を返す（1 回目とキャッシュに当たった 2 回目で同じ絵にする。旧#153）。
    /// JPEG にできなければ元の絵
    fn remember_and_view(&self, idx: i64, f: &Rgb) -> Rgb {
        self.remember(idx, f).and_then(|j| decode_jpeg(&j).ok()).unwrap_or_else(|| f.clone())
    }

    fn start(&self, idx: i64, w: u32, h: u32) -> Result<Decoder> {
        let mut cmd = hidden(Command::new(&self.ffmpeg));
        match self.pts() {
            // コマ番号で読む: 欲しいコマの少し手前から、コマを増やしも間引きもせずに出す
            Some(p) => {
                let t = (p[(idx as usize).min(p.len() - 1)] - 0.001).max(0.0);
                cmd.args(["-v", "error", "-ss", &format!("{t:.4}"), "-i", &self.path, "-an", "-fps_mode", "passthrough",
                          "-vf", &format!("scale={w}:{h}:flags=bilinear")]);
            }
            None => {
                // 欲しいコマの半コマ手前から読む（コマの時刻ちょうどだと、小数の丸めでそのコマが捨てられることがある。旧#153）
                let t = ((idx as f64 - 0.5) / self.info.fps).max(0.0);
                cmd.args(["-v", "error", "-ss", &format!("{t:.4}"), "-i", &self.path, "-an", "-vf",
                          &format!("scale={w}:{h}:flags=bilinear,fps={}", self.info.fps)]);
            }
        }
        let mut child = cmd
            .args(["-f", "rawvideo", "-pix_fmt", "rgb24", "-"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .context("ffmpeg を起動できません")?;
        let out = child.stdout.take().unwrap();
        Ok(Decoder { child, out, next: idx, w, h, last: None, used: 0 })
    }
}

pub fn encode_jpeg(f: &Rgb, q: u8) -> Result<Vec<u8>> {
    let mut out = Vec::with_capacity((f.w * f.h / 6) as usize);
    let enc = jpeg_encoder::Encoder::new(&mut out, q);
    enc.encode(&f.data, f.w as u16, f.h as u16, jpeg_encoder::ColorType::Rgb)?;
    Ok(out)
}

pub fn decode_jpeg(b: &[u8]) -> Result<Rgb> {
    let opts = zune_jpeg::zune_core::options::DecoderOptions::default()
        .jpeg_set_out_colorspace(zune_jpeg::zune_core::colorspace::ColorSpace::RGB);
    let mut d = zune_jpeg::JpegDecoder::new_with_options(b, opts);
    let data = d.decode().map_err(|e| anyhow!("JPEG を読めません: {e:?}"))?;
    let (w, h) = d.dimensions().ok_or_else(|| anyhow!("JPEG の大きさが分かりません"))?;
    Ok(Rgb { w: w as u32, h: h as u32, data })
}

/// 画像（立ち絵など）。縮めた版を大きさごとに覚える。
pub struct ImageSource {
    pub full: Rgba,
    scaled: Mutex<HashMap<(u32, u32, bool), Arc<Rgba>>>,
}

impl ImageSource {
    pub fn open(path: &str) -> Result<Self> {
        let im = image::open(path).with_context(|| format!("画像を読めません: {path}"))?.to_rgba8();
        let (w, h) = im.dimensions();
        Ok(ImageSource { full: Rgba { w, h, data: im.into_raw() }, scaled: Mutex::new(HashMap::new()) })
    }

    pub fn at(&self, w: u32, h: u32, flip: bool) -> Result<Arc<Rgba>> {
        if let Some(s) = self.scaled.lock().get(&(w, h, flip)).cloned() {
            return Ok(s);
        }
        let src = image::RgbaImage::from_raw(self.full.w, self.full.h, self.full.data.clone())
            .ok_or_else(|| anyhow!("画像のデータの長さが大きさ（{}×{}）と合いません", self.full.w, self.full.h))?;
        let mut r = image::imageops::resize(&src, w.max(1), h.max(1), image::imageops::FilterType::Triangle);
        if flip {
            r = image::imageops::flip_horizontal(&r);
        }
        let out = Arc::new(Rgba { w: r.width(), h: r.height(), data: r.into_raw() });
        let mut m = self.scaled.lock();
        if m.len() > 16 {
            m.clear();
        }
        m.insert((w, h, flip), out.clone());
        Ok(out)
    }
}

/// 素材の置き場（パスごとに 1 つ）。
#[derive(Default)]
pub struct MediaPool {
    videos: Mutex<HashMap<String, Arc<VideoSource>>>,
    images: Mutex<HashMap<String, Arc<ImageSource>>>,
    /// 開けなかった素材（いつ・なぜ・そのときのファイルの更新時刻）。毎コマ ffmpeg で調べ直してログを埋めないよう、
    /// しばらくか、ファイルが変わるまでは同じエラーを返す
    failed: Mutex<HashMap<String, Failure>>,
    pub ffmpeg: Mutex<String>,
}

struct Failure {
    at: std::time::Instant,
    /// 種類・和文・元の連鎖（返すときに同じ形で作り直す）
    err: ak_proto::ApiError,
    mtime: Option<std::time::SystemTime>,
}

const RETRY_FAILED: std::time::Duration = std::time::Duration::from_secs(10);

/// コマ番号で読む動画の、置き場での鍵（普通の読み方とぶつからない）
fn pts_key(path: &str) -> String {
    format!("{path}#pts")
}

fn mtime(path: &str) -> Option<std::time::SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

impl MediaPool {
    fn recent_failure(&self, path: &str) -> Option<anyhow::Error> {
        let f = self.failed.lock();
        let x = f.get(path)?;
        (x.at.elapsed() < RETRY_FAILED && x.mtime == mtime(path))
            .then(|| crate::app::error::coded_detail(x.err.code, x.err.message.clone(), x.err.detail.clone()))
    }

    fn remember_failure(&self, path: &str, e: &anyhow::Error) {
        let f = Failure { at: std::time::Instant::now(), err: crate::app::error::api_error(e), mtime: mtime(path) };
        self.failed.lock().insert(path.to_string(), f);
    }

    pub fn video(&self, path: &str) -> Result<Arc<VideoSource>> {
        if let Some(v) = self.videos.lock().get(path).cloned() {
            return Ok(v);
        }
        if let Some(e) = self.recent_failure(path) {
            return Err(e);
        }
        let ff = self.ffmpeg.lock().clone();
        if ff.is_empty() {
            return Err(crate::app::error::coded(ak_proto::ErrorCode::NeedFfmpeg, "ffmpeg が見つかりません"));
        }
        let v = Arc::new(VideoSource::new(&ff, path).inspect_err(|e| self.remember_failure(path, e))?);
        self.failed.lock().remove(path);
        // 同時に開いた別のスレッドが先に入れていたら、そちらを使う（読み手・キャッシュ・コマの時刻の表を 1 つにする）
        Ok(self.videos.lock().entry(path.to_string()).or_insert(v).clone())
    }

    pub fn image(&self, path: &str) -> Result<Arc<ImageSource>> {
        if let Some(v) = self.images.lock().get(path).cloned() {
            return Ok(v);
        }
        if let Some(e) = self.recent_failure(path) {
            return Err(e);
        }
        let v = Arc::new(ImageSource::open(path).inspect_err(|e| self.remember_failure(path, e))?);
        self.failed.lock().remove(path);
        self.images.lock().insert(path.to_string(), v.clone());
        Ok(v)
    }

    /// コマ番号で読む動画（`VideoSource::with_pts`）。普通の読み方（`video`）とは別に覚える。
    /// 同じパスでも pts の中身が変わったら（解析し直し）作り直す
    pub fn video_pts(&self, path: &str, pts: &Arc<Vec<f64>>) -> Result<Arc<VideoSource>> {
        let key = pts_key(path);
        if let Some(v) = self.videos.lock().get(&key).cloned() {
            if v.pts().is_some_and(|p| Arc::ptr_eq(p, pts) || **p == **pts) {
                return Ok(v);
            }
        }
        if let Some(e) = self.recent_failure(path) {
            return Err(e);
        }
        let ff = self.ffmpeg.lock().clone();
        if ff.is_empty() {
            return Err(crate::app::error::coded(ak_proto::ErrorCode::NeedFfmpeg, "ffmpeg が見つかりません"));
        }
        let v = Arc::new(VideoSource::with_pts(&ff, path, pts.clone()).inspect_err(|e| self.remember_failure(path, e))?);
        self.failed.lock().remove(path);
        self.videos.lock().insert(key, v.clone());
        Ok(v)
    }

    pub fn forget(&self, path: &str) {
        self.failed.lock().remove(path);
        self.videos.lock().remove(path);
        self.videos.lock().remove(&pts_key(path));
        self.images.lock().remove(path);
    }

    pub fn is_image(path: &str) -> bool {
        IMAGE_EXTS.contains(&ext_of(path).as_str())
    }

    pub fn is_audio(path: &str) -> bool {
        AUDIO_EXTS.contains(&ext_of(path).as_str())
    }
}

/// 素材の拡張子（画面は ui/src/logic/media.ts に写しがあり、見本 ui_rules.json の media_exts で縛っている）。
/// 動画は「画像でも音声でもないもの」（知らない拡張子も ffmpeg に任せる）。VIDEO_EXTS は画面の絞り込み用
pub const IMAGE_EXTS: &[&str] = &["png", "jpg", "jpeg", "webp", "bmp"];
pub const VIDEO_EXTS: &[&str] = &["mp4", "mov", "mkv", "webm", "avi", "m4v"];
pub const AUDIO_EXTS: &[&str] = &["wav", "mp3", "m4a", "aac", "flac", "ogg"];

/// 拡張子（小文字・点なし）。無ければ ""
pub fn ext_of(path: &str) -> String {
    std::path::Path::new(path).extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default()
}

/// ファイルの Content-Type（`/file` が返す）
pub fn mime_of(path: &str) -> &'static str {
    match ext_of(path).as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "bmp" => "image/bmp",
        "mp4" | "m4v" => "video/mp4",
        "mov" => "video/quicktime",
        "webm" => "video/webm",
        "mkv" => "video/x-matroska",
        "avi" => "video/x-msvideo",
        "wav" => "audio/wav",
        "mp3" => "audio/mpeg",
        "m4a" | "aac" => "audio/mp4",
        "flac" => "audio/flac",
        "ogg" => "audio/ogg",
        _ => "application/octet-stream",
    }
}

// ======================================================================= 合成
/// RGB の上に RGBA（ストレートアルファ）を (x, y) に重ねる。opacity と明るさ（立ち絵の減光）つき。
pub fn blend(dst: &mut Rgb, src: &Rgba, x: i64, y: i64, opacity: f32, bright: f32) {
    blend_in(dst, src, x, y, opacity, bright, [0, 0, i64::MAX, i64::MAX]);
}

/// 境目のぼかし: rect（フレームの画素、小数）の内側 px のあいだで、外へ向かって薄くする。
/// クロップしていない辺は ±無限大にしておけば、その辺はぼかさない。
#[derive(Clone, Copy)]
pub struct Soft {
    pub rect: [f64; 4],
    pub px: f64,
}

impl Soft {
    /// (xx, yy) の画素の濃さ（0〜256）
    fn k(&self, xx: i64, yy: i64) -> u32 {
        let (cx, cy) = (xx as f64 + 0.5, yy as f64 + 0.5);
        let d = (cx - self.rect[0]).min(self.rect[2] - cx).min(cy - self.rect[1]).min(self.rect[3] - cy);
        ((d / self.px).clamp(0.0, 1.0) * 256.0) as u32
    }
}

/// blend の、rect（[x0, y0, x1, y1]、フレームの画素）の外には描かない版（ネストの窓の切り抜き）。
pub fn blend_in(dst: &mut Rgb, src: &Rgba, x: i64, y: i64, opacity: f32, bright: f32, rect: [i64; 4]) {
    blend_soft(dst, src, x, y, opacity, bright, rect, None);
}

/// blend_in に、境目のぼかし（soft）をつけたもの。
#[allow(clippy::too_many_arguments)]
pub fn blend_soft(dst: &mut Rgb, src: &Rgba, x: i64, y: i64, opacity: f32, bright: f32, rect: [i64; 4],
                  soft: Option<Soft>) {
    use rayon::prelude::*;
    let (dw, dh) = (dst.w as i64, dst.h as i64);
    let x0 = x.max(0).max(rect[0]);
    let y0 = y.max(0).max(rect[1]);
    let x1 = (x + src.w as i64).min(dw).min(rect[2]);
    let y1 = (y + src.h as i64).min(dh).min(rect[3]);
    if x0 >= x1 || y0 >= y1 || opacity <= 0.001 {
        return;
    }
    let op = (opacity.clamp(0.0, 1.0) * 256.0) as u32;
    let br = (bright.clamp(0.0, 2.0) * 256.0) as u32;
    let sw = src.w as usize;
    let rows: Vec<i64> = (y0..y1).collect();
    let dst_w = dw as usize;
    let dptr = dst.data.as_mut_ptr() as usize;
    rows.par_iter().for_each(|&yy| {
        let sy = (yy - y) as usize;
        let srow = &src.data[sy * sw * 4..(sy + 1) * sw * 4];
        // SAFETY: 行ごとに書き込み先が重ならない
        let drow = unsafe {
            std::slice::from_raw_parts_mut((dptr as *mut u8).add(yy as usize * dst_w * 3), dst_w * 3)
        };
        for xx in x0..x1 {
            let sx = (xx - x) as usize;
            let s = &srow[sx * 4..sx * 4 + 4];
            let mut a = (s[3] as u32 * op) >> 8;
            if let Some(sf) = soft {
                a = (a * sf.k(xx, yy)) >> 8;
            }
            if a == 0 {
                continue;
            }
            let d = &mut drow[xx as usize * 3..xx as usize * 3 + 3];
            for c in 0..3 {
                let sc = ((s[c] as u32 * br) >> 8).min(255);
                d[c] = ((sc * a + d[c] as u32 * (255 - a)) / 255) as u8;
            }
        }
    });
}

/// RGB の上に RGB（不透明の動画のコマ）を重ねる。rect（[x0, y0, x1, y1]、フレームの画素）の外には描かない。
#[allow(dead_code)]
pub fn blit_in(dst: &mut Rgb, src: &Rgb, x: i64, y: i64, opacity: f32, rect: [i64; 4]) {
    blit_soft(dst, src, x, y, opacity, rect, None);
}

/// blit_in に、境目のぼかし（soft）をつけたもの。
pub fn blit_soft(dst: &mut Rgb, src: &Rgb, x: i64, y: i64, opacity: f32, rect: [i64; 4], soft: Option<Soft>) {
    let (dw, dh) = (dst.w as i64, dst.h as i64);
    let x0 = x.max(0).max(rect[0]);
    let y0 = y.max(0).max(rect[1]);
    let x1 = (x + src.w as i64).min(dw).min(rect[2]);
    let y1 = (y + src.h as i64).min(dh).min(rect[3]);
    if x0 >= x1 || y0 >= y1 {
        return;
    }
    let a = (opacity.clamp(0.0, 1.0) * 255.0) as u32;
    for yy in y0..y1 {
        let sy = (yy - y) as usize;
        let srow = &src.data[(sy * src.w as usize + (x0 - x) as usize) * 3..(sy * src.w as usize + (x1 - x) as usize) * 3];
        let di = (yy as usize * dst.w as usize + x0 as usize) * 3;
        let drow = &mut dst.data[di..di + srow.len()];
        if let Some(sf) = soft {
            for (i, (d, s)) in drow.chunks_mut(3).zip(srow.chunks(3)).enumerate() {
                let k = (a * sf.k(x0 + i as i64, yy)) >> 8;
                for c in 0..3 {
                    d[c] = ((s[c] as u32 * k + d[c] as u32 * (255 - k)) / 255) as u8;
                }
            }
        } else if a >= 255 {
            drow.copy_from_slice(srow);
        } else {
            for (d, s) in drow.iter_mut().zip(srow) {
                *d = ((*s as u32 * a + *d as u32 * (255 - a)) / 255) as u8;
            }
        }
    }
}

/// ワイプの走査線（fxkit の Sprite の wipe の線に見た目を似せただけ。線の太さ・光り方は縛っていない。位置＝見えている割合は trans.rs の `curve` が縛っている）。
/// pos: 線の位置（vertical なら x、そうでなければ y。フレームの画素）、span: 線の長さの範囲、rect の外には描かない。
pub fn scan_line(dst: &mut Rgb, pos: f64, vertical: bool, span: [i64; 2], rect: [i64; 4], opacity: f32, unit: f64) {
    const ICE: [f32; 3] = [214.0, 238.0, 255.0];
    let core = (1.0 * unit).max(0.6);
    let glow = (3.5 * unit).max(1.5);
    let (dw, dh) = (dst.w as i64, dst.h as i64);
    let lo = (pos - glow).floor() as i64;
    let hi = (pos + glow).ceil() as i64;
    let op = opacity.clamp(0.0, 1.0) * 0.9;
    for p in lo..=hi {
        let d = ((p as f64 + 0.5) - pos).abs();
        let k = if d <= core { 1.0 } else { (1.0 - (d - core) / (glow - core)).max(0.0) * 0.5 } as f32 * op;
        if k <= 0.001 {
            continue;
        }
        for q in span[0]..span[1] {
            let (x, y) = if vertical { (p, q) } else { (q, p) };
            if x < rect[0].max(0) || x >= rect[2].min(dw) || y < rect[1].max(0) || y >= rect[3].min(dh) {
                continue;
            }
            let i = (y as usize * dst.w as usize + x as usize) * 3;
            for (c, ice) in ICE.iter().enumerate() {
                dst.data[i + c] = (ice * k + dst.data[i + c] as f32 * (1.0 - k)) as u8;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// ffmpeg が使えるなら、その場所（PATH → imageio-ffmpeg）。無ければテストを飛ばす
    fn test_ffmpeg() -> Option<String> {
        crate::settings::find_ffmpeg(&crate::settings::Settings::default())
    }

    /// コマごとに明るさを変えた、時刻が不揃いな（可変 fps の）動画を作り、showinfo で実際の pts を読む。
    /// 戻り: (動画のパス, 各コマの pts, 各コマの明るさ)
    fn make_vfr_video(ff: &str, name: &str) -> Option<(std::path::PathBuf, Vec<f64>, Vec<u8>)> {
        let dir = std::env::temp_dir().join(format!("ak-media-pts-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let durs = [0.04, 0.10, 0.05, 0.20, 0.04, 0.04, 0.12, 0.04, 0.08, 0.04, 0.30, 0.04];
        let levels: Vec<u8> = (0..durs.len()).map(|i| (i * 20 + 10) as u8).collect();
        let mut list = String::new();
        for (i, (&d, &g)) in durs.iter().zip(&levels).enumerate() {
            let png = dir.join(format!("f{i}.png"));
            image::RgbImage::from_pixel(64, 48, image::Rgb([g, g, g])).save(&png).unwrap();
            list.push_str(&format!("file 'f{i}.png'
duration {d}
"));
        }
        std::fs::write(dir.join("list.txt"), list).unwrap();
        let out = dir.join("v.mp4");
        let o = hidden(Command::new(ff))
            .current_dir(&dir)
            .args(["-hide_banner", "-loglevel", "error", "-y", "-f", "concat", "-safe", "0", "-i", "list.txt",
                   "-fps_mode", "vfr", "-c:v", "libx264", "-pix_fmt", "yuv420p", "-g", "4", "-video_track_timescale",
                   "90000"])
            .arg(&out)
            .output()
            .ok()?;
        if !o.status.success() {
            eprintln!("可変 fps の動画を作れないので skip: {}", String::from_utf8_lossy(&o.stderr));
            return None;
        }
        // 実際の pts（guide_scan.py と同じく showinfo から）
        let o = hidden(Command::new(ff))
            .args(["-hide_banner", "-nostdin", "-i"])
            .arg(&out)
            .args(["-an", "-fps_mode", "passthrough", "-vf", "showinfo", "-f", "null", "-"])
            .output()
            .ok()?;
        let text = String::from_utf8_lossy(&o.stderr).to_string();
        let re = regex::Regex::new(r"pts_time:\s*([0-9.]+)").unwrap();
        let pts: Vec<f64> = re.captures_iter(&text).map(|c| c[1].parse().unwrap()).collect();
        Some((out, pts, levels))
    }

    #[test]
    fn with_pts_returns_the_frame_of_each_index_on_variable_fps() {
        let Some(ff) = test_ffmpeg() else {
            eprintln!("ffmpeg が無いので skip");
            return;
        };
        let Some((path, pts, levels)) = make_vfr_video(&ff, "idx") else { return };
        assert_eq!(pts.len(), levels.len(), "コマが増えたり間引かれたりしていない: {pts:?}");
        // 時刻が不揃い（可変 fps）であること
        assert!(pts.windows(2).any(|w| (w[1] - w[0] - 0.04).abs() > 0.01));
        let v = VideoSource::with_pts(&ff, path.to_str().unwrap(), Arc::new(pts.clone())).unwrap();
        assert_eq!(v.frame_count(), levels.len() as i64);
        let check = |idx: usize| {
            let f = v.frame_opt(idx as i64, 32, 24, false).unwrap();
            let got = f.data[(12 * 32 + 16) * 3] as i32;
            assert!((got - levels[idx] as i32).abs() <= 12, "idx={idx} 期待 {} 実際 {got}", levels[idx]);
        };
        // 途中から読み始める・戻る・順に進む・飛ぶ、のどれでも、その番号のコマが返る
        for idx in [7, 3, 4, 5, 11, 0, 10, 1, 2] {
            check(idx);
        }
        // 範囲の外は端のコマ
        let last = v.frame_opt(99, 32, 24, false).unwrap();
        assert!((last.data[(12 * 32 + 16) * 3] as i32 - levels[11] as i32).abs() <= 12);
        // キャッシュありでも同じ
        for idx in [6, 6, 8] {
            let f = v.frame_opt(idx as i64, 32, 24, true).unwrap();
            assert!((f.data[(12 * 32 + 16) * 3] as i32 - levels[idx] as i32).abs() <= 12, "cache idx={idx}");
        }
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    /// 書き出しの読み方（cache=false）: 同じコマを続けて引いても、クロスフェードのように 2 か所を交互に引いても、
    /// 読み手を作り直さずに順読みのまま正しいコマを返す
    #[test]
    fn export_reads_repeat_and_two_places_without_restart() {
        let Some(ff) = test_ffmpeg() else {
            eprintln!("ffmpeg が無いので skip");
            return;
        };
        let Some((path, pts, levels)) = make_vfr_video(&ff, "dual") else { return };
        let v = VideoSource::with_pts(&ff, path.to_str().unwrap(), Arc::new(pts)).unwrap();
        let level = |idx: i64| v.frame_opt(idx, 32, 24, false).unwrap().data[(12 * 32 + 16) * 3] as i32;
        let starts = || v.starts.load(std::sync::atomic::Ordering::Relaxed);
        // 同じコマが続く（60 fps で出すと素材の 1 コマが 2 回出る）
        for idx in [0, 1, 1, 2, 2, 2, 3] {
            assert!((level(idx) - levels[idx as usize] as i32).abs() <= 12, "idx={idx}");
        }
        assert_eq!(starts(), 1, "同じコマを引き直しても作り直さない");
        // A（4〜）と B（8〜）を交互に。読み手は 2 本目が増えるだけ
        for (a, b) in [(4, 8), (5, 9), (5, 10), (6, 11)] {
            assert!((level(a) - levels[a as usize] as i32).abs() <= 12, "A={a}");
            assert!((level(b) - levels[b as usize] as i32).abs() <= 12, "B={b}");
        }
        assert_eq!(starts(), 2, "クロスフェードの A・B はそれぞれ順読み");
        // 戻ったら作り直す（読み手は 2 本のまま）
        assert!((level(0) - levels[0] as i32).abs() <= 12);
        assert_eq!(starts(), 3);
        assert!(v.dec.lock().list.len() <= MAX_DECODERS);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn video_pts_is_cached_apart_from_plain_video_and_follows_pts_changes() {
        let Some(ff) = test_ffmpeg() else {
            eprintln!("ffmpeg が無いので skip");
            return;
        };
        let Some((path, pts, _)) = make_vfr_video(&ff, "pool") else { return };
        let p = path.to_str().unwrap();
        let pool = MediaPool::default();
        *pool.ffmpeg.lock() = ff;
        let pts = Arc::new(pts);
        let a = pool.video_pts(p, &pts).unwrap();
        let b = pool.video_pts(p, &pts).unwrap();
        assert!(Arc::ptr_eq(&a, &b));
        // 同じ中身の別の Arc でも作り直さない
        let same = Arc::new((*pts).clone());
        assert!(Arc::ptr_eq(&a, &pool.video_pts(p, &same).unwrap()));
        // 普通の読み方は別のもの（既存のクリップの読み方は変わらない）
        let plain = pool.video(p).unwrap();
        assert!(!Arc::ptr_eq(&a, &plain));
        // 普通の読み方は自分で作った表（解析の表を変えても影響しない）
        assert_eq!(plain.pts().map(|p| p.len()), Some(pts.len()));
        // 中身が変わったら作り直す
        let mut other = (*pts).clone();
        other.pop();
        let c = pool.video_pts(p, &Arc::new(other)).unwrap();
        assert!(!Arc::ptr_eq(&a, &c));
        assert_eq!(c.frame_count(), a.frame_count() - 1);
        // 空の表は断る
        assert!(pool.video_pts(p, &Arc::new(vec![])).is_err());
        pool.forget(p);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    /// 実際の可変 fps の HEVC 素材で、-ss から読み始めたコマが、順に読んで着いたコマと同じか（手で走らせる）:
    /// AK_TEST_GUIDE_VIDEO=<素材の mp4> AK_TEST_GUIDE_ANALYSIS=<guide_analysis.json> cargo test -p ak-studio real_vfr -- --ignored
    #[test]
    #[ignore = "実際の素材と解析の結果が要る（AK_TEST_GUIDE_VIDEO・AK_TEST_GUIDE_ANALYSIS）"]
    fn real_vfr_seek_matches_sequential_read() {
        let (Ok(video), Ok(ana)) = (std::env::var("AK_TEST_GUIDE_VIDEO"), std::env::var("AK_TEST_GUIDE_ANALYSIS")) else {
            panic!("AK_TEST_GUIDE_VIDEO と AK_TEST_GUIDE_ANALYSIS を指定してください");
        };
        let ff = test_ffmpeg().expect("ffmpeg が見つかりません");
        let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(ana).unwrap()).unwrap();
        let pts: Arc<Vec<f64>> = Arc::new(v["pts"].as_array().unwrap().iter().map(|x| x.as_f64().unwrap()).collect());
        for idx in [600usize, 5000, 11778, 15000] {
            let seek = VideoSource::with_pts(&ff, &video, pts.clone()).unwrap();
            let a = seek.frame_opt(idx as i64, 160, 120, false).unwrap();
            let seq = VideoSource::with_pts(&ff, &video, pts.clone()).unwrap();
            seq.frame_opt(idx as i64 - 20, 160, 120, false).unwrap();
            let b = seq.frame_opt(idx as i64, 160, 120, false).unwrap();
            let diff: u64 = a.data.iter().zip(&b.data).map(|(x, y)| (*x as i32 - *y as i32).unsigned_abs() as u64).sum();
            assert_eq!(diff, 0, "idx={idx}: -ss で読み始めたコマが、順に読んだコマと違います");
        }
        // 先頭から（-ss の誤差が入らない）順に読んで着いたコマとも同じ
        let head = VideoSource::with_pts(&ff, &video, pts.clone()).unwrap();
        for i in [0i64, 250, 500, 600] {
            head.frame_opt(i, 160, 120, false).unwrap();
        }
        let a = head.frame_opt(600, 160, 120, false).unwrap();
        let b = VideoSource::with_pts(&ff, &video, pts.clone()).unwrap().frame_opt(600, 160, 120, false).unwrap();
        assert_eq!(a.data, b.data, "先頭から順に読んだ 600 コマ目と、-ss で読み始めた 600 コマ目が違います");
    }

    /// 固定 fps（30）の、コマごとに絵の違う短い動画（testsrc2、1 秒 = 30 コマ）
    fn make_cfr_video(ff: &str, name: &str) -> Option<std::path::PathBuf> {
        let dir = std::env::temp_dir().join(format!("ak-media-cfr-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let out = dir.join("v.mp4");
        let o = hidden(Command::new(ff))
            .args(["-hide_banner", "-loglevel", "error", "-y", "-f", "lavfi", "-i", "testsrc2=size=128x96:rate=30:duration=1",
                   "-c:v", "libx264", "-pix_fmt", "yuv420p", "-g", "15"])
            .arg(&out)
            .output()
            .ok()?;
        if !o.status.success() {
            eprintln!("動画を作れないので skip: {}", String::from_utf8_lossy(&o.stderr));
            return None;
        }
        Some(out)
    }

    /// 2 枚の絵の画素の差の最大
    fn max_diff(a: &Rgb, b: &Rgb) -> i32 {
        a.data.iter().zip(&b.data).map(|(x, y)| (*x as i32 - *y as i32).abs()).max().unwrap_or(0)
    }

    /// 旧#153: 途中から読み始めた（-ss）コマが、先頭から順に読んで着いたコマと同じ。
    /// コマの時刻ちょうどから読むと、-ss の小数 4 桁の丸めで idx が 3 の倍数 + 2 のとき（x.x667 秒）1 つ先のコマが返っていた
    #[test]
    fn plain_seek_returns_same_frame_as_sequential_read() {
        let Some(ff) = test_ffmpeg() else {
            eprintln!("ffmpeg が無いので skip");
            return;
        };
        let Some(path) = make_cfr_video(&ff, "seek") else { return };
        let p = path.to_str().unwrap();
        let seq = VideoSource::new(&ff, p).unwrap();
        let reference: Vec<Rgb> = (0..12).map(|i| seq.frame_opt(i, 64, 48, false).unwrap()).collect();
        let mut wrong = vec![];
        for idx in 0..12 {
            let f = VideoSource::new(&ff, p).unwrap().frame_opt(idx, 64, 48, false).unwrap();
            if max_diff(&f, &reference[idx as usize]) > 0 {
                wrong.push(idx);
            }
        }
        assert!(wrong.is_empty(), "-ss で読み始めると順読みと違うコマになる idx: {wrong:?}");
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    /// 旧#153: モニターの読み方（cache=true）で同じコマを 2 回引くと同じ絵。
    /// 1 回目に ffmpeg の生の絵、2 回目に JPEG（品質 90）にして覚えた絵を返すと、画素で 30 前後違っていた
    #[test]
    fn cached_frame_is_same_on_second_read() {
        let Some(ff) = test_ffmpeg() else {
            eprintln!("ffmpeg が無いので skip");
            return;
        };
        let Some(path) = make_cfr_video(&ff, "cache") else { return };
        let v = VideoSource::new(&ff, path.to_str().unwrap()).unwrap();
        // 19 は前から -ss の丸めでずれなかった番号、20 はずれていた番号
        for idx in [19, 20] {
            let a = v.frame_opt(idx, 64, 48, true).unwrap();
            let b = v.frame_opt(idx, 64, 48, true).unwrap();
            assert_eq!(max_diff(&a, &b), 0, "idx={idx}: 1 回目と 2 回目で絵が違う");
        }
        // 書き出しの読み方（確認画像）が読み手に残した生の絵も、モニターには JPEG の絵で返す
        v.frame_opt(25, 64, 48, false).unwrap();
        let a = v.frame_opt(25, 64, 48, true).unwrap();
        let b = v.frame_opt(25, 64, 48, true).unwrap();
        assert_eq!(max_diff(&a, &b), 0, "書き出しの読み方のあとのモニター");
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    /// 旧#153: モニター（cache=true）と書き出し（cache=false）で、同じ idx に同じコマが来る（JPEG の誤差は許す）。
    /// -ss の丸めで 1 つ先から読むと、モニターが 17 から読み始めたときに 19 として 20 のコマを覚え、書き出しの 19 と別のコマになっていた
    #[test]
    fn preview_and_export_pick_the_same_frame() {
        let Some(ff) = test_ffmpeg() else {
            eprintln!("ffmpeg が無いので skip");
            return;
        };
        let Some(path) = make_cfr_video(&ff, "pv") else { return };
        let p = path.to_str().unwrap();
        let export = VideoSource::new(&ff, p).unwrap();
        let e19 = export.frame_opt(19, 64, 48, false).unwrap();
        let e20 = export.frame_opt(20, 64, 48, false).unwrap();
        let preview = VideoSource::new(&ff, p).unwrap();
        preview.frame_opt(17, 64, 48, true).unwrap();
        let p19 = preview.frame_opt(19, 64, 48, true).unwrap();
        let (to19, to20) = (max_diff(&p19, &e19), max_diff(&p19, &e20));
        assert!(to19 < to20, "モニターの 19 が書き出しの 20 に近い（19 との差 {to19}、20 との差 {to20}）");
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    /// 旧#153: 最後のコマ（ffmpeg の fps フィルタが出さない）は、1 つ前のコマで代える。
    /// 新しい読み手で引いても、手前から順に読んでいる読み手で引いても、書き出し・モニターのどちらでも
    #[test]
    fn last_frame_falls_back_to_previous() {
        let Some(ff) = test_ffmpeg() else {
            eprintln!("ffmpeg が無いので skip");
            return;
        };
        let Some(path) = make_cfr_video(&ff, "last") else { return };
        let p = path.to_str().unwrap();
        let last = VideoSource::new(&ff, p).unwrap().frame_count() - 1;
        assert_eq!(last, 29);
        let before = VideoSource::new(&ff, p).unwrap().frame_opt(last - 1, 64, 48, false).unwrap();
        let fresh = VideoSource::new(&ff, p).unwrap().frame_opt(last, 64, 48, false).unwrap();
        assert_eq!(max_diff(&fresh, &before), 0, "新しい読み手");
        let seq = VideoSource::new(&ff, p).unwrap();
        seq.frame_opt(25, 64, 48, false).unwrap();
        assert_eq!(max_diff(&seq.frame_opt(last, 64, 48, false).unwrap(), &before), 0, "順に読んでいる読み手");
        // 範囲の外も最後のコマ
        assert_eq!(max_diff(&seq.frame_opt(99, 64, 48, false).unwrap(), &before), 0, "範囲の外");
        // モニターの読み方: 1 つ前のコマ（覚えた JPEG の絵）と同じ
        let mon = VideoSource::new(&ff, p).unwrap();
        let a = mon.frame_opt(last, 64, 48, true).unwrap();
        assert_eq!(max_diff(&a, &mon.frame_opt(last - 1, 64, 48, true).unwrap()), 0, "モニター");
        assert!(max_diff(&a, &before) <= 40, "モニターも 1 つ前のコマ（JPEG の誤差だけ）");
        // 読めないと分かったら、もう最後のコマのために読み手を作り直さない
        let n = mon.starts.load(std::sync::atomic::Ordering::Relaxed);
        for _ in 0..3 {
            mon.frame_opt(last, 64, 48, true).unwrap();
            seq.frame_opt(last, 64, 48, false).unwrap();
        }
        assert_eq!(mon.starts.load(std::sync::atomic::Ordering::Relaxed), n, "モニター");
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    /// st 以前でいちばん後のコマの明るさ
    fn level_at(pts: &[f64], levels: &[u8], st: f64) -> i32 {
        let i = pts.iter().rposition(|&p| p <= st + 1e-6).unwrap_or(0);
        levels[i] as i32
    }

    const READ_ORDER: [f64; 8] = [0.55, 0.05, 0.9, 0.12, 0.33, 1.0, 0.2, 0.6];

    #[test]
    fn scan_pts_matches_showinfo() {
        let Some(ff) = test_ffmpeg() else {
            eprintln!("ffmpeg が無いので skip");
            return;
        };
        let Some((path, pts, _)) = make_vfr_video(&ff, "scan") else { return };
        let got = scan_pts(&ff, path.to_str().unwrap()).unwrap();
        assert_eq!(got.len(), pts.len());
        for (a, b) in got.iter().zip(&pts) {
            assert!((a - b).abs() < 1e-4, "{got:?} vs {pts:?}");
        }
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    /// 頭を切り出した（-c copy の）動画は、編集リストで捨てる pts が負のパケットを持つ。デコードで出るコマだけを数える
    #[test]
    fn scan_pts_skips_packets_cut_by_edit_list() {
        let Some(ff) = test_ffmpeg() else {
            eprintln!("ffmpeg が無いので skip");
            return;
        };
        let Some(path) = make_cfr_video(&ff, "cut") else { return };
        let cut = path.with_file_name("cut.mov");
        let o = hidden(Command::new(&ff))
            .args(["-hide_banner", "-loglevel", "error", "-y", "-ss", "0.3", "-i"])
            .arg(&path)
            .args(["-c", "copy"])
            .arg(&cut)
            .output()
            .unwrap();
        assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
        let o = hidden(Command::new(&ff))
            .args(["-hide_banner", "-nostdin", "-i"])
            .arg(&cut)
            .args(["-an", "-fps_mode", "passthrough", "-vf", "showinfo", "-f", "null", "-"])
            .output()
            .unwrap();
        let re = regex::Regex::new(r"pts_time:\s*([0-9.]+)").unwrap();
        let shown: Vec<f64> =
            re.captures_iter(&String::from_utf8_lossy(&o.stderr)).map(|c| c[1].parse().unwrap()).collect();
        let got = scan_pts(&ff, cut.to_str().unwrap()).unwrap();
        assert_eq!(got.len(), shown.len(), "{got:?} vs {shown:?}");
        for (a, b) in got.iter().zip(&shown) {
            assert!((a - b).abs() < 1e-4, "{got:?} vs {shown:?}");
        }
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    /// 旧#155: 可変 fps の素材は、読む順によらず「st 以前でいちばん後のコマ」が返る（書き出し・モニターのどちらでも）
    #[test]
    fn pool_reads_variable_fps_by_pts() {
        let Some(ff) = test_ffmpeg() else {
            eprintln!("ffmpeg が無いので skip");
            return;
        };
        let Some((path, pts, levels)) = make_vfr_video(&ff, "poolvfr") else { return };
        let p = path.to_str().unwrap();
        let pool = MediaPool::default();
        *pool.ffmpeg.lock() = ff.clone();
        let v = pool.video(p).unwrap();
        assert!(v.pts().is_some(), "可変 fps の素材は表つきで開く");
        let mut exported = vec![];
        for cache in [false, true] {
            for st in READ_ORDER {
                let f = v.frame_opt(v.index_at(st), 32, 24, cache).unwrap();
                let got = f.data[(12 * 32 + 16) * 3] as i32;
                assert!((got - level_at(&pts, &levels, st)).abs() <= 12, "cache={cache} st={st} 期待 {} 実際 {got}",
                        level_at(&pts, &levels, st));
                if !cache {
                    exported.push((st, f));
                }
            }
        }
        // 新しい置き場で引いても同じ絵（読み始めの位置によらない）
        let pool2 = MediaPool::default();
        *pool2.ffmpeg.lock() = ff;
        let v2 = pool2.video(p).unwrap();
        for (st, f) in exported.iter().rev() {
            let g = v2.frame_opt(v2.index_at(*st), 32, 24, false).unwrap();
            assert_eq!(max_diff(f, &g), 0, "st={st}");
        }
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn pool_keeps_constant_fps_plain() {
        let Some(ff) = test_ffmpeg() else {
            eprintln!("ffmpeg が無いので skip");
            return;
        };
        let Some(path) = make_cfr_video(&ff, "poolcfr") else { return };
        let pool = MediaPool::default();
        *pool.ffmpeg.lock() = ff;
        let v = pool.video(path.to_str().unwrap()).unwrap();
        assert!(v.pts().is_none());
        assert_eq!(v.index_at(0.5), 15);
        assert_eq!(v.index_at(0.99), 29);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn is_regular_detects_grid() {
        let grid: Vec<f64> = (0..60).map(|i| i as f64 / 30.0).collect();
        assert!(is_regular(&grid, 30.0));
        assert!(is_regular(&[], 30.0));
        let fast: Vec<f64> = (0..200).map(|i| i as f64 / 60.0).collect();
        assert!(!is_regular(&fast, 57.48));
        let shifted: Vec<f64> = grid.iter().map(|p| p + 0.5 / 30.0).collect();
        assert!(!is_regular(&shifted, 30.0));
        let mut gap = grid.clone();
        gap.remove(20);
        assert!(!is_regular(&gap, 30.0));
    }

    #[test]
    fn plain_video_source_has_no_pts_mode() {
        // 既存のクリップ（pts なし）の frame_count は今までの式のまま
        let v = VideoSource {
            path: String::new(),
            info: VideoInfo { w: 64, h: 48, fps: 30.0, dur: 2.0, image: false, has_audio: false, has_video: true },
            ffmpeg: String::new(),
            dec: Mutex::new(Decoders::default()),
            starts: Default::default(),
            cache: Mutex::new(HashMap::new()),
            cache_bytes: Mutex::new(0),
            budget: 0,
            pts: std::sync::OnceLock::from(Ok(None)),
            last_readable: std::sync::atomic::AtomicI64::new(i64::MAX),
        };
        assert_eq!(v.frame_count(), 60);
    }

    #[test]
    fn mime_and_kind_by_ext() {
        assert_eq!(mime_of("a.bmp"), "image/bmp");
        assert_eq!(mime_of("a.MOV"), "video/quicktime");
        assert_eq!(mime_of("C:\\x\\a.webm"), "video/webm");
        assert_eq!(mime_of("a.xyz"), "application/octet-stream");
        assert_eq!(mime_of("noext"), "application/octet-stream");
        assert!(MediaPool::is_image("A.BMP") && !MediaPool::is_image("a.mp4"));
        assert!(MediaPool::is_audio("x.Ogg") && !MediaPool::is_audio("x.png"));
        // 表にある拡張子はどれも Content-Type が決まっている
        for e in IMAGE_EXTS.iter().chain(VIDEO_EXTS).chain(AUDIO_EXTS) {
            assert_ne!(mime_of(&format!("a.{e}")), "application/octet-stream", "{e}");
        }
    }

    #[test]
    fn blit_in_clips_to_rect() {
        let mut dst = Rgb { w: 8, h: 8, data: vec![0; 8 * 8 * 3] };
        let src = Rgb { w: 8, h: 8, data: vec![200; 8 * 8 * 3] };
        blit_in(&mut dst, &src, -2, -2, 1.0, [2, 3, 6, 5]);
        for y in 0..8 {
            for x in 0..8 {
                let inside = (2..6).contains(&x) && (3..5).contains(&y);
                assert_eq!(dst.data[(y * 8 + x) * 3], if inside { 200 } else { 0 }, "({x}, {y})");
            }
        }
    }

    #[test]
    fn soft_edge_fades_only_cropped_sides() {
        // 右の辺だけ x=6 でクロップし、4px でぼかす
        let soft = Soft { rect: [f64::NEG_INFINITY, f64::NEG_INFINITY, 6.0, f64::INFINITY], px: 4.0 };
        let mut dst = Rgb { w: 8, h: 1, data: vec![0; 8 * 3] };
        let src = Rgb { w: 8, h: 1, data: vec![200; 8 * 3] };
        blit_soft(&mut dst, &src, 0, 0, 1.0, [0, 0, 6, 1], Some(soft));
        let v: Vec<u8> = dst.data.chunks(3).map(|p| p[0]).collect();
        assert_eq!(v[0], 200); // 左の辺はクロップしていないので、ぼかさない
        assert!(v[3] < 200 && v[3] > v[5]); // 右の辺に近いほど薄い
        assert_eq!(v[6], 0); // クロップした外は描かない
    }

    #[test]
    fn scan_line_stays_in_rect() {
        let mut dst = Rgb { w: 10, h: 4, data: vec![0; 10 * 4 * 3] };
        scan_line(&mut dst, 5.0, true, [0, 4], [0, 1, 10, 3], 1.0, 1.0);
        let px = |x: usize, y: usize| dst.data[(y * 10 + x) * 3];
        assert!(px(5, 1) > 150 && px(4, 2) > 150); // 線の上
        assert_eq!(px(5, 0), 0); // rect の外
        assert_eq!(px(0, 1), 0); // 離れたところ
    }

    #[test]
    fn blend_in_clips_to_rect() {
        let mut dst = Rgb { w: 6, h: 4, data: vec![0; 6 * 4 * 3] };
        let src = Rgba { w: 6, h: 4, data: [255u8, 255, 255, 255].repeat(24) };
        blend_in(&mut dst, &src, 0, 0, 1.0, 1.0, [1, 1, 3, 2]);
        let lit: usize = dst.data.chunks(3).filter(|p| p[0] == 255).count();
        assert_eq!(lit, 2);
    }
}
