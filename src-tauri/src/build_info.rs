//! このアプリをビルドしたときのコミットと時刻（build.rs が埋め込む）。どの修正が入った版かを画面で分かるようにする（旧#163）。

/// 画面とウィンドウのタイトルに出す文（例 `eb13fe9（10/30 21:46）`。コミットしていない変更が入っていれば `eb13fe9+`）
pub fn label() -> String {
    format_label(env!("AK_BUILD_COMMIT"), !env!("AK_BUILD_DIRTY").is_empty(), env!("AK_BUILD_UNIX").parse().unwrap_or(0))
}

/// commit が空（git が無いところでビルドした）なら「版不明」。時刻はこの PC の時刻で月/日 時:分
pub fn format_label(commit: &str, dirty: bool, unix: i64) -> String {
    let commit = if commit.is_empty() { "版不明".to_string() } else { format!("{commit}{}", if dirty { "+" } else { "" }) };
    match chrono::DateTime::from_timestamp(unix, 0).filter(|_| unix > 0) {
        Some(t) => format!("{commit}（{}）", t.with_timezone(&chrono::Local).format("%m/%d %H:%M")),
        None => commit,
    }
}

/// 演出の kit の表示（例 `kit: private 1a2b3c (API 1)`。素の kit は `kit: 素`）。
/// commit は非公開の kit をビルドしたときのコミット（build.rs が埋め込む。無ければ空）で、素の kit には添えない
pub fn kit_label(kit: &crate::render::engine::KitInfo, commit: &str) -> String {
    if kit.name == "basic" {
        return "kit: 素".to_string();
    }
    let mut s = format!("kit: {}", kit.name);
    if !commit.is_empty() {
        s.push(' ');
        s.push_str(commit);
    }
    if let Some(api) = kit.api {
        s.push_str(&format!(" (API {api})"));
    }
    s
}

/// この版に埋め込んだ非公開の kit のコミット（無ければ空）
pub fn kit_commit() -> &'static str {
    env!("AK_BUILD_KIT_COMMIT")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kit_の表示() {
        use crate::render::engine::KitInfo;
        let k = |name: &str, api| KitInfo { name: name.into(), api, features: vec![], path: None };
        assert_eq!(kit_label(&k("private", Some(1)), ""), "kit: private (API 1)");
        assert_eq!(kit_label(&k("private", Some(1)), "1a2b3c"), "kit: private 1a2b3c (API 1)");
        assert_eq!(kit_label(&k("private", None), "1a2b3c"), "kit: private 1a2b3c");
        assert_eq!(kit_label(&k("basic", Some(1)), "1a2b3c"), "kit: 素");
    }

    #[test]
    fn コミットと時刻の文() {
        let s = format_label("eb13fe9", false, 1_793_364_360);
        assert!(s.starts_with("eb13fe9（") && s.ends_with("）"), "{s}");
        assert!(format_label("eb13fe9", true, 1_793_364_360).starts_with("eb13fe9+（"));
        assert_eq!(format_label("", false, 0), "版不明");
        assert_eq!(format_label("abc", true, 0), "abc+");
        // 埋め込んだ値で作れる
        assert!(!label().is_empty());
    }
}
