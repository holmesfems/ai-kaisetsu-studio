//! 素材そのままの 1 コマの確認画像（`guide_still`）の、純粋な部分。
//!
//! 見せるコマの決め方・切り抜きと倍率・目盛り（100px ごとの線、200px ごとの数字）。
//! 演出・レイヤーは入れない（素材の画素の位置を目で確かめるため）。素材を読んで保存するのは `core/guide.rs`。

use super::plan::{frame_at_time, snap_to_frame, GuidePlan};
use crate::render::media::Rgb;
use ak_proto::guide::{REF_H, REF_W};
use ak_proto::http::GuideStillReq;
use ak_proto::GuideOp;

/// 確認画像の長い辺の既定（px）
pub const DEFAULT_LONG: f64 = 1600.0;
/// 倍率の上限
pub const MAX_SCALE: f64 = 2.0;

/// 解析した素材の大きさと今の素材の大きさが違えば Err（解析の大きさが 0 = 分からないときは通す）
pub fn check_size(analysis_size: [u32; 2], w: u32, h: u32) -> Result<(), String> {
    if analysis_size[0] != 0 && (analysis_size[0] != w || analysis_size[1] != h) {
        return Err(format!(
            "解析した素材の大きさ（{}x{}）と今の素材（{}x{}）が違います。もう一度解析してください",
            analysis_size[0], analysis_size[1], w, h
        ));
    }
    Ok(())
}

/// 長い辺を max までに縮めた大きさ。縮めなくてよければ None（各辺は 1 以上）
pub fn fit_long(w: u32, h: u32, max: u32) -> Option<(u32, u32)> {
    let long = w.max(h);
    if long > max {
        let k = max as f64 / long as f64;
        Some((((w as f64 * k).round() as u32).max(1), ((h as f64 * k).round() as u32).max(1)))
    } else {
        None
    }
}

/// どのコマを見せるか
#[derive(Clone, Debug, PartialEq)]
pub enum Target {
    /// 操作の id
    Op(String),
    /// 素材の秒
    Src(f64),
    /// 素材のコマ番号
    Frame(u32),
    /// 出力の秒
    Out(f64),
}

/// 見せるコマ
#[derive(Clone, Debug, PartialEq)]
pub struct Pick {
    pub frame: u32,
    /// そのコマの素材の時刻
    pub t_src: f64,
    /// パート名（手動パートの中、または出力の時刻から決めたとき）
    pub part: Option<String>,
    /// 手動パートでの出力の時刻
    pub t_out: Option<f64>,
}

/// 要求から対象を取り出す。1 つも無ければ None、2 つ以上は Err
pub fn parse_target(r: &GuideStillReq) -> Result<Option<Target>, String> {
    let n = r.op.is_some() as u8 + r.t.is_some() as u8 + r.frame.is_some() as u8 + r.t_out.is_some() as u8;
    if n > 1 {
        return Err("op・t・frame・t_out のどれか 1 つだけ渡してください".into());
    }
    Ok(if let Some(op) = &r.op {
        Some(Target::Op(op.clone()))
    } else if let Some(t) = r.t {
        Some(Target::Src(t))
    } else if let Some(f) = r.frame {
        Some(Target::Frame(f))
    } else {
        r.t_out.map(Target::Out)
    })
}

/// 素材の時刻 t_src の、手動パートでの出力の時刻と、そのパート名
fn manual_of(plan: Option<&GuidePlan>, t_src: f64) -> (Option<String>, Option<f64>) {
    match plan.and_then(|p| p.manual_out_of(t_src)) {
        Some(t) => (Some("manual".into()), Some(t)),
        None => (None, None),
    }
}

/// 対象のコマを決める。pts は解析のコマごとの素材の時刻
pub fn pick_frame(target: &Target, pts: &[f64], plan: Option<&GuidePlan>, ops: &[GuideOp]) -> Result<Pick, String> {
    if pts.is_empty() {
        return Err("素材の解析がまだです。攻略タブの「解析」を押してください".into());
    }
    let at_src = |t: f64| -> Pick {
        let frame = frame_at_time(pts, t);
        let t_src = pts[frame as usize];
        let (part, t_out) = manual_of(plan, t_src);
        Pick { frame, t_src, part, t_out }
    };
    match target {
        Target::Op(id) => {
            let o = ops.iter().find(|o| o.id == *id).ok_or_else(|| format!("操作「{id}」がありません（guide_read の ops[].id を使ってください）"))?;
            if !o.t.is_finite() {
                return Err(format!("操作「{id}」の時刻が数ではありません"));
            }
            Ok(at_src(snap_to_frame(pts, o.t)))
        }
        Target::Src(t) => {
            if !t.is_finite() {
                return Err("t が数ではありません".into());
            }
            Ok(at_src(*t))
        }
        Target::Frame(f) => {
            let Some(&t_src) = pts.get(*f as usize) else {
                return Err(format!("frame {f} は素材のコマの数（{}）の外です", pts.len()));
            };
            let (part, t_out) = manual_of(plan, t_src);
            Ok(Pick { frame: *f, t_src, part, t_out })
        }
        Target::Out(t) => {
            let Some(plan) = plan else {
                return Err("パートを組み立てられていないので、出力の時刻からは選べません。素材を解析して、アンカーを確かめてください".into());
            };
            let Some((frame, t_src, part)) = plan.src_at(*t) else {
                return Err(format!("出力の {t:.2} 秒は攻略のパートの外です（0〜{:.2} 秒）", plan.end()));
            };
            let t_out = (part == "manual").then_some(*t);
            Ok(Pick { frame, t_src, part: Some(part.to_string()), t_out })
        }
    }
}

/// 切り抜き（素材の画素、REF の大きさでの値）を素材の範囲に収める。省けば全体。空・数でないものは Err
pub fn resolve_crop(crop: Option<[f64; 4]>) -> Result<[f64; 4], String> {
    let Some(c) = crop else { return Ok([0.0, 0.0, REF_W, REF_H]) };
    if !c.iter().all(|v| v.is_finite()) {
        return Err("crop が数ではありません".into());
    }
    let r = [c[0].clamp(0.0, REF_W), c[1].clamp(0.0, REF_H), c[2].clamp(0.0, REF_W), c[3].clamp(0.0, REF_H)];
    if r[2] - r[0] < 1.0 || r[3] - r[1] < 1.0 {
        return Err(format!("crop（{c:?}）が空か、素材（{REF_W}x{REF_H}）の外です。[x0, y0, x1, y1] で、終わりを始めより大きくしてください"));
    }
    Ok(r)
}

/// 素材の 1 画素を何画素で出すか。省けば長い辺が `DEFAULT_LONG` に収まる倍率（切り抜きは 1.0 まで）。上限 `MAX_SCALE`
pub fn resolve_scale(scale: Option<f64>, crop: [f64; 4], cropped: bool) -> Result<f64, String> {
    if let Some(s) = scale {
        if !s.is_finite() || s <= 0.0 {
            return Err(format!("scale（{s}）は 0 より大きい数にしてください"));
        }
        return Ok(s.min(MAX_SCALE));
    }
    let long = (crop[2] - crop[0]).max(crop[3] - crop[1]);
    let fit = DEFAULT_LONG / long;
    Ok(if cropped { fit.min(1.0) } else { fit })
}

/// 出す画像の大きさ（切り抜きの素材の画素 × scale）
pub fn out_size(crop: [f64; 4], scale: f64) -> (u32, u32) {
    (((crop[2] - crop[0]) * scale).round().max(1.0) as u32, ((crop[3] - crop[1]) * scale).round().max(1.0) as u32)
}

/// 読んだコマ im（素材の大きさ。REF と同じ縦横比）から crop（REF の画素）を切り抜いて、scale 倍の大きさにする
pub fn crop_and_scale(im: &Rgb, crop: [f64; 4], scale: f64) -> Rgb {
    let (fx, fy) = (im.w as f64 / REF_W, im.h as f64 / REF_H);
    let x0 = ((crop[0] * fx).round() as i64).clamp(0, im.w as i64 - 1) as u32;
    let y0 = ((crop[1] * fy).round() as i64).clamp(0, im.h as i64 - 1) as u32;
    let x1 = ((crop[2] * fx).round() as i64).clamp(x0 as i64 + 1, im.w as i64) as u32;
    let y1 = ((crop[3] * fy).round() as i64).clamp(y0 as i64 + 1, im.h as i64) as u32;
    let (w, h) = out_size(crop, scale);
    let src = image::RgbImage::from_raw(im.w, im.h, im.data.clone()).expect("Rgb の大きさと中身が合っている");
    let view = image::imageops::crop_imm(&src, x0, y0, x1 - x0, y1 - y0).to_image();
    if view.width() == w && view.height() == h {
        return Rgb { w, h, data: view.into_raw() };
    }
    let r = image::imageops::resize(&view, w, h, image::imageops::FilterType::Triangle);
    Rgb { w, h, data: r.into_raw() }
}

// 数字の字（5x7。上の行から、左が最上位ビット）
const DIGITS: [[u8; 7]; 10] = [
    [0b01110, 0b10001, 0b10011, 0b10101, 0b11001, 0b10001, 0b01110],
    [0b00100, 0b01100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110],
    [0b01110, 0b10001, 0b00001, 0b00010, 0b00100, 0b01000, 0b11111],
    [0b11110, 0b00001, 0b00001, 0b01110, 0b00001, 0b00001, 0b11110],
    [0b00010, 0b00110, 0b01010, 0b10010, 0b11111, 0b00010, 0b00010],
    [0b11111, 0b10000, 0b11110, 0b00001, 0b00001, 0b10001, 0b01110],
    [0b00110, 0b01000, 0b10000, 0b11110, 0b10001, 0b10001, 0b01110],
    [0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b01000, 0b01000],
    [0b01110, 0b10001, 0b10001, 0b01110, 0b10001, 0b10001, 0b01110],
    [0b01110, 0b10001, 0b10001, 0b01111, 0b00001, 0b00010, 0b01100],
];

/// 目盛りの数字の 1 画素の大きさ（字は 5x7 の点が (5·K)x(7·K) になる）
pub const GLYPH_K: i64 = 2;
/// 字と字の間（点の数）
const GLYPH_GAP: i64 = 1;

/// 数字の文字列の大きさ (幅, 高さ)（画素）
pub fn text_size(text: &str) -> (i64, i64) {
    let n = text.len() as i64;
    ((n * 5 + (n - 1).max(0) * GLYPH_GAP) * GLYPH_K, 7 * GLYPH_K)
}

fn put(img: &mut Rgb, x: i64, y: i64, rgb: [u8; 3]) {
    if x < 0 || y < 0 || x >= img.w as i64 || y >= img.h as i64 {
        return;
    }
    let i = (y as usize * img.w as usize + x as usize) * 3;
    img.data[i..i + 3].copy_from_slice(&rgb);
}

/// 数字の文字列を (x, y) を左上に描く（白字・黒の縁取り代わりの箱つき。絵の外ははみ出さない）
fn draw_label(img: &mut Rgb, x: i64, y: i64, text: &str) {
    let (w, h) = text_size(text);
    // 黒い箱（1 画素ぶん広く）
    for yy in y - 1..y + h + 1 {
        for xx in x - 1..x + w + 1 {
            if xx < 0 || yy < 0 || xx >= img.w as i64 || yy >= img.h as i64 {
                continue;
            }
            let i = (yy as usize * img.w as usize + xx as usize) * 3;
            for k in 0..3 {
                img.data[i + k] = (img.data[i + k] as u32 * 3 / 10) as u8;
            }
        }
    }
    let mut cx = x;
    for ch in text.chars() {
        if let Some(d) = ch.to_digit(10) {
            for (row, bits) in DIGITS[d as usize].iter().enumerate() {
                for col in 0..5 {
                    if bits >> (4 - col) & 1 == 1 {
                        for dy in 0..GLYPH_K {
                            for dx in 0..GLYPH_K {
                                put(img, cx + col * GLYPH_K + dx, y + row as i64 * GLYPH_K + dy, [255, 255, 255]);
                            }
                        }
                    }
                }
            }
        }
        cx += (5 + GLYPH_GAP) * GLYPH_K;
    }
}

/// 線 1 本ぶんの画素を、明るい所は暗く・暗い所は明るく寄せる（どんな画面でも見える）
fn blend_line_px(img: &mut Rgb, x: usize, y: usize, alpha: f32) {
    let i = (y * img.w as usize + x) * 3;
    let l = 0.299 * img.data[i] as f32 + 0.587 * img.data[i + 1] as f32 + 0.114 * img.data[i + 2] as f32;
    let target = if l > 128.0 { 0.0 } else { 255.0 };
    for k in 0..3 {
        let c = img.data[i + k] as f32;
        img.data[i + k] = (c + (target - c) * alpha).round().clamp(0.0, 255.0) as u8;
    }
}

/// 線の間隔（素材の画素）
pub const GRID_STEP: i64 = 100;
/// 濃い線の間隔
pub const GRID_MAJOR: i64 = 500;
/// 数字を付ける間隔
pub const GRID_LABEL: i64 = 200;

/// 絵 img（crop を scale 倍にしたもの）に目盛りを重ねる。素材の画素で 100px ごとに線（500px ごとに濃く）、
/// 200px ごとに数字（素材の座標のまま。切り抜いても変わらない）。x の数字は上端、y の数字は左端に付ける
pub fn draw_grid(img: &mut Rgb, crop: [f64; 4], scale: f64) {
    let first = |a: f64| ((a / GRID_STEP as f64).ceil() as i64) * GRID_STEP;
    let mut vlines = vec![];
    let mut v = first(crop[0]);
    while (v as f64) < crop[2] {
        vlines.push(v);
        v += GRID_STEP;
    }
    let mut hlines = vec![];
    let mut v = first(crop[1]);
    while (v as f64) < crop[3] {
        hlines.push(v);
        v += GRID_STEP;
    }
    let alpha = |v: i64| if v % GRID_MAJOR == 0 { 0.55 } else { 0.28 };
    for &v in &vlines {
        let x = ((v as f64 - crop[0]) * scale).round() as i64;
        if x < 0 || x >= img.w as i64 {
            continue;
        }
        for y in 0..img.h as usize {
            blend_line_px(img, x as usize, y, alpha(v));
        }
    }
    for &v in &hlines {
        let y = ((v as f64 - crop[1]) * scale).round() as i64;
        if y < 0 || y >= img.h as i64 {
            continue;
        }
        for x in 0..img.w as usize {
            blend_line_px(img, x, y as usize, alpha(v));
        }
    }
    for &v in vlines.iter().filter(|v| *v % GRID_LABEL == 0) {
        let x = ((v as f64 - crop[0]) * scale).round() as i64;
        draw_label(img, x + 3, 2, &v.to_string());
    }
    for &v in hlines.iter().filter(|v| *v % GRID_LABEL == 0) {
        let y = ((v as f64 - crop[1]) * scale).round() as i64;
        draw_label(img, 2, y + 3, &v.to_string());
    }
}

/// 確認画像のファイル名（拡張子なし）。同じ頼みは同じ名前で置き換える
pub fn still_stem(frame: u32, crop: Option<[f64; 4]>, grid: bool) -> String {
    let mut s = format!("guide_{frame}");
    if let Some(c) = crop {
        s.push_str(&format!("_{}-{}-{}-{}", c[0].round() as i64, c[1].round() as i64, c[2].round() as i64, c[3].round() as i64));
    }
    if grid {
        s.push_str("_grid");
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::guide::plan::plan;
    use ak_proto::{Anchor, Cut, Guide, GuideFixed};

    fn pts() -> Vec<f64> {
        (0..24000).map(|i| i as f64 / 60.0).collect()
    }

    fn guide() -> Guide {
        let mut g = Guide { src: "A.MP4".into(), ..Default::default() };
        for (k, t) in [("formation_dark", 5.0), ("manual_cost0", 10.0), ("manual_end", 100.0), ("auto_cost0", 200.0), ("auto_end", 300.0)] {
            g.anchors.insert(k.into(), Anchor { t: Some(t), auto: None });
        }
        g.ops = vec![GuideOp { id: "op1".into(), t: 20.0, kind: "deploy".into(), ..Default::default() }];
        g
    }

    fn req() -> GuideStillReq {
        GuideStillReq::default()
    }

    #[test]
    fn check_size_allows_unknown_and_equal_and_reports_both_sizes() {
        assert!(check_size([0, 0], 2752, 2064).is_ok(), "解析の大きさが 0 は通す");
        assert!(check_size([2752, 2064], 2752, 2064).is_ok());
        let e = check_size([2752, 2064], 1920, 1080).unwrap_err();
        assert_eq!(e, "解析した素材の大きさ（2752x2064）と今の素材（1920x1080）が違います。もう一度解析してください");
        assert!(check_size([2752, 2064], 2752, 1080).is_err(), "高さだけ違っても Err");
    }

    #[test]
    fn fit_long_shrinks_only_when_longer_than_max() {
        assert_eq!(fit_long(2752, 2064, 2000), Some((2000, 1500)));
        assert_eq!(fit_long(2064, 2752, 2000), Some((1500, 2000)));
        assert_eq!(fit_long(2000, 1000, 2000), None);
        assert_eq!(fit_long(1920, 1080, 2000), None);
        // 極端に細い絵でも各辺は 1 以上
        assert_eq!(fit_long(40000, 3, 2000), Some((2000, 1)));
    }

    #[test]
    fn only_one_target() {
        assert_eq!(parse_target(&req()).unwrap(), None);
        let mut r = req();
        r.op = Some("op1".into());
        assert_eq!(parse_target(&r).unwrap(), Some(Target::Op("op1".into())));
        r.frame = Some(3);
        assert!(parse_target(&r).unwrap_err().contains("どれか 1 つだけ"));
        let mut r = req();
        r.t_out = Some(12.5);
        assert_eq!(parse_target(&r).unwrap(), Some(Target::Out(12.5)));
    }

    #[test]
    fn pick_by_op_t_frame_and_out() {
        let g = guide();
        let ps = pts();
        let p = plan(&g, &GuideFixed::default(), &ps).unwrap();
        let by_op = pick_frame(&Target::Op("op1".into()), &ps, Some(&p), &g.ops).unwrap();
        assert_eq!((by_op.frame, by_op.t_src), (1200, 20.0));
        assert_eq!(by_op.part.as_deref(), Some("manual"));
        // 手動パートの頭（素材 8 秒）から 12 秒
        assert!((by_op.t_out.unwrap() - (p.parts[1].t0 + 12.0)).abs() < 0.02);
        let by_t = pick_frame(&Target::Src(20.004), &ps, None, &g.ops).unwrap();
        assert_eq!(by_t.frame, 1200);
        assert!(by_t.t_out.is_none() && by_t.part.is_none());
        let by_f = pick_frame(&Target::Frame(60), &ps, Some(&p), &g.ops).unwrap();
        assert_eq!(by_f.t_src, 1.0);
        // 出力の時刻からは、その時刻に見せている素材のコマ
        let t = p.parts[1].t0 + 12.0;
        let by_out = pick_frame(&Target::Out(t), &ps, Some(&p), &g.ops).unwrap();
        assert_eq!(by_out.part.as_deref(), Some("manual"));
        assert!((by_out.t_src - 20.0).abs() < 0.05);
        let by_out = pick_frame(&Target::Out(1.0), &ps, Some(&p), &g.ops).unwrap();
        assert_eq!(by_out.part.as_deref(), Some("formation"));
        assert!(by_out.t_out.is_none());
    }

    #[test]
    fn pick_errors() {
        let g = guide();
        let ps = pts();
        let p = plan(&g, &GuideFixed::default(), &ps).unwrap();
        assert!(pick_frame(&Target::Op("op9".into()), &ps, Some(&p), &g.ops).unwrap_err().contains("op9"));
        assert!(pick_frame(&Target::Frame(99999), &ps, None, &g.ops).unwrap_err().contains("外"));
        assert!(pick_frame(&Target::Out(9999.0), &ps, Some(&p), &g.ops).unwrap_err().contains("パートの外"));
        assert!(pick_frame(&Target::Out(1.0), &ps, None, &g.ops).unwrap_err().contains("組み立てられていない"));
        assert!(pick_frame(&Target::Frame(0), &[], None, &g.ops).unwrap_err().contains("解析"));
        // カットの中の素材の時刻でも、素材そのままなら見られる
        let mut g2 = guide();
        g2.cuts.push(Cut { src_in: 50.0, src_out: 60.0, fade: 0.25 });
        let p2 = plan(&g2, &GuideFixed::default(), &ps).unwrap();
        let r = pick_frame(&Target::Src(55.0), &ps, Some(&p2), &g2.ops).unwrap();
        assert_eq!(r.frame, 3300);
    }

    #[test]
    fn crop_and_scale_resolution() {
        assert_eq!(resolve_crop(None).unwrap(), [0.0, 0.0, REF_W, REF_H]);
        assert_eq!(resolve_crop(Some([2400.0, 1500.0, 3000.0, 1750.0])).unwrap(), [2400.0, 1500.0, REF_W, 1750.0]);
        assert!(resolve_crop(Some([10.0, 10.0, 10.0, 50.0])).unwrap_err().contains("空"));
        assert!(resolve_crop(Some([f64::NAN, 0.0, 1.0, 1.0])).is_err());
        // 全体: 長い辺が 1600
        let s = resolve_scale(None, [0.0, 0.0, REF_W, REF_H], false).unwrap();
        assert!((s * REF_W - 1600.0).abs() < 1e-6);
        // 切り抜き: 1.0 まで（小さい範囲は拡大しない）
        assert_eq!(resolve_scale(None, [2400.0, 1500.0, 2752.0, 1750.0], true).unwrap(), 1.0);
        // 大きい切り抜きは 1600 に収める
        let s = resolve_scale(None, [0.0, 0.0, 2400.0, 1000.0], true).unwrap();
        assert!((s * 2400.0 - 1600.0).abs() < 1e-6);
        // 指定は最大 2.0
        assert_eq!(resolve_scale(Some(3.0), [0.0, 0.0, 100.0, 100.0], true).unwrap(), 2.0);
        assert!(resolve_scale(Some(0.0), [0.0, 0.0, 1.0, 1.0], true).is_err());
        assert_eq!(out_size([2400.0, 1500.0, 2752.0, 1750.0], 1.0), (352, 250));
    }

    /// 縦に走る色の違いで、切り抜きの位置・倍率が合っているか見る（素材 2752x2064 の縮小版 688x516 = 1/4）
    #[test]
    fn crop_and_scale_picks_the_right_pixels() {
        let (w, h) = (688u32, 516u32);
        let mut data = vec![0u8; (w * h * 3) as usize];
        for y in 0..h {
            for x in 0..w {
                let i = ((y * w + x) * 3) as usize;
                // 赤 = x の 4 画素ごとの帯の番号、緑 = y
                data[i] = (x % 256) as u8;
                data[i + 1] = (y % 256) as u8;
            }
        }
        let im = Rgb { w, h, data };
        // REF の (400,200)-(800,600) = 縮小版の (100,50)-(200,150)。倍率 1（REF 1 画素 = 出力 1 画素）で 400x400
        let c = crop_and_scale(&im, [400.0, 200.0, 800.0, 600.0], 1.0);
        assert_eq!((c.w, c.h), (400, 400));
        // 出力の左上は縮小版の (100, 50) 付近
        assert!((c.data[0] as i32 - 100).abs() <= 2, "{}", c.data[0]);
        assert!((c.data[1] as i32 - 50).abs() <= 2, "{}", c.data[1]);
        // 出力の右下寄りは縮小版の (200, 150) 付近
        let i = ((399 * c.w + 399) * 3) as usize;
        assert!((c.data[i] as i32 - 199).abs() <= 3, "{}", c.data[i]);
        assert!((c.data[i + 1] as i32 - 149).abs() <= 3, "{}", c.data[i + 1]);
    }

    #[test]
    fn grid_lines_at_hundreds_and_labels_use_source_coordinates() {
        let crop = [2400.0, 1500.0, 2752.0, 1750.0];
        let (w, h) = out_size(crop, 1.0);
        let mut img = Rgb { w, h, data: vec![200u8; (w * h * 3) as usize] };
        let before = img.clone();
        draw_grid(&mut img, crop, 1.0);
        let px = |im: &Rgb, x: u32, y: u32| im.data[((y * im.w + x) * 3) as usize];
        // 縦線: 2400（x=0）・2500（x=100）・2600（x=200）…。線の無い列は変わらない（数字の無い下のほうの行で見る）
        let y = h - 5;
        for x in [0u32, 100, 200, 300] {
            assert_ne!(px(&img, x, y), px(&before, x, y), "x={x} に線が無い");
        }
        assert_eq!(px(&img, 50, y), px(&before, 50, y));
        // 横線: 1500（y=0）・1600（y=100）・1700（y=200）
        let x = w - 5;
        for yy in [0u32, 100, 200] {
            assert_ne!(px(&img, x, yy), px(&before, x, yy), "y={yy} に線が無い");
        }
        assert_eq!(px(&img, x, 50), px(&before, x, 50));
        // 500 の倍数（2500 は x=100）の線は濃い: 明るい背景なので、より暗くなる（2600 は x=200 で薄い）
        let major = 200 - px(&img, 100, y) as i32;
        let minor = 200 - px(&img, 200, y) as i32;
        assert!(major > minor && minor > 0, "{major} {minor}");
        // 数字: 2400 の線（x=0）の右 3 画素に「2400」。最初の字「2」の最上段（.XXX.）は x = 3 + 2〜7、y = 2〜3 が白
        assert_eq!(px(&img, 3 + 2 * GLYPH_K as u32, 2), 255);
        // 最下段（11111）の右端の点
        assert_eq!(px(&img, 3 + 4 * GLYPH_K as u32, 2 + 6 * GLYPH_K as u32), 255);
        // 2 の最上段の左端（col 0）は字の点ではない（箱の暗い所）
        assert!(px(&img, 3, 2) < 100);
        // 数字は素材の座標: 2500 の線は数字を付けない（200 の倍数ではない）。x=100 の右 3 画素の上端は線のまま（白くない）
        assert!(px(&img, 103 + 2 * GLYPH_K as u32, 2) < 255);
        // 左端の y の数字（1600）: 線 y=100 の下 3 画素から
        assert_eq!(px(&img, 2 + 2 * GLYPH_K as u32, 103), 255);
    }

    #[test]
    fn text_size_counts_digits() {
        assert_eq!(text_size("2400"), ((4 * 5 + 3) * GLYPH_K, 7 * GLYPH_K));
        assert_eq!(text_size("0"), (5 * GLYPH_K, 7 * GLYPH_K));
    }

    #[test]
    fn still_names_replace_same_request() {
        assert_eq!(still_stem(1200, None, false), "guide_1200");
        assert_eq!(still_stem(1200, Some([2400.0, 1500.0, 2752.0, 1750.0]), true), "guide_1200_2400-1500-2752-1750_grid");
    }
}
