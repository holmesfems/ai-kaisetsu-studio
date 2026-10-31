//! 合成（`core/compose.rs`）の重ね順・どれを出すか・置き方・切り抜き・先読みの範囲の決め方（純粋）。
//! 描く（画素を置く）のは compose.rs の側。ここは数と並びを決めるだけで、状態もファイルも持たない。
//! 式・丸め・計算の順番は、描いた絵が変わらないように切り出す前のまま。

use crate::project::{MediaClip, Track};
use crate::render::media::Soft;
use crate::render::trans::{Dir, TransState};

/// 拡大した素材の一辺の上限（画素）。超えると描かずにエラーにする
pub const MAX_IMAGE_PX: u32 = 12000;
pub const MAX_VIDEO_PX: u32 = 8192;

/// 拡大した大きさが上限を超えているときの、描けない理由の文
pub fn too_large(wi: u32, hi: u32, max: u32) -> Option<String> {
    (wi > max || hi > max)
        .then(|| format!("拡大しすぎて描けません（{wi}×{hi} 画素。上限は {max}）。倍率を下げてください"))
}

/// 素材クリップの、時刻 t に映す素材の時刻（再生速度つき。ループの折り返しは `video_src_time`）
pub fn src_time(c: &MediaClip, t: f64) -> f64 {
    c.src_in + (t - c.start) * c.speed.clamp(0.1, 10.0)
}

/// 重ねる 1 層。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Layer {
    /// 攻略動画の戦闘画面と、RecWindow に差し込んだ録画（ネスト）。演出の下
    GuideAndNests,
    /// 素材のトラック（番号はトラックの並びの中の位置）
    Media(usize),
    /// 演出のコマ
    Fx,
}

/// 下から上への重ね順。トラックの順に並べ、演出のトラックの手前で攻略動画のレイヤーと録画の窓を入れる
/// （演出のトラックを隠していても入れる）。隠したトラックは出さない。video / image / fx 以外（audio など）も出さない。
pub fn layer_order(tracks: &[Track]) -> Vec<Layer> {
    let mut out = vec![];
    for (i, tr) in tracks.iter().enumerate() {
        if tr.kind == "fx" {
            out.push(Layer::GuideAndNests);
        }
        if tr.hidden {
            continue;
        }
        match tr.kind.as_str() {
            "video" | "image" => out.push(Layer::Media(i)),
            "fx" => out.push(Layer::Fx),
            _ => {}
        }
    }
    out
}

/// このクリップが t に出ているか（区間の中で、素材が決まっている）。終わりちょうどは出ない。
pub fn clip_live(c: &MediaClip, t: f64) -> bool {
    !(t < c.start || t >= c.start + c.dur || c.src.is_empty())
}

/// 話している人を明るくする倍率。話者の指定が無ければ 1.0、本人が話していれば 1.0、
/// ほかの人が話していれば 0.62、誰も話していなければ 0.85。
pub fn speaker_bright(speaker: &str, speaking: Option<&str>) -> f32 {
    if speaker.is_empty() {
        1.0
    } else {
        match speaking {
            Some(sp) if sp == speaker => 1.0,
            Some(_) => 0.62,
            None => 0.85,
        }
    }
}

/// 素材の大きさ (sw, sh) → フレームの画素での (左, 上, 幅, 高さ)。
/// 録画の窓の中の素材は、キャンバスの原点 `origin` と窓にはめる倍率 `k` がある。ふつうの素材は origin (0, 0)・k (1, 1)
/// （掛ける 1.0・足す 0.0 は値を変えないので、ふつうの素材の式と同じ値になる）。
pub fn place(c: &MediaClip, ts: &TransState, scale: f64, origin: (f64, f64), k: (f64, f64), sw: f64, sh: f64)
             -> (f64, f64, f64, f64) {
    let (ox, oy) = origin;
    let (sx, sy) = k;
    let (w, h) = (sw * c.scale * ts.scale * sx * scale, sh * c.scale * ts.scale * sy * scale);
    ((ox + (c.x + ts.dx) * sx) * scale - w / 2.0, (oy + (c.y + ts.dy) * sy) * scale - h / 2.0, w, h)
}

/// 切り抜きと、ワイプ・ぼかしで決まる、描く範囲。
pub struct CropRegion {
    /// 描く範囲 [x0, y0, x1, y1]（フレームの画素。呼び出し元の rect との重なり）
    pub rect: [i64; 4],
    /// クロップした辺のぼかし
    pub soft: Option<Soft>,
    /// ワイプの走査線（位置, 縦か, 線の長さの範囲）。ワイプが途中のときだけ
    pub edge: Option<(f64, bool, [i64; 2])>,
}

/// 置いた位置 (x, y, w, h) から、クロップ（crop_l/t/r/b、%）した後の範囲・ワイプで見える範囲・ぼかしを決める。
/// rect の外は切る。fscale はフレームの倍率（1920 に対する幅）。
#[allow(clippy::too_many_arguments)]
pub fn crop_region(c: &MediaClip, ts: &TransState, x: f64, y: f64, w: f64, h: f64, rect: [i64; 4], fscale: f64)
                   -> CropRegion {
    let mut edge: Option<(f64, bool, [i64; 2])> = None;
    let p = |v: f64| v.clamp(0.0, 100.0) / 100.0;
    let cr = [x + w * p(c.crop_l), y + h * p(c.crop_t), x + w * (1.0 - p(c.crop_r)), y + h * (1.0 - p(c.crop_b))];
    // ワイプ: 見えている範囲（vr）を走査線の側から広げる
    let mut vr = cr;
    if let Some((k, d)) = ts.wipe {
        let (cw, ch) = (cr[2] - cr[0], cr[3] - cr[1]);
        let (pos, vert) = match d {
            Dir::Left => {
                vr[2] = cr[0] + cw * k;
                (vr[2], true)
            }
            Dir::Right => {
                vr[0] = cr[2] - cw * k;
                (vr[0], true)
            }
            Dir::Up => {
                vr[3] = cr[1] + ch * k;
                (vr[3], false)
            }
            Dir::Down => {
                vr[1] = cr[3] - ch * k;
                (vr[1], false)
            }
        };
        if k > 0.0 && k < 1.0 {
            let span = if vert { [cr[1].round() as i64, cr[3].round() as i64] }
                       else { [cr[0].round() as i64, cr[2].round() as i64] };
            edge = Some((pos, vert, span));
        }
    }
    let r = [rect[0].max(vr[0].round() as i64), rect[1].max(vr[1].round() as i64),
             rect[2].min(vr[2].round() as i64), rect[3].min(vr[3].round() as i64)];
    let soft = (c.feather > 0.0).then(|| {
        let inf = f64::INFINITY;
        Soft {
            rect: [if c.crop_l > 0.0 { cr[0] } else { -inf }, if c.crop_t > 0.0 { cr[1] } else { -inf },
                   if c.crop_r > 0.0 { cr[2] } else { inf }, if c.crop_b > 0.0 { cr[3] } else { inf }],
            px: (c.feather * fscale).max(0.5),
        }
    });
    CropRegion { rect: r, soft, edge }
}

/// 素材の動画の、時刻 t に映す素材の秒。ループなら素材の長さ dur（0 より大きいとき）で折り返す
/// （最後のコマまで。`dur - 1/fps`）。再生速度は `src_time` に入っている。コマ番号にするのは `VideoSource::index_at`
pub fn video_src_time(c: &MediaClip, t: f64, dur: f64, fps: f64) -> f64 {
    let mut st = src_time(c, t);
    if c.looped && dur > 0.0 {
        st = st.rem_euclid(dur - 1.0 / fps);
    }
    st
}

/// 置いた位置 (x, y, w, h) の動画を、偶数の大きさに丸めて中心で合わせる。戻りは (幅, 高さ, 置く x, 置く y)。
pub fn even_fit(x: f64, y: f64, w: f64, h: f64) -> (u32, u32, i64, i64) {
    let wi = (w.round() as u32).max(2) & !1;
    let hi = (h.round() as u32).max(2) & !1;
    let xi = (x + (w - wi as f64) / 2.0).round() as i64;
    let yi = (y + (h - hi as f64) / 2.0).round() as i64;
    (wi, hi, xi, yi)
}

/// 先読みのコマ番号の範囲 [f0, f1)。t0 は 0 から、t1 は終わり（end）まで。
pub fn prefetch_frames(t0: f64, t1: f64, fps: f64, end: f64) -> (i64, i64) {
    ((t0.max(0.0) * fps).floor() as i64, ((t1.min(end)) * fps).ceil() as i64)
}

/// 合成の大きさ（scale 倍の 1920x1080。最小は 2）。
pub fn frame_size(scale: f64) -> (u32, u32) {
    ((1920.0 * scale).round().max(2.0) as u32, (1080.0 * scale).round().max(2.0) as u32)
}

/// 録画の窓の見える範囲 [x0, y0, x1, y1]（フレームの画素）。窓 bx とキャンバス（map = (sx, sy, ox, oy)、
/// 大きさ nest_w x nest_h）の重なり。空なら None。
pub fn nest_visible(bx: [f64; 4], map: (f64, f64, f64, f64), nest_w: f64, nest_h: f64, scale: f64) -> Option<[i64; 4]> {
    let (sx, sy, ox, oy) = map;
    let vis = [
        (bx[0].max(ox) * scale).round() as i64,
        (bx[1].max(oy) * scale).round() as i64,
        (bx[2].min(ox + nest_w * sx) * scale).round() as i64,
        (bx[3].min(oy + nest_h * sy) * scale).round() as i64,
    ];
    if vis[2] <= vis[0] || vis[3] <= vis[1] {
        None
    } else {
        Some(vis)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clip() -> MediaClip {
        MediaClip { src: "a.png".into(), start: 10.0, dur: 5.0, scale: 1.0, ..Default::default() }
    }

    fn ts0() -> TransState {
        TransState { alpha: 1.0, dx: 0.0, dy: 0.0, scale: 1.0, wipe: None, glitch: 0.0 }
    }

    fn track(kind: &str, hidden: bool) -> Track {
        Track { kind: kind.into(), hidden, ..Default::default() }
    }

    #[test]
    fn too_large_reports_only_over_the_limit() {
        assert!(too_large(12000, 100, MAX_IMAGE_PX).is_none());
        assert!(too_large(100, MAX_VIDEO_PX, MAX_VIDEO_PX).is_none());
        let m = too_large(96000, 54000, MAX_IMAGE_PX).unwrap();
        assert!(m.contains("拡大しすぎ"), "{m}");
        assert!(m.contains("96000") && m.contains("54000") && m.contains("12000"), "{m}");
        assert!(too_large(100, 8194, MAX_VIDEO_PX).is_some());
    }

    #[test]
    fn src_time_follows_speed() {
        let mut c = MediaClip { start: 10.0, src_in: 2.0, ..Default::default() };
        assert_eq!(src_time(&c, 13.0), 5.0);
        c.speed = 2.0;
        assert_eq!(src_time(&c, 13.0), 8.0);
        c.speed = 0.5;
        assert_eq!(src_time(&c, 14.0), 4.0);
    }

    #[test]
    fn layer_order_puts_guide_before_fx() {
        let tr = [track("video", false), track("fx", false), track("image", false)];
        assert_eq!(layer_order(&tr), vec![Layer::Media(0), Layer::GuideAndNests, Layer::Fx, Layer::Media(2)]);
    }

    #[test]
    fn layer_order_hidden_fx_still_draws_guide() {
        let tr = [track("video", false), track("fx", true), track("image", false)];
        assert_eq!(layer_order(&tr), vec![Layer::Media(0), Layer::GuideAndNests, Layer::Media(2)]);
    }

    #[test]
    fn layer_order_skips_hidden_and_audio() {
        let tr = [track("image", true), track("audio", false), track("video", false)];
        assert_eq!(layer_order(&tr), vec![Layer::Media(2)]);
    }

    #[test]
    fn layer_order_two_fx_tracks_repeat_guide() {
        let tr = [track("fx", false), track("fx", false)];
        assert_eq!(layer_order(&tr), vec![Layer::GuideAndNests, Layer::Fx, Layer::GuideAndNests, Layer::Fx]);
        assert!(layer_order(&[]).is_empty());
    }

    #[test]
    fn clip_live_edges() {
        let mut c = clip();
        assert!(!clip_live(&c, 9.999));
        assert!(clip_live(&c, 10.0));
        assert!(clip_live(&c, 14.999));
        assert!(!clip_live(&c, 15.0));
        c.src.clear();
        assert!(!clip_live(&c, 12.0));
    }

    #[test]
    fn speaker_bright_cases() {
        assert_eq!(speaker_bright("", Some("A")), 1.0);
        assert_eq!(speaker_bright("", None), 1.0);
        assert_eq!(speaker_bright("A", Some("A")), 1.0);
        assert_eq!(speaker_bright("A", Some("B")), 0.62);
        assert_eq!(speaker_bright("A", None), 0.85);
    }

    #[test]
    fn place_default_matches_plain_formula_bitwise() {
        let mut c = clip();
        c.x = 960.3;
        c.y = 540.7;
        c.scale = 0.37;
        let ts = TransState { alpha: 1.0, dx: 12.1, dy: -3.3, scale: 1.13, wipe: None, glitch: 0.0 };
        for scale in [1.0, 2.0 / 3.0, 0.5, 0.123] {
            let (sw, sh) = (1234.0, 567.0);
            // 切り出す前のふつうの素材の式
            let (w, h) = (sw * c.scale * ts.scale * scale, sh * c.scale * ts.scale * scale);
            let old = ((c.x + ts.dx) * scale - w / 2.0, (c.y + ts.dy) * scale - h / 2.0, w, h);
            assert_eq!(place(&c, &ts, scale, (0.0, 0.0), (1.0, 1.0), sw, sh), old);
        }
    }

    #[test]
    fn place_applies_origin_and_k() {
        let mut c = clip();
        c.x = 100.0;
        c.y = 50.0;
        let (x, y, w, h) = place(&c, &ts0(), 1.0, (200.0, 300.0), (0.5, 0.25), 80.0, 40.0);
        assert_eq!((w, h), (40.0, 10.0));
        assert_eq!(x, 200.0 + 100.0 * 0.5 - 20.0);
        assert_eq!(y, 300.0 + 50.0 * 0.25 - 5.0);
    }

    #[test]
    fn crop_region_no_crop_only_overlaps_rect() {
        let c = clip();
        let r = crop_region(&c, &ts0(), 10.0, 20.0, 100.0, 50.0, [0, 0, i64::MAX, i64::MAX], 1.0);
        assert_eq!(r.rect, [10, 20, 110, 70]);
        assert!(r.soft.is_none() && r.edge.is_none());
        // rect の外は切る
        let r = crop_region(&c, &ts0(), 10.0, 20.0, 100.0, 50.0, [30, 0, 60, 40], 1.0);
        assert_eq!(r.rect, [30, 20, 60, 40]);
    }

    #[test]
    fn crop_region_crop_left_half() {
        let mut c = clip();
        c.crop_l = 50.0;
        let r = crop_region(&c, &ts0(), 0.0, 0.0, 100.0, 50.0, [0, 0, i64::MAX, i64::MAX], 1.0);
        assert_eq!(r.rect, [50, 0, 100, 50]);
    }

    #[test]
    fn crop_region_wipe_directions() {
        let c = clip();
        let full = [0, 0, i64::MAX, i64::MAX];
        let wipe = |k: f64, d: Dir| {
            let ts = TransState { wipe: Some((k, d)), ..ts0() };
            crop_region(&c, &ts, 0.0, 0.0, 100.0, 50.0, full, 1.0)
        };
        let r = wipe(0.5, Dir::Left);
        assert_eq!(r.rect, [0, 0, 50, 50]);
        assert_eq!(r.edge, Some((50.0, true, [0, 50])));
        let r = wipe(0.25, Dir::Right);
        assert_eq!(r.rect, [75, 0, 100, 50]);
        assert_eq!(r.edge, Some((75.0, true, [0, 50])));
        let r = wipe(0.5, Dir::Up);
        assert_eq!(r.rect, [0, 0, 100, 25]);
        assert_eq!(r.edge, Some((25.0, false, [0, 100])));
        let r = wipe(0.5, Dir::Down);
        assert_eq!(r.rect, [0, 25, 100, 50]);
        assert_eq!(r.edge, Some((25.0, false, [0, 100])));
        // 始まりと出きったときは走査線なし
        assert!(wipe(0.0, Dir::Left).edge.is_none());
        assert!(wipe(1.0, Dir::Left).edge.is_none());
        assert_eq!(wipe(1.0, Dir::Left).rect, [0, 0, 100, 50]);
    }

    #[test]
    fn crop_region_feather_only_on_cropped_edges() {
        let mut c = clip();
        c.crop_l = 10.0;
        c.feather = 2.0;
        let full = [0, 0, i64::MAX, i64::MAX];
        let s = crop_region(&c, &ts0(), 0.0, 0.0, 100.0, 50.0, full, 1.0).soft.unwrap();
        assert_eq!(s.rect[0], 10.0);
        assert!(s.rect[1] == f64::NEG_INFINITY && s.rect[2] == f64::INFINITY && s.rect[3] == f64::INFINITY);
        assert_eq!(s.px, 2.0);
        c.feather = 0.1;
        assert_eq!(crop_region(&c, &ts0(), 0.0, 0.0, 100.0, 50.0, full, 1.0).soft.unwrap().px, 0.5);
        c.feather = 0.0;
        assert!(crop_region(&c, &ts0(), 0.0, 0.0, 100.0, 50.0, full, 1.0).soft.is_none());
    }

    #[test]
    fn video_src_time_loop_and_speed() {
        let mut c = clip();
        // ループなし
        assert!((video_src_time(&c, 12.0, 4.0, 30.0) - 2.0).abs() < 1e-9);
        // ループで折り返す（dur - 1/fps で）
        c.looped = true;
        c.speed = 1.0;
        let st = 5.0_f64.rem_euclid(4.0 - 1.0 / 30.0);
        assert!((video_src_time(&c, 15.0, 4.0, 30.0) - st).abs() < 1e-9);
        // dur 0 は折り返さない
        assert!((video_src_time(&c, 15.0, 0.0, 30.0) - 5.0).abs() < 1e-9);
        // 速度つき
        c.looped = false;
        c.speed = 2.0;
        assert!((video_src_time(&c, 12.0, 4.0, 30.0) - 4.0).abs() < 1e-9);
    }

    #[test]
    fn even_fit_rounds_to_even_and_keeps_center() {
        let (wi, hi, x, y) = even_fit(0.0, 0.0, 101.0, 51.0);
        assert_eq!((wi, hi), (100, 50));
        // 1 画素ぶん小さくなったので、中心が合うように半分ずらす（0.5 → 四捨五入で 1）
        assert_eq!((x, y), (1, 1));
        let (wi, hi, _, _) = even_fit(0.0, 0.0, 0.2, 0.2);
        assert_eq!((wi, hi), (2, 2));
        let (wi, hi, x, y) = even_fit(10.0, 20.0, 100.0, 50.0);
        assert_eq!((wi, hi, x, y), (100, 50, 10, 20));
    }

    #[test]
    fn prefetch_frames_clamps_and_rounds() {
        assert_eq!(prefetch_frames(-3.0, 2.0, 30.0, 100.0), (0, 60));
        assert_eq!(prefetch_frames(1.0, 500.0, 30.0, 10.0), (30, 300));
        // 端数: 頭は floor、終わりは ceil
        assert_eq!(prefetch_frames(1.04, 2.04, 30.0, 100.0), (31, 62));
    }

    #[test]
    fn frame_size_cases() {
        assert_eq!(frame_size(1.0), (1920, 1080));
        assert_eq!(frame_size(2.0 / 3.0), (1280, 720));
        assert_eq!(frame_size(0.0), (2, 2));
    }

    #[test]
    fn nest_visible_cases() {
        let map = (1.0, 1.0, 100.0, 50.0);
        // 窓の中にキャンバスが収まる
        assert_eq!(nest_visible([0.0, 0.0, 1000.0, 1000.0], map, 200.0, 100.0, 1.0), Some([100, 50, 300, 150]));
        // はみ出すと窓で切る
        assert_eq!(nest_visible([150.0, 80.0, 250.0, 120.0], map, 200.0, 100.0, 1.0), Some([150, 80, 250, 120]));
        // 重ならない
        assert_eq!(nest_visible([400.0, 0.0, 500.0, 100.0], map, 200.0, 100.0, 1.0), None);
        // scale が掛かる
        assert_eq!(nest_visible([0.0, 0.0, 1000.0, 1000.0], map, 200.0, 100.0, 0.5), Some([50, 25, 150, 75]));
    }
}
