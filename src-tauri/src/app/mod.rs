//! 画面（Tauri のコマンド）と Claude（ローカル HTTP API）の入り口。

pub mod api;
pub mod commands;
pub mod error;
pub mod dto;

#[cfg(test)]
mod rules_fixture;

#[cfg(test)]
mod tests {
    use regex::Regex;
    use std::collections::BTreeSet;
    use std::path::{Path, PathBuf};

    fn manifest() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
    }

    fn read(p: &Path) -> String {
        std::fs::read_to_string(p).unwrap_or_else(|e| panic!("{} を読めません: {e}", p.display()))
    }

    /// ui/src/io/commands.ts の `export interface <name> {` の中の、行頭 2 スペースのキー
    fn ts_keys(name: &str) -> BTreeSet<String> {
        let ts = read(&manifest().join("../ui/src/io/commands.ts")).replace("\r\n", "\n");
        let head = format!("export interface {name} {{\n");
        let start = ts.find(&head).unwrap_or_else(|| panic!("commands.ts に {name} がありません")) + head.len();
        let body = &ts[start..];
        let body = &body[..body.find("\n}").expect("interface の終わりが見つかりません")];
        let re = Regex::new(r#"(?m)^  "?([A-Za-z_][A-Za-z0-9_-]*)"?\??:"#).unwrap();
        re.captures_iter(body).map(|c| c[1].to_string()).collect()
    }

    fn rs_files(dir: &Path, out: &mut Vec<PathBuf>) {
        for e in std::fs::read_dir(dir).unwrap().flatten() {
            let p = e.path();
            if p.is_dir() {
                rs_files(&p, out);
            } else if p.extension().is_some_and(|x| x == "rs") {
                out.push(p);
            }
        }
    }

    fn diff(a: &BTreeSet<String>, b: &BTreeSet<String>) -> Vec<String> {
        a.difference(b).cloned().collect()
    }

    /// lib.rs の generate_handler! に並べたコマンド ＝ commands.ts の Commands のキー
    #[test]
    fn commands_table_matches_generate_handler() {
        let lib = read(&manifest().join("src/lib.rs"));
        let start = lib.find("generate_handler![").expect("generate_handler! が見つかりません");
        let body = &lib[start..];
        let body = &body[..body.find(']').unwrap()];
        let re = Regex::new(r"app::commands::([a-z_0-9]+)").unwrap();
        let rust: BTreeSet<String> = re.captures_iter(body).map(|c| c[1].to_string()).collect();
        assert!(rust.len() > 30, "コマンド名を取り出せていません: {rust:?}");
        let ts = ts_keys("Commands");
        assert!(diff(&rust, &ts).is_empty(), "ui/src/io/commands.ts の Commands に無いコマンド: {:?}", diff(&rust, &ts));
        assert!(diff(&ts, &rust).is_empty(), "generate_handler! に無いのに Commands にあるコマンド: {:?}", diff(&ts, &rust));
    }

    /// src-tauri/src の emit("…" の名前 ＝ commands.ts の Events のキー
    #[test]
    fn events_table_matches_emit_names() {
        let mut files = vec![];
        rs_files(&manifest().join("src"), &mut files);
        let re = Regex::new(r#"emit\(\s*"([A-Za-z_][A-Za-z0-9_-]*)""#).unwrap();
        let mut rust = BTreeSet::new();
        for f in files {
            for c in re.captures_iter(&read(&f)) {
                rust.insert(c[1].to_string());
            }
        }
        assert!(rust.len() > 10, "emit の名前を取り出せていません: {rust:?}");
        let ts = ts_keys("Events");
        assert!(diff(&rust, &ts).is_empty(), "ui/src/io/commands.ts の Events に無い知らせ: {:?}", diff(&rust, &ts));
        assert!(diff(&ts, &rust).is_empty(), "emit していないのに Events にある知らせ: {:?}", diff(&ts, &rust));
    }
}
