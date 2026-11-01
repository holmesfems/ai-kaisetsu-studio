//! 本体の HTTP API の接続先（ポートと合言葉）を知らせるファイルの場所。本体が書き、MCP サーバーと Mod が読む。
//!
//! `%LOCALAPPDATA%` には置かない。Claude のデスクトップアプリ（MSIX）の中で動くプロセスからは、
//! `%LOCALAPPDATA%` に `Packages\Claude_…\LocalCache\Local` の写しが重なって見える。アプリの中で開発版を
//! 起動したときの古い session.json が写しに残り、MCP が古い合言葉で 401 になる。ホームのフォルダは重ならない。

use std::path::PathBuf;

/// `%USERPROFILE%\.ak-studio\session.json`（Windows 以外は `$HOME`）。ホームが分からなければ None
pub fn session_file() -> Option<PathBuf> {
    session_file_in(std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")).map(PathBuf::from))
}

fn session_file_in(home: Option<PathBuf>) -> Option<PathBuf> {
    let h = home.filter(|h| !h.as_os_str().is_empty())?;
    Some(h.join(".ak-studio").join("session.json"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_file_is_under_home_not_appdata() {
        let p = session_file_in(Some(PathBuf::from(r"C:\Users\a"))).unwrap();
        assert_eq!(p, PathBuf::from(r"C:\Users\a").join(".ak-studio").join("session.json"));
        assert_eq!(session_file_in(None), None);
        assert_eq!(session_file_in(Some(PathBuf::new())), None);
    }
}
