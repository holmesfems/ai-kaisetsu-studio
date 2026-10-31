//! VoiSona Talk との橋渡し。埋め込んだ voice/voice_server.py を裏で動かして使う
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

    /// 起動（もう動いていれば、その回に切り替えるだけ）。
    pub fn ensure(&self, s: &Settings, project: &Path) -> Result<Value> {
        let _one = self.ensure_lock.lock();
        *self.port.lock() = s.voice_port;
        let name = project.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        if let Some(p) = self.ping() {
            // もう動いている（別の回で起動したものも含む）: この回に切り替え、切り替わったかを確かめる。
            // 古い中継（台本エディタのフォルダから起動したもの）は別の辞書を使っている。辞書が割れるので使わない
            if !crate::settings::dict_path_matches(p["dict"].as_str(), &crate::settings::voice_dict_path()) {
                return Err(anyhow!(
                    "VoiSona の中継が別の辞書（{}）を使っています。古い中継（台本エディタのフォルダから起動したもの）を閉じてから再起動してください",
                    p["dict"].as_str().filter(|d| !d.is_empty()).unwrap_or("不明")
                ));
            }
            // 切り替わらないまま進むと、声が別の回の voice_cache に入り、再生は無音になる
            if p["project"].as_str() != Some(name.as_str()) {
                self.post("/api/project", &serde_json::json!({"path": project.to_string_lossy()}))
                    .with_context(|| format!("VoiSona の中継を、この回（{name}）に切り替えられませんでした"))?;
            }
            let p = self.ping().ok_or_else(|| crate::app::error::coded(ak_proto::ErrorCode::VoiceDown, format!("VoiSona の中継が応答しません（ポート {}）", s.voice_port)))?;
            if let Some(other) = p["project"].as_str().filter(|x| *x != name) {
                return Err(anyhow!(
                    "VoiSona の中継が別の回（{other}）を開いたままです。別の回で開いた中継が残っていたら閉じてください"
                ));
            }
            return Ok(p);
        }
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
            .env("AK_VOICE_DICT", &dict)
            .env("PYTHONUTF8", "1")
            .env("PYTHONIOENCODING", "utf-8")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        if !s.an_root.trim().is_empty() {
            cmd.env("AK_AN_ROOT", &s.an_root);
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
            Err(ureq::Error::Status(404, _)) => Err(crate::app::error::not_found(format!("VoiSona の中継に {path} がありません"))),
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

/// 中継に接続できなかった失敗。英語の原因は detail に残す
fn relay_down(e: &dyn std::fmt::Display) -> anyhow::Error {
    crate::app::error::coded_detail(ak_proto::ErrorCode::VoiceDown, "VoiSona の中継サーバーに接続できません", e.to_string())
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
    use super::keep_tail;

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
