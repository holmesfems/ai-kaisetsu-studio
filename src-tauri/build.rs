use std::path::{Path, PathBuf};
use std::process::Command;

// 埋め込む kit の表を作る関数（本体のテストと同じファイル）
#[path = "src/settings/embed_table.rs"]
mod embed_table;
// プリセットのフォルダの探し方（mcp/build.rs と共有）
#[path = "src/settings/preset_dir.rs"]
mod preset_dir;
use preset_dir::preset_dir;

/// git を呼んで標準出力を返す（git が無い・リポジトリでないときは None）
fn git(args: &[&str]) -> Option<String> {
    git_in(None, args)
}

fn git_in(dir: Option<&Path>, args: &[&str]) -> Option<String> {
    let mut c = Command::new("git");
    if let Some(d) = dir {
        c.arg("-C").arg(d);
    }
    let o = c.args(args).output().ok()?;
    o.status.success().then(|| String::from_utf8_lossy(&o.stdout).trim().to_string())
}

/// 非公開の kit のフォルダ（環境変数 AK_FX_PRIVATE。無ければ ../fx-private/kit）。フォルダが無くても場所は返す
fn private_kit_dir(manifest: &Path) -> PathBuf {
    match std::env::var_os("AK_FX_PRIVATE").filter(|v| !v.is_empty()) {
        Some(v) => PathBuf::from(v),
        None => manifest.join("..").join("fx-private").join("kit"),
    }
}

/// 埋め込みの表（FXKIT_FILES）を OUT_DIR/fxkit_files.rs に書く。公開の ../fxkit と、あれば非公開の kit を入れる。
/// 同じ相対パスが 2 つのフォルダにあれば止める。非公開の kit が git のリポジトリなら、その短いコミットを返す
fn embed_kit() -> String {
    let manifest = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let public = manifest.join("..").join("fxkit");
    let private = private_kit_dir(&manifest);
    println!("cargo:rerun-if-env-changed=AK_FX_PRIVATE");
    println!("cargo:rerun-if-changed={}", public.display());
    let mut roots: Vec<&Path> = vec![&public];
    if private.is_dir() {
        println!("cargo:rerun-if-changed={}", private.display());
        roots.push(&private);
    } else if let Some(parent) = private.parent() {
        // kit のフォルダが後から現れたら作り直す
        println!("cargo:rerun-if-changed={}", parent.display());
    }
    let table = match embed_table::merge(&roots) {
        Ok(t) => t,
        Err(e) => panic!("{e}"),
    };
    let out = PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR")).join("fxkit_files.rs");
    std::fs::write(&out, embed_table::render(&table)).expect("fxkit_files.rs を書けませんでした");

    // 非公開の kit のコミット（kit のフォルダがそれ自身のリポジトリのときだけ。このリポジトリの中に置いただけなら無し）
    if !private.is_dir() {
        return String::new();
    }
    let top = git_in(Some(&private), &["rev-parse", "--show-toplevel"]);
    let own_top = git_in(Some(&manifest), &["rev-parse", "--show-toplevel"]);
    if top.is_none() || top == own_top {
        return String::new();
    }
    if let Some(dir) = git_in(Some(&private), &["rev-parse", "--absolute-git-dir"]) {
        println!("cargo:rerun-if-changed={dir}/HEAD");
        println!("cargo:rerun-if-changed={dir}/refs/heads");
    }
    git_in(Some(&private), &["rev-parse", "--short", "HEAD"]).unwrap_or_default()
}

/// 埋め込みの表（PRESET_FILES）を OUT_DIR/preset_files.rs に書く。プリセットが無ければ空の表
fn embed_presets() {
    let manifest = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    println!("cargo:rerun-if-env-changed=AK_FX_PRIVATE");
    // どの候補も、後から現れたら作り直す
    println!("cargo:rerun-if-changed={}", manifest.join("..").join("presets").display());
    println!("cargo:rerun-if-changed={}", manifest.join("..").join("fx-private").display());
    let table = match preset_dir(&manifest) {
        Some(d) => {
            println!("cargo:rerun-if-changed={}", d.display());
            embed_table::collect(&d).unwrap_or_else(|e| {
                println!("cargo:warning=プリセットのフォルダ {} を読めませんでした: {e}。素の既定で作ります", d.display());
                vec![]
            })
        }
        None => vec![],
    };
    let out = PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR")).join("preset_files.rs");
    std::fs::write(&out, embed_table::render_named("PRESET_FILES", &table)).expect("preset_files.rs を書けませんでした");
}

fn main() {
    embed_presets();
    println!("cargo:rustc-env=AK_BUILD_KIT_COMMIT={}", embed_kit());
    // ビルドのコミット・コミットしていない変更の有無・ビルドの時刻を埋め込む（画面とウィンドウのタイトルに出す。旧#163）
    let commit = git(&["rev-parse", "--short", "HEAD"]).unwrap_or_default();
    let dirty = git(&["status", "--porcelain", "--untracked-files=no"]).is_some_and(|s| !s.is_empty());
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    println!("cargo:rustc-env=AK_BUILD_COMMIT={commit}");
    println!("cargo:rustc-env=AK_BUILD_DIRTY={}", if dirty { "1" } else { "" });
    println!("cargo:rustc-env=AK_BUILD_UNIX={now}");
    // コミットした・切り替えた・直したときに作り直す（worktree でも本体の .git を見る）
    if let Some(dir) = git(&["rev-parse", "--git-dir"]) {
        for f in ["HEAD", "index"] {
            println!("cargo:rerun-if-changed={dir}/{f}");
        }
    }
    if let Some(dir) = git(&["rev-parse", "--git-common-dir"]) {
        println!("cargo:rerun-if-changed={dir}/refs/heads");
    }
    for d in ["src", "../ui/src", "../crates", "build.rs"] {
        println!("cargo:rerun-if-changed={d}");
    }
    tauri_build::build()
}
