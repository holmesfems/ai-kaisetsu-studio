//! cue ごとのコマを並べた確認画像（`render_sheet`）の、並べ方の計算。コマを描く所は `core/compose.rs` の `still_sheet`。
//! 1 ページは 1920x1080 に 4x4 の 16 マ（1 マ 480x270）。16 を超えたら次のページ。

use crate::app::error::{bad_input, not_found};
use ak_proto::Cue;
use anyhow::Result;
use std::ops::Range;

pub const COLS: usize = 4;
pub const PER_PAGE: usize = 16;
pub const CELL_W: u32 = 480;
pub const CELL_H: u32 = 270;
pub const PAGE_W: u32 = CELL_W * COLS as u32;
pub const PAGE_H: u32 = CELL_H * (PER_PAGE / COLS) as u32;
/// 1 回に並べられるコマの数（描くのに時間がかかるので、広すぎる頼みは断る）
pub const MAX_CELLS: usize = 160;

/// cue の見どころの時刻: cue の頭から、長さの 60% か 2.2 秒の早い方
pub fn highlight_time(start: f64, end: f64) -> f64 {
    start + 2.2f64.min((end - start) * 0.6)
}

/// 時刻 t を含む cue の番号（セリフの末尾から次の cue までの間は None）
pub fn cue_at(cues: &[Cue], t: f64) -> Option<i64> {
    cues.iter().find(|c| c.start <= t && t < c.end).map(|c| c.n)
}

/// 並べるコマ（cue 番号, 時刻）を決める。`cue_range` は [最初, 最後]、`ts` は秒の並び。どちらか一方だけ
pub fn plan(cues: &[Cue], cue_range: Option<&[i64]>, ts: Option<&[f64]>) -> Result<Vec<(Option<i64>, f64)>> {
    let out: Vec<(Option<i64>, f64)> = match (cue_range, ts) {
        (Some(_), Some(_)) => return Err(bad_input(format!("cues と ts は、どちらか一方だけ渡してください"))),
        (None, None) => return Err(bad_input(format!("cues（[最初, 最後]）か ts（秒の並び）を渡してください"))),
        (Some(r), None) => {
            let [a, b] = r else {
                return Err(bad_input(format!("cues は [最初, 最後] の 2 つで渡してください（今は {} 個）", r.len())));
            };
            if a > b {
                return Err(bad_input(format!("cues は [最初, 最後] の順で渡してください（{a} > {b}）")));
            }
            let v: Vec<_> = cues
                .iter()
                .filter(|c| (*a..=*b).contains(&c.n))
                .map(|c| (Some(c.n), highlight_time(c.start, c.end)))
                .collect();
            if v.is_empty() {
                return Err(not_found(format!("cue {a}〜{b} に当たるセリフがありません")));
            }
            v
        }
        (None, Some(ts)) => {
            if ts.is_empty() {
                return Err(bad_input(format!("ts が空です")));
            }
            if let Some(t) = ts.iter().find(|t| !t.is_finite() || **t < 0.0) {
                return Err(bad_input(format!("ts に使えない時刻があります: {t}")));
            }
            ts.iter().map(|&t| (cue_at(cues, t), t)).collect()
        }
    };
    if out.len() > MAX_CELLS {
        return Err(bad_input(format!("コマが多すぎます（{} 個。{MAX_CELLS} 個まで）。範囲を分けてください", out.len())));
    }
    Ok(out)
}

/// n 個のコマを 16 個ずつのページに分ける（各ページの範囲）
pub fn pages(n: usize) -> Vec<Range<usize>> {
    (0..n).step_by(PER_PAGE).map(|a| a..(a + PER_PAGE).min(n)).collect()
}

/// ページの中の i 番目（左上から右へ）のマの左上の位置
pub fn cell_origin(i: usize) -> (u32, u32) {
    ((i % COLS) as u32 * CELL_W, (i / COLS) as u32 * CELL_H)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cue(n: i64, start: f64, end: f64) -> Cue {
        Cue { n, start, end, text: String::new(), speaker: String::new() }
    }

    #[test]
    fn highlight_is_sixty_percent_or_2_2_sec() {
        assert!((highlight_time(10.0, 12.0) - 11.2).abs() < 1e-9); // 長さ 2 秒 → 1.2 秒
        assert!((highlight_time(10.0, 20.0) - 12.2).abs() < 1e-9); // 長さ 10 秒 → 2.2 秒
        assert!((highlight_time(5.0, 5.0) - 5.0).abs() < 1e-9);
    }

    #[test]
    fn plan_by_cue_range_and_ts() {
        let cs = [cue(1, 0.0, 2.0), cue(2, 2.5, 12.5), cue(3, 13.0, 14.0)];
        let p = plan(&cs, Some(&[2, 3]), None).unwrap();
        assert_eq!(p.len(), 2);
        assert_eq!(p[0].0, Some(2));
        assert!((p[0].1 - 4.7).abs() < 1e-9);
        assert!((p[1].1 - 13.6).abs() < 1e-9);
        // ts は cue を引く。cue の間は None
        let p = plan(&cs, None, Some(&[1.0, 2.2, 13.5])).unwrap();
        assert_eq!(p.iter().map(|x| x.0).collect::<Vec<_>>(), [Some(1), None, Some(3)]);
    }

    #[test]
    fn plan_rejects_bad_requests() {
        let cs = [cue(1, 0.0, 2.0)];
        assert!(plan(&cs, None, None).is_err());
        assert!(plan(&cs, Some(&[1, 2]), Some(&[1.0])).is_err());
        assert!(plan(&cs, Some(&[1]), None).is_err());
        assert!(plan(&cs, Some(&[3, 2]), None).is_err());
        assert!(plan(&cs, Some(&[5, 9]), None).is_err());
        assert!(plan(&cs, None, Some(&[])).is_err());
        assert!(plan(&cs, None, Some(&[f64::NAN])).is_err());
        assert!(plan(&cs, None, Some(&[-1.0])).is_err());
        assert!(plan(&cs, None, Some(&vec![1.0; MAX_CELLS + 1])).is_err());
        assert_eq!(plan(&cs, None, Some(&vec![1.0; MAX_CELLS])).unwrap().len(), MAX_CELLS);
    }

    #[test]
    fn pages_split_every_16() {
        assert!(pages(0).is_empty());
        assert_eq!(pages(1), [0..1]);
        assert_eq!(pages(16), [0..16]);
        assert_eq!(pages(17), [0..16, 16..17]);
        assert_eq!(pages(21), [0..16, 16..21]);
        assert_eq!(pages(32).len(), 2);
    }

    #[test]
    fn cells_are_laid_out_4x4_in_1920x1080() {
        assert_eq!((PAGE_W, PAGE_H), (1920, 1080));
        assert_eq!(cell_origin(0), (0, 0));
        assert_eq!(cell_origin(3), (1440, 0));
        assert_eq!(cell_origin(4), (0, 270));
        assert_eq!(cell_origin(15), (1440, 810));
    }
}
