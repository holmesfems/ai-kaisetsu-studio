//! ファイルの見張り（notify）と、変更の取り込み。

use crate::project::Project;
use super::{Core, PState};
use anyhow::Result;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

impl Core {
    // ------------------------------------------------------------------ ファイルの見張り
    pub(super) fn watch(self: &Arc<Self>, root: &Path) {
        use notify::{RecursiveMode, Watcher};
        const HINT: &str = "台本・演出を外で直しても自動では読み直しません（回を開き直すと、もう一度試します）";
        let (tx, rx) = crossbeam_channel::unbounded::<Result<PathBuf, String>>();
        let w = notify::recommended_watcher(move |res: notify::Result<notify::Event>| match res {
            Ok(ev) => {
                if matches!(ev.kind, notify::EventKind::Access(_)) {
                    return;
                }
                for p in ev.paths {
                    let _ = tx.send(Ok(p));
                }
            }
            Err(e) => {
                let _ = tx.send(Err(e.to_string()));
            }
        });
        let mut w = match w {
            Ok(w) => w,
            Err(e) => {
                let (t, d) = crate::app::error::notice_text("ファイルの見張りを始められませんでした", &e.into());
                self.notice_detail(format!("{t}。{HINT}"), Some(d));
                return;
            }
        };
        let mut failures: Vec<(String, String)> = vec![];
        {
            let st = self.st.read();
            let Some(ps) = st.as_ref() else { return };
            // 回のフォルダ自身も見張る（後から作った fx/・files/・script/ を拾うため）
            if let Err(e) = w.watch(root, RecursiveMode::NonRecursive) {
                let head = format!("ファイルの見張りを始められませんでした: {}", root.display());
                failures.push(crate::app::error::notice_text(&head, &e.into()));
            }
            for d in watched_dirs(&ps.project) {
                if d.is_dir() {
                    if let Err(e) = w.watch(&d, RecursiveMode::NonRecursive) {
                        failures.push(crate::app::error::notice_text(&format!("{} を見張れませんでした", d.display()), &e.into()));
                    }
                }
            }
        }
        for (t, d) in failures {
            self.notice_detail(format!("{t}。{HINT}"), Some(d));
        }
        // 読み上げ辞書（回の外）のフォルダ。見張れなくても、辞書の変更は次の保存・中継の再起動で拾えるのでログだけ
        if let Some(dir) = crate::settings::voice_dict_path().parent().filter(|d| d.is_dir()) {
            if let Err(e) = w.watch(dir, RecursiveMode::NonRecursive) {
                self.engine.push_log(format!("読み上げ辞書のフォルダ {} を見張れませんでした: {e}（辞書を外で直したときは、次の台本の保存まで反映されません）", dir.display()));
            }
        }
        *self.watcher.lock() = Some(w);
        let me = Arc::downgrade(self);
        let root = root.to_path_buf();
        let mut watching: std::collections::HashSet<PathBuf> = {
            let st = self.st.read();
            st.as_ref().map(|ps| watched_dirs(&ps.project).into_iter().filter(|d| d.is_dir()).collect()).unwrap_or_default()
        };
        std::thread::spawn(move || {
            // 見張りのエラーは、同じ文が続くときは 1 回だけログに出す
            let mut last_err: Option<String> = None;
            while let Ok(first) = rx.recv() {
                let mut paths = vec![];
                let mut errs = vec![];
                let until = Instant::now() + Duration::from_millis(450);
                for r in std::iter::once(first).chain(std::iter::from_fn(|| rx.recv_deadline(until).ok())) {
                    match r {
                        Ok(p) => paths.push(p),
                        Err(e) => errs.push(e),
                    }
                }
                let Some(core) = me.upgrade() else { return };
                if core.root().as_deref() != Some(root.as_path()) {
                    return; // 別の回を開いた
                }
                for e in errs {
                    if last_err.as_deref() != Some(e.as_str()) {
                        core.engine.push_log(format!("ファイルの見張りでエラーが出ました: {e}"));
                        last_err = Some(e);
                    }
                }
                if paths.is_empty() {
                    continue;
                }
                last_err = None;
                core.watch_new_dirs(&root, &mut watching, &mut paths);
                core.on_files_changed(paths);
            }
        });
    }

    /// root 直下に fx・files・script が現れたら見張りに足し、中にあるファイルも変わったものとして paths に足す
    /// （フォルダと同時にできたファイルを取りこぼさないため）
    fn watch_new_dirs(&self, root: &Path, watching: &mut std::collections::HashSet<PathBuf>, paths: &mut Vec<PathBuf>) {
        use notify::{RecursiveMode, Watcher};
        let new: Vec<PathBuf> = {
            let st = self.st.read();
            let Some(ps) = st.as_ref() else { return };
            let known = watched_dirs(&ps.project);
            paths
                .iter()
                .filter(|p| p.parent() == Some(root) && known.contains(p))
                .filter(|p| {
                    if p.is_dir() {
                        watching.insert(p.to_path_buf())
                    } else {
                        watching.remove(*p); // 消えたので、作り直されたらまた見張る
                        false
                    }
                })
                .cloned()
                .collect()
        };
        for d in new {
            if let Some(w) = self.watcher.lock().as_mut() {
                if let Err(e) = w.watch(&d, RecursiveMode::NonRecursive) {
                    self.engine.push_log(format!("{} を見張れませんでした: {e}（回を開き直すと、もう一度試します）", d.display()));
                }
            }
            if let Ok(rd) = std::fs::read_dir(&d) {
                paths.extend(rd.flatten().map(|e| e.path()));
            }
        }
    }

    /// 外で書き換わった fx/ の JSON が読めなかったとき（ファイルは退避済み）。
    /// 画面が持っている内容を書き戻して続け、そのことを知らせる。
    fn keep_after_broken(&self, warning: String, path: fn(&Project) -> PathBuf, save: impl FnOnce(&PState) -> Result<()>) {
        let saved = {
            let st = self.st.read();
            let Some(ps) = st.as_ref() else { return };
            // 書き戻しの変更通知で読み直さないように、書く前に印を付ける
            self.mark_self_write(&path(&ps.project));
            save(ps)
        };
        match saved {
            Ok(()) => self.notice_error(format!("{warning}今の内容で続けます。")),
            Err(e) => self.notice_err(&format!("{warning}今の内容も保存できませんでした"), &e),
        }
    }

    fn on_files_changed(&self, paths: Vec<PathBuf>) {
        let (script_dir, fx_dir, ov_p, clips_p, files_dir) = {
            let st = self.st.read();
            let Some(ps) = st.as_ref() else { return };
            (ps.project.script_dir(), ps.project.fx_dir(), ps.project.overrides_path(), ps.project.clips_path(),
             ps.project.root.join("files"))
        };
        let mut script = false;
        let mut fx = false;
        let mut ov = false;
        let mut guide = false;
        let mut dict = false;
        let dict_p = crate::settings::voice_dict_path();
        let studio_dir = files_dir.with_file_name("studio");
        for p in &paths {
            if self.recently_written(p) {
                continue;
            }
            // 読み上げ辞書（外で直した）。そのフォルダのほかのファイルは無視する
            if p == &dict_p {
                dict = true;
                continue;
            }
            // 攻略動画の guide.json・解析（外で直した・解析が書き終えた）
            if p.parent() == Some(studio_dir.as_path())
                && p.file_name().is_some_and(|n| n == crate::guide::GUIDE_FILE || n == crate::guide::ANALYSIS_FILE)
            {
                guide = true;
                continue;
            }
            let ext = p.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
            let parent = p.parent().map(|d| d.to_path_buf()).unwrap_or_default();
            if parent == script_dir && (ext == "yaml" || ext == "md" || ext == "json") {
                script = true;
            } else if p == &ov_p {
                ov = true;
            } else if p == &clips_p
                || (parent == fx_dir && ext == "py")
                || (parent == files_dir && (ext == "json" || ext == "srt" || ext == "csv"))
            {
                fx = true;
            }
        }
        if guide {
            self.guide_load();
        }
        if dict {
            // 辞書を読み直し、署名が変わった行は中継に問い合わせて、無ければ音声なしで並べ直す
            self.refresh_voice(true);
        }
        if script {
            // 読めなければ（書きかけの _voice.json など）少し待ってもう一度。それでもだめなら知らせて前の中身のまま
            for attempt in 0..2 {
                let mut st = self.st.write();
                let Some(store) = st.as_mut().and_then(|p| p.script.as_mut()) else { break };
                if !store.changed_on_disk() {
                    break;
                }
                let prev = store.script.clone();
                match store.reload(Some(&prev)) {
                    Ok(()) => {
                        self.emit_script_notices(store);
                        let s = serde_json::to_value(&store.script).unwrap_or(Value::Null);
                        drop(st);
                        self.emit("script", json!({"script": s, "external": true}));
                        self.refresh_voice(true);
                        break;
                    }
                    Err(e) if attempt == 0 => {
                        drop(st);
                        self.engine.push_log(format!("台本を再読み込みできませんでした（もう一度試します）: {e:#}"));
                        std::thread::sleep(Duration::from_millis(600));
                    }
                    Err(e) => {
                        drop(st);
                        self.notice_err("外で変わった台本を読み込めませんでした", &e);
                    }
                }
            }
        }
        if ov {
            let v = self.st.read().as_ref().map(|p| p.project.load_overrides());
            if let Some((_, Some(w))) = v {
                // 読めないファイルは退避済み。画面の調整を書き戻して、そのまま続ける
                let _one = self.ov_lock.lock();
                self.keep_after_broken(w, Project::overrides_path, |ps| ps.project.save_overrides(&ps.overrides));
            } else if let Some((v, None)) = v {
                if v != self.overrides() {
                    if let Some(ps) = self.st.write().as_mut() {
                        ps.overrides = v.clone();
                    }
                    if self.preview.read().is_none() {
                        self.engine.set_overrides(&v);
                    }
                    self.emit("overrides", v);
                }
            }
        }
        if fx {
            let loaded = self.st.read().as_ref().map(|p| p.project.load_gui_clips());
            match loaded {
                Some((_, Some(w))) => {
                    self.keep_after_broken(w, Project::clips_path, |ps| ps.project.save_gui_clips(&ps.gui_clips))
                }
                Some((v, None)) => {
                    if let Some(ps) = self.st.write().as_mut() {
                        ps.gui_clips = v;
                    }
                }
                None => {}
            }
            self.emit("toast", json!({"text": "演出のコードが変わったので再読み込みします"}));
            self.reload_fx();
        }
    }
}

/// 見張るフォルダ（回のフォルダ直下の script・files・studio と、タイムラインのある fx）
fn watched_dirs(p: &Project) -> [PathBuf; 4] {
    [p.script_dir(), p.fx_dir(), p.root.join("files"), p.root.join("studio")]
}
