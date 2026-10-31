//! 戦闘画面のレイヤーの座標と合成（素材を 70% で中央に置き、HUD の切り抜きを重ねる。手動パートは縮めて左上へ）。
//!
//! 座標の決め方は `geometry`（純粋な関数）に 1 か所。素材を 1 回だけ読み（`read`）、切り抜きもその絵から取る。

use super::plan::{ops_out, GuidePlan, PlanFrame};
use super::GuideAnalysis;
use crate::render::media::{blit_in, MediaPool, Rgb};
use ak_proto::guide::{REF_H, REF_W};
use ak_proto::{Guide, GuideBench, GuideFormation, GuideLayout, GuideOp, HudZoom};
use ak_proto::guide::OpOut;
use anyhow::{bail, Result};
use std::path::Path;

/// 1 コマの置き方（出力の画素）
#[derive(Clone, Debug, PartialEq)]
pub struct Geo {
    /// 素材を読む大きさ（偶数）
    pub read: (u32, u32),
    /// 読んだ絵の左上を置く位置
    pub base: (i64, i64),
    /// レイヤーの範囲 [x0, y0, x1, y1]。この外には描かない
    pub clip: [i64; 4],
    pub crops: Vec<CropOp>,
}

/// 読んだ絵から切り抜いて置くもの
#[derive(Clone, Debug, PartialEq)]
pub struct CropOp {
    pub name: String,
    /// 読んだ絵の中の範囲 [x0, y0, x1, y1]
    pub src: [i64; 4],
    /// 置く位置（左上）
    pub dst: (i64, i64),
    /// 置く大きさ（幅, 高さ）。切り抜きと違えば拡大・縮小して置く（待機欄だけ。ほかは切り抜きの大きさのまま）
    pub size: Option<(u32, u32)>,
    /// 不透明度（0〜1）
    pub opacity: f32,
}

/// 待機欄の切り抜きの名前（`GuideBench` で置き方が決まる）
pub const BENCH_NAME: &str = "bench";

/// part（"formation" / "manual" / "auto" / "result"）のレイヤーの置き方。
/// src_w, src_h は素材の大きさ、scale は出力の倍率（1920x1080 に対する）。
/// 手動パートはレイヤー全体を manual_scale 倍にして (manual_x, manual_y) に置く。ほかは全面。
/// 待機欄（名前 bench）の切り抜きだけは、`to` ではなく bench の大きさ・不透明度で、レイヤーの左下（左端・下端）に置く
/// （手動パートはレイヤーごと縮むので、同じ見え方の割合になる）
pub fn geometry(layout: &GuideLayout, bench: &GuideBench, part: &str, src_w: u32, src_h: u32, scale: f64) -> Geo {
    let (m, ox, oy) = if part == "manual" { (layout.manual_scale, layout.manual_x, layout.manual_y) } else { (1.0, 0.0, 0.0) };
    let even = |v: f64| ((v.round().max(2.0)) as u32) & !1;
    let rw = even(src_w as f64 * layout.scale * m * scale);
    let rh = even(src_h as f64 * layout.scale * m * scale);
    // レイヤーの中心に素材の中心を合わせる
    let cx = (ox + 960.0 * m) * scale;
    let cy = (oy + 540.0 * m) * scale;
    let base = ((cx - rw as f64 / 2.0).round() as i64, (cy - rh as f64 / 2.0).round() as i64);
    let clip = [
        (ox * scale).round() as i64,
        (oy * scale).round() as i64,
        ((ox + 1920.0 * m) * scale).round() as i64,
        ((oy + 1080.0 * m) * scale).round() as i64,
    ];
    let mut crops = vec![];
    for c in layout.crops.iter().filter(|c| c.parts.iter().any(|p| p == part)) {
        let px = |v: f64, refv: f64, n: u32| ((v / refv * n as f64).round() as i64).clamp(0, n as i64);
        let src = [px(c.src[0], REF_W, rw), px(c.src[1], REF_H, rh), px(c.src[2], REF_W, rw), px(c.src[3], REF_H, rh)];
        if src[2] <= src[0] || src[3] <= src[1] {
            continue;
        }
        if c.name == BENCH_NAME {
            let k = bench.scale * m * scale;
            let dw = (((c.src[2] - c.src[0]) * k).round() as i64).max(1) as u32;
            let dh = (((c.src[3] - c.src[1]) * k).round() as i64).max(1) as u32;
            let dst = (clip[0], clip[3] - dh as i64);
            crops.push(CropOp { name: c.name.clone(), src, dst, size: Some((dw, dh)), opacity: bench.opacity as f32 });
            continue;
        }
        let dst = (((ox + c.to[0] * m) * scale).round() as i64, ((oy + c.to[1] * m) * scale).round() as i64);
        crops.push(CropOp { name: c.name.clone(), src, dst, size: None, opacity: 1.0 });
    }
    Geo { read: (rw, rh), base, clip, crops }
}

/// 拡大の切り抜き 1 つの置き方（出力の画素）
#[derive(Clone, Debug, PartialEq)]
pub struct ZoomOp {
    pub name: String,
    /// 読んだ絵（geo.read の大きさ）の中の切り抜きの範囲 [x0, y0, x1, y1]
    pub src: [i64; 4],
    /// 出力での置き先 [x, y, 幅, 高さ]
    pub dst: [i64; 4],
    /// 名前が `cost` の拡大だけ: 普段の 1 行（`MINI_H`）と、そこに置く今のコマの切り抜き
    pub mini: Option<MiniOp>,
}

/// コストの 1 行の置き方（出力の画素）
#[derive(Clone, Debug, PartialEq)]
pub struct MiniOp {
    /// 1 行の箱 [x, y, 幅, 高さ]（拡大の箱の下端に揃う）
    pub row: [i64; 4],
    /// 1 行に置く切り抜きの置き先 [x, y, 幅, 高さ]
    pub img: [i64; 4],
    /// 読んだ絵の中の切り抜きの範囲 [x0, y0, x1, y1]（拡大の src の上 `MINI_FRAC`・左 `MINI_LEFT` を除く）
    pub src: [i64; 4],
}

/// コストの 1 行の高さ（出力 1080p の画素）
pub const MINI_H: f64 = 56.0;
/// 1 行に使う、拡大の切り抜きの上からの割合（数字とバー）
pub const MINI_FRAC: f64 = 0.56;
/// 1 行に使わない、拡大の切り抜きの左からの割合（数字の左の地図）
pub const MINI_LEFT: f64 = 0.5;
/// 1 行の左の「COST」の見出しに空ける幅
pub const MINI_LABEL_W: f64 = 96.0;
/// 1 行の切り抜きの上下・右の余白
pub const MINI_PAD: f64 = 4.0;
/// 拡大が広がる・畳まれる長さ（秒）
pub const COST_RAMP: f64 = 0.3;
/// 精密な操作の瞬間のあとに拡大を残す長さ（秒）
pub const COST_TAIL: f64 = 0.5;

/// 拡大の切り抜きの出力での箱 [x, y, 幅, 高さ]（倍率 1。1920x1080 での値）。演出が枠を描く所
pub fn zoom_box(z: &HudZoom) -> [f64; 4] {
    [z.to[0], z.to[1], (z.src[2] - z.src[0]).max(0.0) * z.scale, (z.src[3] - z.src[1]).max(0.0) * z.scale]
}

/// コストの 1 行の箱と、そこに置く切り抜きの箱（どちらも倍率 1。1920x1080 での値）。
/// 1 行は拡大の箱の下端に揃え（箱が `MINI_H` より低ければ箱の高さ）、切り抜きは縦横比を保って 1 行の右に寄せる
pub fn mini_boxes(z: &HudZoom) -> ([f64; 4], [f64; 4]) {
    let b = zoom_box(z);
    let mh = MINI_H.min(b[3]);
    let row = [b[0], b[1] + b[3] - mh, b[2], mh];
    let cw = ((z.src[2] - z.src[0]) * (1.0 - MINI_LEFT)).max(1e-6);
    let ch = ((z.src[3] - z.src[1]) * MINI_FRAC).max(1e-6);
    let aspect = cw / ch;
    let mut dh = (mh - 2.0 * MINI_PAD).max(1.0);
    let mut dw = dh * aspect;
    let max_w = (row[2] - MINI_LABEL_W).max(1.0);
    if dw > max_w {
        dw = max_w;
        dh = dw / aspect;
    }
    let img = [row[0] + row[2] - MINI_PAD * 2.0 - dw, row[1] + (mh - dh) / 2.0, dw, dh];
    (row, img)
}

/// 拡大の切り抜きとして使えない理由（和文）。使えれば None。
/// 範囲が空・逆さ、倍率が 0 以下、数でない値、素材の範囲（2752x2064）の外は使えない
pub fn zoom_problem(z: &HudZoom) -> Option<String> {
    if !(z.src.iter().chain(&z.to).all(|v| v.is_finite()) && z.scale.is_finite()) {
        return Some("数でない値があります".into());
    }
    if !(z.src[2] > z.src[0] && z.src[3] > z.src[1]) {
        return Some(format!("切り抜きの範囲（{:?}）が空です。終わりを始めより大きくしてください", z.src));
    }
    if !(z.scale > 0.0) {
        return Some(format!("倍率（{}）は 0 より大きくしてください", z.scale));
    }
    if z.src[0] < 0.0 || z.src[1] < 0.0 || z.src[2] > REF_W || z.src[3] > REF_H {
        return Some(format!("切り抜きの範囲（{:?}）が素材の範囲（{}x{}）の外です", z.src, REF_W, REF_H));
    }
    None
}

/// layout.zooms のうち使えるものと、使えないものの理由（警告。和文）。描く側（`zoom_ops`）と対応表（`map_json`）が同じものを使う
pub fn valid_zooms(layout: &GuideLayout) -> (Vec<&HudZoom>, Vec<String>) {
    let (mut ok, mut warns) = (vec![], vec![]);
    for (i, z) in layout.zooms.iter().enumerate() {
        match zoom_problem(z) {
            None => ok.push(z),
            Some(why) => warns.push(format!("拡大の切り抜き {}（{}）: {why}。この拡大は出しません", i + 1, z.name)),
        }
    }
    (ok, warns)
}

/// part に出す拡大の切り抜き。geo は同じパートの `geometry`（読んだ絵の大きさを使う）、scale は出力の倍率。
/// 拡大はレイヤーの外（手動の縮めの後）に置くので、置き先はレイヤーの位置に左右されない
pub fn zoom_ops(layout: &GuideLayout, part: &str, geo: &Geo, scale: f64) -> Vec<ZoomOp> {
    let mut v = vec![];
    for z in valid_zooms(layout).0.into_iter().filter(|z| z.parts.iter().any(|p| p == part)) {
        let px = |x: f64, refv: f64, n: u32| ((x / refv * n as f64).round() as i64).clamp(0, n as i64);
        let src = [px(z.src[0], REF_W, geo.read.0), px(z.src[1], REF_H, geo.read.1),
                   px(z.src[2], REF_W, geo.read.0), px(z.src[3], REF_H, geo.read.1)];
        let b = zoom_box(z);
        if src[2] <= src[0] || src[3] <= src[1] || !(b[2] > 0.0 && b[3] > 0.0) || !b.iter().all(|x| x.is_finite()) {
            continue;
        }
        let dst = [(b[0] * scale).round() as i64, (b[1] * scale).round() as i64,
                   ((b[2] * scale).round() as i64).max(1), ((b[3] * scale).round() as i64).max(1)];
        // 1 行を持つのは手動パートの cost だけ（演出の COST の帯・列の下端も手動パートだけ）
        let mini = (z.name == "cost" && part == "manual").then(|| {
            let (row, img) = mini_boxes(z);
            let r = |b: [f64; 4]| [(b[0] * scale).round() as i64, (b[1] * scale).round() as i64,
                                  ((b[2] * scale).round() as i64).max(1), ((b[3] * scale).round() as i64).max(1)];
            let msrc_h = (((src[3] - src[1]) as f64 * MINI_FRAC).round() as i64).max(1);
            let msrc_x = src[0] + ((src[2] - src[0]) as f64 * MINI_LEFT).round() as i64;
            MiniOp { row: r(row), img: r(img), src: [msrc_x.min(src[2] - 1), src[1], src[2], src[1] + msrc_h] }
        });
        v.push(ZoomOp { name: z.name.clone(), src, dst, mini });
    }
    v
}

/// 読んだ絵 im の一部を切り抜いて拡大し、出力の dst に重ねる（レイヤーの外にも描く）
pub fn draw_zoom(fr: &mut Rgb, im: &Rgb, z: &ZoomOp) {
    draw_crop_into(fr, im, z.src, z.dst, 1.0);
}

/// 読んだ絵 im の src を切り抜いて dst（[x, y, 幅, 高さ]）の大きさにして、不透明度 opacity で重ねる
fn draw_crop_into(fr: &mut Rgb, im: &Rgb, src: [i64; 4], dst: [i64; 4], opacity: f32) {
    let c = crop_rgb(im, src);
    if c.w == 0 || c.h == 0 || dst[2] <= 0 || dst[3] <= 0 {
        return;
    }
    let (w, h) = (dst[2] as u32, dst[3] as u32);
    let big = match image::RgbImage::from_raw(c.w, c.h, c.data) {
        Some(img) => image::imageops::resize(&img, w, h, image::imageops::FilterType::Triangle),
        None => return,
    };
    let big = Rgb { w, h, data: big.into_raw() };
    blit_in(fr, &big, dst[0], dst[1], opacity, [0, 0, fr.w as i64, fr.h as i64]);
}

/// 度合い k（0〜1）で下から広げたコストの拡大: 見える高さ `hk = 行の高さ + (箱の高さ − 行の高さ) × k` だけ、箱の下から。
/// 中身は縮めず、見えるぶんの src と dst を下から切る。mini が無ければ（コストの拡大でない）そのまま
pub fn reveal_zoom(z: &ZoomOp, k: f64) -> ZoomOp {
    let Some(m) = &z.mini else { return z.clone() };
    let (h, mh) = (z.dst[3] as f64, m.row[3] as f64);
    let hk = ((mh + (h - mh).max(0.0) * k.clamp(0.0, 1.0)).round() as i64).clamp(1, z.dst[3]);
    let sh = z.src[3] - z.src[1];
    let sy0 = z.src[3] - (((sh as f64) * hk as f64 / h).round() as i64).clamp(1, sh.max(1));
    ZoomOp { name: z.name.clone(), src: [z.src[0], sy0, z.src[2], z.src[3]],
             dst: [z.dst[0], z.dst[1] + z.dst[3] - hk, z.dst[2], hk], mini: z.mini.clone() }
}

/// 拡大 1 つを描く。コストの拡大（mini がある）以外は今のまま。コストの拡大は度合い k で、
/// k > 0 なら止めた絵（held）を下から広げ、k < 1 なら 1 行に今の絵（im）の数字とバーを不透明度 1 − k で重ねる
fn draw_zoom_k(fr: &mut Rgb, im: &Rgb, held: Option<&Rgb>, z: &ZoomOp, k: f64) {
    let Some(m) = &z.mini else {
        return draw_zoom(fr, im, z);
    };
    if k > 0.0 {
        draw_zoom(fr, held.unwrap_or(im), &reveal_zoom(z, k));
    }
    if k < 1.0 {
        draw_crop_into(fr, im, m.src, m.img, (1.0 - k) as f32);
    }
}

/// 補足素材の小窓の置き先（出力の画素 [x, y, 幅, 高さ]。幅・高さは偶数）。sub_box は 1920x1080 での値、
/// (w, h) は補足素材の絵の大きさ。箱に縦横比を保って収めて中央に置く
pub fn sub_target(sub_box: [f64; 4], w: u32, h: u32, scale: f64) -> [i64; 4] {
    let b = super::map::fit_box(sub_box, w, h);
    let even = |v: f64| ((v.round().max(2.0)) as i64) & !1;
    [(b[0] * scale).round() as i64, (b[1] * scale).round() as i64, even(b[2] * scale), even(b[3] * scale)]
}

/// 暗転の何秒前から、使わない枠の暗さを戻すか
pub const DIM_RELEASE: f64 = 0.3;
/// 使わない枠の明るさ（1 = そのまま）
pub const DIM_LEVEL: f32 = 0.35;

/// 編成パートで暗くする枠と暗さ（0〜1）。ts はこのコマの素材の時刻、dark は編成の暗転のアンカーの素材の時刻。
/// used が空（全部使う）・暗さが 0 なら None。暗転の手前 `DIM_RELEASE` 秒かけて 1 → 0 に戻す（暗転の後のロード画面に枠が残らないように）
pub fn formation_dim(f: &GuideFormation, ts: f64, dark: f64) -> Option<(Vec<[f64; 4]>, f32)> {
    if f.used.is_empty() {
        return None;
    }
    let strength = ((dark - ts) / DIM_RELEASE).clamp(0.0, 1.0) as f32;
    if strength <= 0.0 {
        return None;
    }
    let slots: Vec<[f64; 4]> =
        f.effective_slots().into_iter().enumerate().filter(|(i, _)| !f.is_used(*i)).map(|(_, r)| r).collect();
    (!slots.is_empty()).then_some((slots, strength))
}

/// 素材の画素（REF_W x REF_H での矩形）→ 出力の画素。素材は geo.read の大きさで geo.base に置いてある
pub fn slot_rect(geo: &Geo, r: [f64; 4]) -> [i64; 4] {
    let x = |v: f64| geo.base.0 + (v / REF_W * geo.read.0 as f64).round() as i64;
    let y = |v: f64| geo.base.1 + (v / REF_H * geo.read.1 as f64).round() as i64;
    [x(r[0]), y(r[1]), x(r[2]), y(r[3])]
}

/// 矩形の中を暗くして彩度を落とす。strength 1 で明るさ DIM_LEVEL（彩度は半分）、0 でそのまま。rect は絵の中と clip に収める
pub fn dim_region(fr: &mut Rgb, rect: [i64; 4], clip: [i64; 4], strength: f32) {
    let s = strength.clamp(0.0, 1.0);
    if s <= 0.0 {
        return;
    }
    let x0 = rect[0].max(clip[0]).clamp(0, fr.w as i64) as usize;
    let x1 = rect[2].min(clip[2]).clamp(0, fr.w as i64) as usize;
    let y0 = rect[1].max(clip[1]).clamp(0, fr.h as i64) as usize;
    let y1 = rect[3].min(clip[3]).clamp(0, fr.h as i64) as usize;
    if x1 <= x0 || y1 <= y0 {
        return;
    }
    let bright = 1.0 - (1.0 - DIM_LEVEL) * s;
    let desat = 0.5 * s;
    for y in y0..y1 {
        for x in x0..x1 {
            let i = (y * fr.w as usize + x) * 3;
            let (r, g, b) = (fr.data[i] as f32, fr.data[i + 1] as f32, fr.data[i + 2] as f32);
            let l = 0.299 * r + 0.587 * g + 0.114 * b;
            for (k, c) in [r, g, b].into_iter().enumerate() {
                fr.data[i + k] = ((c + (l - c) * desat) * bright).round().clamp(0.0, 255.0) as u8;
            }
        }
    }
}

/// 絵の一部を切り抜く（範囲は絵の中に収める）
pub fn crop_rgb(im: &Rgb, r: [i64; 4]) -> Rgb {
    let x0 = r[0].clamp(0, im.w as i64) as usize;
    let x1 = r[2].clamp(x0 as i64, im.w as i64) as usize;
    let y0 = r[1].clamp(0, im.h as i64) as usize;
    let y1 = r[3].clamp(y0 as i64, im.h as i64) as usize;
    let mut data = Vec::with_capacity((x1 - x0) * (y1 - y0) * 3);
    for y in y0..y1 {
        let row = (y * im.w as usize + x0) * 3;
        data.extend_from_slice(&im.data[row..row + (x1 - x0) * 3]);
    }
    Rgb { w: (x1 - x0) as u32, h: (y1 - y0) as u32, data }
}

/// 絵を w x h に拡大・縮小する
fn resize_rgb(im: Rgb, w: u32, h: u32) -> Rgb {
    match image::RgbImage::from_raw(im.w, im.h, im.data) {
        Some(img) => Rgb { w, h, data: image::imageops::resize(&img, w, h, image::imageops::FilterType::Triangle).into_raw() },
        None => Rgb { w: 0, h: 0, data: vec![] },
    }
}

/// 戦闘画面のレイヤーを fr に描く（im は geo.read の大きさで読んだ素材のコマ）
pub fn draw_layer(fr: &mut Rgb, im: &Rgb, geo: &Geo) {
    blit_in(fr, im, geo.base.0, geo.base.1, 1.0, geo.clip);
    for c in &geo.crops {
        let sub = crop_rgb(im, c.src);
        let sub = match c.size {
            Some((w, h)) if (w, h) != (sub.w, sub.h) && sub.w > 0 && sub.h > 0 => resize_rgb(sub, w, h),
            _ => sub,
        };
        blit_in(fr, &sub, c.dst.0, c.dst.1, c.opacity, geo.clip);
    }
}

/// 3p²−2p³（p は 0〜1 に切る）
pub fn smooth(p: f64) -> f64 {
    let p = p.clamp(0.0, 1.0);
    p * p * (3.0 - 2.0 * p)
}

/// コストの拡大の度合い（0 = 1 行だけ、1 = 拡大いっぱい）。区間 [t0, t1] の頭から `COST_RAMP` 秒で広がり、t1 から `COST_RAMP` 秒で畳む。
/// `k(t) = smooth((t − t0) / 0.3) × (1 − smooth((t − t1) / 0.3))`。Python（`fxkit/guide_template.py` の `cost_k`）も同じ式
pub fn cost_k(t: f64, t0: f64, t1: f64) -> f64 {
    smooth((t - t0) / COST_RAMP) * (1.0 - smooth((t - t1) / COST_RAMP))
}

/// コストの拡大を出す区間（精密な操作 1 つ分）
#[derive(Clone, Debug, PartialEq)]
pub struct Hold {
    pub op: String,
    /// 広がり始める出力の時刻（その操作の t_now）
    pub t0: f64,
    /// 畳み始める出力の時刻（t_out + `COST_TAIL`）
    pub t1: f64,
    /// 止めて出す素材のコマ番号（操作の 1 コマ前。0 未満は 0）
    pub frame: u32,
    /// t0 のコマのコスト
    pub cost_from: Option<i32>,
    /// 目標のコスト（`frame` のコマのコスト。読めなければ操作のコマのコスト）
    pub cost_to: Option<i32>,
}

/// 精密で信頼枠でない操作（`t_now < t_out`。先出し 0 は出さない）ごとの区間。ops と outs は同じ並び（`ops_out`）、
/// cost は解析のコマごとのコスト。t0 のコマは plan で引く
pub fn holds(ops: &[GuideOp], outs: &[OpOut], plan: &GuidePlan, cost: &[Option<i32>]) -> Vec<Hold> {
    ops.iter()
        .zip(outs)
        .filter(|(o, _)| o.precise && o.kind != "trust")
        .filter_map(|(o, u)| {
            let (tn, to) = (u.t_now?, u.t_out?);
            if !(tn < to) {
                return None;
            }
            let frame = u.frame.saturating_sub(1);
            let at = |i: u32| cost.get(i as usize).copied().flatten();
            Some(Hold {
                op: o.id.clone(),
                t0: tn,
                t1: to + COST_TAIL,
                frame,
                cost_from: plan.frame_at(tn).and_then(|f| at(f.src)),
                cost_to: at(frame).or(u.cost),
            })
        })
        .collect()
}

/// 時刻 t のコストの拡大の状態
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CostHold {
    /// 度合い（0 = 1 行だけ）
    pub k: f64,
    /// 止めて出す素材のコマ番号（k > 0 のとき）
    pub frame: Option<u32>,
}

/// 時刻 t のコストの拡大の度合いと止めるコマ。区間が重なれば度合いは大きいほう、
/// 止めるコマは度合いが 0 より大きい区間のうち t0 が遅いもの。どれも無ければ k = 0・frame なし
pub fn hold_state(holds: &[Hold], t: f64) -> CostHold {
    let mut k = 0.0f64;
    let mut pick: Option<&Hold> = None;
    for h in holds {
        let v = cost_k(t, h.t0, h.t1);
        if v > 0.0 {
            k = k.max(v);
            if pick.map_or(true, |p| h.t0 > p.t0) {
                pick = Some(h);
            }
        }
    }
    CostHold { k, frame: pick.map(|h| h.frame) }
}

/// 時刻 t の 1 コマを描くための入力（コマ・パート・編成の暗くする枠・コストの拡大の止め方）
pub struct FrameInputs<'a> {
    pub pf: &'a PlanFrame,
    pub part: &'static str,
    pub dim: Option<(Vec<[f64; 4]>, f32)>,
    pub hold: CostHold,
}

/// 時刻 t の 1 コマの描き方。t が組み立ての外なら None。
/// 編成パートは使わない枠を暗くする（暗転の手前で戻す）。コストの拡大は精密な操作のときだけ、操作の 1 コマ前の止めた絵で 1 行から広げる
/// （どのコストで押すかを先に見せる）。t・guide・plan・解析だけで決まるので、コマのキャッシュの鍵に足すものは無い
pub fn frame_inputs<'a>(guide: &Guide, plan: &'a GuidePlan, analysis: &GuideAnalysis, t: f64) -> Option<FrameInputs<'a>> {
    let pf = plan.frame_at(t)?;
    let part = plan.part_name(pf);
    let dim = match (part, plan.anchor_src_of("formation_dark"), analysis.pts.get(pf.src as usize)) {
        ("formation", Some(dark), Some(&ts)) => formation_dim(&guide.formation, ts, dark),
        _ => None,
    };
    let outs = ops_out(guide, plan, &analysis.cost, &analysis.game);
    let hold = hold_state(&holds(&guide.ops, &outs, plan, &analysis.cost), t);
    Some(FrameInputs { pf, part, dim, hold })
}

/// 編成の枠の絵を読む大きさ（偶数）。素材の大きさ w x h に倍率（0.05〜1 に収める）を掛けて、偶数に丸める（2 未満は 2）
pub fn even_scaled(w: u32, h: u32, scale: f64) -> (u32, u32) {
    let even = |x: f64| (x.round().max(2.0) as u32) & !1;
    let s = scale.clamp(0.05, 1.0);
    (even(w as f64 * s), even(h as f64 * s))
}

/// 編成の枠（REF_W x REF_H の素材の画素の矩形 [x0, y0, x1, y1]）→ w x h の絵の画素の矩形。絵の中（0〜w・0〜h）に収める
pub fn slot_crop_px(rect: [f64; 4], w: u32, h: u32) -> [i64; 4] {
    let px = |x: f64, refv: f64, n: u32| ((x / refv * n as f64).round() as i64).clamp(0, n as i64);
    [px(rect[0], REF_W, w), px(rect[1], REF_H, h), px(rect[2], REF_W, w), px(rect[3], REF_H, h)]
}

/// 組み立ての 1 コマ（pf）を fr に描く。素材は analysis の pts でコマ番号を引いて読む（`MediaPool::video_pts`）。
/// カットの継ぎ目（pf.mix）は、同じ下地に A と B のコマをそれぞれ描いて重みで混ぜる。
/// cost_hold: 名前が cost の拡大だけ、度合い k で 1 行 → 止めたコマ（frame）の拡大に切り替わる（継ぎ目でも止めた 1 枚）。
/// ほかの拡大と戦闘画面のレイヤーは今のコマのまま
/// 素材が解析したものと大きさが違えば Err（別の動画のコマを引かないように）
#[allow(clippy::too_many_arguments)]
pub fn render_plan_frame(fr: &mut Rgb, pool: &MediaPool, path: &Path, analysis: &GuideAnalysis, layout: &GuideLayout,
                         bench: &GuideBench, part: &str, pf: &PlanFrame, dim: Option<(Vec<[f64; 4]>, f32)>, cost_hold: CostHold,
                         scale: f64, video_cache: bool) -> Result<()> {
    let v = pool.video_pts(&path.to_string_lossy(), &analysis.pts)?;
    if !v.info.has_video {
        bail!("映像がありません: {}", path.display());
    }
    if analysis.size[0] != 0 && (analysis.size[0] != v.info.w || analysis.size[1] != v.info.h) {
        bail!(
            "解析した素材の大きさ（{}x{}）と今の素材（{}x{}）が違います。もう一度解析してください",
            analysis.size[0], analysis.size[1], v.info.w, v.info.h
        );
    }
    let geo = geometry(layout, bench, part, v.info.w, v.info.h, scale);
    let zooms = zoom_ops(layout, part, &geo, scale);
    // 止めるコマは 1 回だけ読む（同じ大きさ・同じ読み方）
    let held = match cost_hold.frame {
        Some(idx) if cost_hold.k > 0.0 && zooms.iter().any(|z| z.mini.is_some()) => Some(v.frame_opt(idx as i64, geo.read.0, geo.read.1, video_cache)?),
        _ => None,
    };
    let layer = |canvas: &mut Rgb, idx: u32| -> Result<()> {
        let im = v.frame_opt(idx as i64, geo.read.0, geo.read.1, video_cache)?;
        draw_layer(canvas, &im, &geo);
        // 拡大の切り抜きは、同じ読んだ絵から取る（素材を 2 回読まない）。コストの拡大は止めたコマから広げ、1 行は今の絵から。
        // クロスフェードは A・B それぞれで作って混ぜる
        for z in &zooms {
            draw_zoom_k(canvas, &im, held.as_ref(), z, cost_hold.k);
        }
        Ok(())
    };
    match pf.mix {
        None => layer(fr, pf.src)?,
        Some((b, w)) => {
            let mut fb = fr.clone();
            layer(fr, pf.src)?;
            layer(&mut fb, b)?;
            mix_into(fr, &fb, w);
        }
    }
    // 編成パート: 使わない枠を暗くする
    if let Some((slots, strength)) = dim {
        for r in slots {
            dim_region(fr, slot_rect(&geo, r), geo.clip, strength);
        }
    }
    Ok(())
}

/// a を b に重み w（0〜1）で近づける（カットの継ぎ目のクロスフェード）
pub fn mix_into(a: &mut Rgb, b: &Rgb, w: f32) {
    if a.w != b.w || a.h != b.h {
        return;
    }
    let k = (w.clamp(0.0, 1.0) * 256.0) as u32;
    for (x, y) in a.data.iter_mut().zip(&b.data) {
        *x = ((*x as u32 * (256 - k) + *y as u32 * k) >> 8) as u8;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layout() -> GuideLayout {
        GuideLayout::default()
    }

    #[test]
    fn auto_layer_is_70_percent_centered_with_hud_crops() {
        // 実際の 2752x2064 を 1920x1080 の出力（倍率 1）に
        let g = geometry(&layout(), &Default::default(), "auto", 2752, 2064, 1.0);
        assert_eq!(g.read, (1926, 1444));
        // 中央: x = (1920 − 1926)/2 = −3, y = (1080 − 1444)/2 = −182
        assert_eq!(g.base, (-3, -182));
        assert_eq!(g.clip, [0, 0, 1920, 1080]);
        let by = |n: &str| g.crops.iter().find(|c| c.name == n).unwrap();
        // 敵数・耐久: 素材 (980, 20)〜(1760, 110) → 読んだ絵では 0.7 倍
        assert_eq!(by("top").src, [686, 14, 1232, 77]);
        assert_eq!(by("top").dst, (683, 30));
        // 待機欄は横全体を切り抜いて、左下に 0.5 倍（1376x137）・不透明度 0.5 で置く
        let b = by("bench");
        assert_eq!(b.src[2], g.read.0 as i64, "右端は読んだ絵の右端（12 枠すべて）");
        assert_eq!(b.size, Some((1376, 137)));
        assert_eq!(b.dst, (0, 1080 - 137));
        assert_eq!(b.opacity, 0.5);
        // ほかの切り抜きは等倍・不透明
        assert_eq!((by("top").size, by("top").opacity), (None, 1.0));
        assert_eq!(by("speed").dst, (1593, 30));
    }

    #[test]
    fn formation_and_result_have_no_hud_crops() {
        for part in ["formation", "result"] {
            let g = geometry(&layout(), &Default::default(), part, 2752, 2064, 1.0);
            assert!(g.crops.is_empty(), "{part}");
            assert_eq!(g.clip, [0, 0, 1920, 1080]);
        }
    }

    #[test]
    fn manual_layer_is_75_percent_at_top_left() {
        let g = geometry(&layout(), &Default::default(), "manual", 2752, 2064, 1.0);
        // レイヤー全体が 1440x810 で左上
        assert_eq!(g.clip, [0, 0, 1440, 810]);
        // 素材は 70% × 75% = 52.5%: 1444 × 0.75 ≒ 1445 → 偶数に丸めて
        assert_eq!(g.read, (1444, 1084));
        // レイヤーの中心 (720, 405) に素材の中心
        assert_eq!(g.base, (720 - 722, 405 - 542));
        // HUD の置き先も 0.75 倍: top は (683, 30) → (512, 22.5)
        let top = g.crops.iter().find(|c| c.name == "top").unwrap();
        assert_eq!(top.dst, (512, 23));
    }

    #[test]
    fn bench_follows_settings_at_bottom_left_and_scales_with_manual_layer() {
        let bench = GuideBench { scale: 0.4, opacity: 0.8 };
        let by = |g: &Geo| g.crops.iter().find(|c| c.name == "bench").unwrap().clone();
        // 自動: 2752x274 の 0.4 倍 = 1101x110、左下
        let a = by(&geometry(&layout(), &bench, "auto", 2752, 2064, 1.0));
        assert_eq!((a.size, a.dst, a.opacity), (Some((1101, 110)), (0, 970), 0.8));
        // 手動はレイヤーごと 75%: 826x82、レイヤー（0,0,1440,810）の左下
        let m = by(&geometry(&layout(), &bench, "manual", 2752, 2064, 1.0));
        assert_eq!((m.size, m.dst), (Some((826, 82)), (0, 810 - 82)));
        // レイヤーを動かすと左下も動く
        let mut l = layout();
        l.manual_x = 100.0;
        l.manual_y = 50.0;
        let m2 = by(&geometry(&l, &bench, "manual", 2752, 2064, 1.0));
        assert_eq!(m2.dst, (100, 860 - 82));
        // 出力の倍率で縮む
        let h = by(&geometry(&layout(), &bench, "auto", 2752, 2064, 0.5));
        assert_eq!((h.size, h.dst), (Some((550, 55)), (0, 540 - 55)));
        // bench の切り抜きの to は使わない
        let mut l = layout();
        l.crops.iter_mut().find(|c| c.name == "bench").unwrap().to = [500.0, 10.0];
        assert_eq!(by(&geometry(&l, &bench, "auto", 2752, 2064, 1.0)).dst, (0, 970));
    }

    #[test]
    fn draw_layer_pastes_crop_with_opacity_and_size() {
        // 4x2 の絵（全部 200）の左半分 2x2 を、4x4 に拡大して不透明度 0.5 で下地 0 の上に
        let im = Rgb { w: 4, h: 2, data: vec![200; 4 * 2 * 3] };
        let geo = Geo {
            read: (4, 2),
            base: (100, 100),
            clip: [0, 0, 10, 10],
            crops: vec![CropOp { name: "bench".into(), src: [0, 0, 2, 2], dst: (1, 5), size: Some((4, 4)), opacity: 0.5 }],
        };
        let mut fr = Rgb { w: 10, h: 10, data: vec![0; 10 * 10 * 3] };
        draw_layer(&mut fr, &im, &geo);
        let p = px(&fr, 2, 6);
        assert!((p[0] as i32 - 100).abs() <= 2, "{p:?}");
        assert_eq!(px(&fr, 0, 6), [0, 0, 0]);
        assert_eq!(px(&fr, 5, 6), [0, 0, 0]);
        assert_eq!(px(&fr, 2, 4), [0, 0, 0]);
        assert_ne!(px(&fr, 4, 8), [0, 0, 0]);
    }

    #[test]
    fn manual_offset_moves_the_layer() {
        let mut l = layout();
        l.manual_x = 100.0;
        l.manual_y = 50.0;
        let g = geometry(&l, &Default::default(), "manual", 2752, 2064, 1.0);
        assert_eq!(g.clip, [100, 50, 1540, 860]);
        let top = g.crops.iter().find(|c| c.name == "top").unwrap();
        assert_eq!(top.dst, (100 + 512, 50 + 23));
    }

    #[test]
    fn output_scale_scales_everything() {
        let a = geometry(&layout(), &Default::default(), "manual", 2752, 2064, 1.0);
        let b = geometry(&layout(), &Default::default(), "manual", 2752, 2064, 0.5);
        assert_eq!(b.clip, [0, 0, 720, 405]);
        assert_eq!(b.read, (722, 542));
        assert_eq!(a.crops.len(), b.crops.len());
    }

    #[test]
    fn crops_follow_source_size_ratio() {
        // 素材が 2752x2064 の半分の大きさでも、切り抜きは比で同じ場所
        let g = geometry(&layout(), &Default::default(), "auto", 1376, 1032, 1.0);
        let full = geometry(&layout(), &Default::default(), "auto", 2752, 2064, 1.0);
        let top = |g: &Geo| g.crops.iter().find(|c| c.name == "top").unwrap().dst;
        assert_eq!(top(&g), top(&full));
        assert_eq!(g.read, (962, 722));
    }

    #[test]
    fn crop_with_empty_rect_or_other_part_is_skipped() {
        let mut l = layout();
        l.crops[0].src = [100.0, 100.0, 100.0, 200.0];
        l.crops[1].parts = vec!["manual".into()];
        let g = geometry(&l, &Default::default(), "auto", 2752, 2064, 1.0);
        let names: Vec<_> = g.crops.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["bench"]);
    }

    /// 縦に色が違う小さな絵で、置き場所と切り抜きを確かめる
    fn gradient(w: u32, h: u32) -> Rgb {
        let mut data = vec![];
        for y in 0..h {
            for x in 0..w {
                data.extend_from_slice(&[(x * 10) as u8, (y * 10) as u8, 7]);
            }
        }
        Rgb { w, h, data }
    }

    fn px(im: &Rgb, x: u32, y: u32) -> [u8; 3] {
        let i = ((y * im.w + x) * 3) as usize;
        [im.data[i], im.data[i + 1], im.data[i + 2]]
    }

    #[test]
    fn draw_layer_centers_base_and_pastes_crops() {
        // 出力 20x12、素材 12x8 を等倍で中央に（scale 1.0）。切り抜きは素材の (2,1)〜(5,3) を出力の (0,9) へ
        let geo = Geo {
            read: (12, 8),
            base: (4, 2),
            clip: [0, 0, 20, 12],
            crops: vec![CropOp { name: "t".into(), src: [2, 1, 5, 3], dst: (0, 9), size: None, opacity: 1.0 }],
        };
        let mut fr = Rgb { w: 20, h: 12, data: vec![0; 20 * 12 * 3] };
        let im = gradient(12, 8);
        draw_layer(&mut fr, &im, &geo);
        // 素材の左上 (0,0) → 出力 (4,2)
        assert_eq!(px(&fr, 4, 2), [0, 0, 7]);
        assert_eq!(px(&fr, 15, 9), [110, 70, 7]);
        // 素材の外は触らない
        assert_eq!(px(&fr, 3, 2), [0, 0, 0]);
        // 切り抜き: 素材 (2,1) → 出力 (0,9)
        assert_eq!(px(&fr, 0, 9), [20, 10, 7]);
        assert_eq!(px(&fr, 2, 10), [40, 20, 7]);
        assert_eq!(px(&fr, 3, 9), [0, 0, 0]);
    }

    #[test]
    fn draw_layer_clips_to_layer_rect() {
        let geo = Geo { read: (12, 8), base: (-2, -2), clip: [0, 0, 8, 6], crops: vec![] };
        let mut fr = Rgb { w: 20, h: 12, data: vec![0; 20 * 12 * 3] };
        draw_layer(&mut fr, &gradient(12, 8), &geo);
        assert_eq!(px(&fr, 0, 0), [20, 20, 7]); // 素材の (2,2)
        assert_eq!(px(&fr, 7, 5), [90, 70, 7]);
        assert_eq!(px(&fr, 8, 5), [0, 0, 0]); // レイヤーの外
        assert_eq!(px(&fr, 7, 6), [0, 0, 0]);
    }

    #[test]
    fn crop_rgb_clamps() {
        let im = gradient(6, 4);
        let c = crop_rgb(&im, [-3, 1, 100, 3]);
        assert_eq!((c.w, c.h), (6, 2));
        assert_eq!(px(&c, 0, 0), [0, 10, 7]);
        let e = crop_rgb(&im, [4, 4, 2, 2]);
        assert_eq!((e.w, e.h), (0, 0));
    }

    /// 実際の素材と解析の結果で、各パートのコマを PNG に描く（手で走らせて目で見る）:
    /// AK_TEST_GUIDE_VIDEO=<素材> AK_TEST_GUIDE_ANALYSIS=<guide_analysis.json> AK_TEST_OUT=<出力フォルダ> cargo test -p ak-studio real_guide_frames -- --ignored
    #[test]
    #[ignore = "実際の素材と解析の結果が要る（AK_TEST_GUIDE_VIDEO・AK_TEST_GUIDE_ANALYSIS・AK_TEST_OUT）"]
    fn real_guide_frames() {
        use crate::guide::plan::{effective_fixed, plan};
        let (Ok(video), Ok(ana), Ok(out)) = (
            std::env::var("AK_TEST_GUIDE_VIDEO"),
            std::env::var("AK_TEST_GUIDE_ANALYSIS"),
            std::env::var("AK_TEST_OUT"),
        ) else {
            panic!("AK_TEST_GUIDE_VIDEO・AK_TEST_GUIDE_ANALYSIS・AK_TEST_OUT を指定してください");
        };
        let a = crate::guide::parse_analysis(&std::fs::read_to_string(ana).unwrap()).unwrap();
        let mut g = ak_proto::Guide { src: "x.mp4".into(), ..Default::default() };
        for (k, v) in &a.anchors {
            g.anchors.insert(k.clone(), ak_proto::Anchor { t: None, auto: Some(*v) });
        }
        let p = plan(&g, &effective_fixed(&g, &Default::default()), &a.pts).unwrap();
        let pool = MediaPool::default();
        *pool.ffmpeg.lock() = crate::settings::find_ffmpeg(&crate::settings::Settings::default()).expect("ffmpeg");
        std::fs::create_dir_all(&out).unwrap();
        let at = |n: &str, dt: f64| p.parts.iter().find(|x| x.name == n).unwrap().t0 + dt;
        for (name, t) in [("formation", 6.0), ("manual_cost0", 10.0), ("manual_mid", 60.0), ("auto_mid", at("auto", 30.0)), ("result", at("result", 2.0))] {
            let pf = p.frame_at(t).unwrap();
            let part = p.part_name(pf);
            let mut fr = Rgb { w: 960, h: 540, data: [12u8, 18, 28].repeat(960 * 540) };
            render_plan_frame(&mut fr, &pool, Path::new(&video), &a, &g.layout, &Default::default(), part, pf, None, Default::default(), 0.5, false).unwrap();
            let img = image::RgbImage::from_raw(fr.w, fr.h, fr.data).unwrap();
            img.save(Path::new(&out).join(format!("{name}_{part}.png"))).unwrap();
        }
    }

    #[test]
    fn formation_dim_dims_unused_slots_and_releases_before_dark() {
        let f = GuideFormation { used: vec![0, 2], ..Default::default() };
        // 暗転の十分手前: 使わない 10 枠が全力で暗い
        let (slots, s) = formation_dim(&f, 1.0, 3.475).unwrap();
        assert_eq!(slots.len(), 10);
        assert_eq!(s, 1.0);
        assert!(!slots.contains(&f.effective_slots()[0]) && !slots.contains(&f.effective_slots()[2]));
        assert!(slots.contains(&f.effective_slots()[1]));
        // 暗転の 0.3 秒前から戻り始め、暗転で 0
        let (_, s) = formation_dim(&f, 3.475 - 0.15, 3.475).unwrap();
        assert!((s - 0.5).abs() < 1e-6, "{s}");
        assert!(formation_dim(&f, 3.475, 3.475).is_none());
        assert!(formation_dim(&f, 4.0, 3.475).is_none(), "暗転の後には枠が残らない");
        // used が空なら何もしない
        assert!(formation_dim(&GuideFormation::default(), 1.0, 3.475).is_none());
        // 全部の枠が used なら暗くするものが無い
        let all = GuideFormation { used: (0..12).collect(), ..Default::default() };
        assert!(formation_dim(&all, 1.0, 3.475).is_none());
    }

    #[test]
    fn slot_rect_follows_the_layer_geometry() {
        // 2752x2064 を 1920x1080 の出力に（70% の中央）。枠 0 は素材の (230, 426)〜(500, 1010)
        let g = geometry(&layout(), &Default::default(), "formation", 2752, 2064, 1.0);
        let r = slot_rect(&g, [230.0, 426.0, 500.0, 1010.0]);
        assert_eq!(r[0], -3 + (230.0f64 / 2752.0 * 1926.0).round() as i64);
        assert_eq!(r[1], -182 + (426.0f64 / 2064.0 * 1444.0).round() as i64);
        assert!(r[2] > r[0] && r[3] > r[1]);
        // 素材全体の矩形はレイヤー（読んだ絵）の範囲と一致する
        let all = slot_rect(&g, [0.0, 0.0, REF_W, REF_H]);
        assert_eq!(all, [-3, -182, -3 + 1926, -182 + 1444]);
    }

    #[test]
    fn dim_region_darkens_inside_only_and_zero_strength_is_noop() {
        let mut fr = Rgb { w: 4, h: 3, data: [200u8, 100, 50].repeat(12) };
        let before = fr.clone();
        dim_region(&mut fr, [1, 1, 3, 3], [0, 0, 4, 3], 0.0);
        assert_eq!(fr.data, before.data);
        dim_region(&mut fr, [1, 1, 3, 3], [0, 0, 4, 3], 1.0);
        // 外は触らない
        assert_eq!(px(&fr, 0, 0), [200, 100, 50]);
        assert_eq!(px(&fr, 3, 1), [200, 100, 50]);
        assert_eq!(px(&fr, 1, 0), [200, 100, 50]);
        // 中は 35% の明るさ（彩度は半分に落ちて、赤が減り青が増える）
        let p = px(&fr, 1, 1);
        assert!(p[0] < 76 && p[0] > 40, "{p:?}");
        let (b0, b1) = (px(&before, 1, 1), p);
        assert!((b1[0] as i32 - b1[2] as i32) < (b0[0] as i32 - b0[2] as i32));
        // clip の外は暗くしない
        let mut fr = before.clone();
        dim_region(&mut fr, [0, 0, 4, 3], [2, 0, 4, 3], 1.0);
        assert_eq!(px(&fr, 1, 1), [200, 100, 50]);
        assert_ne!(px(&fr, 2, 1), [200, 100, 50]);
        // 矩形が絵の外にはみ出しても落ちない
        dim_region(&mut fr, [-5, -5, 100, 100], [0, 0, 4, 3], 0.5);
    }

    #[test]
    fn cost_k_ramps_up_at_t0_and_down_after_t1() {
        // 見本 tests/fixtures/guide_samples.json（kit の cost_k のテストも同じ表を読む）
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/guide_samples.json");
        let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        let c = &v["cost_k"];
        let (s0, s1) = (c["t0"].as_f64().unwrap(), c["t1"].as_f64().unwrap());
        assert!(!c["cases"].as_array().unwrap().is_empty());
        for r in c["cases"].as_array().unwrap() {
            let (t, want) = (r[0].as_f64().unwrap(), r[1].as_f64().unwrap());
            assert!((cost_k(t, s0, s1) - want).abs() < 1e-9, "t={t}");
        }
        let (t0, t1) = (10.0, 12.5);
        assert_eq!(cost_k(9.0, t0, t1), 0.0, "t0 の前");
        assert_eq!(cost_k(t0, t0, t1), 0.0);
        assert!((cost_k(t0 + 0.15, t0, t1) - 0.5).abs() < 1e-12, "t0 + 0.15 で半分");
        assert_eq!(cost_k(t0 + 0.3, t0, t1), 1.0);
        assert_eq!(cost_k(11.0, t0, t1), 1.0);
        assert_eq!(cost_k(t1, t0, t1), 1.0);
        assert!((cost_k(t1 + 0.15, t0, t1) - 0.5).abs() < 1e-12, "畳みの半分");
        assert_eq!(cost_k(t1 + 0.3, t0, t1), 0.0);
        assert_eq!(cost_k(99.0, t0, t1), 0.0);
        assert!((smooth(0.25) - 0.15625).abs() < 1e-12 && smooth(-1.0) == 0.0 && smooth(2.0) == 1.0);
    }

    fn hold(op: &str, t0: f64, t1: f64, frame: u32) -> Hold {
        Hold { op: op.into(), t0, t1, frame, cost_from: None, cost_to: None }
    }

    #[test]
    fn hold_state_takes_the_larger_k_and_the_later_start_frame() {
        assert_eq!(hold_state(&[], 5.0), CostHold { k: 0.0, frame: None });
        let hs = [hold("a", 10.0, 14.0, 200), hold("b", 11.0, 13.0, 150)];
        assert_eq!(hold_state(&hs, 9.99), CostHold::default());
        // a だけ（k > 0 の間は frame がある。k = 0 ちょうど（t0）は無い）
        assert_eq!(hold_state(&hs, 10.0).frame, None);
        assert_eq!(hold_state(&hs, 10.5).frame, Some(200));
        // 重なれば t0 の遅い b のコマ、度合いは大きいほう（a は 1）
        let s = hold_state(&hs, 11.1);
        assert_eq!((s.frame, s.k), (Some(150), 1.0));
        // b が畳み終わったら a のコマに戻る
        assert_eq!(hold_state(&hs, 13.4).frame, Some(200));
        // 全部畳み終わるまで（最後の t1 + 0.3 まで）止めたコマを出す。操作の瞬間（t_out）で戻さない
        assert_eq!(hold_state(&hs, 14.2).frame, Some(200));
        assert_eq!(hold_state(&hs, 14.3), CostHold::default());
    }

    fn fi_guide() -> Guide {
        use ak_proto::guide::{Anchor, ANCHOR_NAMES};
        let mut g = Guide::default();
        for (name, v) in ANCHOR_NAMES.iter().zip([3.475, 9.208, 181.043, 202.393, 300.945]) {
            g.anchors.insert(name.to_string(), Anchor { t: Some(v), auto: None });
        }
        g.ops = vec![GuideOp { id: "op1".into(), t: 20.0, kind: "deploy".into(), precise: true, ..Default::default() }];
        g
    }

    fn fi_built(g: &Guide) -> (GuidePlan, GuideAnalysis) {
        let n = 20000;
        let a = GuideAnalysis {
            src: "a.mp4".into(),
            size: [2752, 2064],
            pts: std::sync::Arc::new((0..n).map(|i| i as f64 / 60.0).collect()),
            cost: (0..n).map(|i| Some(i % 50)).collect(),
            ..Default::default()
        };
        (crate::guide::plan::plan(g, &Default::default(), &a.pts).unwrap(), a)
    }

    #[test]
    fn frame_inputs_is_none_outside_the_plan() {
        let g = fi_guide();
        let (p, a) = fi_built(&g);
        assert!(frame_inputs(&g, &p, &a, -0.1).is_none());
        assert!(frame_inputs(&g, &p, &a, p.end() + 1.0).is_none());
        assert!(frame_inputs(&g, &p, &a, 1.0).is_some());
    }

    #[test]
    fn frame_inputs_dims_unused_slots_only_in_formation_before_dark() {
        let mut g = fi_guide();
        g.formation.used = vec![0, 2];
        let (p, a) = fi_built(&g);
        let dark_out = p.anchor_out.iter().find(|(k, _)| k == "formation_dark").unwrap().1;
        let fi = frame_inputs(&g, &p, &a, 0.5).unwrap();
        assert_eq!(fi.part, "formation");
        assert_eq!(fi.dim.as_ref().map(|d| d.0.len()), Some(10));
        // 暗転の後（手動パート）は暗くしない
        let fi = frame_inputs(&g, &p, &a, dark_out + 5.0).unwrap();
        assert_ne!(fi.part, "formation");
        assert!(fi.dim.is_none());
        // 暗転のアンカーが無い組み立てでは暗くしない
        let mut p2 = p.clone();
        p2.anchor_src.retain(|(k, _)| k != "formation_dark");
        assert!(frame_inputs(&g, &p2, &a, 0.5).unwrap().dim.is_none());
        // used が空なら暗くしない
        g.formation.used.clear();
        assert!(frame_inputs(&g, &p, &a, 0.5).unwrap().dim.is_none());
    }

    #[test]
    fn frame_inputs_holds_cost_zoom_only_around_a_precise_op() {
        let g = fi_guide();
        let (p, a) = fi_built(&g);
        let outs = ops_out(&g, &p, &a.cost, &a.game);
        let hs = holds(&g.ops, &outs, &p, &a.cost);
        assert_eq!(hs.len(), 1);
        let mid = (hs[0].t0 + hs[0].t1) / 2.0;
        let fi = frame_inputs(&g, &p, &a, mid).unwrap();
        assert!(fi.hold.k > 0.0, "{:?}", fi.hold);
        assert_eq!(fi.hold.frame, Some(hs[0].frame));
        let far = frame_inputs(&g, &p, &a, hs[0].t0 - 5.0).unwrap();
        assert_eq!(far.hold.k, 0.0);
        assert_eq!(far.hold.frame, None);
    }

    #[test]
    fn even_scaled_rounds_to_even_and_clamps_scale() {
        assert_eq!(even_scaled(2752, 2064, 0.5), (1376, 1032));
        assert_eq!(even_scaled(2752, 2064, 1.0), (2752, 2064));
        // 奇数になる倍率は偶数に（1001 * 0.5 = 500.5 → 501 → 500）
        assert_eq!(even_scaled(1001, 999, 0.5), (500, 500));
        // 倍率の上限・下限
        assert_eq!(even_scaled(2752, 2064, 5.0), (2752, 2064));
        assert_eq!(even_scaled(2000, 1000, 0.0), even_scaled(2000, 1000, 0.05));
        assert_eq!(even_scaled(2000, 1000, 0.05), (100, 50));
        // 小さすぎると 2
        assert_eq!(even_scaled(10, 10, 0.05), (2, 2));
    }

    #[test]
    fn slot_crop_px_converts_ref_pixels_and_stays_inside() {
        assert_eq!(slot_crop_px([0.0, 0.0, REF_W, REF_H], 1376, 1032), [0, 0, 1376, 1032]);
        let r = slot_crop_px([230.0, 426.0, 500.0, 1010.0], 1376, 1032);
        assert_eq!(r, [(230.0f64 / REF_W * 1376.0).round() as i64, (426.0f64 / REF_H * 1032.0).round() as i64,
                       (500.0f64 / REF_W * 1376.0).round() as i64, (1010.0f64 / REF_H * 1032.0).round() as i64]);
        // 範囲外は 0〜n に収まる
        assert_eq!(slot_crop_px([-100.0, -50.0, 99999.0, 99999.0], 100, 80), [0, 0, 100, 80]);
    }

    fn cost_zoom() -> ZoomOp {
        let l = layout();
        let g = geometry(&l, &Default::default(), "manual", 2752, 2064, 1.0);
        zoom_ops(&l, "manual", &g, 1.0).remove(0)
    }

    #[test]
    fn mini_row_sits_at_the_bottom_of_the_zoom_box() {
        let z = cost_zoom();
        let m = z.mini.as_ref().unwrap();
        // 拡大の箱 [1456, 600, 448, 195] の下端に揃う 56px の 1 行
        assert_eq!(z.dst, [1456, 600, 448, 195]);
        assert_eq!(m.row, [1456, 600 + 195 - 56, 448, 56]);
        // 切り抜きは行の中（余白 4）・右寄せで、縦横比は拡大の src の上 56%・左半分を除いたもの
        assert_eq!(m.img[3], 48);
        assert!(m.img[0] >= m.row[0] + 96 && m.img[0] + m.img[2] <= m.row[0] + m.row[2], "{:?}", m.img);
        assert!(m.img[1] >= m.row[1] && m.img[1] + m.img[3] <= m.row[1] + m.row[3]);
        assert_eq!(m.src[0], z.src[0] + ((z.src[2] - z.src[0]) as f64 * MINI_LEFT).round() as i64);
        assert_eq!(m.src[1..3], [z.src[1], z.src[2]]);
        assert_eq!(m.src[3] - m.src[1], ((z.src[3] - z.src[1]) as f64 * MINI_FRAC).round() as i64);
        // 名前が cost 以外の拡大は 1 行を持たない
        let mut l = layout();
        l.zooms[0].name = "other".into();
        let g = geometry(&l, &Default::default(), "manual", 2752, 2064, 1.0);
        assert!(zoom_ops(&l, "manual", &g, 1.0)[0].mini.is_none());
        // 手動パートの外（auto にも入れた cost）は 1 行を持たない
        let mut l = layout();
        l.zooms[0].parts.push("auto".into());
        let g = geometry(&l, &Default::default(), "auto", 2752, 2064, 1.0);
        assert!(zoom_ops(&l, "auto", &g, 1.0)[0].mini.is_none());
    }

    #[test]
    fn reveal_zoom_shows_the_bottom_part_without_shrinking() {
        let z = cost_zoom();
        // k = 0: 1 行の高さだけ（下から）。k = 1: 全部
        let r0 = reveal_zoom(&z, 0.0);
        assert_eq!((r0.dst[1], r0.dst[3]), (600 + 195 - 56, 56));
        assert_eq!(r0.src[3], z.src[3]);
        let r1 = reveal_zoom(&z, 1.0);
        assert_eq!((r1.dst, r1.src), (z.dst, z.src));
        // k = 0.5: 高さ 56 + (195 − 56) × 0.5 = 125.5 → 126。下端は動かず、src の見える割合も同じ（縮めない）
        let r = reveal_zoom(&z, 0.5);
        assert_eq!(r.dst[1] + r.dst[3], 600 + 195);
        assert_eq!(r.dst[3], 126);
        let sh = (z.src[3] - z.src[1]) as f64;
        assert!(((r.src[3] - r.src[1]) as f64 - sh * 126.0 / 195.0).abs() <= 1.0);
        // コストの拡大でなければそのまま
        let mut o = z.clone();
        o.mini = None;
        assert_eq!(reveal_zoom(&o, 0.3), o);
    }

    #[test]
    fn draw_zoom_k_shows_mini_row_at_k0_and_held_zoom_at_k1() {
        let z = cost_zoom();
        let (w, h) = (z.src[2] as u32, z.src[3] as u32);
        let now = Rgb { w, h, data: vec![200; (w * h * 3) as usize] };
        let held = Rgb { w, h, data: vec![60; (w * h * 3) as usize] };
        let px = |fr: &Rgb, x: i64, y: i64| fr.data[((y as usize * fr.w as usize) + x as usize) * 3];
        let mk = || Rgb { w: 1920, h: 1080, data: vec![0; 1920 * 1080 * 3] };
        let m = z.mini.as_ref().unwrap();
        let (ix, iy) = (m.img[0] + m.img[2] / 2, m.img[1] + m.img[3] / 2);
        // k = 0: 箱の上のほうは空、1 行の切り抜きは今のコマ（200）
        let mut fr = mk();
        draw_zoom_k(&mut fr, &now, Some(&held), &z, 0.0);
        assert_eq!(px(&fr, 1500, 620), 0, "拡大は出さない");
        assert_eq!(px(&fr, ix, iy), 200);
        // k = 1: 拡大は止めた絵（60）が箱いっぱい。1 行の切り抜きは出さない
        let mut fr = mk();
        draw_zoom_k(&mut fr, &now, Some(&held), &z, 1.0);
        assert_eq!(px(&fr, 1500, 620), 60);
        assert_eq!(px(&fr, ix, iy), 60);
        // k = 0.5: 箱の上のほうはまだ空（下から現れる）、下は止めた絵
        let mut fr = mk();
        draw_zoom_k(&mut fr, &now, Some(&held), &z, 0.5);
        assert_eq!(px(&fr, 1500, 610), 0);
        assert_eq!(px(&fr, 1500, 700), 60);
        // コストの拡大でないもの（mini なし）は k にかかわらず今のコマから
        let mut o = z.clone();
        o.mini = None;
        let mut fr = mk();
        draw_zoom_k(&mut fr, &now, Some(&held), &o, 0.0);
        assert_eq!(px(&fr, 1500, 620), 200);
    }

    #[test]
    fn zoom_ops_follow_layout_and_part() {
        let l = layout();
        // 手動パートだけ。既定のコストの拡大: 素材 (2280,1560)〜(2752,1765) → 448x195 を (1456, 600) へ（倍率 1）
        let g = geometry(&l, &Default::default(), "manual", 2752, 2064, 1.0);
        let z = zoom_ops(&l, "manual", &g, 1.0);
        assert_eq!(z.len(), 1);
        assert_eq!(z[0].dst, [1456, 600, 448, 195]);
        assert_eq!(z[0].src[2], g.read.0 as i64, "右端は読んだ絵の右端");
        assert!(zoom_ops(&l, "auto", &geometry(&l, &Default::default(), "auto", 2752, 2064, 1.0), 1.0).is_empty());
        // 出力の倍率で置き先だけ縮む
        let g5 = geometry(&l, &Default::default(), "manual", 2752, 2064, 0.5);
        let z5 = zoom_ops(&l, "manual", &g5, 0.5);
        assert_eq!(z5[0].dst, [728, 300, 224, 97]);
        // 手動の縮め・位置を変えても、拡大の置き先は動かない（レイヤーの外）
        let mut l2 = layout();
        l2.manual_x = 100.0;
        let z2 = zoom_ops(&l2, "manual", &geometry(&l2, &Default::default(), "manual", 2752, 2064, 1.0), 1.0);
        assert_eq!(z2[0].dst, z[0].dst);
        // 空の範囲・倍率 0 は飛ばす
        let mut l3 = layout();
        l3.zooms[0].src = [10.0, 10.0, 10.0, 50.0];
        assert!(zoom_ops(&l3, "manual", &g, 1.0).is_empty());
        l3.zooms[0] = layout().zooms[0].clone();
        l3.zooms[0].scale = 0.0;
        assert!(zoom_ops(&l3, "manual", &g, 1.0).is_empty());
        assert_eq!(zoom_box(&layout().zooms[0])[0..2], [1456.0, 600.0]);
    }

    #[test]
    fn draw_zoom_enlarges_the_crop_into_the_box() {
        // 4x4 の絵の左上 2x2（赤・緑・青・白の固まり）を、出力 10x8 の (3, 2) に 4x4 へ拡大
        let mut im = Rgb { w: 4, h: 4, data: vec![0; 48] };
        for y in 0..2 {
            for x in 0..2 {
                let i = (y * 4 + x) * 3;
                im.data[i..i + 3].copy_from_slice(&[200, 100, 50]);
            }
        }
        let z = ZoomOp { name: "z".into(), src: [0, 0, 2, 2], dst: [3, 2, 4, 4], mini: None };
        let mut fr = Rgb { w: 10, h: 8, data: vec![9; 10 * 8 * 3] };
        draw_zoom(&mut fr, &im, &z);
        // 箱の中は切り抜きの色、外は触らない
        assert_eq!(px(&fr, 3, 2), [200, 100, 50]);
        assert_eq!(px(&fr, 6, 5), [200, 100, 50]);
        assert_eq!(px(&fr, 2, 2), [9, 9, 9]);
        assert_eq!(px(&fr, 7, 5), [9, 9, 9]);
        assert_eq!(px(&fr, 3, 6), [9, 9, 9]);
        // 箱が出力の外へはみ出しても落ちず、中だけ描く
        let z = ZoomOp { name: "z".into(), src: [0, 0, 2, 2], dst: [8, 6, 4, 4], mini: None };
        let mut fr = Rgb { w: 10, h: 8, data: vec![9; 10 * 8 * 3] };
        draw_zoom(&mut fr, &im, &z);
        assert_eq!(px(&fr, 9, 7), [200, 100, 50]);
        // 切り抜きが空なら何もしない
        let before = fr.data.clone();
        draw_zoom(&mut fr, &im, &ZoomOp { name: "z".into(), src: [3, 3, 3, 3], dst: [0, 0, 4, 4], mini: None });
        assert_eq!(fr.data, before);
    }

    #[test]
    fn sub_target_fits_sub_box_with_even_size() {
        let b = [1480.0, 300.0, 400.0, 300.0];
        assert_eq!(sub_target(b, 640, 480, 1.0), [1480, 300, 400, 300]);
        // 16:9 は 400x225 → 偶数に丸めて 400x224、y は 337.5 → 338
        assert_eq!(sub_target(b, 1920, 1080, 1.0), [1480, 338, 400, 224]);
        assert_eq!(sub_target(b, 640, 480, 0.5), [740, 150, 200, 150]);
    }

    #[test]
    fn mix_blends_by_weight() {
        let mut a = Rgb { w: 2, h: 1, data: vec![0; 6] };
        let b = Rgb { w: 2, h: 1, data: vec![200; 6] };
        mix_into(&mut a, &b, 0.0);
        assert_eq!(a.data[0], 0);
        mix_into(&mut a, &b, 0.5);
        assert!((a.data[0] as i32 - 100).abs() <= 1);
        let mut a = Rgb { w: 2, h: 1, data: vec![0; 6] };
        mix_into(&mut a, &b, 1.0);
        assert!(a.data[0] >= 199);
        // 大きさが違えば触らない
        let mut a = Rgb { w: 1, h: 1, data: vec![5; 3] };
        mix_into(&mut a, &b, 1.0);
        assert_eq!(a.data[0], 5);
    }
}
