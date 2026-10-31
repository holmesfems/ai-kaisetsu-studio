//! プリセットのフォルダの探し方（旧#173）。ビルドスクリプトだけが読む（本体の `src-tauri/build.rs` と `mcp/build.rs` が
//! `#[path]` で共有する）。std だけを使い、本体のクレートには入れない。

use std::path::{Path, PathBuf};

/// プリセットのフォルダ。探す順は `AK_FX_PRIVATE` の親の `presets/` → `../fx-private/presets` →
/// リポジトリ直下の `../presets`（旧#175 までのつなぎ）。最初にあったものを使う。どれも無ければ None。
/// `manifest` は `src-tauri/` か `mcp/`（リポジトリ直下の子）
pub fn preset_dir(manifest: &Path) -> Option<PathBuf> {
    let mut c: Vec<PathBuf> = vec![];
    if let Some(v) = std::env::var_os("AK_FX_PRIVATE").filter(|v| !v.is_empty()) {
        if let Some(parent) = PathBuf::from(v).parent() {
            c.push(parent.join("presets"));
        }
    }
    c.push(manifest.join("..").join("fx-private").join("presets"));
    c.push(manifest.join("..").join("presets"));
    c.into_iter().find(|d| d.is_dir())
}
