//! 声のエンジン（VoiSona Talk・VOICEVOX・自作）との橋渡し。埋め込んだ voice/voice_server.py を裏で動かして使う
//! （音声のキャッシュの鍵・辞書・アクセント確認の画面が今までと同じになるように）。

use crate::settings::{hidden, Settings};
use anyhow::{anyhow, Context, Result};
use parking_lot::Mutex;
use serde_json::Value;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

pub struct Voice {
    child: Mutex<Option<Child>>,
    /// ensure は 1 つずつ（回を続けて開くと 2 本目が同じポートに入れずに終わり、1 本目の子を見失っていた）
    ensure_lock: Mutex<()>,
    pub port: Mutex<u16>,
    pub last_error: std::sync::Arc<Mutex<Option<String>>>,
}

impl Voice {
    pub fn new() -> Self {
        Voice {
            child: Mutex::new(None),
            ensure_lock: Mutex::new(()),
            port: Mutex::new(8765),
            last_error: std::sync::Arc::new(Mutex::new(None)),
        }
    }

    fn base(&self) -> String {
        format!("http://127.0.0.1:{}", *self.port.lock())
    }

    pub fn ping(&self) -> Option<Value> {
        ureq::get(&format!("{}/api/ping", self.base()))
            .timeout(Duration::from_millis(800))
            .call()
            .ok()
            .and_then(|r| r.into_json().ok())
    }

    /// ensure_lock を取ってから設定を読む（順番が逆だと、ロックを待つ間に変わった設定を見落とす）
    fn lock_and_settings(&self, get_settings: impl Fn() -> Settings) -> (parking_lot::MutexGuard<'_, ()>, Settings) {
        let guard = self.ensure_lock.lock();
        let s = get_settings();
        (guard, s)
    }

    /// 起動（もう動いていれば、その回に切り替えるだけ）。
    pub fn ensure(&self, s: &Settings, project: &Path) -> Result<Value> {
        self.ensure_with(|| s.clone(), project)
    }

    /// `ensure` の、設定を ensure_lock を取ってから読む版。回を開く裏のスレッドと設定の変更（声のエンジンの切り替え）が
    /// 重なったとき、ロックを待つ間に古くなった設定で起動し直して、切り替えを元に戻さないため
    pub fn ensure_with(&self, get_settings: impl Fn() -> Settings, project: &Path) -> Result<Value> {
        let (_one, s) = self.lock_and_settings(get_settings);
        let s = &s;
        *self.port.lock() = s.voice_port;
        let name = project.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        if let Some(p) = self.ping() {
            // もう動いている（別の回で起動したものも含む）: この回に切り替え、切り替わったかを確かめる。
            // 古い中継（台本エディタのフォルダから起動したもの）は別の辞書を使っている。辞書が割れるので使わない
            if !crate::settings::dict_path_matches(p["dict"].as_str(), &crate::settings::voice_dict_path()) {
                return Err(anyhow!(
                    "声の中継が別の辞書（{}）を使っています。古い中継（台本エディタのフォルダから起動したもの）を閉じてから再起動してください",
                    p["dict"].as_str().filter(|d| !d.is_empty()).unwrap_or("不明")
                ));
            }
            // 別の声のエンジンで動いている中継は使えない（声も辞書の読みも別物になる）。こちらの子なら止めて起動し直す
            if !engine_matches(&p, s) {
                let running = describe_engine(&p);
                if self.child.lock().is_none() {
                    return Err(anyhow!(
                        "声の中継が別の声のエンジン（{running}）で動いています。設定の声のエンジン（{}）で使うには、その中継を閉じてから再起動してください",
                        s.voice_engine
                    ));
                }
                self.stop();
                return self.spawn(s, project);
            }
            // 切り替わらないまま進むと、声が別の回の voice_cache に入り、再生は無音になる
            if p["project"].as_str() != Some(name.as_str()) {
                self.post("/api/project", &serde_json::json!({"path": project.to_string_lossy()}))
                    .with_context(|| format!("声の中継を、この回（{name}）に切り替えられませんでした"))?;
            }
            let p = self.ping().ok_or_else(|| crate::app::error::coded(ak_proto::ErrorCode::VoiceDown, format!("声の中継が応答しません（ポート {}）", s.voice_port)))?;
            if let Some(other) = p["project"].as_str().filter(|x| *x != name) {
                return Err(anyhow!(
                    "声の中継が別の回（{other}）を開いたままです。別の回で開いた中継が残っていたら閉じてください"
                ));
            }
            return Ok(p);
        }
        self.spawn(s, project)
    }

    /// 中継を起こして、応答するまで待つ（呼ぶのは ensure だけ。ensure_lock を持っている）
    fn spawn(&self, s: &Settings, project: &Path) -> Result<Value> {
        *self.last_error.lock() = None;
        // 辞書が無ければ作る（起動時に作れていなければここでもう一度。作れなかったときは進めずに知らせる）
        let dict = crate::settings::ensure_voice_dict().map_err(|e| anyhow!(e))?;
        let script = crate::settings::voice_dir().map_err(|e| anyhow!(e))?.join("voice_server.py");
        if !script.exists() {
            return Err(anyhow!("voice_server.py が見つかりません: {}", script.display()));
        }
        let py = crate::settings::resolve_python(&s.python);
        let mut cmd = hidden(Command::new(&py));
        cmd.arg("-u")
            .arg(&script)
            .arg("--project")
            .arg(project)
            .arg("--port")
            .arg(s.voice_port.to_string())
            .arg("--engine")
            .arg(&s.voice_engine)
            .env("AK_VOICE_DICT", &dict)
            .env("PYTHONUTF8", "1")
            .env("PYTHONIOENCODING", "utf-8")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        if !s.an_root.trim().is_empty() {
            cmd.env("AK_AN_ROOT", &s.an_root);
        }
        if !s.voicevox_url.trim().is_empty() {
            cmd.env("VOICEVOX_URL", s.voicevox_url.trim());
        }
        if !s.voisona_user.is_empty() {
            cmd.env("VOISONA_USER", &s.voisona_user).env("VOISONA_PASS", &s.voisona_pass);
        }
        let mut child = cmd.spawn().with_context(|| format!("voice_server を起動できません（python: {py}）"))?;
        let stderr = child.stderr.take();
        *self.child.lock() = Some(child);
        if let Some(se) = stderr {
            let le = self.last_error.clone();
            std::thread::spawn(move || {
                use std::io::BufRead;
                // 中継が終わるまで読み続ける（読むのをやめるとパイプが詰まり、中継が書き込みで止まる・落ちる）。
                // UTF-8 でない行も捨てずに読む
                let mut r = std::io::BufReader::new(se);
                let mut buf = String::new();
                let mut line = Vec::new();
                loop {
                    line.clear();
                    match r.read_until(b'\n', &mut line) {
                        Ok(0) | Err(_) => break,
                        Ok(_) => {}
                    }
                    buf.push_str(String::from_utf8_lossy(&line).trim_end_matches(['\r', '\n']));
                    buf.push('\n');
                    keep_tail(&mut buf, 4000, 3000);
                    *le.lock() = Some(buf.clone());
                }
            });
        }
        for _ in 0..50 {
            std::thread::sleep(Duration::from_millis(150));
            if let Some(p) = self.ping() {
                return Ok(p);
            }
            if let Some(c) = self.child.lock().as_mut() {
                if let Ok(Some(st)) = c.try_wait() {
                    return Err(crate::app::error::coded(ak_proto::ErrorCode::VoiceDown, format!(
                        "voice_server が止まりました（{st}）\n{}",
                        self.last_error.lock().clone().unwrap_or_default()
                    )));
                }
            }
        }
        // 待ち切った: 残しておくと次の起動が同じポートに入れないので止める
        if let Some(mut c) = self.child.lock().take() {
            let _ = c.kill();
            let _ = c.wait();
        }
        Err(crate::app::error::coded(ak_proto::ErrorCode::VoiceDown, format!(
            "voice_server が 7 秒たっても応答しないので止めました\n{}",
            self.last_error.lock().clone().unwrap_or_default()
        )))
    }

    pub fn post(&self, path: &str, body: &Value) -> Result<Value> {
        self.post_timeout(path, body, Duration::from_secs(180))
    }

    /// 待つ時間を指定する POST（解析の問い合わせなど、すぐ返るもの用）
    pub fn post_timeout(&self, path: &str, body: &Value, timeout: Duration) -> Result<Value> {
        let r = ureq::post(&format!("{}{}", self.base(), path))
            .timeout(timeout)
            .send_json(body.clone());
        match r {
            Ok(r) => Ok(r.into_json()?),
            Err(ureq::Error::Status(_, r)) => {
                let v: Value = r.into_json().unwrap_or(Value::Null);
                Err(anyhow!("{}", v["error"].as_str().unwrap_or("voice_server でエラーが起きました")))
            }
            Err(e) => Err(relay_down(&e)),
        }
    }

    pub fn get(&self, path: &str) -> Result<Value> {
        match ureq::get(&format!("{}{}", self.base(), path)).timeout(Duration::from_secs(10)).call() {
            Ok(r) => Ok(r.into_json()?),
            // 古い中継には無い道（/api/speakers など）。画面はコードで見分ける
            Err(ureq::Error::Status(404, _)) => Err(crate::app::error::not_found(format!("声の中継に {path} がありません"))),
            Err(ureq::Error::Status(_, r)) => {
                let v: Value = r.into_json().unwrap_or(Value::Null);
                Err(anyhow!("{}", v["error"].as_str().unwrap_or("voice_server でエラーが起きました")))
            }
            Err(e) => Err(relay_down(&e)),
        }
    }

    pub fn stop(&self) {
        if let Some(mut c) = self.child.lock().take() {
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}

/// 動いている中継（ping の返事）が、設定の声のエンジンで動いているか。
/// 名前のエンジンは `engine.name`、パスのエンジンは `engine.arg`（中継を起こしたときの --engine の値のまま。
/// resolve した `engine.path` は 8.3 名・`..`・ジャンクションで設定の文字列と合わないことがある。`arg` が無い中継は `path`）を比べる
/// （区切り・大文字小文字・先頭の `\?\` は見ない。Windows）。
/// `engine` を返さない古い中継は voisona として動いている
pub(crate) fn engine_matches(ping: &Value, s: &Settings) -> bool {
    fn norm(p: &str) -> String {
        let p = p.trim().replace('\\', "/");
        let p = p.strip_prefix("//?/").unwrap_or(&p).to_string();
        if cfg!(windows) { p.to_lowercase() } else { p }
    }
    let want = s.voice_engine.trim();
    let e = &ping["engine"];
    if want == "voisona" || want == "voicevox" {
        // パスで読んだエンジンの NAME がたまたま同じでも、名前で選んだものとは別（path があれば不一致）
        return e["path"].as_str().is_none() && e["name"].as_str().unwrap_or("voisona") == want;
    }
    e["arg"].as_str().or(e["path"].as_str()).is_some_and(|rp| norm(rp) == norm(want))
}

/// 動いている中継の声のエンジンの見せ方（エラー文用）
fn describe_engine(ping: &Value) -> String {
    let e = &ping["engine"];
    match (e["path"].as_str(), e["label"].as_str().or(e["name"].as_str())) {
        (Some(p), _) => p.to_string(),
        (None, Some(n)) => n.to_string(),
        _ => "VoiSona".to_string(),
    }
}

/// 中継に接続できなかった失敗。英語の原因は detail に残す
fn relay_down(e: &dyn std::fmt::Display) -> anyhow::Error {
    crate::app::error::coded_detail(ak_proto::ErrorCode::VoiceDown, "声の中継サーバーに接続できません", e.to_string())
}

/// 文字列が max バイトを超えたら、末尾の約 keep バイトだけ残す（文字の途中では切らない）
fn keep_tail(buf: &mut String, max: usize, keep: usize) {
    if buf.len() <= max {
        return;
    }
    let mut cut = buf.len().saturating_sub(keep);
    while !buf.is_char_boundary(cut) {
        cut += 1;
    }
    buf.drain(..cut);
}

#[cfg(test)]
mod tests {
    use super::{engine_matches, keep_tail};
    use crate::settings::Settings;
    use serde_json::json;

    fn st(engine: &str) -> Settings {
        Settings { voice_engine: engine.into(), ..Settings::default() }
    }

    #[test]
    fn engine_matches_by_name_and_old_relay() {
        // engine を報告しない古い中継は voisona
        assert!(engine_matches(&json!({"ok": true}), &st("voisona")));
        assert!(!engine_matches(&json!({"ok": true}), &st("voicevox")));
        let vv = json!({"engine": {"name": "voicevox", "label": "VOICEVOX", "features": [], "api": 1}});
        assert!(engine_matches(&vv, &st("voicevox")));
        assert!(!engine_matches(&vv, &st("voisona")));
        let vs = json!({"engine": {"name": "voisona", "label": "VoiSona", "features": ["accent"], "api": 1}});
        assert!(engine_matches(&vs, &st("voisona")));
        assert!(!engine_matches(&vs, &st("voicevox")));
        // パスで読んだエンジンは、名前が同じでも名前で選んだものとは別
        let custom = json!({"engine": {"name": "voicevox", "path": "D:/e/my_tts.py"}});
        assert!(!engine_matches(&custom, &st("voicevox")));
    }

    #[test]
    fn engine_matches_by_path() {
        let ping = json!({"engine": {"name": "mine", "path": "D:\\Engines\\My_TTS.py"}});
        assert!(engine_matches(&ping, &st("D:\\Engines\\My_TTS.py")));
        assert!(engine_matches(&ping, &st("D:/Engines/My_TTS.py")));
        if cfg!(windows) {
            assert!(engine_matches(&ping, &st("d:/engines/my_tts.py")));
        }
        assert!(!engine_matches(&ping, &st("D:/Engines/other_tts.py")));
        // パスの設定に、名前だけ（path 無し）の中継は合わない
        assert!(!engine_matches(&json!({"engine": {"name": "mine"}}), &st("D:/Engines/My_TTS.py")));
        assert!(!engine_matches(&json!({"ok": true}), &st("D:/Engines/My_TTS.py")));
    }

    #[test]
    fn settings_are_read_after_taking_the_lock() {
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::Arc;
        use std::time::Duration;
        let voice = Arc::new(super::Voice::new());
        let engine = Arc::new(parking_lot::Mutex::new("voisona".to_string()));
        let called = Arc::new(AtomicBool::new(false));
        let guard = voice.ensure_lock.lock(); // 別のスレッドの ensure が動いている間
        let t = {
            let (voice, engine, called) = (voice.clone(), engine.clone(), called.clone());
            std::thread::spawn(move || {
                let (_g, s) = voice.lock_and_settings(|| {
                    called.store(true, Ordering::SeqCst);
                    st(&engine.lock())
                });
                s.voice_engine
            })
        };
        std::thread::sleep(Duration::from_millis(100));
        assert!(!called.load(Ordering::SeqCst), "ロックを取る前に設定を読んだ");
        *engine.lock() = "voicevox".into(); // 待っている間に声のエンジンが変わった
        drop(guard);
        assert_eq!(t.join().unwrap(), "voicevox"); // 待った後の（新しい）設定で動く
    }

    #[test]
    fn engine_matches_by_arg_when_path_is_resolved() {
        // path は resolve 後（8.3 名が長い名前になる・.. が消える）で、設定の文字列と合わない。arg は与えた値のまま
        let ping = json!({"engine": {"name": "mine", "path": r"C:\Users\holmesfems\e\my_tts.py", "arg": r"C:\Users\HOLMES~1\x\..\e\my_tts.py"}});
        assert!(engine_matches(&ping, &st(r"C:\Users\HOLMES~1\x\..\e\my_tts.py")));
        assert!(engine_matches(&ping, &st("C:/Users/HOLMES~1/x/../e/my_tts.py")));
        // arg があれば path とは比べない
        assert!(!engine_matches(&ping, &st(r"C:\Users\holmesfems\e\my_tts.py")));
        // \?\ の接頭辞は見ない
        let ping = json!({"engine": {"name": "mine", "path": "x", "arg": r"\\?\D:\e\a.py"}});
        assert!(engine_matches(&ping, &st("D:/e/a.py")));
    }

    #[test]
    fn keep_tail_does_not_split_chars() {
        // 「拒」などの 3 バイトの文字が切れ目にかかっても落ちない（落ちると中継のエラー出力を読むスレッドが死に、中継が止まっていた）
        for pad in 0..3 {
            let mut s = "a".repeat(pad) + &"接続が拒否されました".repeat(200);
            keep_tail(&mut s, 4000, 3000);
            assert!(s.len() <= 3002 && s.len() >= 3000 - 2, "{}", s.len());
            assert!(s.ends_with("ました"));
        }
        let mut short = "短い".to_string();
        keep_tail(&mut short, 4000, 3000);
        assert_eq!(short, "短い");
    }
}
