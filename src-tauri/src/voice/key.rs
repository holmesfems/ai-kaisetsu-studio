//! 読み上げ・字幕の文と、声の分析・音声キャッシュの鍵（台本エディタと同じ決まり）。

use regex::Regex;
use std::sync::OnceLock;

fn ruby() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"｜([^｜《》]+)《([^《》]+)》").unwrap())
}

/// 読み上げる文（ルビは読み）
pub fn tts_text(t: &str) -> String {
    ruby().replace_all(t, "$2").to_string()
}

/// 字幕の文（ルビは表記）
pub fn sub_text(t: &str) -> String {
    ruby().replace_all(t, "$1").to_string()
}

/// 読み上げる文を前後の空白（char::is_whitespace）で削ったもの。手直しの `text`・声の鍵・「読み上げる行か」の判定の正。
/// 画面（ui/src/logic/voice.ts の ttsKeyText）・Python（voice/akscript.py の tts_text）も同じ結果になるように合わせてある
pub fn tts_key_text(t: &str) -> String {
    tts_text(t).trim().to_string()
}

/// 声の分析・キャッシュの鍵と、古い形（.md 時代）の手直しの鍵。内容（話者・文）で決まる
pub fn vkey(sp: &str, text: &str) -> String {
    format!("{}\u{1}{}", sp, tts_key_text(text))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ruby_and_key() {
        assert_eq!(tts_text("どうも｜A《えー》"), "どうもえー");
        assert_eq!(sub_text("どうも｜A《えー》"), "どうもA");
        assert_eq!(vkey("トーク1", " どうも｜A《えー》 "), "トーク1\u{1}どうもえー");
    }

    /// 3 言語（Rust・TS・Python）が同じ表を読む。期待値は Rust の挙動
    #[test]
    fn shared_table() {
        let cases: Vec<serde_json::Value> = serde_json::from_str(include_str!("../../tests/fixtures/voice_keys.json")).unwrap();
        assert!(cases.len() >= 20);
        for c in &cases {
            let (name, sp, text) = (c["name"].as_str().unwrap(), c["sp"].as_str().unwrap(), c["text"].as_str().unwrap());
            assert_eq!(tts_key_text(text), c["tts"].as_str().unwrap(), "tts: {name}");
            assert_eq!(sub_text(text), c["sub"].as_str().unwrap(), "sub: {name}");
            assert_eq!(vkey(sp, text), c["key"].as_str().unwrap(), "key: {name}");
        }
    }
}
