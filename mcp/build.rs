//! 非公開の `presets/mcp_context.md`（回の題材に固有の補足）を埋め込む（旧#184）。
//! 探す順は本体の `embed_presets()`（旧#173）と同じ。ファイルが無ければ補足なし。

use std::path::PathBuf;

// プリセットのフォルダの探し方（src-tauri/build.rs と共有）
#[path = "../src-tauri/src/settings/preset_dir.rs"]
mod preset_dir;

fn main() {
    let manifest = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    println!("cargo:rerun-if-env-changed=AK_FX_PRIVATE");
    // どの候補も、後から現れたら作り直す
    println!("cargo:rerun-if-changed={}", manifest.join("..").join("presets").display());
    println!("cargo:rerun-if-changed={}", manifest.join("..").join("fx-private").display());
    let text = match preset_dir::preset_dir(&manifest) {
        Some(d) => {
            let f = d.join("mcp_context.md");
            println!("cargo:rerun-if-changed={}", f.display());
            match std::fs::read_to_string(&f) {
                Ok(t) => t.replace("\r\n", "\n"),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
                Err(e) => {
                    println!("cargo:warning={} を読めませんでした: {e}。補足なしで作ります", f.display());
                    String::new()
                }
            }
        }
        None => String::new(),
    };
    let out = PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR")).join("mcp_context.txt");
    std::fs::write(&out, text.trim()).expect("mcp_context.txt を書けませんでした");
}
