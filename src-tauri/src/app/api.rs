//! Claude（MCP サーバー ak-studio-mcp）向けのローカル HTTP API。127.0.0.1 だけで待ち受け、合言葉（トークン）が要る。
//! ポートとトークンは %USERPROFILE%\.ak-studio\session.json（settings::session_file）に書き、MCP サーバーと Mod がそれを読む。

use ak_proto::http::{ApplyReq, ClipIdReq, ClipsQ, ExportReq, GuideCoordsReq, GuidePatch, GuideStillReq, MarkAddReq, MarkResolveReq, MarksQ, MediaPatch, MemoReq, OvReq, ProposeReq, ReplyReq, SeSet, SheetReq, StillReq, UiFocus};
use super::error::{api_error, coded, no_project};
use ak_proto::{ApiError, ErrorCode};
use crate::core::Core;
use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::Duration;

type S = State<Arc<Core>>;

/// 失敗の応答。`error` は和文の理由（MCP とプラグインはこれを読む）、`code` は種類、`detail` は元の連鎖
fn err_api(status: StatusCode, a: ApiError) -> Response {
    (status, Json(json!({"ok": false, "error": a.message, "code": a.code, "detail": a.detail, "cause": a.cause}))).into_response()
}

/// 文だけの失敗。種類は状態から決める（404 は NotFound、400 は BadInput）
fn err(status: StatusCode, msg: impl ToString) -> Response {
    let msg = msg.to_string();
    let code = match status {
        StatusCode::NOT_FOUND => ErrorCode::NotFound,
        StatusCode::BAD_REQUEST => ErrorCode::BadInput,
        StatusCode::UNAUTHORIZED => ErrorCode::Denied,
        _ => ErrorCode::Failed,
    };
    err_api(status, ApiError { code, message: msg.clone(), detail: msg, cause: None })
}

/// anyhow の失敗の応答。コードは連鎖から決め、分類できなければ `fallback`
fn err_e(e: &anyhow::Error, fallback: ErrorCode) -> Response {
    let mut a = api_error(e);
    if a.code == ErrorCode::Failed {
        a.code = fallback;
    }
    let status = match a.code {
        ErrorCode::NotFound | ErrorCode::NoProject => StatusCode::NOT_FOUND,
        _ => StatusCode::BAD_REQUEST,
    };
    err_api(status, a)
}

fn ok(v: Value) -> Response {
    Json(v).into_response()
}

fn check(c: &Core, h: &HeaderMap) -> Result<(), Box<Response>> {
    let want = format!("Bearer {}", c.api_token);
    if h.get("authorization").and_then(|v| v.to_str().ok()) == Some(want.as_str()) {
        *c.hub.last_poll.lock() = Some(std::time::Instant::now()); // Claude が何か使った = つながっている
        Ok(())
    } else {
        Err(Box::new(err(StatusCode::UNAUTHORIZED, "bad token")))
    }
}

macro_rules! guard {
    ($c:expr, $h:expr) => {
        if let Err(r) = check(&$c, &$h) {
            return *r;
        }
    };
}

async fn blocking<F: FnOnce() -> anyhow::Result<Value> + Send + 'static>(f: F) -> Response {
    match tokio::task::spawn_blocking(f).await {
        Ok(Ok(v)) => ok(v),
        Ok(Err(e)) => err_e(&e, ErrorCode::Failed),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e),
    }
}

pub async fn serve(core: Arc<Core>) {
    let app = Router::new()
        .route("/api/status", get(status))
        .route("/api/script", get(script))
        .route("/api/script/propose", post(propose))
        .route("/api/script/apply", post(apply))
        .route("/api/clips", get(clips))
        .route("/api/clips/override", post(clip_override))
        .route("/api/clips/gui", post(gui_clips))
        .route("/api/scenes", get(scenes))
        .route("/api/still", post(still))
        .route("/api/still/sheet", post(still_sheet))
        .route("/api/guide", get(guide_read))
        .route("/api/guide/update", post(guide_update))
        .route("/api/guide/still", post(guide_still))
        .route("/api/guide/coords", post(guide_coords))
        .route("/api/marks", get(marks_list))
        .route("/api/marks/add", post(mark_add))
        .route("/api/marks/resolve", post(mark_resolve))
        .route("/api/sync", get(sync_report))
        .route("/api/fx/reload", post(fx_reload))
        .route("/api/fx/log", get(fx_log))
        .route("/api/inbox", get(inbox))
        .route("/api/media", get(media_list))
        .route("/api/media/add", post(media_add))
        .route("/api/media/update", post(media_update))
        .route("/api/media/remove", post(media_remove))
        .route("/api/subs", get(subs_read))
        .route("/api/subs/set", post(subs_set))
        .route("/api/reply", post(reply))
        .route("/api/agent/activity", post(agent_activity))
        .route("/api/chat/user", post(chat_user))
        .route("/api/chat/ack", post(chat_ack))
        .route("/api/ui", post(ui))
        .route("/api/export", post(export))
        .route("/api/memo", post(memo))
        .route("/api/se", get(se_list))
        .route("/api/se/set", post(se_set))
        .with_state(core.clone());
    let want = core.settings.get().api_port;
    let listener = match tokio::net::TcpListener::bind(("127.0.0.1", want)).await {
        Ok(l) => l,
        Err(_) => match tokio::net::TcpListener::bind(("127.0.0.1", 0)).await {
            Ok(l) => l,
            Err(e) => {
                // リリース版にはコンソールが無いので、eprintln だけでは誰にも見えない。画面（Claude のピル）とログに出す
                core.api_failed(format!("Claude 用のローカル API を開けません（127.0.0.1）: {}", super::error::reason(e)));
                return;
            }
        },
    };
    let port = listener.local_addr().map(|a| a.port()).unwrap_or(0);
    *core.api_port.lock() = port;
    let sess = crate::settings::session_file();
    // 前の版の置き場所に残った接続先は古い合言葉なので消す（古い MCP・Mod が読んで 401 にならないように）
    let legacy = crate::settings::legacy_session_file();
    if legacy != sess {
        let _ = std::fs::remove_file(&legacy);
    }
    if let Err(e) = crate::settings::write_atomic(
        &sess,
        serde_json::to_string_pretty(&json!({"port": port, "token": core.api_token, "pid": std::process::id()}))
            .unwrap_or_default(),
    ) {
        core.api_failed(format!("Claude に接続先を知らせるファイル（{}）を保存できません: {}", sess.display(), super::error::reason(e)));
    }
    if let Err(e) = axum::serve(listener, app).await {
        core.api_failed(format!("Claude 用のローカル API が止まりました: {}", super::error::reason(e)));
    }
}

// ---------------------------------------------------------------- 状況
async fn status(State(c): S, h: HeaderMap) -> Response {
    guard!(c, h);
    let ui = c.ui.read().clone();
    let t = ui.t;
    let desc = c.desc();
    let root = c.root();
    let ov = c.overrides();
    let cur_line = c.layout_guard().iter().find(|it| it.start <= t && t < it.start + it.dur).cloned();
    // 今の行のメモ（あれば）。Claude が演出を作るときの参考
    let cur_line = cur_line.map(|it| {
        let memo = c.script().and_then(|s| crate::script::ops::memo_of(&s, &it.line_id)).unwrap_or_default();
        let mut v = json!(it);
        if !memo.is_empty() {
            v["memo"] = json!(memo);
        }
        v
    });
    let alive: Vec<Value> = desc
        .as_ref()
        .map(|d| {
            d.groups
                .iter()
                .filter(|g| {
                    let (a, b, hid) = crate::render::cache::group_span(g, &ov);
                    !hid && a <= t && t < b
                })
                .map(|g| json!({"key": g.key, "label": g.label, "line": g.line}))
                .collect()
        })
        .unwrap_or_default();
    // ターミナルで打ったときに添える 1 行（画面は scene を送らないので、再生位置から求めて足す）
    let project = root.as_ref().map(|r| r.to_string_lossy().replace('\\', "/"));
    ok(json!({
        "project": project,
        "situation": crate::agent::status_line(project.as_deref(), &ui, &json!(c.scene_at(t))),
        "timeline": desc.as_ref().map(|d| d.path.clone()),
        "engine": c.engine_info(),
        "ui": ui,
        "playhead": {"t": t, "scene": c.scene_at(t), "line": cur_line, "clips_on_screen": alive},
        "end": desc.as_ref().map(|d| d.end),
        "pending_proposals": c.hub.proposals.lock().iter().filter(|p| p.items.iter().any(|i| i.status == "pending")).count(),
        "pending_items": crate::agent::pending_items(&c.hub.proposals.lock()),
    }))
}

async fn script(State(c): S, h: HeaderMap) -> Response {
    guard!(c, h);
    match c.script() {
        Some(s) => ok(crate::script::ops::numbered(&s, &c.layout_guard())),
        None => err(StatusCode::NOT_FOUND, crate::msg::NO_SCRIPT),
    }
}

async fn propose(State(c): S, h: HeaderMap, Json(r): Json<ProposeReq>) -> Response {
    guard!(c, h);
    let Some(s) = c.script() else { return err(StatusCode::NOT_FOUND, crate::msg::NO_SCRIPT) };
    match crate::script::ops::resolve_items(&s, &r.items) {
        Ok(items) => {
            let title = if r.title.is_empty() { "台本の推敲案".to_string() } else { r.title };
            let p = c.hub.add_proposal(&title, &r.note, items);
            c.emit("proposals", serde_json::to_value(&*c.hub.proposals.lock()).unwrap_or(Value::Null));
            c.emit("toast", json!({"text": format!("Claude から推敲案「{}」が届きました（{} 件）", p.title, p.items.len())}));
            ok(json!({"ok": true, "proposal_id": p.id, "items": p.items.len(),
                      "note": "ユーザーが画面で 1 件ずつ採用・不採用を決める。結果はチャットで届く（採用・適用できずの案だけ。不採用は届かない）"}))
        }
        Err(e) => err_e(&e, ErrorCode::BadInput),
    }
}

async fn apply(State(c): S, h: HeaderMap, Json(r): Json<ApplyReq>) -> Response {
    guard!(c, h);
    blocking(move || {
        let s = c.script().ok_or_else(|| anyhow::anyhow!(crate::msg::NO_SCRIPT))?;
        let items = crate::script::ops::resolve_items(&s, &r.items)?;
        let title = if r.title.is_empty() { "Claude による直接修正".to_string() } else { r.title };
        let p = c.hub.add_proposal(&title, &r.note, items);
        c.resolve_proposal(p.id, None, true)
    })
    .await
}

async fn clips(State(c): S, h: HeaderMap, Query(q): Query<ClipsQ>) -> Response {
    guard!(c, h);
    let Some(d) = c.desc() else { return err(StatusCode::NOT_FOUND, crate::msg::NO_FX) };
    let ov = c.overrides();
    let scenes = c.scenes();
    let doc = c.doc();
    let list: Vec<Value> = d
        .groups
        .iter()
        .filter(|g| q.key.as_ref().map(|k| &g.key == k).unwrap_or(true))
        .filter(|g| {
            let (a, b, _) = crate::render::cache::group_span(g, &ov);
            q.from.map(|f| b >= f).unwrap_or(true) && q.to.map(|t| a <= t).unwrap_or(true)
        })
        .map(|g| {
            let (a, b, hid) = crate::render::cache::group_span(g, &ov);
            let scene = scenes.iter().find(|s| s.1 <= a + 1e-3 && a < s.2).map(|s| s.0.clone());
            let mut v = json!({"key": g.key, "label": g.label, "src_line": g.line, "src": g.src, "t0": a, "t1": b,
                   "hidden": hid, "scene": scene, "kinds": g.kinds, "override": ov.get(&g.key), "gui": g.gui,
                   "bbox": g.bbox});
            if !g.rec.is_empty() {
                v["nest"] = nest_info(g, doc.as_ref().and_then(|d| d.nests.get(&g.key)), a, b);
            }
            v
        })
        .collect();
    ok(json!({"timeline": d.path, "end": d.end, "fps": d.fps, "clips": list}))
}

/// RecWindow に差し込んだ録画の様子（Claude が「どこに録画が足りないか」を言えるように）。
fn nest_info(g: &crate::render::engine::Group, nest: Option<&crate::project::Nest>, a: f64, b: f64) -> Value {
    let covered = nest.map(|n| n.covered()).unwrap_or_default();
    // 窓が見えている区間のうち、素材が無い所（0.2 秒より短い隙間は数えない）
    let mut missing = vec![];
    let mut cur = a;
    for r in covered.iter() {
        if r[1] <= cur {
            continue;
        }
        if r[0] > cur + 0.2 && cur < b {
            missing.push(json!([cur, r[0].min(b)]));
        }
        cur = cur.max(r[1]);
    }
    if b > cur + 0.2 {
        missing.push(json!([cur, b]));
    }
    let segs: Vec<Value> = g.rec.iter().flat_map(|r| r.segs.iter().cloned()).collect();
    json!({
        "clips": nest.map(|n| n.clip_count()).unwrap_or(0),
        "canvas": nest.map(|n| json!([n.w, n.h])),
        "fit": nest.map(|n| n.fit.clone()),
        "covered": covered,
        "missing": missing,
        "segs": segs,
        "note": "録画は AK Studio の画面で RecWindow をダブルクリックして差し込む（ネスト）。missing は窓が出ているのに素材が無い区間",
    })
}

async fn clip_override(State(c): S, h: HeaderMap, Json(r): Json<OvReq>) -> Response {
    guard!(c, h);
    blocking(move || {
        let v = c.patch_override(&r.key, &r.patch)?;
        Ok(json!({"ok": true, "key": r.key, "override": v}))
    })
    .await
}

async fn gui_clips(State(c): S, h: HeaderMap, Json(v): Json<Value>) -> Response {
    guard!(c, h);
    blocking(move || {
        c.set_gui_clips(v)?;
        Ok(json!({"ok": true}))
    })
    .await
}

async fn scenes(State(c): S, h: HeaderMap) -> Response {
    guard!(c, h);
    let sc: Vec<ak_proto::Scene> = c.scenes().into_iter().map(|(scene, t0, t1)| ak_proto::Scene { scene, t0, t1 }).collect();
    ok(json!({"scenes": sc}))
}

async fn still(State(c): S, h: HeaderMap, Json(r): Json<StillReq>) -> Response {
    guard!(c, h);
    blocking(move || {
        let t = if let Some(t) = r.t {
            t
        } else if let Some(sc) = &r.scene {
            crate::render::scenes::scene_focus_t(&c.scenes(), sc).ok_or_else(|| coded(ErrorCode::NotFound, format!("シーン {sc} がありません")))?
        } else if let Some(n) = r.cue {
            let d = c.desc().ok_or_else(|| anyhow::anyhow!(crate::msg::NO_FX))?;
            d.cues.iter().find(|x| x.n == n).map(|x| x.start + 0.5).ok_or_else(|| coded(ErrorCode::NotFound, format!("cue {n} がありません")))?
        } else {
            c.ui.read().t
        };
        let root = c.root().ok_or_else(no_project)?;
        let p = root.join("studio").join("stills").join(format!("still_{:08.3}.png", t));
        c.still(t, &p, r.subs, r.solo.as_deref())?;
        Ok(json!({"path": p.to_string_lossy().replace('\\', "/"), "t": t, "scene": c.scene_at(t)}))
    })
    .await
}

async fn still_sheet(State(c): S, h: HeaderMap, Json(r): Json<SheetReq>) -> Response {
    guard!(c, h);
    blocking(move || {
        let root = c.root().ok_or_else(no_project)?;
        c.still_sheet(r.cues.as_deref(), r.ts.as_deref(), r.subs, &root.join("studio").join("stills"))
    })
    .await
}

// ---------------------------------------------------------------- モニターの印（studio/marks.json）
/// 印の一覧。既定は送られていて済でないもの（sent・Claude の印）。all=true で下書き・済も
async fn marks_list(State(c): S, h: HeaderMap, Query(q): Query<MarksQ>) -> Response {
    guard!(c, h);
    let all = q.all.unwrap_or(false);
    let marks: Vec<_> = c.marks_file().marks.into_iter().filter(|m| all || (m.state != "draft" && m.state != "done")).collect();
    ok(json!({"count": marks.len(), "marks": marks, "note": "座標は出力の画素（1920x1080）。state: draft=まだ送っていない / sent=送った・Claude の作業待ち / done=済"}))
}

/// Claude の印を足す
async fn mark_add(State(c): S, h: HeaderMap, Json(body): Json<Value>) -> Response {
    guard!(c, h);
    let r: MarkAddReq = match parse_body("mark_add", body) {
        Ok(r) => r,
        Err(r) => return *r,
    };
    blocking(move || Ok(json!({"ok": true, "mark": c.mark_add_by_claude(r)?}))).await
}

/// 印を済にする（一言と、直したあとの確認画像つき）
async fn mark_resolve(State(c): S, h: HeaderMap, Json(body): Json<Value>) -> Response {
    guard!(c, h);
    let r: MarkResolveReq = match parse_body("mark_resolve", body) {
        Ok(r) => r,
        Err(r) => return *r,
    };
    blocking(move || Ok(json!({"ok": true, "mark": c.mark_resolve(r.id, r.note, r.image)?}))).await
}

// ---------------------------------------------------------------- 攻略動画（studio/guide.json）
/// 本文を型に読む。形が違えば（知らない項目も）本文を取り違えず、和文の頭をつけて 400 で返す
fn parse_body<T: serde::de::DeserializeOwned>(what: &str, v: Value) -> Result<T, Box<Response>> {
    serde_json::from_value(v).map_err(|e| Box::new(err(StatusCode::BAD_REQUEST, format!("{what} の引数が正しくありません: {e}"))))
}

/// 攻略動画の今の状態（パート・アンカー・操作・編成・補足素材・座標の変換・警告）
async fn guide_read(State(c): S, h: HeaderMap) -> Response {
    guard!(c, h);
    blocking(move || c.guide_read()).await
}

/// 操作・編成・補足素材を直す。保存して組み立て直し、`guide_read` と同じ形に `changed`（変えたことの和文の一覧）・`warnings_new`（この変更で新しく出た警告）を足して返す
async fn guide_update(State(c): S, h: HeaderMap, Json(body): Json<Value>) -> Response {
    guard!(c, h);
    let p: GuidePatch = match parse_body("guide_update", body) {
        Ok(p) => p,
        Err(r) => return *r,
    };
    blocking(move || {
        let (mut v, changed, warnings_new) = c.guide_update(p)?;
        v["changed"] = json!(changed);
        v["warnings_new"] = json!(warnings_new);
        Ok(v)
    })
    .await
}

/// 素材そのままの 1 コマの確認画像（目盛り・切り抜きつき）
async fn guide_still(State(c): S, h: HeaderMap, Json(body): Json<Value>) -> Response {
    guard!(c, h);
    let r: GuideStillReq = match parse_body("guide_still", body) {
        Ok(r) => r,
        Err(r) => return *r,
    };
    blocking(move || c.guide_still(r)).await
}

/// 素材の画素 ↔ 出力の画素
async fn guide_coords(State(c): S, h: HeaderMap, Json(body): Json<Value>) -> Response {
    guard!(c, h);
    let r: GuideCoordsReq = match parse_body("guide_coords", body) {
        Ok(r) => r,
        Err(r) => return *r,
    };
    blocking(move || c.guide_coords(r)).await
}

/// 演出が引いた同期点のうち、文字位置からの推定（ratio）のものを見分けられる一覧
async fn sync_report(State(c): S, h: HeaderMap) -> Response {
    guard!(c, h);
    blocking(move || {
        let d = c.desc().ok_or_else(|| anyhow::anyhow!(crate::msg::NO_FX))?;
        let rows = crate::render::syncrep::report(&d.sync_used)?;
        let estimated = rows.iter().filter(|r| r.estimated).count();
        Ok(json!({"sync": rows, "total": rows.len(), "estimated": estimated}))
    })
    .await
}

async fn fx_reload(State(c): S, h: HeaderMap) -> Response {
    guard!(c, h);
    blocking(move || {
        c.reload_fx();
        let t0 = std::time::Instant::now();
        loop {
            std::thread::sleep(Duration::from_millis(250));
            let st = c.engine.status.read().clone();
            if st.state == "ready" || st.state == "error" || st.state == "none" {
                return Ok(json!({"status": st, "log": c.engine.log_tail(30)}));
            }
            if t0.elapsed() > Duration::from_secs(120) {
                return Ok(json!({"status": st, "note": "まだ読み込み中"}));
            }
        }
    })
    .await
}

async fn fx_log(State(c): S, h: HeaderMap) -> Response {
    guard!(c, h);
    ok(json!({"status": c.engine.status.read().clone(), "log": c.engine.log_tail(120)}))
}

#[derive(Deserialize)]
struct InboxQ {
    wait: Option<u64>,
    /// channel / hook。設定の届け方と違う方には渡さない（片方が読み捨てないように）
    consumer: Option<String>,
    /// フックの待ち受けを起動した時刻（ミリ秒）。新しい待ち受けが来たら古い方は終わらせる
    started: Option<u64>,
    /// "notice": 取り消しの知らせだけを受け取る（Mod が Claude の作業中に使う）
    only: Option<String>,
}

async fn inbox(State(c): S, h: HeaderMap, Query(q): Query<InboxQ>) -> Response {
    guard!(c, h);
    let wait = q.wait.unwrap_or(25).min(55);
    let mode = c.settings.get().delivery;
    if let Some(cons) = &q.consumer {
        if cons != mode.as_str() {
            // 届け方が違う: 何も渡さずに待たせる（すぐ返すと busy loop になる）
            tokio::time::sleep(Duration::from_secs(wait.max(if cons == "mod" { 0 } else { 5 }))).await;
            return ok(json!({"messages": [], "note": format!("delivery is {mode}")}));
        }
    }
    // フックの待ち受け: いちばん後に起動したもの（started が大きい）だけに渡す
    let is_current = |c: &Core| -> bool {
        match (q.consumer.as_deref(), q.started) {
            (Some("hook" | "mod"), Some(st)) => c.hub.watcher.lock().map(|(cur, _)| cur == st).unwrap_or(true),
            _ => true,
        }
    };
    if let (Some("hook" | "mod"), Some(st)) = (q.consumer.as_deref(), q.started) {
        let mut w = c.hub.watcher.lock();
        let take = match *w {
            Some((cur, seen)) => st >= cur || seen.elapsed().as_secs() > 70,
            None => true,
        };
        if take {
            *w = Some((st, std::time::Instant::now()));
        } else {
            return ok(json!({"messages": [], "superseded": true}));
        }
    }
    // only=notice: 取り消しの知らせだけ（Mod が Claude の作業中に、作業を止めるかどうか決めるため）
    let notice_only = q.only.as_deref() == Some("notice");
    let take = |c: &Core| {
        if notice_only {
            c.hub.drain_where(crate::agent::is_cancel_notice)
        } else {
            c.hub.drain()
        }
    };
    let deliver = |c: &Core, msgs: Vec<crate::agent::ChatMsg>| -> Response {
        if !msgs.is_empty() {
            if msgs.iter().any(|m| !crate::agent::is_cancel_notice(m)) {
                *c.hub.claude_status.lock() = "メッセージを読んでいます".into();
            }
            // 画面の「送信待ち」を「届いた」にする（ここからの取り消しは、Claude への知らせになる）
            for m in &msgs {
                c.emit("chat-update", serde_json::to_value(m).unwrap_or(Value::Null));
            }
        }
        c.emit("claude", c.claude_info());
        // 各メッセージに、Claude に渡す本文（"prompt"）を足す（ChatMsg の型は変えない）。印は今の持ち物から引く
        let marks = c.marks_file().marks;
        let names: std::collections::HashMap<String, String> = c.doc()
            .map(|d| d.tracks.iter().flat_map(|t| t.clips.iter()).map(|m| (m.id.clone(), m.name.clone())).collect())
            .unwrap_or_default();
        let out: Vec<Value> = msgs
            .iter()
            .map(|m| {
                let mut v = serde_json::to_value(m).unwrap_or(Value::Null);
                v["prompt"] = json!(crate::agent::prompt_text(m, &marks, &|id| names.get(id).cloned()));
                v
            })
            .collect();
        ok(json!({"messages": out}))
    };
    let was = c.hub.connected();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(wait);
    let mut first = true;
    loop {
        let notified = c.hub.notify.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        let msgs = take(&c);
        if first && !was {
            c.emit("claude", c.claude_info());
        }
        if !msgs.is_empty() || (first && wait == 0) {
            return deliver(&c, msgs);
        }
        first = false;
        let now = tokio::time::Instant::now();
        if now >= deadline {
            return deliver(&c, msgs);
        }
        // 取り消しの猶予中のメッセージがあれば、その猶予が切れたら起きる
        let until = c
            .hub
            .next_ready()
            .map(|t| tokio::time::Instant::from_std(t).max(now).min(deadline))
            .unwrap_or(deadline);
        tokio::select! {
            _ = &mut notified => {}
            _ = tokio::time::sleep_until(until) => {}
        }
        if !is_current(&c) {
            return ok(json!({"messages": [], "superseded": true}));
        }
    }
}

#[derive(Deserialize)]
struct AckReq {
    /// 取り消したメッセージの id
    id: u64,
}

/// Mod が、取り消したメッセージで始まった Claude のターンを止めた（$.turn.abort）ことの知らせ。
async fn chat_ack(State(c): S, h: HeaderMap, Json(r): Json<AckReq>) -> Response {
    guard!(c, h);
    match c.hub.ack_abort(r.id) {
        Some(m) => {
            c.hub.claude_status.lock().clear();
            c.emit("chat-update", serde_json::to_value(&m).unwrap_or(Value::Null));
            c.emit("claude", c.claude_info());
            ok(json!({"ok": true}))
        }
        None => err(StatusCode::NOT_FOUND, "そのメッセージが無い"),
    }
}

async fn reply(State(c): S, h: HeaderMap, Json(r): Json<ReplyReq>) -> Response {
    guard!(c, h);
    if let Some(s) = r.status {
        *c.hub.claude_status.lock() = s;
        c.emit("claude", c.claude_info());
    } else if !r.text.is_empty() {
        c.hub.claude_status.lock().clear();
        c.emit("claude", c.claude_info());
    }
    if !r.text.is_empty() || !r.attachments.is_empty() {
        // セリフの指し方（通し番号・"#12"・行の id）は、ここで今の台本から行の id に引く（画面は引かない）
        let attachments = crate::script::ops::resolve_line_attachments(c.script().as_ref(), r.attachments);
        let m = c.hub.push("claude", &r.text, Value::Null, attachments);
        c.emit("chat", serde_json::to_value(&m).unwrap_or(Value::Null));
        return ok(json!({"ok": true, "id": m.id}));
    }
    ok(json!({"ok": true}))
}

#[derive(Deserialize)]
struct ActivityReq {
    working: bool,
    #[serde(default)]
    text: String,
}

/// Mod が、Claude の作業中か・何をしているかを知らせる（画面の「考え中」の表示用）
async fn agent_activity(State(c): S, h: HeaderMap, Json(r): Json<ActivityReq>) -> Response {
    guard!(c, h);
    if c.hub.set_activity(r.working, &r.text, crate::agent::now_ms()) {
        c.emit("claude", c.claude_info());
    }
    ok(json!({"ok": true}))
}

#[derive(Deserialize)]
struct UserMsg {
    text: String,
    #[serde(default)]
    ctx: Value,
}

/// 画面の外（コマンドラインや Stream Deck など）から、ユーザーとしてチャットに書く。
async fn chat_user(State(c): S, h: HeaderMap, Json(r): Json<UserMsg>) -> Response {
    guard!(c, h);
    let mut ctx = r.ctx;
    if ctx.is_null() {
        let t = c.ui.read().t;
        ctx = json!({"t": t, "scene": c.scene_at(t)});
    }
    let m = c.hub.push_held("user", &r.text, ctx, vec![], c.settings.get().cancel_hold());
    c.emit("chat", serde_json::to_value(&m).unwrap_or(Value::Null));
    ok(json!({"ok": true, "id": m.id}))
}

async fn ui(State(c): S, h: HeaderMap, Json(mut f): Json<UiFocus>) -> Response {
    guard!(c, h);
    // 行は Rust が id に引いて渡す（引けなければ付けず、画面が通知を出す）
    f.line_id = match (&f.line, c.script()) {
        (Some(l), Some(s)) => crate::script::ops::line_id(&s, l),
        _ => None,
    };
    // シーンの指し方も Rust が時刻に引いて渡す（t が先。引けなければ画面に流さず MCP にエラーを返す）
    if let (None, Some(sc)) = (f.t, &f.scene) {
        let name = match sc {
            Value::String(s) => s.clone(),
            v => v.to_string(),
        };
        match crate::render::scenes::scene_focus_t(&c.scenes(), &name) {
            Some(t) => f.t = Some(t),
            None => return err_e(&coded(ErrorCode::NotFound, format!("シーン {} がありません", name.trim_start_matches('s'))), ErrorCode::NotFound),
        }
    }
    c.emit("ui-focus", f);
    ok(json!({"ok": true}))
}

async fn export(State(c): S, h: HeaderMap, Json(r): Json<ExportReq>) -> Response {
    guard!(c, h);
    blocking(move || crate::export::run(&c, &r.kind, r.from, r.to)).await
}

async fn memo(State(c): S, h: HeaderMap, Json(r): Json<MemoReq>) -> Response {
    guard!(c, h);
    blocking(move || {
        let mut s = c.script().ok_or_else(|| anyhow::anyhow!(crate::msg::NO_SCRIPT))?;
        s.memo = r.memo;
        let v = c.save_script(s)?;
        c.emit("script", json!({"script": v["script"], "external": true}));
        Ok(json!({"ok": true}))
    })
    .await
}

// ---------------------------------------------------------------- 素材トラック（背景・録画・立ち絵・BGM・ネスト）
async fn media_list(State(c): S, h: HeaderMap) -> Response {
    guard!(c, h);
    match c.doc() {
        Some(d) => ok(crate::mediaops::summary(&d)),
        None => err_e(&no_project(), ErrorCode::NoProject),
    }
}

async fn media_add(State(c): S, h: HeaderMap, Json(v): Json<Value>) -> Response {
    guard!(c, h);
    blocking(move || crate::mediaops::add(&c, &v)).await
}

async fn media_update(State(c): S, h: HeaderMap, Json(r): Json<MediaPatch>) -> Response {
    guard!(c, h);
    blocking(move || crate::mediaops::update(&c, &r.clip_id, &r.patch)).await
}

async fn media_remove(State(c): S, h: HeaderMap, Json(r): Json<ClipIdReq>) -> Response {
    guard!(c, h);
    blocking(move || crate::mediaops::remove(&c, &r.clip_id)).await
}

// ---------------------------------------------------------------- セリフごとの字幕（改行・大きさ・フォント）
async fn subs_read(State(c): S, h: HeaderMap, Query(q): Query<ak_proto::http::SubsQ>) -> Response {
    guard!(c, h);
    blocking(move || {
        let refs: Option<Vec<Value>> = q.lines.as_deref().filter(|s| !s.trim().is_empty()).map(|s| {
            s.split(',').map(|x| Value::String(x.trim().to_string())).filter(|v| v.as_str() != Some("")).collect()
        });
        crate::subsops::read(&c, refs.as_deref())
    })
    .await
}

async fn subs_set(State(c): S, h: HeaderMap, Json(r): Json<ak_proto::http::SubsSetReq>) -> Response {
    guard!(c, h);
    blocking(move || crate::subsops::set(&c, &r.items, &r.reason)).await
}

// ---------------------------------------------------------------- 効果音の設定（fx/se/bank.json）
async fn se_list(State(c): S, h: HeaderMap) -> Response {
    guard!(c, h);
    blocking(move || Ok(serde_json::to_value(crate::sebank::get(&c)?)?)).await
}

async fn se_set(State(c): S, h: HeaderMap, Json(r): Json<SeSet>) -> Response {
    guard!(c, h);
    blocking(move || se_set_reply(crate::sebank::set(&c, &r.kinds, r.render)?)).await
}

/// Claude（MCP の se_set）に返す形は前のまま: 効果音の設定を上に置き、作り直したときだけ rendered: {wav, count, rev}。
/// 画面の呼び出し・知らせ（SeBankUpdate）とは形が違う
fn se_set_reply(u: ak_proto::SeBankUpdate) -> anyhow::Result<Value> {
    let mut v = serde_json::to_value(&u.bank)?;
    if u.rendered {
        let o = v.as_object_mut().ok_or_else(|| anyhow::anyhow!("効果音の設定の形が想定と違います"))?;
        let mut r = json!({"wav": u.wav, "count": u.count, "rev": u.rev});
        if !u.warnings.is_empty() {
            r["warnings"] = json!(u.warnings);
        }
        o.insert("rendered".into(), r);
    }
    Ok(v)
}

#[cfg(test)]
mod tests {
    use super::se_set_reply;
    use ak_proto::{SeBank, SeBankUpdate};
    use serde_json::{json, Value};

    fn bank(path: &str) -> SeBank {
        let mut v: Value = serde_json::from_str(include_str!("../../tests/fixtures/se_bank.json")).unwrap();
        v["path"] = json!(path);
        serde_json::from_value(v).unwrap()
    }

    #[test]
    fn se_set_reply_keeps_old_http_shape() {
        // 作り直したとき: 設定が上、rendered は {wav, count, rev}（MCP の se_set が前から返していた形）
        let u = SeBankUpdate { bank: bank("p"), rendered: true, rev: 7, wav: Some("a.wav".into()), count: Some(3), warnings: vec![] };
        let v = se_set_reply(u).unwrap();
        assert_eq!(v["path"], "p");
        assert_eq!(v["rendered"], json!({"wav": "a.wav", "count": 3, "rev": 7}));
        assert_eq!(v["kinds"].as_array().unwrap().len(), 3);
        // 警告があれば rendered に付く（棚の音を読めず合成の音で鳴らした、など）
        let u = SeBankUpdate { bank: bank("p"), rendered: true, rev: 7, wav: None, count: Some(1), warnings: vec!["x".into()] };
        assert_eq!(se_set_reply(u).unwrap()["rendered"]["warnings"], json!(["x"]));
        // 作り直さないとき: rendered は付かない
        let u = SeBankUpdate { bank: bank("p"), rendered: false, rev: 7, wav: None, count: None, warnings: vec![] };
        assert!(se_set_reply(u).unwrap().get("rendered").is_none());
    }
}
