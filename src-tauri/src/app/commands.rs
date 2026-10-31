//! 画面（React）から呼ぶコマンド。

use super::error::{coded, no_project, CmdError};
use super::dto::{ClaudeInfo, EngineInfo, PlayReady, SettingsPatch, Snapshot, UiState};
use ak_proto::{ErrorCode, Mark, MarkFile, MarkFound, MediaProbe, Scene, SeBank, SeBankUpdate};
use crate::core::Core;
use crate::voice::analysis::Analysis;
use crate::voice::layout::LayoutState;
use crate::project::ProjectDoc;
use crate::script::Script;
use crate::settings::{detached, hidden};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use tauri::State;

type C<'a> = State<'a, Arc<Core>>;
type R<T> = Result<T, CmdError>;

/// 何かのエラーを画面に返す形にする（io・serde の種類は連鎖から分類される）
fn e<E: Into<anyhow::Error>>(x: E) -> CmdError {
    CmdError::from(x.into())
}

async fn bg<T: Send + 'static>(f: impl FnOnce() -> anyhow::Result<T> + Send + 'static) -> R<T> {
    tauri::async_runtime::spawn_blocking(f).await.map_err(e)?.map_err(CmdError::from)
}

#[tauri::command]
pub async fn get_state(core: C<'_>) -> R<Snapshot> {
    core.flush_startup_notices();
    Ok(core.snapshot())
}

#[tauri::command]
pub async fn list_projects(core: C<'_>) -> R<Value> {
    let s = core.settings.get();
    Ok(json!({"root": s.an_root, "projects": crate::project::list_projects(Path::new(&s.an_root), crate::settings::effective_template_dir(&s)),
              "recent": s.recent}))
}

#[tauri::command]
pub async fn open_project(core: C<'_>, path: String) -> R<Snapshot> {
    let c = core.inner().clone();
    bg(move || c.open_project(Path::new(&path))).await
}

/// ほかの回の台本を、書き換えずに読む（path は回のフォルダ）
#[tauri::command]
pub async fn script_peek(core: C<'_>, path: String) -> R<Script> {
    let dir = PathBuf::from(&path);
    if let Some(root) = core.root() {
        let same = match (dir.canonicalize(), root.canonicalize()) {
            (Ok(a), Ok(b)) => a == b,
            _ => dir == root,
        };
        if same {
            return Err(coded(ErrorCode::BadInput, "今開いている回です").into());
        }
    }
    // 話者の声は、今の声のエンジンのファイルから読む（取り込みで写す先と同じ単位）
    let file = crate::script::speakers_file(&core.settings.get().voice_engine);
    bg(move || crate::script::peek(&dir.join("script"), &file)).await
}

#[tauri::command]
pub async fn save_script(core: C<'_>, script: Script) -> R<Value> {
    let c = core.inner().clone();
    bg(move || c.save_script(script)).await
}

#[tauri::command]
pub async fn set_doc(core: C<'_>, doc: ProjectDoc) -> R<()> {
    let c = core.inner().clone();
    bg(move || c.set_doc(doc)).await
}

#[tauri::command]
pub async fn set_overrides(core: C<'_>, ov: Value) -> R<()> {
    let c = core.inner().clone();
    bg(move || c.set_overrides(ov)).await
}

#[tauri::command]
pub async fn patch_override(core: C<'_>, key: String, patch: Value) -> R<Value> {
    let c = core.inner().clone();
    bg(move || c.patch_override(&key, &patch)).await
}

#[tauri::command]
pub async fn set_gui_clips(core: C<'_>, clips: Value) -> R<()> {
    let c = core.inner().clone();
    bg(move || c.set_gui_clips(clips)).await
}

#[tauri::command]
pub async fn reload_fx(core: C<'_>) -> R<()> {
    core.reload_fx();
    Ok(())
}

/// 演出の同期点を手で動かす（t が null なら自動の位置に戻す）。files/sync_manual.json に書いて演出を読み直す。
#[tauri::command]
pub async fn sync_set(core: C<'_>, cue: i64, keyword: String, t: Option<f64>) -> R<()> {
    let c = core.inner().clone();
    bg(move || crate::syncman::set(&c, cue, &keyword, t)).await
}

#[tauri::command]
pub async fn voice_api(core: C<'_>, path: String, body: Option<Value>) -> R<Value> {
    let c = core.inner().clone();
    bg(move || match body {
        Some(b) => c.voice.post(&path, &b),
        None => c.voice.get(&path),
    })
    .await
}

/// 読み上げ辞書（voice_dict.json）の場所。設定の画面に表示するだけ
#[tauri::command]
pub fn voice_dict_path() -> String {
    crate::settings::voice_dict_path().to_string_lossy().to_string()
}

#[tauri::command]
pub async fn voice_restart(core: C<'_>) -> R<Value> {
    let c = core.inner().clone();
    bg(move || {
        c.voice.stop();
        let root = c.root().ok_or_else(no_project)?;
        let r = c.voice.ensure(&c.settings.get(), &root)?;
        c.voice_relay_up(&root);
        Ok(r)
    })
    .await
}

/// 画面の保存前の台本で並べた結果を返すだけ（打った直後・ドラッグ中の見た目）。Core の並びは変えない
#[tauri::command]
pub async fn voice_layout_preview(core: C<'_>, script: Script, gap: f64, block_gap: f64) -> R<LayoutState> {
    Ok(core.layout_preview(&script, gap, block_gap))
}

/// 1 行の音声を作る。画面の台本（保存前でもよい）の値で作り、解析に入れて `analysis`・`layout` で配る。台本に無い行は null
#[tauri::command]
pub async fn voice_synth(core: C<'_>, script: Script, line_id: String) -> R<Option<Analysis>> {
    let c = core.inner().clone();
    bg(move || c.synth_line(&script, &line_id)).await
}

/// 今のセリフの並びと声の解析の全部（回を開いた直後に、先に届いた知らせとの食い違いを直すために取り直す）
#[tauri::command]
pub async fn voice_state(core: C<'_>) -> R<crate::app::dto::VoiceSnapshot> {
    Ok(core.voice_state())
}

/// 解析から消す（辞書を変えたとき）
#[tauri::command]
pub async fn voice_forget(core: C<'_>, keys: Vec<String>) -> R<()> {
    let c = core.inner().clone();
    bg(move || c.forget_voice(&keys)).await
}

#[tauri::command]
pub async fn set_ui(core: C<'_>, ui: UiState) -> R<()> {
    *core.ui.write() = ui;
    Ok(())
}

#[tauri::command]
pub async fn chat_send(core: C<'_>, text: String, ctx: Value, attachments: Option<Vec<Value>>) -> R<Value> {
    let hold = core.settings.get().cancel_hold();
    let m = core.hub.push_held("user", &text, ctx, attachments.unwrap_or_default(), hold);
    // 添えた印を送信済みにする（下のクリップは ctx.mark_found。画面が後から呼ぶと、猶予 0 秒のとき記録より先に Claude に渡る）
    // （回が無ければ印も無いので飛ばす）
    if let Some(ids) = m.ctx["marks"].as_array().filter(|_| core.root().is_some()) {
        let ids: Vec<u32> = ids.iter().filter_map(|x| x.as_u64()).map(|x| x as u32).collect();
        let (found, bad) = crate::agent::read_mark_found(&m.ctx);
        if bad > 0 {
            core.notice_error(format!("印の下のクリップの記録のうち {bad} 件が読めず、飛ばしました"));
        }
        if let Err(err) = core.marks_sent(&ids, m.id, &found) {
            core.notice_err("印を送信済みにできませんでした", &err);
        }
    }
    core.emit("claude", core.claude_info());
    serde_json::to_value(m).map_err(e)
}

/// 送り間違いの取り消し（issue 旧#21）。まだ Claude に渡していなければ inbox から抜く（retracted）。
/// もう届いていれば、Claude に「直前のメッセージは無視して」と知らせる（late）。
#[tauri::command]
pub async fn chat_cancel(core: C<'_>, id: u64) -> R<Value> {
    let r = core.hub.cancel(id).map_err(e)?;
    let out = match r {
        crate::agent::Cancel::Retracted(m) => {
            core.emit("chat-update", serde_json::to_value(&m).unwrap_or(Value::Null));
            json!({"result": "retracted", "msg": m})
        }
        crate::agent::Cancel::Late(m, notice) => {
            core.emit("chat-update", serde_json::to_value(&m).unwrap_or(Value::Null));
            core.emit("chat", serde_json::to_value(&notice).unwrap_or(Value::Null));
            json!({"result": "late", "msg": m, "notice": notice})
        }
    };
    core.emit("claude", core.claude_info());
    Ok(out)
}

/// Claude の返事への「好き！」（reaction = "like"、空で取り消し）。履歴を書き換えて画面に知らせる。
/// Claude には押した瞬間には送らず、次のメッセージに画面が添える。
#[tauri::command]
pub async fn chat_react(core: C<'_>, id: u64, reaction: String) -> R<Value> {
    let m = core.hub.react(id, &reaction).map_err(e)?;
    core.emit("chat-update", serde_json::to_value(&m).unwrap_or(Value::Null));
    serde_json::to_value(m).map_err(e)
}

#[tauri::command]
pub async fn chat_clear(core: C<'_>) -> R<()> {
    core.hub.msgs.lock().clear();
    if let Some(root) = core.root() {
        // 無いだけなら何も言わない。消せなかったら、開き直すと戻るので知らせる
        let not_found = |r: std::io::Result<()>| r.or_else(|e| if e.kind() == std::io::ErrorKind::NotFound { Ok(()) } else { Err(e) });
        let a = not_found(std::fs::remove_file(root.join("studio").join("chat.jsonl")));
        // 添えた画像も消す
        let b = not_found(std::fs::remove_dir_all(root.join("studio").join("chat")));
        if let Some(err) = a.err().or(b.err()) {
            return Err(e(err));
        }
    }
    Ok(())
}

fn chat_img_dir(core: &Core) -> R<PathBuf> {
    core.root().map(|r| r.join("studio").join("chat")).ok_or_else(|| e(no_project()))
}

/// 貼り付けた画像（base64）を <回>/studio/chat/ に保存して、パスを返す。
#[tauri::command]
pub async fn chat_attach_image(core: C<'_>, data: String, ext: String) -> R<String> {
    use base64::Engine;
    let dir = chat_img_dir(&core)?;
    let bytes = base64::engine::general_purpose::STANDARD.decode(data.trim()).map_err(e)?;
    let p = bg(move || crate::chatimg::save_image(&dir, &bytes, &ext)).await?;
    Ok(p.to_string_lossy().into_owned())
}

/// 📎 で選んだ画像ファイルを <回>/studio/chat/ にコピーして、パスを返す。
#[tauri::command]
pub async fn chat_attach_file(core: C<'_>, path: String) -> R<String> {
    let dir = chat_img_dir(&core)?;
    let p = bg(move || {
        let ext = Path::new(&path).extension().and_then(|x| x.to_str()).unwrap_or("").to_string();
        let bytes = std::fs::read(&path)?;
        crate::chatimg::save_image(&dir, &bytes, &ext)
    })
    .await?;
    Ok(p.to_string_lossy().into_owned())
}

// ---------------------------------------------------------------- モニターの印（core/marks.rs）
#[tauri::command]
pub async fn marks_get(core: C<'_>) -> R<MarkFile> {
    Ok(core.marks_file())
}

/// 印を足す（id 0）か直す。直すのは色・形・一言・t・区間だけ。戻りは保存した印
#[tauri::command]
pub async fn mark_upsert(core: C<'_>, mark: Mark) -> R<Mark> {
    core.mark_upsert(mark).map_err(e)
}

#[tauri::command]
pub async fn mark_remove(core: C<'_>, id: u32) -> R<()> {
    core.mark_remove(id).map_err(e)
}

/// 送った印を sent にして、メッセージの id と下にあったクリップ・素材を記録する
#[tauri::command]
pub async fn marks_sent(core: C<'_>, ids: Vec<u32>, msg: u64, found: Vec<MarkFound>) -> R<()> {
    core.marks_sent(&ids, msg, &found).map_err(e)
}

/// 取り消したメッセージで送った印を draft に戻す
#[tauri::command]
pub async fn marks_unsend(core: C<'_>, msg: u64) -> R<()> {
    core.marks_unsend(msg).map_err(e)
}

/// 状態を変える（"draft" | "sent" | "done"）
#[tauri::command]
pub async fn mark_set_state(core: C<'_>, id: u32, state: String) -> R<Mark> {
    core.mark_set_state(id, &state).map_err(e)
}

/// 印の層（1920x1080 の PNG の base64）を t のコマに重ねた確認画像を `<回>/studio/chat/` に保存して、パスを返す
#[tauri::command]
pub async fn chat_mark_image(core: C<'_>, t: f64, layer: String) -> R<String> {
    use base64::Engine;
    let png = base64::engine::general_purpose::STANDARD.decode(layer.trim()).map_err(e)?;
    let c = core.inner().clone();
    let p = bg(move || c.mark_image(t, &png)).await?;
    Ok(p.to_string_lossy().into_owned())
}

#[tauri::command]
pub async fn proposal_resolve(core: C<'_>, id: u64, idx: Option<usize>, accept: bool) -> R<Value> {
    let c = core.inner().clone();
    let r = bg(move || c.resolve_proposal(id, idx, accept)).await?;
    // Claude に結果を知らせる（チャットには system として残す）
    Ok(r)
}

#[tauri::command]
pub async fn proposal_report(core: C<'_>, id: u64) -> R<bool> {
    // 採用・不採用を決め終えた提案の結果を、Claude に届ける。不採用は送らない（旧#48）。送ったかどうかを返す
    let p = core.hub.proposals.lock().iter().find(|p| p.id == id).cloned();
    let Some(text) = p.and_then(|p| p.report_text()) else { return Ok(false) };
    let m = core.hub.push("user", &text, json!({"kind": "proposal_result", "proposal_id": id}), vec![]);
    // 画面のチャットにも出す（出さないと、開き直すまで見えなかった）
    core.emit("chat", serde_json::to_value(&m).unwrap_or(Value::Null));
    core.emit("claude", core.claude_info());
    Ok(true)
}

#[tauri::command]
pub async fn prefetch(core: C<'_>, t0: f64, t1: f64, scale: f64, tag: String) -> R<()> {
    core.inner().prefetch(t0, t1, scale, &tag);
    Ok(())
}

#[tauri::command]
pub async fn play_ready(core: C<'_>, t0: f64, t1: f64, scale: f64, fx: bool, subs: bool) -> R<PlayReady> {
    let c = core.inner().clone();
    bg(move || Ok(c.play_ready(t0, t1, scale, fx, subs))).await
}

#[tauri::command]
pub async fn cached_ranges(core: C<'_>, scale: f64) -> R<Vec<[f64; 3]>> {
    let c = core.inner().clone();
    bg(move || Ok(c.cached_ranges(scale))).await
}

#[tauri::command]
pub async fn scenes(core: C<'_>) -> R<Vec<Scene>> {
    Ok(core.scenes().into_iter().map(|(scene, t0, t1)| Scene { scene, t0, t1 }).collect())
}

#[tauri::command]
pub async fn export(core: C<'_>, kind: String, from: Option<f64>, to: Option<f64>) -> R<Value> {
    let c = core.inner().clone();
    bg(move || crate::export::run(&c, &kind, from, to)).await
}

#[tauri::command]
pub async fn export_cancel() -> R<()> {
    crate::export::cancel();
    Ok(())
}

/// 入っているフォントの一覧（字幕のフォント選び用）。
#[tauri::command]
pub async fn list_fonts(core: C<'_>) -> R<Value> {
    let c = core.inner().clone();
    bg(move || c.subs.fonts()).await
}

/// 字幕の行の分け方・はみ出し（Python で測る）。
#[tauri::command]
pub async fn subs_check(core: C<'_>) -> R<Value> {
    let c = core.inner().clone();
    bg(move || c.subs_check()).await
}

/// 字幕の見た目（既定・共通・話者・セリフ）と区間・鍵・孤立。重ね方は Rust だけが決め、画面は結果を表示する。
/// doc と items は画面の今のもの（Rust の doc は画面の保存の間引きのぶん遅れているため）。Python は使わないのですぐ返る
#[tauri::command]
pub async fn subs_view(doc: crate::project::ProjectDoc, items: Vec<crate::voice::layout::VoiceItem>)
                       -> R<crate::render::subs::SubView> {
    Ok(crate::render::subs::view(&doc, &items))
}

#[tauri::command]
pub async fn probe_media(core: C<'_>, path: String) -> R<MediaProbe> {
    let c = core.inner().clone();
    bg(move || {
        if crate::render::media::MediaPool::is_image(&path) {
            let im = c.media.image(&path)?;
            return Ok(MediaProbe { w: im.full.w, h: im.full.h, fps: 0.0, dur: 0.0, image: true, has_audio: false, has_video: false });
        }
        crate::render::media::probe(&c.ffmpeg(), &path)
    })
    .await
}

/// 素材の置き方（位置・大きさ・長さ・入れるトラック）を決める。doc は変えない（画面が editDoc で入れる）。
/// Claude の media_add と同じ決まり（mediaops::plan）
#[tauri::command]
pub async fn media_place(core: C<'_>, doc: crate::project::ProjectDoc, path: String, start: Option<f64>,
                         track: Option<String>, nest: Option<String>) -> R<Value> {
    // doc は画面の今のもの（Rust の doc は画面の保存の間引きのぶん遅れていて、作ったばかりのトラック・ネストが見えない）
    let c = core.inner().clone();
    bg(move || {
        let p = crate::mediaops::plan(&c, &doc, &path, start, track.as_deref(), nest.as_deref())?;
        Ok(crate::mediaops::plan_json(&p))
    })
    .await
}

/// 見比べ（B を押している間）: doc と微調整を渡すとモニターだけその姿にし、null で今に戻す。保存はしない（ST-6）
#[tauri::command]
pub async fn preview_state(core: C<'_>, doc: Option<crate::project::ProjectDoc>, ov: Option<Value>) -> R<()> {
    let c = core.inner().clone();
    bg(move || {
        c.set_preview(doc.map(|d| (d, ov.unwrap_or_else(|| json!({})))));
        Ok(())
    })
    .await
}

#[tauri::command]
pub async fn engine_log(core: C<'_>) -> R<Vec<String>> {
    Ok(core.engine.log_tail(300))
}

#[tauri::command]
pub async fn engine_info(core: C<'_>) -> R<EngineInfo> {
    Ok(core.engine_info())
}

#[tauri::command]
pub async fn claude_info(core: C<'_>) -> R<ClaudeInfo> {
    Ok(core.claude_info())
}

/// ビルドのコミットと時刻（上の帯に出す。旧#163）
#[tauri::command]
pub async fn build_info() -> R<String> {
    Ok(crate::build_info::label())
}

/// ⚙ 設定の変更を項目ごとに当てる（渡した項目だけ変わる）。変わった項目に応じてワーカー・ffmpeg・キャッシュを直す
#[tauri::command]
pub async fn settings_patch(core: C<'_>, p: SettingsPatch) -> R<()> {
    let before = core.settings.get();
    // 正しくない値（声のエンジンのパスなど）は、何も保存せずに弾く
    let mut checked = before.clone();
    crate::settings::apply_patch(&mut checked, &p).map_err(|e| coded(ErrorCode::BadInput, e))?;
    core.settings.update(|x| {
        let _ = crate::settings::apply_patch(x, &p);
    });
    let s = core.settings.get();
    if p.python.is_some() && before.python != s.python {
        let py = crate::settings::resolve_python(&s.python);
        core.subs.set_python(&py);
        // 演出のワーカーも新しい Python で作り直して読み直す（入っている fxkit が違うかもしれないので、コマのキャッシュも捨てる）
        core.engine.set_python(&py);
        core.cache.clear();
        core.reload_fx();
    }
    if (p.ffmpeg.is_some() && before.ffmpeg != s.ffmpeg) || (p.an_root.is_some() && before.an_root != s.an_root) {
        if let Some(ff) = crate::settings::find_ffmpeg(&s) {
            *core.media.ffmpeg.lock() = ff;
        }
    }
    if p.cache_mb.is_some() && before.cache_mb != s.cache_mb {
        *core.cache.budget.lock() = s.cache_mb << 20;
    }
    // 声のエンジン・VOICEVOX の URL が変わった: 中継を止めて起こし直し、長さを取り直す。
    // 起こし直しの失敗は設定の保存の失敗ではない（保存はできた）ので、回を開いたときと同じ「voice」の知らせで画面に出す
    if before.voice_engine != s.voice_engine || before.voicevox_url != s.voicevox_url {
        let c = core.inner().clone();
        bg(move || {
            c.restart_voice_for_engine();
            Ok(())
        })
        .await?;
    }
    Ok(())
}

/// VS Code（無ければメモ帳）でファイルのその行を開く。
#[tauri::command]
pub async fn open_in_editor(path: String, line: Option<i64>) -> R<()> {
    let target = match line {
        Some(l) if l > 0 => format!("{path}:{l}"),
        _ => path.clone(),
    };
    let ok = {
        let mut c = hidden(Command::new("cmd"));
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            c.creation_flags(0x0800_0000 | 0x0100_0000);
        }
        c.args(["/c", "code", "-g", &target]).status().map(|s| s.success()).unwrap_or(false)
    };
    if !ok {
        detached(Command::new("notepad")).arg(&path).spawn().map_err(e)?;
    }
    Ok(())
}

#[tauri::command]
pub async fn reveal(path: String) -> R<()> {
    let p = path.replace('/', "\\");
    detached(Command::new("explorer")).arg(format!("/select,{p}")).spawn().map_err(e)?;
    Ok(())
}

fn mcp_exe() -> Option<PathBuf> {
    let me = std::env::current_exe().ok()?;
    let d = me.parent()?;
    for cand in [d.join("ak-studio-mcp.exe"), d.join("..").join("release").join("ak-studio-mcp.exe"),
                 d.join("..").join("debug").join("ak-studio-mcp.exe")] {
        if cand.exists() {
            return cand.canonicalize().ok().map(|p| PathBuf::from(p.to_string_lossy().trim_start_matches(r"\\?\").to_string()));
        }
    }
    None
}

fn run_claude(args: &[&str]) -> (bool, String) {
    let mut c = hidden(Command::new("cmd"));
    c.arg("/c").arg("claude").args(args);
    match c.output() {
        Ok(o) => (o.status.success(), format!("{}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr))),
        Err(x) => (false, x.to_string()),
    }
}

/// Mod（claude-plugin/ak-studio）のマーケットプレイスのフォルダ。
fn plugin_dir() -> Option<PathBuf> {
    let me = std::env::current_exe().ok()?;
    for a in me.ancestors() {
        let p = a.join("claude-plugin");
        if p.join(".claude-plugin").join("marketplace.json").exists() {
            return Some(PathBuf::from(p.to_string_lossy().trim_start_matches(r"\\?\").to_string()));
        }
    }
    None
}

/// Claude Code に、MCP サーバー（ak-studio の道具）と Mod（画面からのメッセージ・状況の帯）を入れる。ユーザー全体で 1 回だけでよい。
#[tauri::command]
pub async fn mcp_setup() -> R<Value> {
    tauri::async_runtime::spawn_blocking(|| {
        let mut log = vec![];
        let exe = mcp_exe().ok_or_else(|| anyhow::anyhow!("ak-studio-mcp.exe が見つかりません（cargo build -p ak-studio-mcp でビルドしてください）"))?;
        let exe_s = exe.to_string_lossy().to_string();
        let (ok, out) = run_claude(&["mcp", "get", "ak-studio"]);
        if ok && out.contains(&exe_s) {
            log.push("MCP: 登録済み".to_string());
        } else {
            if ok {
                run_claude(&["mcp", "remove", "-s", "user", "ak-studio"]);
            }
            let (ok2, out2) = // -e（環境変数）は値をいくつでも取るので、後ろに名前を書くと名前まで値として飲み込まれる。
            // channels 用の MCP_PROTOCOL_NEGOTIATION は「Claude を起動」のコマンドで渡すので、ここでは付けない
            run_claude(&["mcp", "add", "-s", "user", "ak-studio", "--", &exe_s]);
            log.push(format!("MCP: {}{}", if ok2 { "登録しました" } else { "登録できませんでした: " }, if ok2 { String::new() } else { out2 }));
        }
        let mut mod_ok = false;
        if let Some(dir) = plugin_dir() {
            let d = dir.to_string_lossy().to_string();
            let (_, m1) = run_claude(&["plugin", "marketplace", "add", &d]);
            let (ok3, m2) = run_claude(&["plugin", "install", "ak-studio@ak-studio-local", "-s", "user"]);
            mod_ok = ok3 || m2.contains("already");
            log.push(format!("Mod: {}", if mod_ok { "インストールしました（次に起動する Claude Code から有効です）".to_string() } else { format!("インストールできませんでした: {m1} {m2}") }));
        } else {
            log.push("Mod: claude-plugin フォルダが見つかりません".into());
        }
        Ok::<Value, anyhow::Error>(json!({"ok": true, "mod": mod_ok, "exe": exe_s, "log": log, "already": false}))
    })
    .await
    .map_err(e)?
    .map_err(CmdError::from)
}

/// フック方式: 回のフォルダの .claude/settings.local.json に、画面からのメッセージで Claude を起こすフックを書く。
/// - SessionStart と Stop（返事を終えるたび）に、待ち受け（ak-studio-mcp --wait）を張る（asyncRewake）
/// - UserPromptSubmit（ターミナルで打ったとき）に、画面の状況を添える（--context）
///
/// 既にある設定は残し、ak-studio-mcp の分だけ入れ替える。
pub fn write_hooks(root: &Path) -> anyhow::Result<PathBuf> {
    let exe = mcp_exe().ok_or_else(|| anyhow::anyhow!("ak-studio-mcp.exe が見つかりません"))?;
    let exe_s = exe.to_string_lossy().replace('\\', "/");
    let dir = root.join(".claude");
    std::fs::create_dir_all(&dir)?;
    let p = dir.join("settings.local.json");
    let mut v: Value = std::fs::read_to_string(&p).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_else(|| json!({}));
    if !v.is_object() {
        v = json!({});
    }
    let wait = json!({"type": "command", "command": exe_s, "args": ["--wait", "3500"], "async": true, "asyncRewake": true, "timeout": 3600});
    let ctx = json!({"type": "command", "command": exe_s, "args": ["--context"], "timeout": 10});
    let hooks = v.as_object_mut().unwrap().entry("hooks").or_insert_with(|| json!({}));
    for (ev, h) in [("SessionStart", &wait), ("Stop", &wait), ("UserPromptSubmit", &ctx)] {
        let arr = hooks.as_object_mut().unwrap().entry(ev).or_insert_with(|| json!([]));
        if let Some(a) = arr.as_array_mut() {
            // 前に書いた ak-studio の分を消してから足す
            a.retain(|m| !m.to_string().contains("ak-studio-mcp"));
            a.push(json!({"hooks": [h]}));
        }
    }
    // ak-studio のツールは確認なしで使えるように
    let perms = v.as_object_mut().unwrap().entry("permissions").or_insert_with(|| json!({}));
    let allow = perms.as_object_mut().unwrap().entry("allow").or_insert_with(|| json!([]));
    if let Some(a) = allow.as_array_mut() {
        if !a.iter().any(|x| x.as_str() == Some("mcp__ak-studio")) {
            a.push(json!("mcp__ak-studio"));
        }
    }
    crate::settings::write_atomic(&p, serde_json::to_string_pretty(&v)?)?;
    Ok(p)
}

#[tauri::command]
pub async fn hooks_setup(core: C<'_>) -> R<Value> {
    let root = core.root().ok_or_else(no_project)?;
    let p = write_hooks(&root).map_err(e)?;
    Ok(json!({"ok": true, "path": p}))
}

/// その回のフォルダで Claude Code を新しいターミナルで起動する（届け方が channel なら channel つき）。
#[tauri::command]
pub async fn launch_claude(core: C<'_>) -> R<()> {
    let root = core.root().ok_or_else(no_project)?;
    let delivery = core.settings.get().delivery;
    let channel = delivery == crate::settings::Delivery::Channel;
    if delivery == crate::settings::Delivery::Hook {
        write_hooks(&root).map_err(e)?;
    }
    let cmdline = if channel {
        "set MCP_PROTOCOL_NEGOTIATION=legacy&& claude --dangerously-load-development-channels server:ak-studio"
    } else {
        "claude"
    };
    let wt = detached(Command::new("wt")).args(["-d"]).arg(&root).args(["cmd", "/k", cmdline]).spawn();
    if wt.is_err() {
        detached(Command::new("cmd"))
            .args(["/c", "start", "AK Studio - Claude", "/D"])
            .arg(&root)
            .args(["cmd", "/k", cmdline])
            .spawn()
            .map_err(e)?;
    }
    Ok(())
}

// ---------------------------------------------------------------- 攻略動画（studio/guide.json）
/// 攻略動画の状態（guide.json・効いている固定値・パート・解析の要約・理由）
#[tauri::command]
pub async fn guide_get(core: C<'_>) -> R<ak_proto::guide::GuideState> {
    Ok(core.guide_state())
}

/// 攻略動画の編集内容を保存して、パートを組み立て直す。
/// base_rev は画面が持っている `GuideState::rev`（今と違えば保存せずに断る）。
/// 引数の名前は画面の表（commands.ts）と同じ base_rev のまま受ける（Tauri の既定は baseRev）
#[tauri::command(rename_all = "snake_case")]
pub async fn guide_set(core: C<'_>, guide: ak_proto::Guide, base_rev: Option<u64>) -> R<ak_proto::guide::GuideState> {
    let c = core.inner().clone();
    bg(move || c.guide_set(guide, base_rev)).await
}

/// 画面の「Claude の変更を戻す」: いちばん新しい Claude の変更を控えに戻す（その後に直していたら断る）
#[tauri::command]
pub async fn guide_undo_claude(core: C<'_>, id: u32) -> R<ak_proto::guide::GuideState> {
    let c = core.inner().clone();
    bg(move || c.guide_undo_claude(id)).await
}

/// guide.json を作る（src は回のフォルダの中の動画）
#[tauri::command]
pub async fn guide_create(core: C<'_>, src: String) -> R<ak_proto::guide::GuideState> {
    let c = core.inner().clone();
    bg(move || c.guide_create(&src)).await
}

/// 攻略動画の演出の入口 fx/guide_fx.py を作って、演出を読み込む（あれば断る）
#[tauri::command]
pub async fn guide_fx_create(core: C<'_>) -> R<()> {
    let c = core.inner().clone();
    bg(move || c.guide_fx_create()).await
}

/// 素材の解析を裏で始める（進み具合は progress の guide_scan、終わりは guide イベント）
#[tauri::command]
pub async fn guide_scan(core: C<'_>) -> R<()> {
    let c = core.inner().clone();
    bg(move || c.guide_scan()).await
}

/// 出力の時刻 t（手動パートの中）に操作を足す
#[tauri::command]
pub async fn guide_op_add(core: C<'_>, t: f64) -> R<ak_proto::guide::GuideState> {
    let c = core.inner().clone();
    bg(move || c.guide_op_add(t)).await
}

/// 出力の時刻 t に見えている素材のコマ（モニターの今の位置）。組み立てが無い・範囲外は null
#[tauri::command]
pub async fn guide_src_at(core: C<'_>, t: f64) -> R<Option<ak_proto::guide::SrcAt>> {
    Ok(core.guide_src_at(t))
}

/// 素材の時刻 t_src から delta コマ進めた（戻した）コマの時刻（1 コマ送り）。
/// 引数の名前は画面の表（commands.ts）と同じ t_src のまま受ける（Tauri の既定は tSrc）
#[tauri::command(rename_all = "snake_case")]
pub async fn guide_step(core: C<'_>, t_src: f64, delta: i32) -> R<f64> {
    core.guide_step(t_src, delta).map_err(CmdError::from)
}

/// 固定値を設定の guide_fixed（全部の回で使う値）に保存する
#[tauri::command]
pub async fn guide_save_fixed_default(core: C<'_>, fixed: ak_proto::GuideFixed) -> R<ak_proto::guide::GuideState> {
    let c = core.inner().clone();
    bg(move || c.guide_save_fixed_default(fixed)).await
}

/// 待機欄の大きさ・不透明度を設定の guide_bench（全部の回で使う値）に保存する
#[tauri::command]
pub async fn guide_save_bench(core: C<'_>, bench: ak_proto::GuideBench) -> R<ak_proto::guide::GuideState> {
    let c = core.inner().clone();
    bg(move || c.guide_save_bench(bench)).await
}

// ---------------------------------------------------------------- 効果音の設定（fx/se/bank.json）
#[tauri::command]
pub async fn se_bank(core: C<'_>) -> R<SeBank> {
    let c = core.inner().clone();
    bg(move || crate::sebank::get(&c)).await
}

#[tauri::command]
pub async fn se_bank_set(core: C<'_>, kinds: Value, render: Option<bool>) -> R<SeBankUpdate> {
    let c = core.inner().clone();
    bg(move || crate::sebank::set(&c, &kinds, render.unwrap_or(true))).await
}

#[tauri::command]
pub async fn se_audition(core: C<'_>, kind: String, src: String) -> R<Value> {
    let c = core.inner().clone();
    bg(move || crate::sebank::audition(&c, &kind, &src)).await
}

#[tauri::command]
pub async fn se_import(core: C<'_>, kind: String, path: String) -> R<Value> {
    let c = core.inner().clone();
    bg(move || crate::sebank::import(&c, &kind, &path)).await
}
