//! 素材クリップ（立ち絵・画像・録画）の入り・抜けのトランジション（issue 旧#10）。
//!
//! プレビューも mp4 も同じ合成（Core::compose_with）を通るので、ここで「その時刻の見え方」を決めれば両方に同じように効く。
//! 動きは演出側（fxkit/fxkit.py の Sprite の enter と glitch）と同じ式にして、演出と並べても見た目が揃うようにしている。
//! 同じ式であることは見本で縛っている: 入りの曲線 `curve` は tests/fixtures/trans_curves.json（`curves_match_fixture`）、
//! 緩急は ease.json（render/ease.rs）、グリッチの乱数と画素は glitch.json（`glitch_matches_fixture`）。fxkit/test_motion.py が同じ見本を読む。
//! 走査線（media.rs の `scan_line`）だけは見た目を似せただけで、縛っていない。

use super::ease::{clamp01, ease_back, ease_in_out, ease_out, lerp};
use crate::project::{MediaClip, Trans};
use serde::Serialize;

/// 長さが 0 以下のときの既定（秒）
pub const DEFAULT_DUR: f64 = 0.35;
/// slide で動く距離（1920x1080 の画素）
const SLIDE_DIST: f64 = 160.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dir {
    Left,
    Right,
    Up,
    Down,
}

impl Dir {
    pub fn parse(s: &str) -> Dir {
        match s {
            "right" => Dir::Right,
            "up" => Dir::Up,
            "down" => Dir::Down,
            _ => Dir::Left,
        }
    }

    /// その辺の向きの単位ベクトル（画面の座標。下が +y）
    fn vec(self) -> (f64, f64) {
        match self {
            Dir::Left => (-1.0, 0.0),
            Dir::Right => (1.0, 0.0),
            Dir::Up => (0.0, -1.0),
            Dir::Down => (0.0, 1.0),
        }
    }
}

/// ある時刻のクリップの見え方。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TransState {
    /// 不透明度（クリップの opacity・fade_in / fade_out 込み）
    pub alpha: f64,
    /// 位置のずれ（1920x1080 の画素。ネストの中ならキャンバスの画素）
    pub dx: f64,
    pub dy: f64,
    /// 倍率に掛ける値
    pub scale: f64,
    /// ワイプ: (見えている割合 0〜1, 走査線が走り出す辺)
    pub wipe: Option<(f64, Dir)>,
    /// グリッチの強さ（0 = 無し）
    pub glitch: f64,
}

/// トランジションの長さ（クリップより長くはしない）。画面の transDur と見本 ui_rules.json で縛っている（app/rules_fixture.rs）
pub fn trans_dur(tr: &Trans, clip_dur: f64) -> f64 {
    if tr.is_none() {
        return 0.0;
    }
    let d = if tr.dur > 0.0 { tr.dur } else { DEFAULT_DUR };
    d.min(clip_dur.max(0.0))
}

/// 入りの 1 時点の曲線（k: 0 = まだ見えていない、1 = 出きっている）。
/// 距離・向き・画像の縮め方は持たない（それは apply 側）。fxkit の `enter_curve` と同じ値になる（見本 trans_curves.json）
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct Curve {
    /// 掛ける不透明度
    pub alpha: f64,
    /// 倍率に掛ける値
    pub scale: f64,
    /// slide: 動く距離のうち残っている割合 0〜1
    pub slide: f64,
    /// ワイプ: 見えている割合
    pub wipe: Option<f64>,
    /// グリッチの強さ
    pub glitch: f64,
}

impl Curve {
    /// 何もしない値（k >= 1 や、知らない kind）
    pub const IDENTITY: Curve = Curve { alpha: 1.0, scale: 1.0, slide: 0.0, wipe: None, glitch: 0.0 };
}

pub fn curve(kind: &str, k: f64) -> Curve {
    if k >= 1.0 {
        return Curve::IDENTITY;
    }
    let k = clamp01(k);
    let id = Curve::IDENTITY;
    match kind {
        "fade" => Curve { alpha: ease_out(k), ..id },
        "slide" => Curve { alpha: ease_out(k), slide: 1.0 - ease_out(k), ..id },
        "pop" => Curve { alpha: ease_out(k * 1.6), scale: lerp(0.82, 1.0, ease_back(k, 2.2)), ..id },
        "slam" => Curve { alpha: ease_out(k * 2.5), scale: lerp(1.7, 1.0, ease_out(k)), ..id },
        "wipe" => Curve { wipe: Some(ease_in_out(k)), ..id },
        "glitch" => Curve { alpha: ease_out(k * 2.0), glitch: (1.0 - k).powf(1.5), ..id },
        _ => id,
    }
}

/// k: 0 = まだ（もう）見えていない、1 = 出きっている
fn apply(st: &mut TransState, tr: &Trans, k: f64) {
    if k >= 1.0 {
        return;
    }
    let dir = Dir::parse(&tr.dir);
    let c = curve(tr.kind.as_str(), k);
    st.alpha *= c.alpha;
    st.scale *= c.scale;
    if c.slide > 0.0 {
        let (ux, uy) = dir.vec();
        st.dx += ux * c.slide * SLIDE_DIST;
        st.dy += uy * c.slide * SLIDE_DIST;
    }
    if let Some(e) = c.wipe {
        // 入りと抜けが重なる（短いクリップ）ときは、見えている方が狭い方
        match st.wipe {
            Some((w, _)) if w <= e => {}
            _ => st.wipe = Some((e, dir)),
        }
    }
    st.glitch = st.glitch.max(c.glitch);
}

/// 時刻 t のクリップ c の見え方（クリップの外の時刻でも、端の値を返す）。
pub fn state(c: &MediaClip, t: f64) -> TransState {
    let mut st = TransState { alpha: c.opacity, dx: 0.0, dy: 0.0, scale: 1.0, wipe: None, glitch: 0.0 };
    // 今までのフェード（直線）
    if c.fade_in > 0.0 {
        st.alpha *= ((t - c.start) / c.fade_in).clamp(0.0, 1.0);
    }
    if c.fade_out > 0.0 {
        st.alpha *= ((c.start + c.dur - t) / c.fade_out).clamp(0.0, 1.0);
    }
    let di = trans_dur(&c.trans_in, c.dur);
    if di > 0.0 {
        apply(&mut st, &c.trans_in, (t - c.start) / di);
    }
    let dout = trans_dur(&c.trans_out, c.dur);
    if dout > 0.0 {
        apply(&mut st, &c.trans_out, (c.start + c.dur - t) / dout);
    }
    st
}

/// グリッチの乱数の種（クリップとコマで決まる。同じコマなら毎回同じ絵）
pub fn seed(id: &str, frame: i64) -> u64 {
    // FNV-1a（Rust の版で変わらないように自前で）
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in id.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h ^ (frame as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15)
}

/// 決定的な乱数 [0, 1)
fn hrand(seed: u64, i: u64, tag: u64) -> f64 {
    let mut z = seed ^ i.wrapping_mul(0xbf58_476d_1ce4_e5b9) ^ tag.wrapping_mul(0x94d0_49bb_1331_11eb);
    z = z.wrapping_add(0x9e37_79b9_7f4a_7c15);
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^= z >> 31;
    (z >> 11) as f64 / (1u64 << 53) as f64
}

/// 画素の列（ch = 3 か 4）にグリッチを掛ける（横の帯をずらし、R を右・B を左へずらす）。fxkit の `glitch_pixels` と一画素まで同じ（見本 glitch.json）。
/// amt: 強さ 0〜1、unit: 素材の 1px が何画素か（fxkit の数値は 1920x1080 の画素なので、縮小したプレビューでは小さく）。
pub fn glitch(data: &mut [u8], w: u32, h: u32, ch: usize, amt: f64, unit: f64, seed: u64) {
    if amt < 0.03 || w == 0 || h == 0 {
        return;
    }
    let (w, h) = (w as usize, h as usize);
    let row = w * ch;
    let orig = data.to_vec();
    let roll = |dst: &mut [u8], src: &[u8], dx: i64| {
        // 1 行を dx 画素だけ右へ回す
        let n = w as i64;
        for x in 0..w {
            let sx = ((x as i64 - dx).rem_euclid(n)) as usize;
            dst[x * ch..x * ch + ch].copy_from_slice(&src[sx * ch..sx * ch + ch]);
        }
    };
    for i in 0..6u64 {
        if hrand(seed, i, 1) > 0.55 {
            continue;
        }
        let y0 = (hrand(seed, i, 2) * h as f64) as usize;
        let bh = (4.0 * unit + hrand(seed, i, 3) * h as f64 * 0.18).round().max(1.0) as usize;
        let dx = ((hrand(seed, i, 4) - 0.5) * 70.0 * amt * unit).round() as i64;
        if dx == 0 {
            continue;
        }
        for y in y0..(y0 + bh).min(h) {
            roll(&mut data[y * row..(y + 1) * row], &orig[y * row..(y + 1) * row], dx);
        }
    }
    // 色ずれ
    let s = (6.0 * amt * unit).round() as i64;
    if s > 0 {
        let cur = data.to_vec();
        for y in 0..h {
            for x in 0..w {
                let r = (x as i64 - s).clamp(0, w as i64 - 1) as usize;
                let b = (x as i64 + s).clamp(0, w as i64 - 1) as usize;
                data[y * row + x * ch] = cur[y * row + r * ch];
                data[y * row + x * ch + 2] = cur[y * row + b * ch + 2];
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clip(kind_in: &str, kind_out: &str) -> MediaClip {
        MediaClip {
            id: "m1".into(),
            src: "a.png".into(),
            start: 10.0,
            dur: 4.0,
            trans_in: Trans { kind: kind_in.into(), dur: 0.5, dir: String::new() },
            trans_out: Trans { kind: kind_out.into(), dur: 0.5, dir: "right".into() },
            ..Default::default()
        }
    }

    #[test]
    fn no_transition_is_identity() {
        let c = MediaClip { start: 0.0, dur: 3.0, ..Default::default() };
        let st = state(&c, 1.0);
        assert_eq!(st, TransState { alpha: 1.0, dx: 0.0, dy: 0.0, scale: 1.0, wipe: None, glitch: 0.0 });
    }

    #[test]
    fn fade_ramps_in_and_out() {
        let c = clip("fade", "fade");
        assert_eq!(state(&c, 10.0).alpha, 0.0);
        // 緩急は演出の Sprite と同じ ease_out
        assert!((state(&c, 10.25).alpha - 0.875).abs() < 1e-9);
        assert_eq!(state(&c, 12.0).alpha, 1.0);
        assert!((state(&c, 13.9).alpha - (1.0 - 0.8f64.powi(3))).abs() < 1e-9);
    }

    #[test]
    fn slide_comes_from_dir_and_leaves_toward_dir() {
        let c = clip("slide", "slide");
        let a = state(&c, 10.05);
        assert!(a.dx < -100.0 && a.dy == 0.0); // 入りは左（既定）から
        assert_eq!(state(&c, 12.0).dx, 0.0);
        assert!(state(&c, 13.95).dx > 100.0); // 抜けは右へ
    }

    #[test]
    fn pop_slam_scale() {
        assert!(state(&clip("pop", ""), 10.02).scale < 0.9);
        assert!(state(&clip("slam", ""), 10.02).scale > 1.5);
        assert_eq!(state(&clip("slam", ""), 11.0).scale, 1.0);
    }

    #[test]
    fn wipe_and_glitch() {
        let c = clip("wipe", "glitch");
        let (k, d) = state(&c, 10.25).wipe.unwrap();
        assert!((k - 0.5).abs() < 1e-9 && d == Dir::Left);
        assert!(state(&c, 11.0).wipe.is_none());
        assert!(state(&c, 13.95).glitch > 0.5);
        assert_eq!(state(&c, 12.0).glitch, 0.0);
    }

    #[test]
    fn transition_never_longer_than_clip() {
        let mut c = clip("fade", "");
        c.dur = 0.2;
        assert_eq!(trans_dur(&c.trans_in, c.dur), 0.2);
        c.trans_in.dur = 0.0;
        c.dur = 5.0;
        assert_eq!(trans_dur(&c.trans_in, c.dur), DEFAULT_DUR);
    }

    #[test]
    fn glitch_is_deterministic() {
        let base: Vec<u8> = (0..64 * 32 * 4).map(|i| (i * 7 % 251) as u8).collect();
        let (mut a, mut b) = (base.clone(), base.clone());
        glitch(&mut a, 64, 32, 4, 1.0, 1.0, seed("m1", 5));
        glitch(&mut b, 64, 32, 4, 1.0, 1.0, seed("m1", 5));
        assert_eq!(a, b);
        assert_ne!(a, base);
        let mut c = base.clone();
        glitch(&mut c, 64, 32, 4, 0.0, 1.0, seed("m1", 5));
        assert_eq!(c, base);
    }

    fn fixture(name: &str) -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(name)
    }

    /// 見本と一字一句同じか（AK_UPDATE_FIXTURES=1 で作り直す）。fxkit/test_motion.py も同じ見本を読む
    fn check_fixture(name: &str, v: &serde_json::Value) {
        let text = serde_json::to_string_pretty(v).unwrap();
        if std::env::var("AK_UPDATE_FIXTURES").is_ok() {
            std::fs::write(fixture(name), &text).unwrap();
        }
        let read = std::fs::read_to_string(fixture(name)).unwrap().replace("

", "
");
        assert_eq!(read, text, "{name} が今の式と違います（式を変えたなら AK_UPDATE_FIXTURES=1 で作り直し、fxkit/test_motion.py も通す）");
    }

    #[test]
    fn curves_match_fixture() {
        let ks: Vec<f64> = (0..=20).map(|i| i as f64 * 0.05).collect();
        let mut curves = serde_json::Map::new();
        for kind in ["fade", "slide", "pop", "slam", "wipe", "glitch"] {
            let rows: Vec<Curve> = ks.iter().map(|&k| curve(kind, k)).collect();
            curves.insert(kind.to_string(), serde_json::to_value(rows).unwrap());
        }
        check_fixture("trans_curves.json", &serde_json::json!({"ks": ks, "curves": curves}));
        assert_eq!(curve("fade", 1.0), Curve::IDENTITY);
        assert_eq!(curve("nope", 0.3), Curve::IDENTITY);
    }

    /// 乱数の種と、グリッチを掛けた画素（48x24 の RGBA。バイト i の値は (i * 7) % 251）
    #[test]
    fn glitch_matches_fixture() {
        use base64::Engine;
        let b64 = base64::engine::general_purpose::STANDARD;
        let seeds: Vec<_> = [("m1", 5i64), ("clip-b", 17), ("", 0), ("グリッチ", 123456)]
            .iter()
            .map(|&(id, f)| serde_json::json!({"id": id, "frame": f, "seed": seed(id, f)}))
            .collect();
        let (w, h, ch) = (48u32, 24u32, 4usize);
        let base: Vec<u8> = (0..w as usize * h as usize * ch).map(|i| (i * 7 % 251) as u8).collect();
        let cases: Vec<_> = [(1.0, 1.0, ("m1", 5i64)), (0.5, 1.0, ("m1", 5)), (0.1, 1.0, ("clip-b", 17)), (0.02, 1.0, ("m1", 5)), (1.0, 0.5, ("グリッチ", 123456))]
            .iter()
            .map(|&(amt, unit, (id, f))| {
                let sd = seed(id, f);
                let mut d = base.clone();
                glitch(&mut d, w, h, ch, amt, unit, sd);
                serde_json::json!({"amt": amt, "unit": unit, "seed": sd, "out": b64.encode(&d)})
            })
            .collect();
        assert_eq!(b64.decode(cases[3]["out"].as_str().unwrap()).unwrap(), base, "amt 0.02 は変化なし");
        assert_ne!(b64.decode(cases[0]["out"].as_str().unwrap()).unwrap(), base);
        check_fixture("glitch.json", &serde_json::json!({"w": w, "h": h, "ch": ch, "seeds": seeds, "cases": cases}));
    }
}
