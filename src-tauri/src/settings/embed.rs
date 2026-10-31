//! アプリに埋め込むファイル（Python ワーカー・fxkit・声の中継）の書き出しと、読み上げ辞書の場所。

use super::{data_dir, ensure_dict_file, write_atomic};
use crate::app::error::reason;
use std::path::{Path, PathBuf};

/// 埋め込んだ Python のワーカーを書き出して、その場所を返す。
pub fn engine_script() -> PathBuf {
    let dir = data_dir().join("engine");
    let _ = std::fs::create_dir_all(&dir);
    let p = dir.join("akfx_worker.py");
    let src = include_str!("../../../engine/akfx_worker.py");
    if std::fs::read_to_string(&p).map(|t| t != src).unwrap_or(true) {
        let _ = std::fs::write(&p, src);
    }
    p
}

// アプリに埋め込む fxkit の表（`FXKIT_FILES`）。build.rs が OUT_DIR に作る（旧#169）。
// 公開の `fxkit/` と、あれば非公開の kit（`AK_FX_PRIVATE` か `fx-private/kit/`）を平らに並べる。
// examples・pyproject.toml・README・test_*.py・キャッシュは入れない（除外の規則は `embed_table.rs`）
include!(concat!(env!("OUT_DIR"), "/fxkit_files.rs"));

/// 埋め込みの表を dir に書き出す。中身が同じファイルは書かない。書いたファイルの数を返す
pub(crate) fn write_embedded(dir: &Path, files: &[(&str, &[u8])]) -> std::io::Result<usize> {
    let mut n = 0;
    for (rel, data) in files {
        let p = dir.join(rel);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent)?;
        }
        if std::fs::read(&p).map(|t| t != *data).unwrap_or(true) {
            write_atomic(&p, data)?;
            n += 1;
        }
    }
    // 表から外した .py（旧#98 の fxcli.py など）を消す。古い版が残っていると import できてしまい、分かりにくい失敗になる。
    // アプリが持つフォルダのいちばん上の .py だけを見る
    let keep: std::collections::HashSet<&str> = files.iter().map(|(rel, _)| *rel).collect();
    for e in std::fs::read_dir(dir)?.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        if name.ends_with(".py") && !keep.contains(name.as_str()) && e.path().is_file() {
            std::fs::remove_file(e.path())?;
        }
    }
    Ok(n)
}

/// 埋め込んだ fxkit を data_dir()/fxkit/ に書き出して、その場所を返す（起動中に 1 回だけ）。
/// ワーカーが回の fx/ にも pip で入れた版にも fxkit を見つけられないときの最後の手段（AK_FXKIT_FALLBACK）。
/// engine/ の下に置かないこと（engine/ はワーカーの sys.path[0] なので、そこの fxkit/ フォルダが名前空間パッケージとして見えてしまう）
pub fn fxkit_dir() -> Result<PathBuf, String> {
    // 成功だけ覚える（失敗は覚えず、次の呼び出しでやり直す）
    static DIR: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    if let Some(d) = DIR.get() {
        return Ok(d.clone());
    }
    let dir = data_dir().join("fxkit");
    write_embedded(&dir, FXKIT_FILES)
        .map_err(|e| format!("埋め込みの fxkit を {} に書き出せませんでした: {}", dir.display(), reason(e)))?;
    Ok(DIR.get_or_init(|| dir).clone())
}

/// アプリに埋め込む声の中継（`voice/` の .py・.json・.html。test_*.py は入れない）。
/// 足し忘れ・消し忘れは `voice_table_matches_dir` のテストで落ちる
pub(crate) const VOICE_FILES: &[(&str, &[u8])] = &[
    ("accent_review.html", include_bytes!("../../../voice/accent_review.html")),
    ("akscript.py", include_bytes!("../../../voice/akscript.py")),
    ("voice_report.py", include_bytes!("../../../voice/voice_report.py")),
    ("voice_review.py", include_bytes!("../../../voice/voice_review.py")),
    ("voice_server.py", include_bytes!("../../../voice/voice_server.py")),
    ("voisona_presets.json", include_bytes!("../../../voice/voisona_presets.json")),
    ("voisona_tts.py", include_bytes!("../../../voice/voisona_tts.py")),
];

/// 書き出す声の中継の表。プリセットに声の値（voisona_presets.json）があれば、素の既定の代わりにそれを入れる（旧#173）
pub(crate) fn voice_files<'a>(p: &'a super::Presets) -> Vec<(&'a str, &'a [u8])> {
    VOICE_FILES
        .iter()
        .map(|(rel, data)| match (*rel, p.voice.as_deref()) {
            ("voisona_presets.json", Some(v)) => (*rel, v),
            _ => (*rel, *data),
        })
        .collect()
}

/// 埋め込んだ声の中継を data_dir()/voice/ に書き出して、その場所を返す（起動中に 1 回だけ）。
/// engine/ の下には置かない（engine/ はワーカーの sys.path[0] なので、中の .py が import できてしまう）
pub fn voice_dir() -> Result<PathBuf, String> {
    // 成功だけ覚える（失敗は覚えず、次の呼び出しでやり直す）
    static DIR: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    if let Some(d) = DIR.get() {
        return Ok(d.clone());
    }
    let dir = data_dir().join("voice");
    write_embedded(&dir, &voice_files(super::presets()))
        .map_err(|e| format!("埋め込みの声の中継を {} に書き出せませんでした: {}", dir.display(), reason(e)))?;
    Ok(DIR.get_or_init(|| dir).clone())
}

/// 読み上げ辞書の場所（全部の回で共通）
pub fn voice_dict_path() -> PathBuf {
    data_dir().join("voice_dict.json")
}

/// 声の中継を起動する前の最後の確認: 辞書が無ければ空の辞書を作る（起動時に作れていなかった場合。作れなければエラーで返す）
pub fn ensure_voice_dict() -> Result<PathBuf, String> {
    let new = voice_dict_path();
    ensure_dict_file(&new)?;
    Ok(new)
}

/// 動いている中継が報告した辞書の場所が、AK Studio の辞書と同じか
/// （区切りの違い・末尾の区切り・`\\?\` の接頭辞を揃え、Windows では大文字小文字を見ない）
pub(crate) fn dict_path_matches(reported: Option<&str>, expected: &Path) -> bool {
    fn norm(s: &str) -> String {
        let s = s.trim().replace('\\', "/");
        let s = s.strip_prefix("//?/").unwrap_or(&s).trim_end_matches('/').to_string();
        if cfg!(windows) { s.to_lowercase() } else { s }
    }
    match reported {
        Some(r) if !r.trim().is_empty() => norm(r) == norm(&expected.to_string_lossy()),
        _ => false,
    }
}

#[cfg(test)]
mod fxkit_embed_tests {
    use super::*;
    use std::collections::BTreeSet;

    /// 埋め込みの表 = 公開の fxkit/（と非公開の kit）を関数で歩いた結果。中身も同じ
    #[test]
    fn fxkit_table_matches_dir() {
        let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
        let public = manifest.join("..").join("fxkit");
        let private = match std::env::var_os("AK_FX_PRIVATE").filter(|v| !v.is_empty()) {
            Some(v) => PathBuf::from(v),
            None => manifest.join("..").join("fx-private").join("kit"),
        };
        let mut roots: Vec<&Path> = vec![&public];
        if private.is_dir() {
            roots.push(&private);
        }
        let expect = crate::settings::embed_table::merge(&roots).unwrap();
        let table: BTreeSet<&str> = FXKIT_FILES.iter().map(|(p, _)| *p).collect();
        assert_eq!(table.len(), FXKIT_FILES.len(), "表に同じパスが 2 回あります");
        let on_disk: BTreeSet<&str> = expect.iter().map(|(p, _)| p.as_str()).collect();
        assert_eq!(table, on_disk, "埋め込みの表とフォルダのファイルがずれています（cargo build をやり直してください）");
        for (p, abs) in &expect {
            let data = FXKIT_FILES.iter().find(|(r, _)| r == p).unwrap().1;
            assert_eq!(&std::fs::read(abs).unwrap()[..], data, "{p}");
        }
        // 公開の芯は必ず入っている
        assert!(table.contains("fxcore.py"));
    }

    /// 書き出しは中身が同じなら書かない。変わったファイルだけ書き直す
    #[test]
    fn write_embedded_skips_unchanged() {
        let dir = std::env::temp_dir().join(format!("ak-fxkit-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let files: &[(&str, &[u8])] = &[("a.py", b"print(1)\n"), ("fonts/b.txt", b"b")];
        assert_eq!(write_embedded(&dir, files).unwrap(), 2);
        assert_eq!(std::fs::read(dir.join("fonts/b.txt")).unwrap(), b"b");
        let t0 = std::fs::metadata(dir.join("a.py")).unwrap().modified().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(30));
        assert_eq!(write_embedded(&dir, files).unwrap(), 0, "同じ中身は書き直さない");
        assert_eq!(std::fs::metadata(dir.join("a.py")).unwrap().modified().unwrap(), t0);
        let changed: &[(&str, &[u8])] = &[("a.py", b"print(2)\n"), ("fonts/b.txt", b"b")];
        assert_eq!(write_embedded(&dir, changed).unwrap(), 1);
        assert_eq!(std::fs::read(dir.join("a.py")).unwrap(), b"print(2)\n");
        // 一時ファイルが残らない
        let left: Vec<_> = std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().to_string()).collect();
        assert!(left.iter().all(|n| !n.contains(".tmp-")), "{left:?}");
        // 表から外した .py は消す（旧#98 の fxcli.py）。.py でない物は残す
        std::fs::write(dir.join("fxcli.py"), b"old").unwrap();
        std::fs::write(dir.join("note.txt"), b"x").unwrap();
        write_embedded(&dir, changed).unwrap();
        assert!(!dir.join("fxcli.py").exists());
        assert!(dir.join("note.txt").exists() && dir.join("a.py").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod voice_tests {
    use super::*;
    use std::collections::BTreeSet;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("ak-voice-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// 埋め込みの表と voice/ のファイルが一致する（足し忘れ・消し忘れで落ちる）。test_*.py・キャッシュは入れない
    #[test]
    fn voice_table_matches_dir() {
        let base = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("voice");
        let mut on_disk = BTreeSet::new();
        for e in std::fs::read_dir(&base).unwrap() {
            let e = e.unwrap();
            let name = e.file_name().to_string_lossy().to_string();
            if e.path().is_dir() || name.starts_with("test_") || name.starts_with("README") {
                continue;
            }
            on_disk.insert(name);
        }
        let table: BTreeSet<String> = VOICE_FILES.iter().map(|(p, _)| p.to_string()).collect();
        assert_eq!(table.len(), VOICE_FILES.len(), "表に同じパスが 2 回あります");
        assert_eq!(table, on_disk, "埋め込みの表（settings.rs の VOICE_FILES）と voice/ のファイルがずれています");
        for (p, data) in VOICE_FILES {
            assert_eq!(&std::fs::read(base.join(p)).unwrap()[..], *data, "{p}");
        }
        // ログイン情報の入った物・捨てた物は入れない
        for bad in ["start_editor.bat", "script_editor.html", "script2ccs.py", "voice_dict.json"] {
            assert!(!table.contains(bad), "{bad}");
        }
    }

    /// プリセットがあれば声の値はそちら、無ければ素の既定（voice/ のファイル）
    #[test]
    fn voice_files_prefers_preset() {
        let none = super::super::presets::load(&[]);
        let t = voice_files(&none);
        let base = VOICE_FILES.iter().find(|(r, _)| *r == "voisona_presets.json").unwrap().1;
        assert_eq!(t.iter().find(|(r, _)| *r == "voisona_presets.json").unwrap().1, base);
        assert_eq!(t.len(), VOICE_FILES.len());
        let some = super::super::presets::load(&[("voisona_presets.json", br#"{"speakers":{}}"#)]);
        let t = voice_files(&some);
        assert_eq!(t.iter().find(|(r, _)| *r == "voisona_presets.json").unwrap().1, br#"{"speakers":{}}"#);
        assert_eq!(t.len(), VOICE_FILES.len());
    }

    /// 今の手元の声の値（presets/）は 旧#173 の前と同じ（ささら・つづみ）。公開側の素の既定は同じ声 1 人で、速さなどは素の値
    #[test]
    fn voice_preset_and_plain_values() {
        let base = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        // 公開側の clone（プリセットを非公開に動かしたあと）には presets/ が無いので飛ばす
        if !base.join("presets/voisona_presets.json").is_file() {
            return;
        }
        let read = |p: &str| serde_json::from_str::<serde_json::Value>(&std::fs::read_to_string(base.join(p)).unwrap()).unwrap();
        let (pre, plain) = (read("presets/voisona_presets.json"), read("voice/voisona_presets.json"));
        assert_eq!(pre["speakers"]["トーク1"]["voice_name"], "sato-sasara_ja_JP");
        assert_eq!(pre["speakers"]["トーク2"]["voice_name"], "suzuki-tsudumi_ja_JP");
        assert_eq!(pre["speakers"]["トーク1"]["global_parameters"]["speed"], 1.29);
        assert_eq!(pre["speakers"]["トーク2"]["global_parameters"]["pitch"], 108);
        assert_eq!(pre["line_gap"], 0.3);
        assert_eq!(plain["speakers"]["トーク1"]["voice_name"], plain["speakers"]["トーク2"]["voice_name"]);
        assert_eq!(plain["speakers"]["トーク2"]["global_parameters"]["speed"], 1.0);
        assert!(super::super::presets::load(&[("voisona_presets.json", &std::fs::read(base.join("presets/voisona_presets.json")).unwrap())]).warnings.is_empty());
    }

    /// 動いている中継の辞書の場所の比べ方
    #[test]
    fn dict_path_comparison() {
        let exp = Path::new("C:\\Users\\a\\AppData\\Local\\ak-studio\\voice_dict.json");
        assert!(dict_path_matches(Some("C:\\Users\\a\\AppData\\Local\\ak-studio\\voice_dict.json"), exp));
        assert!(dict_path_matches(Some("C:/Users/a/AppData/Local/ak-studio/voice_dict.json"), exp));
        assert!(dict_path_matches(Some(" \\\\?\\C:\\Users\\a\\AppData\\Local\\ak-studio\\voice_dict.json "), exp));
        if cfg!(windows) {
            assert!(dict_path_matches(Some("c:/users/A/appdata/local/AK-Studio/voice_dict.json"), exp));
        }
        assert!(!dict_path_matches(Some("D:/old/script_editor/voice_dict.json"), exp));
        assert!(!dict_path_matches(Some(""), exp));
        assert!(!dict_path_matches(None, exp), "dict を報告しない古い中継は不一致");
    }
}
