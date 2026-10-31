//! AK Studio の中心。画面（Tauri のコマンド）と Claude（ローカル HTTP → MCP）の両方から使う。

use crate::agent::Hub;
use crate::app::dto::{ClaudeActivity, ClaudeInfo, EngineInfo, ProjectInfo, Snapshot};
use crate::render::cache::OverlayCache;
use crate::render::engine::{Desc, Engine};
use crate::render::media::MediaPool;
use crate::project::Project;
use crate::script::{Script, ScriptStore};
use crate::settings::{find_ffmpeg, SettingsStore};
use crate::render::subs::SubRenderer;
use crate::voice::Voice;
use anyhow::{Context, Result};
use parking_lot::{Mutex, RwLock};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64};
use std::sync::Arc;
use std::time::{Duration, Instant};

mod compose;
mod guide;
mod guide_audio;
mod marks;
mod overrides;
mod proposals;
mod voice;
mod watch;

pub use overrides::{clean_overrides, merge_override, nest_flags, rec_window, window_box};
pub use guide_audio::GuideAudio;
pub use marks::{default_range, overlay_layer, MarkStore};
pub use voice::VoiceState;

pub struct PState {
    pub project: Project,
    pub script: Option<ScriptStore>,
    pub overrides: Value,
    pub gui_clips: Value,
}

type Emit = Box<dyn Fn(&str, Value) + Send + Sync>;

thread_local! {
    /// このスレッドが書き出しをしているか（note_issue が警告を集めるのは、このスレッドで起きたものだけ）
    static COLLECT_ISSUES: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// 書き出しのスレッドで呼ぶ: 合成・ミックスで飛ばしたものを書き出しの警告として集めるか
/// （このスレッドでは、見比べの「変更前」の姿も使わない）
pub fn collect_issues_here(on: bool) {
    COLLECT_ISSUES.with(|c| c.set(on));
}

fn in_export_thread() -> bool {
    COLLECT_ISSUES.with(|c| c.get())
}

pub struct Core {
    pub settings: SettingsStore,
    pub engine: Arc<Engine>,
    pub media: MediaPool,
    pub cache: OverlayCache,
    pub hub: Hub,
    /// モニターの印（`studio/marks.json`。`core/marks.rs`）
    pub marks: RwLock<MarkStore>,
    pub voice: Voice,
    /// 字幕の描き手（演出のワーカーとは別）
    pub subs: SubRenderer,
    pub st: RwLock<Option<PState>>,
    pub ui: RwLock<crate::app::dto::UiState>,
    /// 声の解析とセリフの並び（`core/voice.rs`）。中継への問い合わせの間は持たない
    pub vstate: RwLock<VoiceState>,
    /// 並べ直し（`refresh_voice`）を 1 つずつにする。`st`・`vstate` より先に取り、持ったまま中継に問い合わせる
    voice_lock: Mutex<()>,
    /// VoiSona の中継が起きているか（起きる前の解析の失敗は知らせない）
    voice_up: AtomicBool,
    /// 書き出しの間だけ Some。合成・ミックスで飛ばしたもの（読めない素材・字幕の失敗など）を集めて、結果の警告に出す
    pub export_issues: Mutex<Option<Vec<(String, usize)>>>,
    emitter: Mutex<Option<Emit>>,
    /// tag ごとに走っている先読みの依頼（同じ依頼なら止めずに延ばす。`compose.rs` の prefetch）
    prefetch_jobs: Mutex<HashMap<String, compose::PrefetchJob>>,
    watcher: Mutex<Option<notify::RecommendedWatcher>>,
    self_writes: Mutex<HashMap<PathBuf, Instant>>,
    /// 微調整の読んで・直して・書くを 1 つずつにする（画面と Claude の clip_adjust が同時に来ると片方が消えていた）
    ov_lock: Mutex<()>,
    pub api_token: String,
    pub api_port: Mutex<u16>,
    /// Claude 用の HTTP API が開けなかった・止まったときの理由（画面の Claude のピルに出す）
    pub api_error: Mutex<Option<String>>,
    /// 見比べ（B を押している間）にモニターだけに出す、変更前の素材と微調整。保存はしない（ST-6）
    preview: RwLock<Option<(crate::project::ProjectDoc, Value)>>,
    /// モニターで最後にログへ出した演出のエラー（同じ文を毎コマ出さないため）
    last_fx_error: Mutex<Option<String>>,
    /// 攻略動画（studio/guide.json）の状態。書き換えは guide_lock で 1 つずつ（`core/guide.rs`）
    pub(crate) guide_rt: RwLock<crate::guide::GuideRt>,
    guide_lock: Mutex<()>,
    /// 素材の音の作成（読み込み・並べ直し・書き込み）を 1 つずつにする（`core/guide_audio.rs`）
    guide_audio_lock: Mutex<()>,
    /// 読んだ素材の音（素材のパス・大きさ・更新時刻が同じうちは読み直さない）
    guide_audio_src: Mutex<Option<guide_audio::AudioSrc>>,
    /// 攻略動画の効果音の自動の作り直しを 1 つずつにする（書き出しの前の作り直しも同じ鍵）
    guide_se_lock: Mutex<()>,
    /// 裏の作り直しの依頼の番号（続けて来たら最後の 1 回だけ走らせる）
    guide_audio_seq: AtomicU64,
    guide_se_seq: AtomicU64,
    /// 自分自身（裏のスレッドを起こすのに `&self` から Arc が要る）
    me: std::sync::OnceLock<std::sync::Weak<Core>>,
}

impl Core {
    pub fn new() -> Arc<Core> {
        let settings = SettingsStore::load();
        let s = settings.get();
        let engine = Engine::new(&crate::settings::resolve_python(&s.python), crate::settings::engine_script(), s.workers);
        let subs = SubRenderer::new(&crate::settings::resolve_python(&s.python), crate::settings::engine_script(),
                                    engine.log.clone());
        let media = MediaPool::default();
        if let Some(ff) = find_ffmpeg(&s) {
            *media.ffmpeg.lock() = ff;
        }
        let token: String = {
            use rand::Rng;
            let mut r = rand::thread_rng();
            (0..32).map(|_| format!("{:x}", r.gen_range(0..16))).collect()
        };
        let core = Arc::new(Core {
            cache: OverlayCache::new(s.cache_mb),
            settings,
            engine,
            media,
            hub: Hub::new(),
            marks: RwLock::new(MarkStore::default()),
            voice: Voice::new(),
            subs,
            st: RwLock::new(None),
            ui: RwLock::new(Default::default()),
            vstate: RwLock::new(VoiceState::default()),
            voice_lock: Mutex::new(()),
            voice_up: AtomicBool::new(false),
            export_issues: Mutex::new(None),
            emitter: Mutex::new(None),
            prefetch_jobs: Mutex::new(HashMap::new()),
            watcher: Mutex::new(None),
            self_writes: Mutex::new(HashMap::new()),
            ov_lock: Mutex::new(()),
            api_token: token,
            api_port: Mutex::new(0),
            api_error: Mutex::new(None),
            preview: RwLock::new(None),
            last_fx_error: Mutex::new(None),
            guide_rt: RwLock::new(Default::default()),
            guide_lock: Mutex::new(()),
            guide_audio_lock: Mutex::new(()),
            guide_audio_src: Mutex::new(None),
            guide_se_lock: Mutex::new(()),
            guide_audio_seq: AtomicU64::new(0),
            guide_se_seq: AtomicU64::new(0),
            me: std::sync::OnceLock::new(),
        });
        let _ = core.me.set(Arc::downgrade(&core));
        let c2 = Arc::downgrade(&core);
        let c3 = Arc::downgrade(&core);
        let c4 = Arc::downgrade(&core);
        core.hub.set_on_error(move |e| {
            if let Some(c) = c4.upgrade() {
                c.notice_err("", e);
            }
        });
        core.subs.set_on_warn(move || {
            if let Some(c) = c3.upgrade() {
                c.emit("engine", c.engine_info());
            }
        });
        core.engine.set_on_change(move |what| {
            if let Some(c) = c2.upgrade() {
                c.emit("engine", c.engine_info());
                if what == "desc" {
                    // 演出を読み直したら、同じ演出のエラーでもまたログに出す
                    *c.last_fx_error.lock() = None;
                    // 捨てたとき（unload）は null を送って、画面のタイムラインからも消す
                    let d = c.engine.desc.read().clone();
                    c.emit("desc", d.map(|d| serde_json::to_value(&*d).unwrap_or(Value::Null)).unwrap_or(Value::Null));
                    // 演出が読み込めたら、攻略動画の効果音を作り直す（起動・回の切り替えの直後は、演出が先に揃っていない）
                    c.guide_se_kick();
                }
            }
        });
        core
    }

    /// 起動時の知らせ（設定の読み込み・辞書の移し替え）を画面とログに出す。画面が読み込み終えて get_state を呼んだときに出す
    /// （それより前に出しても、まだ聞いている画面が無い）。出したものは空になる
    pub fn flush_startup_notices(&self) {
        for n in self.settings.take_notices() {
            self.engine.push_log(n.text.clone());
            self.emit("toast", json!({"text": n.text, "kind": if n.error { "error" } else { "info" }, "ms": 15000}));
        }
    }

    pub fn set_emitter(&self, f: impl Fn(&str, Value) + Send + Sync + 'static) {
        *self.emitter.lock() = Some(Box::new(f));
    }

    pub fn emit(&self, name: &str, v: impl serde::Serialize) {
        if let Some(f) = self.emitter.lock().as_ref() {
            f(name, serde_json::to_value(v).unwrap_or(Value::Null));
        }
    }

    /// 失敗をログと画面（トースト）の両方に出す
    pub fn notice_error(&self, text: String) {
        self.notice_detail(text, None);
    }

    /// anyhow の失敗を「{head}: 理由」で出す。理由は和文（`api_error` の message）、元の連鎖はログとトーストのツールチップに
    pub fn notice_err(&self, head: &str, e: &anyhow::Error) {
        let (text, detail) = crate::app::error::notice_text(head, e);
        self.notice_detail(text, Some(detail));
    }

    /// 失敗の知らせ。ログには元の連鎖（detail）を、トーストには和文を出し、違うときだけ detail をツールチップに付ける
    pub(crate) fn notice_detail(&self, text: String, detail: Option<String>) {
        self.engine.push_log(detail.clone().unwrap_or_else(|| text.clone()));
        let mut v = json!({"text": text, "kind": "error", "ms": 15000});
        if let Some(d) = detail.filter(|d| *d != text) {
            v["detail"] = d.into();
        }
        self.emit("toast", v);
    }

    /// project.json の保存で、外の変更の控えを残したときの知らせ（Project::save の戻り値）をログと画面に出す
    pub fn notice_external_save(&self, notice: Option<String>) {
        if let Some(w) = notice {
            self.notice_error(w);
        }
    }

    /// 合成・ミックスで飛ばしたものを、ログに出し、書き出しのスレッドなら警告として数える（同じ文はまとめる）。
    /// 書き出しの間にモニターの合成で起きたものは数えない（NEW-4）
    pub fn note_issue(&self, s: String) {
        self.count_issue(&s);
        // モニターの再生ではコマごとに呼ばれるので、直前と同じ文はログに足さない
        self.log_unless_last(s);
    }

    fn count_issue(&self, s: &str) {
        let here = COLLECT_ISSUES.with(|c| c.get());
        if let Some(v) = self.export_issues.lock().as_mut().filter(|_| here) {
            if let Some(i) = v.iter().position(|(m, _)| m == s) {
                v[i].1 += 1;
            } else if v.len() < 40 {
                v.push((s.to_string(), 1));
            }
        }
    }

    /// `note_issue` の anyhow 版。警告には「{head}: 理由」の和文を数え、ログには元の連鎖を残す
    pub fn note_issue_err(&self, head: &str, e: &anyhow::Error) {
        let (text, detail) = crate::app::error::notice_text(head, e);
        self.count_issue(&text);
        self.log_unless_last(detail);
    }

    /// 直前のログと同じ文なら足さない（毎コマ・毎回呼ばれる所の知らせでログを埋めないため）
    pub(crate) fn log_unless_last(&self, s: String) {
        if self.engine.log_tail(1).first() != Some(&s) {
            self.engine.push_log(s);
        }
    }

    pub fn ffmpeg(&self) -> String {
        self.media.ffmpeg.lock().clone()
    }

    pub fn engine_info(&self) -> EngineInfo {
        let st = self.engine.status.read().clone();
        let (n, bytes) = self.cache.stats();
        EngineInfo {
            status: st,
            cache_frames: n,
            cache_mb: bytes as f64 / 1048576.0,
            warnings: self.warnings(),
            fxkit: self.desc().and_then(|d| d.fxkit.clone()),
            kit: self.desc().and_then(|d| d.kit.as_ref().map(|k| crate::build_info::kit_label(k, crate::build_info::kit_commit()))),
        }
    }

    /// 演出の読み込み（効果音の付け方・画面から追加した演出）と字幕のフォントの警告。
    /// 演出の分は、今のタイムラインの読み込みのもの（読み込み中は前回の分を残す。失敗・別のタイムラインなら出さない）
    pub fn warnings(&self) -> Vec<crate::render::engine::Warning> {
        let st = self.engine.status.read().clone();
        let mut w = match self.desc() {
            Some(d) if matches!(st.state.as_str(), "ready" | "loading") && st.timeline.as_deref() == Some(d.path.as_str()) => {
                let mut w = d.warnings.clone();
                w.extend(overrides::rec_fade_warnings(&d));
                w
            }
            _ => vec![],
        };
        // 攻略動画の回で、kit に決まった演出（guide_template）が無いとき。
        // guide_rt を読んだまま engine_info に来る道があっても止まらないよう、下の st と同じく read_recursive で読む
        if self.guide_rt.read_recursive().guide.is_some() {
            w.extend(crate::guide::template_warning(self.desc().and_then(|d| d.kit.clone()).as_ref()));
        }
        w.extend(crate::render::engine::fxkit_write_warning());
        w.extend(self.engine.overrides_warning());
        // reload_fx・snapshot が st を読んだまま engine_info を呼ぶので、書き手が待っていても止まらない read_recursive で読む
        let (fonts, no_bg) = match self.st.read_recursive().as_ref() {
            Some(p) => (
                Some(crate::render::subs::fonts_in_use(&p.project.doc)),
                crate::project::no_background(&p.project.doc),
            ),
            None => (None, false),
        };
        if let Some(fonts) = fonts {
            w.extend(self.subs.warnings(&fonts));
        }
        // 既定の背景（プリセット）が無く、背景のトラックが空のとき。ふつうの状態なので情報で知らせる
        if no_bg && crate::settings::presets().bg_loop.is_none() {
            w.push(crate::render::engine::Warning {
                key: "no-default-bg".into(),
                message: "背景の素材がありません（既定の背景が設定されていないため、背景なしで動いています）。素材の追加から背景を足せます".into(),
                level: Some("info".into()),
            });
        }
        w
    }

    // ------------------------------------------------------------------ 回を開く
    pub fn open_project(self: &Arc<Self>, path: &Path) -> Result<Snapshot> {
        let project = Project::open(path)?;
        if let Some(w) = &project.warning {
            self.notice_detail(w.clone(), project.warning_detail.clone());
        }
        let mut script = if project.script_dir().is_dir() {
            Some(ScriptStore::open(&project.script_dir())?)
        } else {
            None
        };
        if let Some(st) = script.as_mut() {
            self.emit_script_notices(st);
        }
        let (overrides, ov_warn) = project.load_overrides();
        let (gui_clips, clips_warn) = project.load_gui_clips();
        for w in [ov_warn, clips_warn].into_iter().flatten() {
            let w = format!("{w}空の状態で始めます。");
            self.notice_error(w);
        }
        for (n, detail) in self.hub.open(&project.studio_dir()) {
            self.notice_detail(n, detail);
        }
        self.marks_open(&project.studio_dir());
        let root = project.root.clone();
        let name = project.name.clone();
        {
            // st を入れ替えるのと同じ間に gen を進める（前の回の並べ直しが新しい回に書き込まないように）
            let mut st = self.st.write();
            *st = Some(PState { project, script, overrides, gui_clips });
            self.vstate.write().reset();
        }
        self.voice_up.store(false, std::sync::atomic::Ordering::SeqCst);
        self.cache.clear();
        self.settings.update(|s| {
            s.last_project = root.to_string_lossy().replace('\\', "/");
            let r = s.last_project.clone();
            s.recent.retain(|x| x != &r);
            s.recent.insert(0, r);
            s.recent.truncate(12);
        });
        self.guide_reset_and_load();
        self.reload_fx();
        if let Err(e) = self.sync_nest_flags() {
            self.notice_err("録画の差し込みの印を演出の調整に書けませんでした", &e);
        }
        self.watch(&root);
        // VoiSona の中継は裏で起動（時間がかかることがある）
        let me = self.clone();
        let root2 = root.clone();
        std::thread::spawn(move || {
            let s = me.settings.get();
            match me.voice.ensure(&s, &root2) {
                Ok(p) => {
                    me.emit("voice", json!({"ok": true, "info": p}));
                    me.voice_relay_up(&root2);
                }
                Err(e) => {
                    // 画面は error を理由、code を案内の出し分け、detail を元の連鎖として使う
                    let a = crate::app::error::api_error(&e);
                    me.emit("voice", json!({"ok": false, "error": a.message, "code": a.code, "detail": a.detail}))
                }
            }
        });
        let _ = name;
        // 推定の長さで並べる（中継が起きたら解析を受け取って並べ直す）
        self.refresh_voice(false);
        Ok(self.snapshot())
    }

    /// 画面の初期表示に要るもの一式。
    pub fn snapshot(&self) -> Snapshot {
        // st を持ったまま vstate を取らない（並べ直しは vstate を持ったまま st を取らないが、順番をそろえておく）
        let voice = self.voice_snapshot();
        let st = self.st.read();
        let desc = self.engine.desc.read().clone();
        let cur = self.settings.get();
        let Some(ps) = st.as_ref() else {
            return Snapshot {
                project: None,
                settings: cur.for_ui(),
                voisona_pass_set: !cur.voisona_pass.is_empty(),
                script: None,
                overrides: None,
                gui_clips: None,
                desc: None,
                engine: None,
                chat: None,
                proposals: None,
                claude: None,
                voice: None,
            };
        };
        Snapshot {
            project: Some(ProjectInfo {
                root: ps.project.root.to_string_lossy().replace('\\', "/"),
                name: ps.project.name.clone(),
                doc: ps.project.doc.clone(),
                timelines: crate::project::list_timelines(&ps.project.root),
            }),
            settings: cur.for_ui(),
            voisona_pass_set: !cur.voisona_pass.is_empty(),
            script: ps.script.as_ref().map(|s| s.script.clone()),
            overrides: Some(ps.overrides.clone()),
            gui_clips: Some(ps.gui_clips.clone()),
            desc: desc.as_deref().cloned(),
            engine: Some(self.engine_info()),
            chat: Some(self.hub.msgs.lock().clone()),
            proposals: Some(self.hub.proposals.lock().clone()),
            claude: Some(self.claude_info()),
            voice: Some(voice),
        }
    }

    pub fn claude_info(&self) -> ClaudeInfo {
        let a = self.hub.activity_at(crate::agent::now_ms());
        ClaudeInfo {
            activity: ClaudeActivity { working: a.working, text: a.text, since_ms: a.since_ms, updated_ms: a.updated_ms },
            connected: self.hub.connected(),
            pending: self.hub.pending(),
            watching: self.hub.watching(),
            delivery: self.settings.get().delivery,
            status: self.hub.claude_status.lock().clone(),
            port: *self.api_port.lock(),
            api_error: self.api_error.lock().clone(),
        }
    }

    pub fn api_failed(&self, msg: String) {
        self.engine.push_log(msg.clone());
        *self.api_error.lock() = Some(msg.clone());
        self.emit("claude", self.claude_info());
        self.emit("toast", json!({"text": msg, "kind": "error"}));
    }

    /// タイムラインが無い（回を開いた後に fx/ を作った・消えた）ときだけ探し直して、見つかれば画面に知らせる。
    /// 書き込みの錠はここで手放してから返る（呼び出し側が engine・emit に進めるように）
    fn rediscover_fx(&self) {
        let missing = self.st.read().as_ref().is_some_and(|ps| ps.project.fx_timeline().is_none_or(|p| !p.exists()));
        if !missing {
            return;
        }
        let (rel, timelines, ov, clips, warns, save_err) = {
            let mut st = self.st.write();
            let Some(ps) = st.as_mut() else { return };
            if !ps.project.rediscover_timeline() {
                return;
            }
            let mut warns = vec![];
            let mut save_err = None;
            match ps.project.save() {
                Ok(n) => warns.extend(n),
                Err(e) => save_err = Some(e),
            }
            let (ov, ov_warn) = ps.project.load_overrides();
            let (clips, clips_warn) = ps.project.load_gui_clips();
            for w in [ov_warn, clips_warn].into_iter().flatten() {
                warns.push(format!("{w}空の状態で始めます。"));
            }
            ps.overrides = ov.clone();
            ps.gui_clips = clips.clone();
            let rel = ps.project.doc.fx_timeline.clone().unwrap_or_default();
            (rel, crate::project::list_timelines(&ps.project.root), ov, clips, warns, save_err)
        };
        if let Some(e) = save_err {
            self.notice_err("project.json を保存できませんでした", &e);
        }
        for w in warns {
            self.notice_error(w);
        }
        // doc ごと送ると、画面がまだ送っていない編集を消すので、タイムラインの場所だけ
        self.emit("fx_timeline", json!({"fx_timeline": rel, "timelines": timelines}));
        self.emit("overrides", ov);
        self.emit("gui_clips", clips);
        self.emit("toast", json!({"text": format!("{rel} を見つけたので読み込みます")}));
    }

    pub fn reload_fx(&self) {
        self.rediscover_fx();
        let st = self.st.read();
        let Some(ps) = st.as_ref() else { return };
        let Some(tl) = ps.project.fx_timeline() else {
            // 前の回の演出を残すと、演出の無い回を開いても前の回のクリップがタイムラインに出続ける
            self.engine.unload("fx/ にタイムライン（*_fx.py）がありません");
            return;
        };
        self.engine.load(&tl, &ps.project.overrides_path(), &ps.project.clips_path(), ps.project.doc.placeholder);
    }

    // ------------------------------------------------------------------ 台本
    /// 台本を保存して、声の並びを作り直す（返る前に `layout()` が新しい。中継への問い合わせも待つ）
    pub fn save_script(&self, s: Script) -> Result<Value> {
        let r = self.save_script_file(s)?;
        self.refresh_voice(true);
        Ok(r)
    }

    fn save_script_file(&self, s: Script) -> Result<Value> {
        let mut st = self.st.write();
        let ps = st.as_mut().ok_or_else(crate::app::error::no_project)?;
        if ps.script.is_none() {
            ps.script = Some(ScriptStore::open(&ps.project.script_dir())?);
        }
        let store = ps.script.as_mut().unwrap();
        self.emit_script_notices(store);
        // 自分で書いたファイルは、見張りが読み直しても中身が同じ（changed_on_disk が false）なので印は要らない。
        // フォルダごとに印を付けると、打っている間の外からの変更を見落とす
        let changed = store.save(s);
        self.emit_script_notices(store);
        let changed = changed?;
        let external = store.merged_external;
        Ok(json!({"rev": store.script.rev, "changed": changed, "script": store.script, "external": external}))
    }

    /// 台本フォルダを開いた・読み直したときの知らせ（古い形式から移した・.md と .yaml の両方がある）を画面に出す
    fn emit_script_notices(&self, store: &mut ScriptStore) {
        for n in store.take_notices() {
            self.engine.push_log(n.text.clone());
            self.emit("toast", json!({"text": n.text, "kind": if n.error { "error" } else { "info" }, "ms": 12000}));
        }
    }

    pub fn script(&self) -> Option<Script> {
        self.st.read().as_ref().and_then(|p| p.script.as_ref().map(|s| s.script.clone()))
    }

    pub(crate) fn mark_self_write(&self, p: &Path) {
        self.self_writes.lock().insert(p.to_path_buf(), Instant::now());
    }

    fn recently_written(&self, p: &Path) -> bool {
        let w = self.self_writes.lock();
        let fresh = |t: &Instant| t.elapsed() < Duration::from_millis(1500);
        w.get(p).map(fresh).unwrap_or(false)
    }


    pub fn doc(&self) -> Option<crate::project::ProjectDoc> {
        self.st.read().as_ref().map(|p| p.project.doc.clone())
    }

    pub fn root(&self) -> Option<PathBuf> {
        self.st.read().as_ref().map(|p| p.project.root.clone())
    }

    // ------------------------------------------------------------------ 描画
    pub fn desc(&self) -> Option<Arc<Desc>> {
        self.engine.desc.read().clone()
    }

    pub(crate) fn fps(&self) -> f64 {
        self.desc().map(|d| d.fps).or_else(|| self.guide_fps()).unwrap_or(30.0)
    }

    /// モニターに出す doc と微調整（見比べの間は変更前のもの）。書き出しのスレッドではいつも今のもの
    fn view_doc(&self) -> Option<crate::project::ProjectDoc> {
        if in_export_thread() {
            return self.doc();
        }
        self.preview.read().as_ref().map(|(d, _)| d.clone()).or_else(|| self.doc())
    }

    fn view_overrides(&self) -> Value {
        if in_export_thread() {
            return self.overrides();
        }
        self.preview.read().as_ref().map(|(_, o)| o.clone()).unwrap_or_else(|| self.overrides())
    }

    /// 見比べを始める（Some）・終える（None）。演出のワーカーには微調整だけを渡す（ファイルには書かない）
    pub fn set_preview(&self, p: Option<(crate::project::ProjectDoc, Value)>) {
        // 微調整の保存と入れ違うと、終えたときにワーカーへ古い微調整を渡すので、同じ鍵で 1 つずつ
        let _one = self.ov_lock.lock();
        let ov = match &p {
            Some((doc, o)) => nest_flags(clean_overrides(o.clone()), doc),
            None => self.overrides(),
        };
        *self.preview.write() = p.map(|(d, _)| (d, ov.clone()));
        self.engine.set_overrides(&ov);
    }

    pub fn placeholder(&self) -> bool {
        self.st.read().as_ref().map(|p| p.project.doc.placeholder).unwrap_or(true)
    }
}

pub fn ensure_dir(p: &Path) -> Result<()> {
    std::fs::create_dir_all(p).with_context(|| format!("フォルダを作れない: {}", p.display()))
}
