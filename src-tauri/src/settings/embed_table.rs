//! 埋め込む kit のファイルの表を作る（build.rs と本体のテストの両方が読む。旧#169）。
//! 依存は std だけにすること（build.rs から `#[path]` で取り込むため）。

// build.rs 側では使わない関数がある
#![allow(dead_code)]

use std::path::{Path, PathBuf};

/// 埋め込まないフォルダの名前
const SKIP_DIRS: &[&str] = &["examples", "__pycache__", "build"];

/// 埋め込まないファイルか（pyproject.toml・README・test_*.py）
fn skip_file(name: &str) -> bool {
    name == "pyproject.toml" || name.starts_with("README") || name.starts_with("test_")
}

fn skip_dir(name: &str) -> bool {
    SKIP_DIRS.contains(&name) || name.ends_with(".egg-info")
}

/// root の下を歩いて、除外規則を通ったファイルの (公開する相対パス, 実ファイルのパス) を相対パス順に返す。
/// 相対パスの区切りは `/`
pub fn collect(root: &Path) -> std::io::Result<Vec<(String, PathBuf)>> {
    fn go(dir: &Path, root: &Path, out: &mut Vec<(String, PathBuf)>) -> std::io::Result<()> {
        for e in std::fs::read_dir(dir)? {
            let e = e?;
            let p = e.path();
            let name = e.file_name().to_string_lossy().to_string();
            if p.is_dir() {
                if !skip_dir(&name) {
                    go(&p, root, out)?;
                }
            } else if !skip_file(&name) {
                let rel = p.strip_prefix(root).unwrap().to_string_lossy().replace(std::path::MAIN_SEPARATOR, "/");
                out.push((rel, p));
            }
        }
        Ok(())
    }
    let mut out = vec![];
    go(root, root, &mut out)?;
    out.sort();
    Ok(out)
}

/// 複数のフォルダの表を 1 つにまとめる。同じ相対パスが 2 つあれば Err（どれとどれかを文に入れる）
pub fn merge(roots: &[&Path]) -> Result<Vec<(String, PathBuf)>, String> {
    let mut all: Vec<(String, PathBuf)> = vec![];
    for r in roots {
        for (rel, abs) in collect(r).map_err(|e| format!("{} を読めませんでした: {e}", r.display()))? {
            if let Some((_, other)) = all.iter().find(|(p, _)| *p == rel) {
                return Err(format!("埋め込みの fxkit で同じ名前のファイルが 2 つあります: {rel}（{} と {}）", other.display(), abs.display()));
            }
            all.push((rel, abs));
        }
    }
    all.sort();
    Ok(all)
}

/// `FXKIT_FILES` の Rust のソースを作る
pub fn render(table: &[(String, PathBuf)]) -> String {
    render_named("FXKIT_FILES", table)
}

/// 定数の名前を決めて表の Rust のソースを作る（プリセットの `PRESET_FILES` など。旧#173）
pub fn render_named(name: &str, table: &[(String, PathBuf)]) -> String {
    let mut s = format!("pub(crate) const {name}: &[(&str, &[u8])] = &[\n");
    for (rel, abs) in table {
        s.push_str(&format!("    ({rel:?}, include_bytes!(r\"{}\")),\n", abs.display()));
    }
    s.push_str("];\n");
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("ak-embed-table-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }
    fn put(d: &Path, rel: &str) {
        let p = d.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, b"x").unwrap();
    }
    fn rels(t: &[(String, PathBuf)]) -> Vec<&str> {
        t.iter().map(|(r, _)| r.as_str()).collect()
    }

    #[test]
    fn 除外規則() {
        let d = tmp("skip");
        for f in ["a.py", "fonts/f.ttf", "examples/e.py", "__pycache__/a.pyc", "build/b.py", "x.egg-info/PKG", "pyproject.toml", "README.md", "test_a.py", "se/bank.json"] {
            put(&d, f);
        }
        assert_eq!(rels(&collect(&d).unwrap()), ["a.py", "fonts/f.ttf", "se/bank.json"]);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn 足す_と_重なりで失敗() {
        let (a, b) = (tmp("a"), tmp("b"));
        put(&a, "core.py");
        put(&a, "fonts/x.ttf");
        put(&b, "ak_kit.py");
        let t = merge(&[&a, &b]).unwrap();
        assert_eq!(rels(&t), ["ak_kit.py", "core.py", "fonts/x.ttf"]);
        put(&b, "core.py");
        let e = merge(&[&a, &b]).unwrap_err();
        assert!(e.contains("同じ名前のファイルが 2 つあります: core.py"), "{e}");
        let _ = std::fs::remove_dir_all(&a);
        let _ = std::fs::remove_dir_all(&b);
    }

    #[test]
    fn ソースの形() {
        let s = render(&[("a/b.py".to_string(), PathBuf::from("C:/x/b.py"))]);
        assert!(s.contains(r#"("a/b.py", include_bytes!(r"C:/x/b.py"))"#), "{s}");
    }
}
