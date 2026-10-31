//! 合成・コマ・静止画・先読み・シーン・字幕の描画。

use anyhow::Context;
use crate::render::cache::{any_group_at, frame_key, CachedOverlay};
use crate::render::layers::{self, Layer, MAX_IMAGE_PX, MAX_VIDEO_PX};
use crate::render::engine::FrameReq;
use crate::render::media::{blend, encode_jpeg, MediaPool, Rgb, Rgba};
use crate::render::subs::{SubBitmap, SubJob};
use super::{rec_window, Core};
use crate::app::dto::PlayReady;
use crate::voice::layout::VoiceItem;
use anyhow::{anyhow, bail, Result};
use parking_lot::Mutex;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// tag ごとに走っている先読みの依頼
#[derive(Clone)]
pub(super) struct PrefetchJob {
    cancel: Arc<AtomicBool>,
    /// 次に頼むコマ
    next: Arc<AtomicI64>,
    f0: i64,
    /// 終わり（延ばされる）
    f1: Arc<AtomicI64>,
    scale: f64,
    /// 生きているスレッドの数
    live: Arc<AtomicUsize>,
    /// 終えたコマの数
    done: Arc<AtomicI64>,
    /// 描けなかったコマの数と、最初のエラー
    failed: Arc<AtomicUsize>,
    first_err: Arc<Mutex<Option<String>>>,
    /// スレッドの本数の上限
    n: usize,
}

impl Core {
    /// 演出のコマ（キャッシュつき）。hide / solo があるときはキャッシュしない。
    pub fn overlay(&self, f: i64, scale: f64, hide: &[String], solo: Option<&str>, urgent: bool)
                   -> Result<Option<Arc<CachedOverlay>>> {
        let Some(desc) = self.desc() else { return Ok(None) };
        if !self.engine.ready() {
            return Ok(None);
        }
        let t = f as f64 / desc.fps;
        let ov = self.view_overrides();
        // 見比べの間はキャッシュを使わない（ワーカーの微調整と鍵が食い違うことがあるので）
        let plain = hide.is_empty() && solo.is_none() && self.preview.read().is_none();
        let key = frame_key(&desc, &ov, f, scale, self.placeholder());
        if plain {
            if let Some(c) = self.cache.get(key) {
                return Ok(Some(c));
            }
            // 何も出ていないコマは描かない
            if !any_group_at(&desc.groups, &ov, t) {
                return Ok(None);
            }
        }
        let fr = self.engine.frame(
            FrameReq { t, scale, hide: hide.to_vec(), solo: solo.map(|s| s.to_string()) },
            urgent,
            Duration::from_secs(if urgent { 20 } else { 120 }),
        );
        let fr = match fr {
            Ok(fr) => fr,
            Err(e) => {
                if plain {
                    // 再生で待ち続けないよう、描けなかったコマを覚える（直せば鍵が変わる）
                    self.cache.mark_failed(key);
                }
                return Err(e);
            }
        };
        let c = Arc::new(CachedOverlay::from_frame(&fr));
        if plain {
            // 描いている間に微調整が変わっていたら覚えない
            if frame_key(&desc, &self.view_overrides(), f, scale, self.placeholder()) == key {
                self.cache.put(key, c.clone());
            }
        }
        Ok(Some(c))
    }

    /// 1 コマ（背景・録画・立ち絵・演出・字幕を重ねたもの）。演出のコマが失敗したら全体を失敗にする（確認画像用）。
    /// 動画の素材は書き出しと同じ読み方（JPEG のキャッシュを通さない）で、書き出しと画素まで同じ絵にする（旧#153）
    pub fn compose(&self, t: f64, scale: f64, hide: &[String], solo: Option<&str>, with_fx: bool, subs: bool)
                   -> Result<Rgb> {
        self.compose_fx(t, scale, hide, solo, with_fx, subs, false, None)
    }

    /// モニターの 1 コマ。演出のコマが失敗しても演出だけ抜いて描き、そのエラーの文を返す（BR-15）。
    pub fn compose_monitor(&self, t: f64, scale: f64, hide: &[String], solo: Option<&str>, with_fx: bool,
                           subs: bool) -> Result<(Rgb, Option<String>)> {
        let mut fx_err = None;
        let fr = self.compose_fx(t, scale, hide, solo, with_fx, subs, true, Some(&mut fx_err))?;
        // 画面の表示はコマが変わると消えるので、ログ（fx_log）にも残す。文が変わったときだけ
        if let Some(e) = &fx_err {
            let mut last = self.last_fx_error.lock();
            if last.as_deref() != Some(e.as_str()) {
                *last = Some(e.clone());
                self.engine.push_log(e.clone());
            }
        }
        Ok((fr, fx_err))
    }

    #[allow(clippy::too_many_arguments)]
    fn compose_fx(&self, t: f64, scale: f64, hide: &[String], solo: Option<&str>, with_fx: bool, subs: bool,
                  video_cache: bool, fx_err: Option<&mut Option<String>>) -> Result<Rgb> {
        let fps = self.fps();
        let f = (t * fps).round() as i64;
        let ovf = |_: f64| -> Result<Option<Arc<CachedOverlay>>> {
            if with_fx { self.overlay(f, scale, hide, solo, true) } else { Ok(None) }
        };
        self.compose_with(t, scale, &ovf, video_cache, subs, fx_err)
    }

    /// 合成の本体。演出のコマは ovf から（書き出しでは別のワーカーから取る）。字幕は一番上（演出の後）。
    /// fx_err があれば（モニター）演出のコマが失敗しても演出だけ抜いてエラーの文を入れ、
    /// 無ければ（書き出し・確認画像）失敗を返す。黙って演出の抜けた動画を作らないため。
    pub fn compose_with(&self, t: f64, scale: f64, ovf: &dyn Fn(f64) -> Result<Option<Arc<CachedOverlay>>>,
                        video_cache: bool, subs: bool, mut fx_err: Option<&mut Option<String>>) -> Result<Rgb> {
        let (w, h) = layers::frame_size(scale);
        let mut fr = Rgb { w, h, data: [12u8, 18, 28].repeat((w * h) as usize) };
        let Some(doc) = self.view_doc() else { return Ok(fr) };
        let speaking = self.speaking_at(t);
        for layer in layers::layer_order(&doc.tracks) {
            match layer {
                Layer::GuideAndNests => {
                    // 攻略動画の戦闘画面のレイヤーは演出の下。読めない・組み立てられないときは、演出の失敗と同じ出し方で知らせる
                    if let Err(e) = self.draw_guide(&mut fr, t, scale, video_cache) {
                        let msg = format!("攻略動画を描けませんでした: {e:#}");
                        match fx_err.as_deref_mut() {
                            Some(slot) => {
                                slot.get_or_insert(msg);
                            }
                            None => return Err(anyhow!(msg)),
                        }
                    }
                    // RecWindow に差し込んだ録画は演出の下（窓の枠・チップ・テロップが上に来る）
                    self.draw_nests(&mut fr, &doc, t, scale, video_cache);
                }
                Layer::Media(i) => {
                    for c in doc.tracks[i].clips.iter() {
                        if !layers::clip_live(c, t) {
                            continue;
                        }
                        let ts = crate::render::trans::state(c, t);
                        if ts.alpha <= 0.001 {
                            continue;
                        }
                        let bright = layers::speaker_bright(&c.speaker, speaking.as_deref());
                        if let Err(e) = self.draw_clip(&mut fr, c, t, scale, &ts, bright, video_cache) {
                            self.note_issue_err(&format!("素材を描けませんでした: {}", c.src), &e);
                        }
                    }
                }
                Layer::Fx => {
                    if let Some(ov) = fx_frame(ovf, t, fx_err.as_deref_mut())? {
                        if let Some(bb) = ov.rect {
                            let src = Rgba { w: bb[2], h: bb[3], data: ov.rgba() };
                            blend(&mut fr, &src, bb[0] as i64, bb[1] as i64, 1.0, 1.0);
                        }
                    }
                }
            }
        }
        if subs {
            match self.sub_bitmap_at(t, scale) {
                Ok(Some(b)) if !b.is_empty() => blend(&mut fr, &b.rgba(), b.x, b.y, 1.0, 1.0),
                Ok(_) => {}
                Err(e) => self.note_issue_err("字幕を描けませんでした", &e),
            }
        }
        Ok(fr)
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_clip(&self, fr: &mut Rgb, c: &crate::project::MediaClip, t: f64, scale: f64, ts: &crate::render::trans::TransState,
                 bright: f32, video_cache: bool) -> Result<()> {
        let place = |sw: f64, sh: f64| layers::place(c, ts, scale, (0.0, 0.0), (1.0, 1.0), sw, sh);
        self.draw_src(fr, c, t, &place, ts, ts.alpha as f32, bright, [0, 0, i64::MAX, i64::MAX], video_cache)
    }

    /// 素材 1 つを描く。place は素材の大きさ → フレームの画素での (左, 上, 幅, 高さ)。rect の外は描かない。
    /// クロップ（crop_l/t/r/b、%）した辺は切り落とし、feather があればその辺をぼかす。
    /// トランジション（ts）のワイプは見える範囲を狭めて走査線を引き、グリッチは素材の絵を崩す（位置・倍率は place に入れておく）。
    #[allow(clippy::too_many_arguments)]
    fn draw_src(&self, fr: &mut Rgb, c: &crate::project::MediaClip, t: f64,
                place: &dyn Fn(f64, f64) -> (f64, f64, f64, f64), ts: &crate::render::trans::TransState, a: f32, bright: f32,
                rect: [i64; 4], video_cache: bool) -> Result<()> {
        let fscale = fr.w as f64 / 1920.0;
        let gseed = || crate::render::trans::seed(&c.id, (t * self.fps()).round() as i64);
        // ワイプの走査線（位置, 縦か, 線の長さの範囲）
        let mut edge: Option<(f64, bool, [i64; 2])> = None;
        if MediaPool::is_image(&c.src) {
            let img = self.media.image(&c.src)?;
            let (x, y, w, h) = place(img.full.w as f64, img.full.h as f64);
            let (wi, hi) = (w.round().max(1.0) as u32, h.round().max(1.0) as u32);
            if let Some(m) = layers::too_large(wi, hi, MAX_IMAGE_PX) {
                bail!(m);
            }
            let mut im = img.at(wi, hi, c.flip)?;
            if ts.glitch > 0.0 {
                let mut g = (*im).clone();
                crate::render::trans::glitch(&mut g.data, g.w, g.h, 4, ts.glitch, fscale, gseed());
                im = Arc::new(g);
            }
            let reg = layers::crop_region(c, ts, x, y, w, h, rect, fscale);
            edge = reg.edge;
            crate::render::media::blend_soft(fr, &im, x.round() as i64, y.round() as i64, a, bright, reg.rect, reg.soft);
        } else if !MediaPool::is_audio(&c.src) {
            let v = self.media.video(&c.src)?;
            if !v.info.has_video {
                return Ok(());
            }
            let idx = v.index_at(layers::video_src_time(c, t, v.info.dur, v.info.fps));
            let (x, y, w, h) = place(v.info.w as f64, v.info.h as f64);
            let (wi, hi, xi, yi) = layers::even_fit(x, y, w, h);
            if let Some(m) = layers::too_large(wi, hi, MAX_VIDEO_PX) {
                bail!(m);
            }
            let mut im = v.frame_opt(idx, wi, hi, video_cache)?;
            if ts.glitch > 0.0 {
                crate::render::trans::glitch(&mut im.data, im.w, im.h, 3, ts.glitch, fscale, gseed());
            }
            let reg = layers::crop_region(c, ts, x, y, w, h, rect, fscale);
            edge = reg.edge;
            // 偶数に丸めたぶんは中心で合わせる（xi, yi）
            crate::render::media::blit_soft(fr, &im, xi, yi, a, reg.rect, reg.soft);
        }
        if let Some((pos, vert, span)) = edge {
            crate::render::media::scan_line(fr, pos, vert, span, rect, a, fscale);
        }
        Ok(())
    }

    /// RecWindow に差し込んだ録画（ネスト）を描く。窓が見えている間だけ、窓とキャンバスの重なりの中だけに。
    pub fn draw_nests(&self, fr: &mut Rgb, doc: &crate::project::ProjectDoc, t: f64, scale: f64, video_cache: bool) {
        if doc.nests.is_empty() {
            return;
        }
        let Some(desc) = self.desc() else { return };
        let ov = self.view_overrides();
        for (key, nest) in doc.nests.iter() {
            let Some(g) = desc.groups.iter().find(|g| &g.key == key) else { continue };
            let Some((bx, wa)) = rec_window(g, nest, &ov, t) else { continue };
            let (sx, sy, ox, oy) = nest.map(bx);
            let Some(vis) = layers::nest_visible(bx, (sx, sy, ox, oy), nest.w, nest.h, scale) else { continue };
            for tr in nest.tracks.iter() {
                if tr.hidden {
                    continue;
                }
                for c in tr.clips.iter() {
                    if !layers::clip_live(c, t) {
                        continue;
                    }
                    let ts = crate::render::trans::state(c, t);
                    let a = ts.alpha * wa;
                    if a <= 0.001 {
                        continue;
                    }
                    let place = |sw: f64, sh: f64| layers::place(c, &ts, scale, (ox, oy), (sx, sy), sw, sh);
                    if let Err(e) = self.draw_src(fr, c, t, &place, &ts, a as f32, 1.0, vis, video_cache) {
                        self.note_issue_err(&format!("録画の窓の素材を描けませんでした: {}", c.src), &e);
                    }
                }
            }
        }
    }

    pub fn speaking_at(&self, t: f64) -> Option<String> {
        self.layout_guard().iter().find(|it| it.start <= t && t < it.start + it.dur).map(|it| it.sp.clone())
    }

    pub fn frame_jpeg(&self, t: f64, scale: f64, hide: &[String], solo: Option<&str>, q: u8) -> Result<Vec<u8>> {
        let fr = self.compose(t, scale, hide, solo, true, true)?;
        encode_jpeg(&fr, q)
    }

    /// 1 つのクリップだけの透過 PNG（モニターでドラッグするときの見本）と、その位置。
    pub fn clip_png(&self, t: f64, scale: f64, key: &str) -> Result<(Vec<u8>, Option<[u32; 4]>)> {
        let fps = self.fps();
        let f = (t * fps).round() as i64;
        // ドラッグの見本は「微調整なし」の位置で描く（ずらしは画面側で足す）
        let Some(ov) = self.overlay(f, scale, &[], Some(key), true)? else { return Ok((vec![], None)) };
        let Some(bb) = ov.rect else { return Ok((vec![], None)) };
        let img = image::RgbaImage::from_raw(bb[2], bb[3], ov.rgba()).ok_or_else(|| anyhow!("重ねる絵の大きさが合いません"))?;
        let mut out = std::io::Cursor::new(vec![]);
        image::DynamicImage::ImageRgba8(img).write_to(&mut out, image::ImageFormat::Png)?;
        Ok((out.into_inner(), Some(bb)))
    }

    /// 先読み（レンダリング）。tag が同じ前の依頼は止める。
    /// ただし同じ倍率で前の依頼がまだ生きていて、始まりがその範囲の中なら、止めずに終わりを延ばす
    /// （再生中は 1 秒に何度も呼ばれるので、そのたびにスレッドを立て直さない）。
    pub fn prefetch(self: &Arc<Self>, t0: f64, t1: f64, scale: f64, tag: &str) {
        self.prefetch_subs(t0, t1 + 6.0, scale);
        if !self.engine.ready() {
            // 演出が読めていないと overlay は何も描かずに返るので、頼まない。
            // 再生の先読みは黙って返す（演出の状態はモニターに出ている。コマごとに呼ばれるので知らせを重ねない）
            if tag != "play" {
                self.notice_not_ready(tag, 0, 0);
            }
            return;
        }
        let Some(desc) = self.desc() else { return };
        let (f0, f1) = layers::prefetch_frames(t0, t1, desc.fps, desc.end);
        let mut jobs = self.prefetch_jobs.lock();
        if f1 <= f0 {
            // 取り消すだけ（何も頼まない）
            if let Some(old) = jobs.remove(tag) {
                old.cancel.store(true, Ordering::SeqCst);
            }
            return;
        }
        if let Some(old) = jobs.get(tag) {
            let live = old.live.load(Ordering::SeqCst);
            if crate::render::ready::can_extend_prefetch(old.scale, old.f0, old.f1.load(Ordering::SeqCst), live,
                                                         scale, f0) {
                old.f1.fetch_max(f1, Ordering::SeqCst);
                old.next.fetch_max(f0, Ordering::SeqCst);
                // 途中で抜けたスレッドがいたら足す（抜けるときの見直しとは、live を足してから起こすので二重にならない）
                let more = crate::render::ready::missing_workers(old.n, old.live.load(Ordering::SeqCst));
                if more > 0 {
                    old.live.fetch_add(more, Ordering::SeqCst);
                    let job = old.clone();
                    drop(jobs);
                    for _ in 0..more {
                        self.spawn_prefetch_worker(job.clone(), tag.to_string());
                    }
                }
                return;
            }
        }
        let n = self.engine.workers().max(1);
        let job = PrefetchJob {
            cancel: Arc::new(AtomicBool::new(false)),
            next: Arc::new(AtomicI64::new(f0)),
            f0,
            f1: Arc::new(AtomicI64::new(f1)),
            scale,
            live: Arc::new(AtomicUsize::new(n)),
            done: Arc::new(AtomicI64::new(0)),
            failed: Arc::new(AtomicUsize::new(0)),
            first_err: Arc::new(Mutex::new(None)),
            n,
        };
        if let Some(old) = jobs.insert(tag.to_string(), job.clone()) {
            old.cancel.store(true, Ordering::SeqCst);
        }
        drop(jobs);
        for _ in 0..n {
            self.spawn_prefetch_worker(job.clone(), tag.to_string());
        }
    }

    /// 先読みのスレッドを 1 本立てる（live は呼ぶ側が先に足しておく）。
    /// 抜けるときは取った番号を返し、live を減らしたあとに終わりが延ばされていないか見直す（延ばしと行き違いにならない）。
    fn spawn_prefetch_worker(self: &Arc<Self>, job: PrefetchJob, tag: String) {
        let me = self.clone();
        std::thread::spawn(move || {
            let PrefetchJob { cancel, next, f0, f1: f1a, scale, live, done, failed, first_err, .. } = job;
            let mut last_emit = Instant::now();
            loop {
                loop {
                    if cancel.load(Ordering::SeqCst) {
                        break;
                    }
                    if !me.engine.ready() {
                        // 途中で演出の読み直しが始まった・読めなくなった。overlay は何も描かずに返るので「描けた」と数えず、
                        // 依頼を止めて知らせる（描けなかったコマには数えない。読み直しは失敗ではない）
                        if !cancel.swap(true, Ordering::SeqCst) {
                            // 止めた依頼を残すと、次の先読みが「延ばせる依頼」と見なすので外す
                            {
                                let mut jobs = me.prefetch_jobs.lock();
                                if jobs.get(&tag).is_some_and(|j| Arc::ptr_eq(&j.cancel, &cancel)) {
                                    jobs.remove(&tag);
                                }
                            }
                            if tag != "play" {
                                me.notice_not_ready(&tag, done.load(Ordering::SeqCst), f1a.load(Ordering::SeqCst) - f0);
                            }
                        }
                        break;
                    }
                    let f = next.fetch_add(1, Ordering::SeqCst);
                    // 終わりは延ばされることがあるので毎回読み直す
                    let f1 = f1a.load(Ordering::SeqCst);
                    if f >= f1 {
                        // 取った番号を返す（延ばされたあとに飛ばされないように）
                        next.fetch_min(f, Ordering::SeqCst);
                        break;
                    }
                    if let Err(e) = me.overlay(f, scale, &[], None, false) {
                        failed.fetch_add(1, Ordering::SeqCst);
                        first_err.lock().get_or_insert(format!("{e:#}"));
                    }
                    let d = done.fetch_add(1, Ordering::SeqCst) + 1;
                    if tag != "play" && last_emit.elapsed() > Duration::from_millis(700) {
                        last_emit = Instant::now();
                        me.emit("progress", json!({"task": tag, "done": d, "total": f1a.load(Ordering::SeqCst) - f0}));
                    }
                }
                let prev = live.fetch_sub(1, Ordering::SeqCst);
                if crate::render::ready::should_continue(cancel.load(Ordering::SeqCst), next.load(Ordering::SeqCst),
                                                         f1a.load(Ordering::SeqCst)) {
                    live.fetch_add(1, Ordering::SeqCst);
                    continue;
                }
                // 最後に抜けた 1 本だけが、終わったことを知らせる
                let d = done.load(Ordering::SeqCst);
                if prev == 1 && d >= f1a.load(Ordering::SeqCst) - f0 && tag != "play" && !cancel.load(Ordering::SeqCst) {
                    me.finish_prefetch(&tag, &done, &failed, &first_err, f1a.load(Ordering::SeqCst) - f0);
                    me.emit("cache", json!({}));
                }
                break;
            }
        });
    }

    /// 演出が読めていないのでレンダリングしない・止めたことを知らせる。読み直しの途中なら案内（もう一度押す）、
    /// 読めていない（エラー・タイムラインが無い）なら、その理由をエラーで出す
    fn notice_not_ready(&self, tag: &str, done: i64, total: i64) {
        let st = self.engine.status.read().clone();
        let (text, error) = crate::render::ready::not_ready_text(&st.state, st.error.as_deref(), done > 0);
        let mut p = json!({"task": tag, "done": done, "total": total, "finished": true});
        if error {
            p["error"] = json!(text);
            self.notice_error(text);
        } else {
            self.engine.push_log(text.clone());
            self.emit("toast", json!({"text": text, "kind": "info"}));
        }
        self.emit("progress", p);
    }

    /// 先読みが終わった（止めた）ことを知らせる。描けなかったコマがあれば、画面のトーストにも出す
    /// （再生の先読み "play" はログだけ。再生中の演出のエラーはモニターに別に出る）
    fn finish_prefetch(&self, tag: &str, done: &AtomicI64, failed: &AtomicUsize, first_err: &Mutex<Option<String>>, total: i64) {
        let failed = failed.load(Ordering::SeqCst);
        let report = crate::render::ready::prefetch_report(total, failed, first_err.lock().as_deref());
        if tag == "play" {
            if let Some(r) = report {
                self.log_unless_last(r);
            }
            return;
        }
        let d = done.load(Ordering::SeqCst);
        let mut p = json!({"task": tag, "done": d, "total": total, "finished": true});
        if let Some(r) = report {
            p["failed"] = json!(failed);
            p["error"] = json!(r);
            self.notice_error(r);
        }
        self.emit("progress", p);
    }

    /// 再生の前に、t0〜t1 の準備できていない区間（演出のコマ・字幕・声）を返す。先読みも頼む。
    /// fx / subs が false（モニターで隠している）なら、その分は待たない。
    pub fn play_ready(self: &Arc<Self>, t0: f64, t1: f64, scale: f64, fx: bool, subs: bool) -> PlayReady {
        self.prefetch(t0, t1, scale, "play");
        let mut r = PlayReady { fx: vec![], fx_count: [0, 0], subs: vec![], voice: vec![] };
        if fx && self.preview.read().is_none() && self.engine.ready() {
            if let Some(desc) = self.desc() {
                let (f0, f1) = layers::prefetch_frames(t0, t1, desc.fps, desc.end);
                let (g, n) = crate::render::ready::fx_gaps(&self.cache, &desc, &self.view_overrides(), scale,
                                                           self.placeholder(), f0, f1);
                r.fx = g;
                r.fx_count = n;
            }
        }
        // doc（st）を先に取ってから並び（vstate）を写す。ロックを持ったまま別のロックを取らない（open_project と逆順になる）
        let doc = self.doc();
        let items = self.layout();
        r.voice = crate::render::ready::voice_gaps(&items, t0, t1);
        if subs {
            if let Some(doc) = doc {
                if !crate::render::subs::hidden(&doc) {
                    r.subs = crate::render::ready::sub_gaps(&items, t0, t1, |it| {
                        let job = crate::render::subs::resolve(&doc, it);
                        !job.text.trim().is_empty() && self.subs.cached(&job, scale).is_none()
                            && !self.subs.is_failed(&job, scale)
                    });
                }
            }
        }
        r
    }

    pub fn cached_ranges(&self, scale: f64) -> Vec<[f64; 3]> {
        let Some(desc) = self.desc() else { return vec![] };
        crate::render::cache::cached_ranges(&self.cache, &desc, &self.overrides(), scale, self.placeholder())
    }

    /// シーン番号の一覧（クリップのずらし・隠すを入れた後の時刻で。決め方は `render::scenes`）。
    pub fn scenes(&self) -> Vec<(String, f64, f64)> {
        let mut v = match self.desc() {
            Some(desc) => crate::render::scenes::scenes(&desc, &self.overrides()),
            None => vec![],
        };
        // 攻略動画のパート（編成・手動・自動・リザルト）
        v.extend(self.guide_scenes());
        v
    }

    pub fn scene_at(&self, t: f64) -> String {
        crate::render::scenes::scene_at(&self.scenes(), t)
    }

    /// 確認画像（1920x1080 の PNG、字幕つき）。Claude に見せる用。字幕はモニター・書き出しと同じ絵。
    pub fn still(&self, t: f64, path: &Path, subs: bool, solo: Option<&str>) -> Result<PathBuf> {
        let fr = self.compose(t, 1.0, &[], solo, true, subs)?;
        let img = image::RgbImage::from_raw(fr.w, fr.h, fr.data).ok_or_else(|| anyhow!("コマの画像を作れませんでした"))?;
        if let Some(d) = path.parent() {
            std::fs::create_dir_all(d)?;
        }
        img.save(path)?;
        // Claude に見せる用の軽い JPEG（1280x720）
        let small = image::imageops::resize(&img, 1280, 720, image::imageops::FilterType::Triangle);
        let mut jf = std::fs::File::create(path.with_extension("jpg"))?;
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jf, 86).encode_image(&small)?;
        Ok(path.to_path_buf())
    }

    /// cue ごとのコマを並べた確認画像（`render_sheet`）。1 ページ 1920x1080 に 4x4 の 16 マ（1 マ 480x270）で、16 を超えたら次のページ。
    /// 1 マは `still` と同じ合成を縮めたもの。マの中に文字は描かない（時刻・シーン・cue は戻りの JSON）。
    /// 戻り: {"pages": [{"page", "path", "cells": [{"i", "t", "scene", "cue"}]}], "cells": 全部のコマの数}（i はページの中の位置。左上から右へ）
    pub fn still_sheet(&self, cues: Option<&[i64]>, ts: Option<&[f64]>, subs: bool, dir: &Path) -> Result<Value> {
        use crate::render::sheet;
        let desc = self.desc();
        if desc.is_none() && cues.is_some() {
            return Err(anyhow!(crate::msg::NO_FX));
        }
        let plan = sheet::plan(desc.as_ref().map(|d| d.cues.as_slice()).unwrap_or(&[]), cues, ts)?;
        let scenes = self.scenes();
        // 同じ頼みは同じ名前で置き換える（頼みごとに増えていかない）
        let tag = match (cues, ts) {
            (Some([a, b]), _) => format!("c{a}-{b}"),
            _ => {
                use std::hash::{Hash, Hasher};
                let mut h = std::collections::hash_map::DefaultHasher::new();
                plan.iter().for_each(|(_, t)| t.to_bits().hash(&mut h));
                format!("t{:08x}", h.finish() as u32)
            }
        };
        let sub_tag = if subs { "_sub" } else { "" };
        std::fs::create_dir_all(dir)?;
        let mut pages = vec![];
        for (pi, range) in sheet::pages(plan.len()).into_iter().enumerate() {
            let mut page = image::RgbImage::new(sheet::PAGE_W, sheet::PAGE_H);
            let mut cells = vec![];
            for (i, (cue, t)) in plan[range].iter().enumerate() {
                let fr = self.compose(*t, 1.0, &[], None, true, subs)?;
                let img = image::RgbImage::from_raw(fr.w, fr.h, fr.data).ok_or_else(|| anyhow!("コマの画像を作れませんでした"))?;
                let small = image::imageops::resize(&img, sheet::CELL_W, sheet::CELL_H, image::imageops::FilterType::Triangle);
                let (x, y) = sheet::cell_origin(i);
                image::imageops::replace(&mut page, &small, x as i64, y as i64);
                cells.push(json!({"i": i, "t": t, "scene": crate::render::scenes::scene_at(&scenes, *t), "cue": cue}));
            }
            let path = dir.join(format!("sheet_{tag}{sub_tag}_p{}.png", pi + 1));
            let mut png = vec![];
            page.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)?;
            crate::settings::write_atomic(&path, &png).with_context(|| format!("確認画像を保存できませんでした（{}）", path.display()))?;
            // Claude に見せる用の軽い JPEG（1280x720）
            let small = image::imageops::resize(&page, 1280, 720, image::imageops::FilterType::Triangle);
            let mut jpg = vec![];
            image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpg, 86).encode_image(&small)?;
            let jp = path.with_extension("jpg");
            crate::settings::write_atomic(&jp, &jpg).with_context(|| format!("確認画像を保存できませんでした（{}）", jp.display()))?;
            pages.push(json!({"page": pi + 1, "path": path.to_string_lossy().replace('\\', "/"), "cells": cells}));
        }
        Ok(json!({"pages": pages, "cells": plan.len()}))
    }

    /// タイムラインの終わり（画面の total() と同じ: 演出の終わり・セリフの終わり + 1 秒・繰り返しでない素材の終わり）
    pub fn timeline_end(&self, doc: &crate::project::ProjectDoc) -> f64 {
        let voice = self.layout_guard().iter().map(|i| i.start + i.dur).fold(0.0, f64::max);
        let mut m = self.desc().map(|d| d.end).unwrap_or(0.0).max(voice + 1.0).max(self.guide_end());
        for t in &doc.tracks {
            for c in t.clips.iter().filter(|c| !c.looped) {
                m = m.max(c.start + c.dur);
            }
        }
        m
    }

    // ------------------------------------------------------------------ 字幕
    /// セリフの並び（開始の順）。
    pub fn sorted_layout(&self) -> Vec<VoiceItem> {
        let mut v = self.layout();
        v.sort_by(|a, b| a.start.partial_cmp(&b.start).unwrap_or(std::cmp::Ordering::Equal).then(a.n.cmp(&b.n)));
        v
    }

    /// t に出ている字幕（セリフと、描くもの）。字幕のトラックを隠していれば None。
    pub fn sub_job_at(&self, t: f64) -> Option<(VoiceItem, SubJob)> {
        let doc = self.doc()?;
        if crate::render::subs::hidden(&doc) {
            return None;
        }
        let items = self.layout_guard();
        let it = crate::render::subs::active(&items, t)?.clone();
        let job = crate::render::subs::resolve(&doc, &it);
        Some((it, job))
    }

    pub fn sub_bitmap_at(&self, t: f64, scale: f64) -> Result<Option<Arc<SubBitmap>>> {
        let Some((_, job)) = self.sub_job_at(t) else { return Ok(None) };
        if job.text.trim().is_empty() {
            return Ok(None);
        }
        Ok(Some(self.subs.bitmap(&job, scale)?))
    }

    /// モニター用: t の字幕の行の位置とはみ出し（1920 の座標。文は入れない＝ HTTP のヘッダに載せるため ASCII だけ）。
    pub fn sub_info_at(&self, t: f64) -> Option<Value> {
        let (it, job) = self.sub_job_at(t)?;
        let info = self.subs.measure(std::slice::from_ref(&job)).ok()?.pop()?;
        Some(json!({"line": it.line_id, "n": it.n, "lines": info.lines, "overflow": info.overflow,
                    "l": info.l, "r": info.r, "manual": job.manual, "custom": job.custom}))
    }

    /// 全部のセリフの字幕の行の分け方とはみ出し（Python で測った結果。タイムラインの印・セリフの調整画面の「自動の改行」に使う）。
    /// 見た目・区間・鍵・孤立は測らずに決まるので `subs::view`（subs_view）の方。
    pub fn subs_check(&self) -> Result<Value> {
        let Some(doc) = self.doc() else { return Ok(json!({"lines": {}})) };
        let items = self.sorted_layout();
        let jobs: Vec<SubJob> = items.iter().map(|it| crate::render::subs::resolve(&doc, it)).collect();
        let infos = self.subs.measure(&jobs)?;
        let mut out = serde_json::Map::new();
        for (it, info) in items.iter().zip(infos.iter()) {
            out.insert(it.line_id.clone(), json!({"overflow": info.any_overflow(), "texts": info.texts}));
        }
        Ok(json!({"lines": out}))
    }

    /// t0〜t1 に出る字幕を先に描いておく（再生中に描くと、新しい行の頭でコマが遅れるため）。
    pub fn prefetch_subs(self: &Arc<Self>, t0: f64, t1: f64, scale: f64) {
        static BUSY: AtomicBool = AtomicBool::new(false);
        if BUSY.swap(true, Ordering::SeqCst) {
            return;
        }
        let me = self.clone();
        std::thread::spawn(move || {
            if let Err(e) = me.prerender_subs(t0, t1, scale, 1) {
                me.log_unless_last(format!("字幕を先に描けませんでした: {e:#}"));
            }
            BUSY.store(false, Ordering::SeqCst);
        });
    }

    /// t0〜t1 に出る字幕を描いてキャッシュに入れる（書き出しの前など）。threads 本で並べて頼む。
    pub fn prerender_subs(&self, t0: f64, t1: f64, scale: f64, threads: usize) -> Result<usize> {
        let Some(doc) = self.doc() else { return Ok(0) };
        if crate::render::subs::hidden(&doc) {
            return Ok(0);
        }
        let items = self.sorted_layout();
        let todo: Vec<SubJob> = items
            .iter()
            .filter(|it| it.start + it.dur + crate::render::subs::TAIL > t0 && it.start < t1)
            .map(|it| crate::render::subs::resolve(&doc, it))
            .filter(|j| !j.text.trim().is_empty() && self.subs.cached(j, scale).is_none() && !self.subs.is_failed(j, scale))
            .collect();
        let n = todo.len();
        let next = AtomicI64::new(0);
        let err = Mutex::new(None);
        std::thread::scope(|sc| {
            for _ in 0..threads.max(1) {
                sc.spawn(|| loop {
                    let i = next.fetch_add(1, Ordering::SeqCst) as usize;
                    if i >= todo.len() {
                        return;
                    }
                    // 1 件失敗しても残りは描く（失敗した字幕で後ろの先読みが止まらないように）。最初の失敗は最後に返す
                    if let Err(e) = self.subs.bitmap(&todo[i], scale) {
                        err.lock().get_or_insert(e);
                    }
                });
            }
        });
        if let Some(e) = err.into_inner() {
            return Err(e);
        }
        Ok(n)
    }
}

/// 演出のエラーの文。Python のトレースバックなら、原因の行（最後の行）を 1 行目に出す。
fn fx_error_text(e: &str) -> String {
    let last = e.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("").trim();
    if e.contains("Traceback (most recent call last)") && !last.is_empty() {
        format!("演出を描けませんでした: {last}\n{e}")
    } else {
        format!("演出を描けませんでした: {e}")
    }
}

/// 演出のコマを取る。fx_err があれば、失敗しても演出なし（None）にしてエラーの文を入れる（最初の 1 件）。
fn fx_frame(ovf: &dyn Fn(f64) -> Result<Option<Arc<CachedOverlay>>>, t: f64, fx_err: Option<&mut Option<String>>)
            -> Result<Option<Arc<CachedOverlay>>> {
    match (ovf(t), fx_err) {
        (Err(e), Some(slot)) => {
            slot.get_or_insert_with(|| fx_error_text(&format!("{e:#}")));
            Ok(None)
        }
        (r, _) => r,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fx_frame_skips_failed_overlay_only_for_monitor() {
        let fail = |_: f64| -> Result<Option<Arc<CachedOverlay>>> { Err(anyhow!("worker died")) };
        // モニター: 演出だけ抜いて、エラーの文を残す
        let mut err = None;
        assert!(fx_frame(&fail, 1.0, Some(&mut err)).unwrap().is_none());
        assert_eq!(err.as_deref(), Some("演出を描けませんでした: worker died"));
        // 2 件目で上書きしない
        let fail2 = |_: f64| -> Result<Option<Arc<CachedOverlay>>> { Err(anyhow!("second")) };
        assert!(fx_frame(&fail2, 1.0, Some(&mut err)).unwrap().is_none());
        assert_eq!(err.as_deref(), Some("演出を描けませんでした: worker died"));
        // 書き出し・確認画像: 失敗をそのまま返す
        match fx_frame(&fail, 1.0, None) {
            Err(e) => assert_eq!(format!("{e:#}"), "worker died"),
            Ok(_) => panic!("書き出しでは失敗を返すはず"),
        }
        // 成功したときはエラーを入れない
        let ok = |_: f64| -> Result<Option<Arc<CachedOverlay>>> { Ok(None) };
        let mut err = None;
        assert!(fx_frame(&ok, 1.0, Some(&mut err)).unwrap().is_none());
        assert!(err.is_none());
    }

    #[test]
    fn fx_error_text_puts_python_cause_first() {
        let tb = "Traceback (most recent call last):\n  File \"x.py\", line 1, in draw\nKeyError: 'a'\n";
        let s = fx_error_text(tb);
        assert_eq!(s.lines().next(), Some("演出を描けませんでした: KeyError: 'a'"));
        assert!(s.ends_with(tb));
        assert_eq!(fx_error_text("時間内に終わりませんでした"), "演出を描けませんでした: 時間内に終わりませんでした");
    }
}
