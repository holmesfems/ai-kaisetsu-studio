//! AK Studio（AI Kaisetsu Studio）— 解説動画の台本・音声・演出の編集アプリ。

mod agent;
mod app;
mod build_info;
mod chatimg;
pub mod core;
mod export;
mod guide;
mod mediaops;
mod msg;
mod subsops;
mod project;
mod render;
pub mod script;
mod sebank;
mod settings;
mod syncman;
mod voice;

use crate::core::Core;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Arc;
use tauri::http::{Request, Response};
use tauri::{Emitter, Manager};

fn pct_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'%' if i + 2 < b.len() => {
                let h = std::str::from_utf8(&b[i + 1..i + 3]).ok().and_then(|x| u8::from_str_radix(x, 16).ok());
                if let Some(v) = h {
                    out.push(v);
                    i += 3;
                    continue;
                }
                out.push(b[i]);
            }
            b'+' => out.push(b' '),
            c => out.push(c),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).to_string()
}

fn query(req: &Request<Vec<u8>>) -> HashMap<String, String> {
    req.uri()
        .query()
        .unwrap_or("")
        .split('&')
        .filter(|s| !s.is_empty())
        .map(|kv| {
            let mut it = kv.splitn(2, '=');
            (pct_decode(it.next().unwrap_or("")), pct_decode(it.next().unwrap_or("")))
        })
        .collect()
}

fn resp(status: u16, ctype: &str, body: Vec<u8>, extra: &[(&str, String)]) -> Response<Vec<u8>> {
    let mut b = Response::builder()
        .status(status)
        .header("Content-Type", ctype)
        .header("Access-Control-Allow-Origin", "*")
        .header("Access-Control-Expose-Headers", "X-Rect, X-Info")
        .header("Cache-Control", "no-store");
    for (k, v) in extra {
        b = b.header(*k, v.as_str());
    }
    b.body(body).unwrap()
}

/// ヘッダに入れる JSON。ヘッダの値はブラウザで Latin-1 として読まれるので、ASCII 以外は \uXXXX にする。
fn ascii_json(v: &Value) -> String {
    let mut out = String::new();
    for c in v.to_string().chars() {
        if c.is_ascii() {
            out.push(c);
        } else {
            for u in c.encode_utf16(&mut [0u16; 2]) {
                out.push_str(&format!("\\u{u:04x}"));
            }
        }
    }
    out
}

/// 先頭から n 文字まで（長いエラーでヘッダが大きくなりすぎないように）
fn clip_chars(s: &str, n: usize) -> String {
    match s.char_indices().nth(n) {
        Some((i, _)) => format!("{}…", &s[..i]),
        None => s.to_string(),
    }
}

/// akf:// の中身。モニターのコマ（JPEG）、1 クリップだけの PNG、音声・素材のファイル。
fn handle(core: &Arc<Core>, req: Request<Vec<u8>>) -> Response<Vec<u8>> {
    let path = pct_decode(req.uri().path());
    let q = query(&req);
    let f = |k: &str, d: f64| q.get(k).and_then(|v| v.parse::<f64>().ok()).unwrap_or(d);
    let list = |k: &str| -> Vec<String> {
        q.get(k).map(|v| v.split(',').filter(|s| !s.is_empty()).map(|s| s.to_string()).collect()).unwrap_or_default()
    };
    let result: anyhow::Result<Response<Vec<u8>>> = (|| {
        if path == "/frame" {
            let t = f("t", 0.0);
            let s = f("s", 0.5).clamp(0.1, 1.0);
            let hide = list("hide");
            let solo = q.get("solo").filter(|x| !x.is_empty()).cloned();
            let fx = q.get("fx").map(|v| v != "0").unwrap_or(true);
            let subs = q.get("subs").map(|v| v != "0").unwrap_or(true);
            let play = q.get("play").map(|v| v == "1").unwrap_or(false);
            if play {
                core.prefetch(t, t + 4.0, s, "play");
            }
            // 演出のコマが失敗しても、背景・素材・字幕は描いて、演出のエラーを添える（BR-15）
            let (fr, fx_error) = core.compose_monitor(t, s, &hide, solo.as_deref(), fx, subs)?;
            let jpg = crate::render::media::encode_jpeg(&fr, 85)?;
            // 止まっているときは、字幕の行の位置とはみ出し（モニターの目安線用）を添える
            let mut info = serde_json::Map::new();
            if !play && subs {
                if let Some(v) = core.sub_info_at(t) {
                    info.insert("sub".into(), v);
                }
            }
            if let Some(e) = fx_error {
                info.insert("fx_error".into(), json!(clip_chars(&e, 1000)));
            }
            return Ok(resp(200, "image/jpeg", jpg, &[("X-Info", ascii_json(&Value::Object(info)))]));
        }
        if path == "/clip" {
            let (png, bb) = core.clip_png(f("t", 0.0), f("s", 0.5), q.get("key").map(|s| s.as_str()).unwrap_or(""))?;
            let rs = bb.map(|b| format!("{},{},{},{}", b[0], b[1], b[2], b[3])).unwrap_or_default();
            return Ok(resp(200, "image/png", png, &[("X-Rect", rs)]));
        }
        if let Some(id) = path.strip_prefix("/audio/") {
            let id = id.trim_end_matches(".wav");
            if !id.chars().all(|c| c.is_ascii_hexdigit()) {
                anyhow::bail!("bad id");
            }
            let root = core.root().ok_or_else(crate::app::error::no_project)?;
            let b = std::fs::read(root.join("voice_cache").join(format!("{id}.wav")))?;
            return Ok(resp(200, "audio/wav", b, &[]));
        }
        if path == "/file" {
            let p = q.get("p").cloned().unwrap_or_default();
            if !allowed(core, &p) {
                anyhow::bail!("この場所のファイルは開けない: {p}");
            }
            let b = std::fs::read(&p)?;
            let ct = crate::render::media::mime_of(&p);
            return Ok(resp(200, ct, b, &[]));
        }
        if path == "/guide_slot" {
            // 編成の枠 i の絵（攻略タブの枠の一覧）
            let i = f("i", 0.0).max(0.0) as usize;
            let jpg = core.guide_slot_jpeg(i, f("s", 0.3))?;
            return Ok(resp(200, "image/jpeg", jpg, &[]));
        }
        if path == "/thumb" {
            // 動画素材の 1 コマ（タイムラインのサムネイル）
            let p = q.get("p").cloned().unwrap_or_default();
            if !allowed(core, &p) {
                anyhow::bail!("not allowed");
            }
            if crate::render::media::MediaPool::is_image(&p) {
                let im = core.media.image(&p)?;
                let h = 64u32;
                let w = (im.full.w as f64 * h as f64 / im.full.h as f64).round().max(1.0) as u32;
                let s = im.at(w, h, false)?;
                let img = image::RgbaImage::from_raw(s.w, s.h, s.data.clone()).unwrap();
                let mut out = std::io::Cursor::new(vec![]);
                image::DynamicImage::ImageRgba8(img).write_to(&mut out, image::ImageFormat::Png)?;
                return Ok(resp(200, "image/png", out.into_inner(), &[]));
            }
            let v = core.media.video(&p)?;
            let h = 64u32;
            let w = (((v.info.w.max(1) as f64) * h as f64 / v.info.h.max(1) as f64).round() as u32).max(2) & !1;
            let idx = v.index_at(f("t", 0.0));
            let fr = v.frame(idx, w, h)?;
            return Ok(resp(200, "image/jpeg", crate::render::media::encode_jpeg(&fr, 80)?, &[]));
        }
        Ok(resp(404, "text/plain", b"not found".to_vec(), &[]))
    })();
    match result {
        Ok(r) => r,
        Err(e) => resp(error_status(&e), "text/plain; charset=utf-8", format!("{e:#}").into_bytes(), &[]),
    }
}

/// akf:// のエラーの状態。ファイルが無いときは 404（画面は、まだ作っていない効果音の wav などを見分ける）、ほかは 500
fn error_status(e: &anyhow::Error) -> u16 {
    use ak_proto::ErrorCode;
    match crate::app::error::api_error(e).code {
        ErrorCode::NotFound => 404,
        ErrorCode::BadInput => 400,
        _ => 500,
    }
}

/// akf://file で読んでよい場所（回のフォルダ・an_root の下・トラックで使っている素材）。
fn allowed(core: &Core, p: &str) -> bool {
    let norm = |s: &str| s.replace('\\', "/").to_lowercase();
    let pn = norm(p);
    if pn.contains("..") {
        return false;
    }
    // 効果音の設定の試聴の音（一時フォルダ）
    if pn.starts_with(&(norm(&crate::sebank::audition_dir().to_string_lossy()) + "/")) {
        return true;
    }
    let s = core.settings.get();
    if !s.an_root.trim().is_empty() && pn.starts_with(&norm(&s.an_root)) {
        return true;
    }
    if let Some(r) = core.root() {
        if pn.starts_with(&norm(&r.to_string_lossy())) {
            return true;
        }
    }
    core.doc()
        .map(|d| {
            d.tracks.iter().chain(d.nests.values().flat_map(|n| n.tracks.iter()))
                .any(|t| t.clips.iter().any(|c| norm(&c.src) == pn))
        })
        .unwrap_or(false)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    crate::settings::kill_children_on_exit();
    let core = Core::new();
    let core_setup = core.clone();
    let core_exit = core.clone();
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(core.clone())
        .register_asynchronous_uri_scheme_protocol("akf", move |ctx, req, responder| {
            let core = ctx.app_handle().state::<Arc<Core>>().inner().clone();
            std::thread::spawn(move || responder.respond(handle(&core, req)));
        })
        .setup(move |app| {
            let h = app.handle().clone();
            core_setup.set_emitter(move |name, v| {
                let _ = h.emit(name, v);
            });
            tauri::async_runtime::spawn(app::api::serve(core_setup.clone()));
            // どの版か分かるように、ウィンドウのタイトルにビルドのコミットと時刻を出す（旧#163）
            if let Some(w) = app.get_webview_window("main") {
                let kit = build_info::kit_commit();
                let kit = if kit.is_empty() { String::new() } else { format!(" kit {kit}") };
                let _ = w.set_title(&format!("AK Studio {}{kit}", build_info::label()));
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            app::commands::get_state,
            app::commands::list_projects,
            app::commands::open_project,
            app::commands::save_script,
            app::commands::script_peek,
            app::commands::set_doc,
            app::commands::set_overrides,
            app::commands::patch_override,
            app::commands::set_gui_clips,
            app::commands::reload_fx,
            app::commands::sync_set,
            app::commands::voice_api,
            app::commands::voice_restart,
            app::commands::voice_dict_path,
            app::commands::voice_layout_preview,
            app::commands::voice_synth,
            app::commands::voice_forget,
            app::commands::voice_state,
            app::commands::set_ui,
            app::commands::chat_send,
            app::commands::chat_clear,
            app::commands::chat_attach_image,
            app::commands::chat_attach_file,
            app::commands::chat_cancel,
            app::commands::chat_react,
            app::commands::marks_get,
            app::commands::mark_upsert,
            app::commands::mark_remove,
            app::commands::marks_sent,
            app::commands::marks_unsend,
            app::commands::mark_set_state,
            app::commands::chat_mark_image,
            app::commands::proposal_resolve,
            app::commands::proposal_report,
            app::commands::prefetch,
            app::commands::play_ready,
            app::commands::cached_ranges,
            app::commands::scenes,
            app::commands::export,
            app::commands::export_cancel,
            app::commands::probe_media,
            app::commands::media_place,
            app::commands::preview_state,
            app::commands::engine_log,
            app::commands::claude_info,
            app::commands::build_info,
            app::commands::engine_info,
            app::commands::settings_patch,
            app::commands::open_in_editor,
            app::commands::reveal,
            app::commands::mcp_setup,
            app::commands::launch_claude,
            app::commands::hooks_setup,
            app::commands::list_fonts,
            app::commands::subs_check,
            app::commands::subs_view,
            app::commands::guide_get,
            app::commands::guide_set,
            app::commands::guide_undo_claude,
            app::commands::guide_create,
            app::commands::guide_fx_create,
            app::commands::guide_scan,
            app::commands::guide_save_fixed_default,
            app::commands::guide_save_bench,
            app::commands::guide_op_add,
            app::commands::guide_src_at,
            app::commands::guide_step,
            app::commands::se_bank,
            app::commands::se_bank_set,
            app::commands::se_audition,
            app::commands::se_import,
        ])
        .build(tauri::generate_context!())
        .expect("error while building AK Studio")
        .run(move |_app, ev| {
            if let tauri::RunEvent::Exit = ev {
                core_exit.engine.shutdown();
                core_exit.subs.shutdown();
                core_exit.voice.stop();
                // AK Studio を 2 つ起動しているとき、もう片方の接続先を消さないよう、自分が書いたときだけ消す
                let sess = crate::settings::data_dir().join("session.json");
                let mine = std::fs::read_to_string(&sess)
                    .ok()
                    .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
                    .and_then(|v| v["pid"].as_u64())
                    == Some(std::process::id() as u64);
                if mine {
                    let _ = std::fs::remove_file(&sess);
                }
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// ファイルが無いときだけ 404（context を重ねても見分ける）
    #[test]
    fn error_status_not_found_is_404() {
        let nf = anyhow::Error::from(std::fs::read("ak-studio-no-such-file").unwrap_err());
        assert_eq!(error_status(&nf), 404);
        assert_eq!(error_status(&nf.context("読めません")), 404);
        assert_eq!(error_status(&anyhow::anyhow!("bad id")), 500);
        // 渡した値が正しくないときは 400
        use ak_proto::ErrorCode;
        assert_eq!(error_status(&crate::app::error::coded(ErrorCode::BadInput, "値が違います")), 400);
        assert_eq!(error_status(&crate::app::error::coded(ErrorCode::NotFound, "ありません").context("読めません")), 404);
    }

    #[test]
    fn ascii_json_escapes_non_ascii() {
        let v = json!({"fx_error": "演出を描けませんでした: 😀 x"});
        let s = ascii_json(&v);
        assert!(s.is_ascii());
        assert_eq!(serde_json::from_str::<Value>(&s).unwrap(), v);
        // ヘッダの値として入れられる
        assert!(tauri::http::HeaderValue::from_str(&s).is_ok());
    }

    #[test]
    fn clip_chars_cuts_by_chars() {
        assert_eq!(clip_chars("あいう", 5), "あいう");
        assert_eq!(clip_chars("あいう", 3), "あいう");
        assert_eq!(clip_chars("あいうえ", 2), "あい…");
    }
}
