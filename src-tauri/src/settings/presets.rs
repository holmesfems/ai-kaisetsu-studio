//! 見た目・声・背景の「プリセット」（旧#173）。今の動画の色・形・声の値は公開のコードに書かず、
//! ビルド時に `presets/` のフォルダ（`AK_FX_PRIVATE` の親の `presets/` → `../fx-private/presets` → リポジトリ直下の `presets/`）を
//! 埋め込んで、あれば既定として使う。無ければ素の既定。
//!
//! 優先の順は「回の設定（project.json など） → ユーザーの設定（settings.json） → プリセット → 素の既定」。
//! プリセットのファイル:
//! - `subtitle.json` … 字幕の見た目（`SubStyle` の項目。一部だけでもよい）
//! - `project.json` … `{"bg_loop": "背景ループの mp4 のパス", "template_dir": "ひな形のフォルダ"}`
//! - `voisona_presets.json` … 声の値（voice/voisona_presets.json と同じ形。あれば voice/ の素の既定の代わりに書き出す）
//!
//! 読めないプリセットは捨てて素の既定で動き、理由を `warnings` に残す（起動時の知らせに出す）。

use serde_json::{Map, Value};

// 埋め込みの表（`PRESET_FILES`）。build.rs が OUT_DIR に作る
include!(concat!(env!("OUT_DIR"), "/preset_files.rs"));

#[derive(Debug, Default, Clone)]
pub struct Presets {
    /// 字幕の見た目（素の既定に重ねる項目。見た目の項目だけの Map）
    pub subtitle: Option<Map<String, Value>>,
    /// 背景ループの場所
    pub bg_loop: Option<String>,
    /// ひな形のフォルダ（回の一覧に出さない。設定の template_dir が空のときの既定）
    pub template_dir: Option<String>,
    /// 声の値（検証済みの JSON のバイト列）
    pub voice: Option<Vec<u8>>,
    /// 読めなかったプリセットの理由
    pub warnings: Vec<String>,
}

fn get<'a>(files: &'a [(&str, &[u8])], name: &str) -> Option<&'a [u8]> {
    files.iter().find(|(p, _)| *p == name).map(|(_, d)| *d)
}

fn parse(name: &str, data: &[u8], warnings: &mut Vec<String>) -> Option<Value> {
    match serde_json::from_slice::<Value>(data.strip_prefix(b"\xef\xbb\xbf").unwrap_or(data)) {
        Ok(v) => Some(v),
        Err(e) => {
            warnings.push(format!("プリセット {name} を読めませんでした: {e}。素の既定で動きます"));
            None
        }
    }
}

/// 表（埋め込みのものか、テストで渡したもの）からプリセットを読む。読めないものは warnings に理由を残して無いことにする
pub fn load(files: &[(&str, &[u8])]) -> Presets {
    let mut p = Presets::default();
    if let Some(d) = get(files, "subtitle.json") {
        match parse("subtitle.json", d, &mut p.warnings) {
            Some(Value::Object(m)) => {
                // 素の既定に重ねて SubStyle として読めるか確かめる（型違いの項目があれば丸ごと捨てる）
                match crate::render::subs::SubStyle::from_preset(&m) {
                    Ok(_) => p.subtitle = Some(m),
                    Err(e) => p.warnings.push(format!("プリセット subtitle.json の中身が字幕の見た目として読めませんでした: {e}。素の既定で動きます")),
                }
            }
            Some(_) => p.warnings.push("プリセット subtitle.json が { … } の形ではありません。素の既定で動きます".into()),
            None => {}
        }
    }
    if let Some(d) = get(files, "project.json") {
        if let Some(v) = parse("project.json", d, &mut p.warnings) {
            for (key, slot) in [("bg_loop", &mut p.bg_loop), ("template_dir", &mut p.template_dir)] {
                match v.get(key) {
                    None | Some(Value::Null) => {}
                    Some(Value::String(s)) if !s.trim().is_empty() => *slot = Some(s.trim().to_string()),
                    Some(_) => p.warnings.push(format!("プリセット project.json の {key} が文字列ではありません。無いものとして動きます")),
                }
            }
        }
    }
    if let Some(d) = get(files, "voisona_presets.json") {
        match parse("voisona_presets.json", d, &mut p.warnings) {
            Some(v) if v.get("speakers").is_some_and(|s| s.is_object()) => p.voice = Some(d.to_vec()),
            Some(_) => p.warnings.push("プリセット voisona_presets.json に speakers がありません。素の既定の声で動きます".into()),
            None => {}
        }
    }
    p
}

/// ビルドに埋め込んだプリセット（起動中は 1 回だけ読む）
pub fn presets() -> &'static Presets {
    static P: std::sync::OnceLock<Presets> = std::sync::OnceLock::new();
    P.get_or_init(|| load(PRESET_FILES))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 無ければ素の既定() {
        let p = load(&[]);
        assert!(p.subtitle.is_none() && p.bg_loop.is_none() && p.voice.is_none() && p.warnings.is_empty());
    }

    #[test]
    fn 読めないプリセットは警告にして捨てる() {
        let p = load(&[
            ("subtitle.json", "{ぶっこわれ".as_bytes()),
            ("project.json", br#"{"bg_loop": 5, "template_dir": 1}"#),
            ("voisona_presets.json", br#"{"line_gap": 1}"#),
        ]);
        assert!(p.subtitle.is_none() && p.bg_loop.is_none() && p.voice.is_none());
        assert_eq!(p.warnings.len(), 4, "{:?}", p.warnings);
        assert!(p.warnings[0].contains("subtitle.json"), "{:?}", p.warnings);
        // 型違いの項目
        let p = load(&[("subtitle.json", "{\"size\": \"でかい\"}".as_bytes())]);
        assert!(p.subtitle.is_none() && p.warnings.len() == 1, "{:?}", p.warnings);
        // 配列
        let p = load(&[("subtitle.json", b"[]")]);
        assert!(p.subtitle.is_none() && p.warnings.len() == 1);
    }

    #[test]
    fn 読めるプリセットを使う() {
        let p = load(&[
            ("subtitle.json", br##"{"stroke": "#123456"}"##),
            ("project.json", br#"{"bg_loop": " D:/x/bg.mp4 ", "template_dir": "D:/tpl"}"#),
            ("voisona_presets.json", br#"{"speakers": {}}"#),
        ]);
        assert!(p.warnings.is_empty(), "{:?}", p.warnings);
        assert_eq!(p.subtitle.unwrap()["stroke"], "#123456");
        assert_eq!(p.bg_loop.as_deref(), Some("D:/x/bg.mp4"));
        assert_eq!(p.template_dir.as_deref(), Some("D:/tpl"));
        assert!(p.voice.is_some());
    }

    /// 埋め込んだプリセットが（あれば）読める。警告が出ないこと
    #[test]
    fn 埋め込みのプリセットは読める() {
        let p = load(PRESET_FILES);
        assert!(p.warnings.is_empty(), "{:?}", p.warnings);
    }
}
