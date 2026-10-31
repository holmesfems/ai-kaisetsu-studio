//! チャットに添える画像の保存（貼り付け・📎 で選んだファイル）。
//! Claude には Read で見てもらうので、大きすぎる画像は縮めてから <回>/studio/chat/ に置く。

use anyhow::{bail, Context, Result};
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// 長辺の上限（これを超えたら縮める）
pub const MAX_SIDE: u32 = 2560;
/// png がこれを超えたら jpg にする
pub const MAX_PNG_BYTES: usize = 5 * 1024 * 1024;
const JPEG_Q: u8 = 90;

static SEQ: AtomicU64 = AtomicU64::new(1);

/// 拡張子を小文字にそろえる（対応していなければ None）
pub fn norm_ext(ext: &str) -> Option<&'static str> {
    match ext.trim_start_matches('.').to_ascii_lowercase().as_str() {
        "png" => Some("png"),
        "jpg" | "jpeg" => Some("jpg"),
        "webp" => Some("webp"),
        "gif" => Some("gif"),
        "bmp" => Some("bmp"),
        _ => None,
    }
}

fn encode_jpeg(img: &image::DynamicImage) -> Result<Vec<u8>> {
    let rgb = img.to_rgb8();
    let mut out = vec![];
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, JPEG_Q).encode_image(&rgb)?;
    Ok(out)
}

fn encode_png(img: &image::DynamicImage) -> Result<Vec<u8>> {
    let mut out = vec![];
    img.write_to(&mut Cursor::new(&mut out), image::ImageFormat::Png)?;
    Ok(out)
}

/// 画像のバイト列を、送る形（バイト列と拡張子）にする。
/// 長辺 2560px 超なら縮め、png が 5MB を超えるなら jpg（品質 90）にする。どちらでもなければそのまま。
pub fn prepare(data: &[u8], ext: &str) -> Result<(Vec<u8>, &'static str)> {
    let Some(ext) = norm_ext(ext) else { bail!("対応していない画像の形式です（{ext}）") };
    let img = image::load_from_memory(data).context("画像を読めませんでした")?;
    let (w, h) = (img.width(), img.height());
    let big = w.max(h) > MAX_SIDE;
    if !big && !(ext == "png" && data.len() > MAX_PNG_BYTES) {
        return Ok((data.to_vec(), ext));
    }
    let img = if big { img.resize(MAX_SIDE, MAX_SIDE, image::imageops::FilterType::Triangle) } else { img };
    // 縮めたあとの形式: jpg は jpg のまま、それ以外は png（透明を残す）。png が大きければ jpg
    if ext == "jpg" {
        return Ok((encode_jpeg(&img)?, "jpg"));
    }
    let png = encode_png(&img)?;
    if png.len() > MAX_PNG_BYTES {
        Ok((encode_jpeg(&img)?, "jpg"))
    } else {
        Ok((png, "png"))
    }
}

/// dir（<回>/studio/chat）に img-<日時>-<連番>.<ext> で保存して、そのパスを返す。
pub fn save_image(dir: &Path, data: &[u8], ext: &str) -> Result<PathBuf> {
    let (bytes, ext) = prepare(data, ext)?;
    std::fs::create_dir_all(dir)?;
    let seq = SEQ.fetch_add(1, Ordering::SeqCst);
    let name = format!("img-{}-{seq}.{ext}", chrono::Local::now().format("%Y%m%d-%H%M%S"));
    let p = dir.join(name);
    std::fs::write(&p, bytes)?;
    Ok(p)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        let n = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
        std::env::temp_dir().join(format!("ak-studio-test-chatimg-{tag}-{}-{n}", std::process::id()))
    }

    fn png(w: u32, h: u32, noisy: bool) -> Vec<u8> {
        let mut x: u32 = 12345;
        let img = image::RgbImage::from_fn(w, h, |_, _| {
            if noisy {
                x = x.wrapping_mul(1664525).wrapping_add(1013904223);
                image::Rgb([(x >> 24) as u8, (x >> 16) as u8, (x >> 8) as u8])
            } else {
                image::Rgb([10, 20, 30])
            }
        });
        let mut out = vec![];
        image::DynamicImage::ImageRgb8(img).write_to(&mut Cursor::new(&mut out), image::ImageFormat::Png).unwrap();
        out
    }

    #[test]
    fn small_png_is_kept() {
        let src = png(64, 32, false);
        let (out, ext) = prepare(&src, "PNG").unwrap();
        assert_eq!(ext, "png");
        assert_eq!(out, src, "小さければそのまま");
    }

    #[test]
    fn long_side_is_shrunk_to_limit() {
        let (out, ext) = prepare(&png(3000, 1000, false), "png").unwrap();
        assert_eq!(ext, "png");
        let img = image::load_from_memory(&out).unwrap();
        assert_eq!((img.width(), img.height()), (2560, 853));
    }

    #[test]
    fn heavy_png_becomes_jpg() {
        let src = png(1500, 1500, true);
        assert!(src.len() > MAX_PNG_BYTES, "テスト用の画像が足りない: {}", src.len());
        let (out, ext) = prepare(&src, "png").unwrap();
        assert_eq!(ext, "jpg");
        assert_eq!(image::guess_format(&out).unwrap(), image::ImageFormat::Jpeg);
        assert!(out.len() < src.len());
    }

    #[test]
    fn bad_input_is_error() {
        assert!(prepare(b"not an image", "png").is_err());
        assert!(prepare(&png(8, 8, false), "txt").is_err());
        assert_eq!(norm_ext(".JPEG"), Some("jpg"));
    }

    #[test]
    fn save_names_are_unique_and_inside_dir() {
        let dir = tmp("save").join("studio").join("chat");
        let a = save_image(&dir, &png(8, 8, false), "png").unwrap();
        let b = save_image(&dir, &png(8, 8, false), "png").unwrap();
        assert_ne!(a, b);
        assert_eq!(a.parent().unwrap(), dir);
        let n = a.file_name().unwrap().to_string_lossy().to_string();
        assert!(n.starts_with("img-") && n.ends_with(".png"), "{n}");
        assert!(a.exists());
    }
}
