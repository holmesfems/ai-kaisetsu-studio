//! アプリ全体の設定（%LOCALAPPDATA%\ak-studio\settings.json）と、外部ツールの場所。

mod embed;
mod presets;
#[cfg(test)]
mod embed_table;
mod tools;

pub use embed::*;
pub use presets::{presets, Presets};
pub use tools::*;

use crate::app::dto::SettingsPatch;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use ts_rs::TS;

/// 画面のメッセージを Claude に届ける方法
#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq, TS)]
#[serde(rename_all = "lowercase")]
#[ts(export)]
pub enum Delivery {
    /// ak-studio の Mod
    #[default]
    Mod,
    /// フックで起こす
    Hook,
    /// channels 機能
    Channel,
}

impl Delivery {
    /// 待ち受け側（`consumer`）の名前・設定ファイルの値と同じ文字列
    pub fn as_str(self) -> &'static str {
        match self {
            Delivery::Mod => "mod",
            Delivery::Hook => "hook",
            Delivery::Channel => "channel",
        }
    }
}

impl std::fmt::Display for Delivery {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// settings.json の delivery を読む。知らない値（新しい版が書いたものなど）で設定ファイル全体を
/// 読めなくしないよう、Mod にする（知らせは load_in が bad_delivery で出す）
fn delivery_or_mod<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Delivery, D::Error> {
    let v = serde_json::Value::deserialize(d)?;
    Ok(serde_json::from_value::<Delivery>(v).unwrap_or_default())
}

/// settings.json の delivery が知らない値なら、その値を返す（無い・読める値なら None）
fn bad_delivery(t: &str) -> Option<String> {
    let v = serde_json::from_str::<serde_json::Value>(t).ok()?.get("delivery")?.clone();
    serde_json::from_value::<Delivery>(v.clone()).is_err().then(|| v.to_string())
}

#[derive(Serialize, Deserialize, Clone, Debug, TS)]
#[serde(default)]
#[ts(export)]
pub struct Settings {
    /// Python（fxkit が動くもの）
    pub python: String,
    /// ffmpeg。空なら PATH → imageio-ffmpeg の順に探す
    pub ffmpeg: String,
    /// 回のフォルダが並んでいる所。空なら「回を開く」で選んでもらう（既定は無い）
    pub an_root: String,
    /// ひな形のフォルダ（回の一覧に出さない。回のフォルダが並んでいる所の中にあるときの除外用。空なら除外しない）
    pub template_dir: String,
    pub voisona_user: String,
    pub voisona_pass: String,
    pub voice_port: u16,
    /// 声のエンジン。`voisona`・`voicevox`、または自作のエンジンの .py の絶対パス（約束は docs/spec/issues-8-12.md）
    pub voice_engine: String,
    /// VOICEVOX ENGINE の URL。空なら http://127.0.0.1:50021
    pub voicevox_url: String,
    /// 演出ワーカーの数
    pub workers: usize,
    /// 演出のフレームキャッシュの上限（MB）
    pub cache_mb: usize,
    pub last_project: String,
    pub recent: Vec<String>,
    /// Claude との通信（MCP）用のローカル HTTP のポート
    pub api_port: u16,
    /// 画面のメッセージを Claude に届ける方法
    #[serde(deserialize_with = "delivery_or_mod")]
    pub delivery: Delivery,
    /// 送ったメッセージを Claude に渡すまで待つ秒数。この間なら取り消すと Claude に届かない（0 で待たない）
    pub cancel_grace: f64,
    /// 攻略動画の固定値（全部の回で使う。回ごとに変えた分は guide.json の fixed）
    pub guide_fixed: ak_proto::GuideFixed,
    /// 攻略動画の待機欄（編成の下のキャラの待機欄）の大きさ・不透明度（全部の回で使う）
    pub guide_bench: ak_proto::GuideBench,
    /// 攻略動画の操作の効果音（予備・完了 × 通常・精密の 4 つのグループ）で、アプリの効果音の棚から選んだ音（種類 → 音・音量。全部の回で使う）。
    /// 回の fx/se/bank.json にその種類があれば回のほうが勝つ
    pub guide_se: std::collections::BTreeMap<String, ak_proto::GuideSe>,
}

impl Default for Settings {
    fn default() -> Self {
        let cpus = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(8);
        Settings {
            python: "python".into(),
            ffmpeg: String::new(),
            an_root: String::new(),
            template_dir: String::new(),
            voisona_user: String::new(),
            voisona_pass: String::new(),
            voice_port: 8765,
            voice_engine: "voisona".into(),
            voicevox_url: String::new(),
            workers: (cpus / 4).clamp(2, 6),
            cache_mb: 1500,
            last_project: String::new(),
            recent: vec![],
            api_port: 47815,
            delivery: Delivery::Mod,
            cancel_grace: 3.0,
            guide_fixed: ak_proto::GuideFixed::default(),
            guide_bench: ak_proto::GuideBench::default(),
            guide_se: Default::default(),
        }
    }
}

/// 回の一覧から外すひな形のフォルダ。優先は「設定の template_dir → プリセット（presets/project.json）→ 無し（空）」
pub fn effective_template_dir(s: &Settings) -> &str {
    match s.template_dir.trim() {
        "" => presets().template_dir.as_deref().unwrap_or(""),
        t => t,
    }
}

pub fn data_dir() -> PathBuf {
    let d = dirs::data_local_dir().unwrap_or_else(std::env::temp_dir).join("ak-studio");
    let _ = std::fs::create_dir_all(&d);
    d
}

/// 起動時に画面へ出す知らせ（設定の読み込み・辞書の作成の結果）。release ではコンソールが無いので、画面の通知に出す
#[derive(Clone, Debug)]
pub struct Notice {
    pub text: String,
    pub error: bool,
}

pub struct SettingsStore {
    pub inner: RwLock<Settings>,
    dir: PathBuf,
    /// 壊れた settings.json を退避できなかったとき true。上書きしないよう保存しない
    save_blocked: std::sync::atomic::AtomicBool,
    /// まだ画面に出していない起動時の知らせ（画面が読み込み終えたら Core が出す）
    pub notices: parking_lot::Mutex<Vec<Notice>>,
}

/// 数を範囲に収める（NaN・無限は下限）。int なら四捨五入
fn clamp_num(v: f64, min: f64, max: f64, int: bool) -> f64 {
    let mut n = if v.is_finite() { v } else { min };
    if int {
        n = n.round();
    }
    n.clamp(min, max)
}

/// 声のエンジンの値を確かめる。`voisona`・`voicevox`、または存在する .py の絶対パスだけを受け付ける（前後の空白は除く）
pub fn validate_voice_engine(v: &str) -> Result<String, String> {
    let v = v.trim();
    if v == "voisona" || v == "voicevox" {
        return Ok(v.to_string());
    }
    let path = Path::new(v);
    let is_py = path.extension().is_some_and(|e| e.eq_ignore_ascii_case("py"));
    if !v.is_empty() && is_py && path.is_absolute() && path.is_file() {
        return Ok(v.to_string());
    }
    let why = if v.is_empty() {
        "値が空です".to_string()
    } else if is_py && !path.is_absolute() {
        "自作のエンジンは絶対パスで指定してください".to_string()
    } else if is_py {
        "ファイルがありません".to_string()
    } else {
        "voisona・voicevox、または .py の絶対パスを指定してください".to_string()
    };
    Err(format!("声のエンジンが見つかりません: {v}（{why}）"))
}

/// 画面から来た項目ごとの変更を設定に当てる（渡された項目だけ変える。数は範囲に丸める）。
/// 値が正しくなければ、何も変えずに Err
pub fn apply_patch(s: &mut Settings, p: &SettingsPatch) -> Result<(), String> {
    let engine = p.voice_engine.as_deref().map(validate_voice_engine).transpose()?;
    if let Some(v) = engine {
        s.voice_engine = v;
    }
    if let Some(v) = &p.voicevox_url {
        s.voicevox_url = v.trim().to_string();
    }
    if let Some(v) = &p.an_root {
        s.an_root = v.clone();
    }
    if let Some(v) = &p.template_dir {
        s.template_dir = v.clone();
    }
    if let Some(v) = &p.python {
        s.python = v.clone();
    }
    if let Some(v) = &p.ffmpeg {
        s.ffmpeg = v.clone();
    }
    if let Some(v) = &p.voisona_user {
        s.voisona_user = v.clone();
    }
    if let Some(v) = &p.voisona_pass {
        s.voisona_pass = v.clone();
    }
    if let Some(v) = p.workers {
        s.workers = clamp_num(v, 1.0, 32.0, true) as usize;
    }
    if let Some(v) = p.cache_mb {
        s.cache_mb = clamp_num(v, 100.0, 65536.0, true) as usize;
    }
    if let Some(v) = p.cancel_grace {
        s.cancel_grace = clamp_num(v, 0.0, 30.0, false);
    }
    if let Some(v) = p.delivery {
        s.delivery = v;
    }
    Ok(())
}

impl Settings {
    /// 画面に送る写し（VoiSona のパスワードの中身は空にする。設定済みかは別に送る）
    pub fn for_ui(&self) -> Settings {
        Settings { voisona_pass: String::new(), ..self.clone() }
    }

    /// 取り消しの猶予（0〜30 秒に収める）
    pub fn cancel_hold(&self) -> std::time::Duration {
        let g = if self.cancel_grace.is_finite() { self.cancel_grace.clamp(0.0, 30.0) } else { 0.0 };
        std::time::Duration::from_secs_f64(g)
    }
}

/// 壊れた設定ファイルの退避先の名前（settings.json.broken-<日時>）
fn broken_name(p: &Path) -> String {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    format!("{}.broken-{secs}", p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default())
}

/// 読み上げ辞書が無ければ空の辞書を作る（初めて使う PC）。作ったときだけ知らせを返す。
/// 古い台本エディタのフォルダからの移し替えは 旧#173 でやめた（新しい場所の辞書へ移し終えている）
pub(crate) fn ensure_dict_file(new: &Path) -> Result<bool, String> {
    if new.exists() {
        return Ok(false);
    }
    if let Some(parent) = new.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("{} を作れませんでした: {}", parent.display(), crate::app::error::reason(e)))?;
    }
    write_atomic(new, "{
 \"entries\": []
}
")
        .map_err(|e| format!("読み上げ辞書（{}）を作れませんでした: {}", new.display(), crate::app::error::reason(e)))?;
    Ok(true)
}

impl SettingsStore {
    pub fn load() -> Self {
        let st = Self::load_in(&data_dir(), &voice_dict_path());
        // 読めないプリセットは黙らず知らせる（素の既定で動く）
        st.notices.lock().extend(presets().warnings.iter().map(|w| Notice { text: w.clone(), error: true }));
        st
    }

    /// dir の settings.json を読み、辞書が無ければ空の辞書を作ってから保存する。
    /// 読めない settings.json は既定値で上書きせず、settings.json.broken-<日時> に退避して知らせる
    pub(crate) fn load_in(dir: &Path, dict: &Path) -> Self {
        let p = dir.join("settings.json");
        let mut notices = vec![];
        let mut blocked = false;
        let raw = match std::fs::read_to_string(&p) {
            Ok(t) => Some(t),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => {
                let e = crate::app::error::reason(e);
                notices.push(Notice {
                    text: format!("設定ファイル {} を読めませんでした: {e}。上書きせず、既定の設定で始めます（このあいだ設定は保存されません）", p.display()),
                    error: true,
                });
                blocked = true;
                None
            }
        };
        let mut s = Settings::default();
        if let Some(t) = raw.as_deref() {
            match serde_json::from_str::<Settings>(t) {
                Ok(v) => {
                    s = v;
                    if let Some(d) = bad_delivery(t) {
                        // 保存で元の値が消えるので、先に写しを残す
                        let bak = p.with_file_name(broken_name(&p));
                        let text = match std::fs::copy(&p, &bak) {
                            Ok(_) => format!("設定ファイルのメッセージの送信方法（{d}）が読めないので、Mod にしました。元のファイルは {} に退避しました", bak.display()),
                            Err(e) => {
                                blocked = true;
                                let e = crate::app::error::reason(e);
                                format!("設定ファイルのメッセージの送信方法（{d}）が読めないので Mod にしました。退避できなかったため（{e}）、{} は上書きしません（このあいだ設定は保存されません）", p.display())
                            }
                        };
                        notices.push(Notice { text, error: true });
                    }
                }
                Err(e) => {
                    let e = crate::app::error::reason(e);
                    let bak = p.with_file_name(broken_name(&p));
                    match std::fs::rename(&p, &bak) {
                        Ok(()) => notices.push(Notice {
                            text: format!("設定ファイルを読めませんでした（{e}）。{} に退避して、既定の設定で始めます", bak.display()),
                            error: true,
                        }),
                        Err(e2) => {
                            blocked = true;
                            let e2 = crate::app::error::reason(e2);
                            notices.push(Notice {
                                text: format!(
                                    "設定ファイルを読めず（{e}）、退避もできませんでした（{e2}）。{} は上書きせず、既定の設定で始めます（このあいだ設定は保存されません）",
                                    p.display()
                                ),
                                error: true,
                            });
                        }
                    }
                }
            }
        }
        if s.voisona_user.is_empty() {
            // 環境変数から引き継ぐ（settings.json に書いてあればそちらが優先）
            if let Ok(u) = std::env::var("VOISONA_USER") {
                s.voisona_user = u;
            }
            if let Ok(pw) = std::env::var("VOISONA_PASS") {
                s.voisona_pass = pw;
            }
        }
        match ensure_dict_file(dict) {
            Ok(true) => notices.push(Notice { text: format!("新しい読み上げ辞書（{}）を作りました", dict.display()), error: false }),
            Ok(false) => {}
            Err(text) => notices.push(Notice { text, error: true }),
        }
        let st = SettingsStore {
            inner: RwLock::new(s),
            dir: dir.to_path_buf(),
            save_blocked: std::sync::atomic::AtomicBool::new(blocked),
            notices: parking_lot::Mutex::new(notices),
        };
        st.save();
        st
    }

    /// 起動時の知らせを取り出す（取り出したら空になる）
    pub fn take_notices(&self) -> Vec<Notice> {
        std::mem::take(&mut *self.notices.lock())
    }

    pub fn get(&self) -> Settings {
        self.inner.read().clone()
    }

    /// アプリの効果音の棚（`se_library/<種類>/*.wav`。設定のフォルダの隣。全部の回で使う）
    pub fn se_library_dir(&self) -> PathBuf {
        self.dir.join("se_library")
    }

    pub fn update(&self, f: impl FnOnce(&mut Settings)) {
        f(&mut self.inner.write());
        self.save();
    }

    /// 設定を変えて保存し、保存できなければ Err（画面に出す保存の操作から使う。`update` は失敗を返さない）
    pub fn try_update(&self, f: impl FnOnce(&mut Settings)) -> anyhow::Result<()> {
        f(&mut self.inner.write());
        if self.save_blocked.load(std::sync::atomic::Ordering::Relaxed) {
            anyhow::bail!("settings.json が読めなかったので、上書きしないよう保存を止めています（今の値はこの起動の間だけ効きます）");
        }
        let p = self.dir.join("settings.json");
        let t = serde_json::to_string_pretty(&*self.inner.read())?;
        write_atomic(&p, t).map_err(|e| anyhow::anyhow!("{}: {e}", p.display()))
    }

    pub fn save(&self) {
        if self.save_blocked.load(std::sync::atomic::Ordering::Relaxed) {
            return;
        }
        let p = self.dir.join("settings.json");
        if let Ok(t) = serde_json::to_string_pretty(&*self.inner.read()) {
            let _ = write_atomic(&p, t);
        }
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("ak-voice-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// 古い settings.json に script_tools・legacy_script_tools が残っていても読める（無視され、保存し直すと消える）
    #[test]
    fn old_settings_with_script_tools_still_load() {
        let raw = r#"{"python":"py","script_tools":"D:/x/script_editor","legacy_script_tools":"D:/y","voice_port":9000,"an_root":"D:/an"}"#;
        let s: Settings = serde_json::from_str(raw).unwrap();
        assert_eq!((s.python.as_str(), s.voice_port, s.an_root.as_str()), ("py", 9000, "D:/an"));
        let out = serde_json::to_string(&s).unwrap();
        assert!(!out.contains("script_tools"));
    }

    /// 設定ファイルに回のフォルダがあればそのまま使い、無ければ空（既定の場所は持たない）
    #[test]
    fn an_root_default_is_empty_and_saved_value_kept() {
        assert_eq!(Settings::default().an_root, "");
        assert_eq!(Settings::default().template_dir, "");
        let d = tmp("anroot");
        let data = d.join("data");
        std::fs::create_dir_all(&data).unwrap();
        std::fs::write(data.join("settings.json"), r#"{"an_root":"D:/an"}"#).unwrap();
        let st = SettingsStore::load_in(&data, &data.join("voice_dict.json"));
        assert_eq!(st.get().an_root, "D:/an");
        assert!(std::fs::read_to_string(data.join("settings.json")).unwrap().contains("D:/an"));
        let st2 = SettingsStore::load_in(&d.join("empty"), &d.join("empty").join("voice_dict.json"));
        assert_eq!(st2.get().an_root, "");
        let _ = std::fs::remove_dir_all(&d);
    }

    /// 古い場所の記録も古い辞書も無い（初めて使う PC）: 空の辞書を作って 1 回だけ知らせる
    #[test]
    fn fresh_pc_creates_empty_dict_once() {
        let d = tmp("fresh");
        let data = d.join("data");
        std::fs::create_dir_all(&data).unwrap();
        let dict = data.join("voice_dict.json");
        let st = SettingsStore::load_in(&data, &dict);
        let n = st.take_notices();
        assert!(n.iter().any(|x| !x.error && x.text.contains("作りました")), "{n:?}");
        let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&dict).unwrap()).unwrap();
        assert_eq!(v["entries"], serde_json::json!([]));
        let st2 = SettingsStore::load_in(&data, &dict);
        assert!(st2.take_notices().is_empty(), "2 回目は知らせない");
        let _ = std::fs::remove_dir_all(&d);
    }

    /// Delivery は小文字の名前で読み書きし、知らない値は設定ファイル全体を壊さず Mod になる
    #[test]
    fn guide_se_defaults_empty_and_round_trips() {
        // 古い settings.json（guide_se が無い）はそのまま読める
        let old: Settings = serde_json::from_str(r#"{"python": "python", "workers": 3}"#).unwrap();
        assert!(old.guide_se.is_empty() && old.workers == 3);
        let mut s = Settings::default();
        s.guide_se.insert("guide_done".into(), ak_proto::GuideSe { src: "guide_done/a.wav".into(), db: -2.5 });
        let t = serde_json::to_string(&s).unwrap();
        let back: Settings = serde_json::from_str(&t).unwrap();
        assert_eq!(back.guide_se["guide_done"].src, "guide_done/a.wav");
        assert_eq!(back.guide_se["guide_done"].db, -2.5);
    }

    #[test]
    fn se_library_is_next_to_settings() {
        let dir = tmp("se-lib");
        let st = SettingsStore::load_in(&dir, &dir.join("voice_dict.json"));
        assert_eq!(st.se_library_dir(), dir.join("se_library"));
        // 選びは settings.json に保存され、開き直しても残る
        st.try_update(|s| { s.guide_se.insert("guide_pre".into(), ak_proto::GuideSe { src: "guide_pre/x.wav".into(), db: 1.0 }); }).unwrap();
        let st2 = SettingsStore::load_in(&dir, &dir.join("voice_dict.json"));
        assert_eq!(st2.get().guide_se["guide_pre"].src, "guide_pre/x.wav");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn delivery_serde_round_trip_and_unknown() {
        for (d, name) in [(Delivery::Mod, "mod"), (Delivery::Hook, "hook"), (Delivery::Channel, "channel")] {
            assert_eq!(serde_json::to_value(d).unwrap(), serde_json::json!(name));
            assert_eq!(serde_json::from_value::<Delivery>(serde_json::json!(name)).unwrap(), d);
            assert_eq!(d.as_str(), name);
        }
        assert!(serde_json::from_value::<Delivery>(serde_json::json!("zzz")).is_err());
        let s: Settings = serde_json::from_str(r#"{"delivery":"zzz","python":"py2","workers":5}"#).unwrap();
        assert_eq!((s.delivery, s.python.as_str(), s.workers), (Delivery::Mod, "py2", 5));
        let s: Settings = serde_json::from_str(r#"{"delivery":"hook"}"#).unwrap();
        assert_eq!(s.delivery, Delivery::Hook);
        let s: Settings = serde_json::from_str(r#"{"delivery":7}"#).unwrap();
        assert_eq!(s.delivery, Delivery::Mod);
        assert_eq!(bad_delivery(r#"{"delivery":"zzz"}"#).as_deref(), Some("\"zzz\""));
        assert_eq!(bad_delivery(r#"{"delivery":"hook"}"#), None);
        assert_eq!(bad_delivery(r#"{"python":"py"}"#), None);
    }

    /// 声のエンジンの値: 名前・存在する .py の絶対パスだけ通す
    #[test]
    fn validate_voice_engine_accepts_names_and_existing_py() {
        assert_eq!(validate_voice_engine("voisona").as_deref(), Ok("voisona"));
        assert_eq!(validate_voice_engine(" voicevox ").as_deref(), Ok("voicevox"));
        let dir = std::env::temp_dir().join(format!("ak-engine-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let py = dir.join("my_tts.py");
        std::fs::write(&py, "# engine").unwrap();
        let py_s = py.to_string_lossy().to_string();
        assert_eq!(validate_voice_engine(&py_s).as_deref(), Ok(py_s.as_str()));
        // 無いパス・.py でないもの・相対パス・知らない名前・空
        let missing = dir.join("none_tts.py").to_string_lossy().to_string();
        for bad in [missing.as_str(), "zzz", "", "my_tts.py", &dir.to_string_lossy()] {
            let e = validate_voice_engine(bad).unwrap_err();
            assert!(e.starts_with("声のエンジンが見つかりません: "), "{bad}: {e}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// パッチの声のエンジン: 正しくなければ何も変えずに弾く
    #[test]
    fn apply_patch_voice_engine_fields() {
        let mut s = Settings::default();
        assert_eq!((s.voice_engine.as_str(), s.voicevox_url.as_str()), ("voisona", ""));
        apply_patch(&mut s, &SettingsPatch { voice_engine: Some("voicevox".into()), voicevox_url: Some(" http://127.0.0.1:50022 ".into()), ..Default::default() }).unwrap();
        assert_eq!((s.voice_engine.as_str(), s.voicevox_url.as_str()), ("voicevox", "http://127.0.0.1:50022"));
        let before = s.clone();
        let e = apply_patch(&mut s, &SettingsPatch { voice_engine: Some("nope".into()), voicevox_url: Some("x".into()), workers: Some(5.0), ..Default::default() }).unwrap_err();
        assert!(e.contains("声のエンジンが見つかりません"));
        assert_eq!((s.voice_engine.as_str(), s.voicevox_url.as_str(), s.workers), (before.voice_engine.as_str(), before.voicevox_url.as_str(), before.workers));
        // 古い settings.json（項目が無い）は既定で読める
        let old: Settings = serde_json::from_str(r#"{"python":"py"}"#).unwrap();
        assert_eq!((old.voice_engine.as_str(), old.voicevox_url.as_str()), ("voisona", ""));
    }

    /// パッチは渡した項目だけを変える。数は範囲に丸める
    #[test]
    fn apply_patch_changes_only_given_fields() {
        let mut s = Settings { last_project: "E:/an/x".into(), recent: vec!["a".into()], python: "py".into(), ..Settings::default() };
        apply_patch(&mut s, &SettingsPatch { cache_mb: Some(1e9), workers: Some(2.6), ..Default::default() }).unwrap();
        assert_eq!(s.cache_mb, 65536);
        assert_eq!(s.workers, 3);
        assert_eq!(s.last_project, "E:/an/x");
        assert_eq!(s.recent, vec!["a".to_string()]);
        assert_eq!(s.python, "py");
        apply_patch(&mut s, &SettingsPatch { workers: Some(0.0), cancel_grace: Some(f64::NAN), delivery: Some(Delivery::Hook), ..Default::default() }).unwrap();
        assert_eq!((s.workers, s.cancel_grace, s.delivery.as_str()), (1, 0.0, "hook"));
        apply_patch(&mut s, &SettingsPatch { cancel_grace: Some(99.0), ..Default::default() }).unwrap();
        assert_eq!((s.delivery, s.cancel_grace), (Delivery::Hook, 30.0));
        apply_patch(&mut s, &SettingsPatch { python: Some("p2".into()), ..Default::default() }).unwrap();
        assert_eq!(s.python, "p2");
        // パスワード: None は変えない・Some(非空) は書き換え・Some("") は消す
        s.voisona_pass = "a".into();
        apply_patch(&mut s, &SettingsPatch::default()).unwrap();
        assert_eq!(s.voisona_pass, "a");
        apply_patch(&mut s, &SettingsPatch { voisona_pass: Some("b".into()), ..Default::default() }).unwrap();
        assert_eq!(s.voisona_pass, "b");
        apply_patch(&mut s, &SettingsPatch { voisona_pass: Some(String::new()), ..Default::default() }).unwrap();
        assert_eq!(s.voisona_pass, "");
        assert_eq!(s.workers, 1);
    }

    /// 読めない settings.json は既定値で上書きせず、settings.json.broken-<日時> に退避する
    #[test]
    fn broken_settings_is_backed_up_not_overwritten() {
        let d = tmp("broken");
        let text = r#"{"python": "py", 壊れた"#;
        std::fs::write(d.join("settings.json"), text).unwrap();
        let st = SettingsStore::load_in(&d, &d.join("voice_dict.json"));
        let n = st.take_notices();
        assert!(n.iter().any(|x| x.error && x.text.contains("退避")), "{n:?}");
        // serde の英語ではなく、和文の書式の案内
        assert!(n.iter().any(|x| x.text.contains("書式が正しくありません（行 1、")), "{n:?}");
        assert!(n.iter().all(|x| !x.text.contains("expected") && !x.text.contains("at line")), "{n:?}");
        let baks: Vec<_> = std::fs::read_dir(&d)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
            .filter(|n| n.starts_with("settings.json.broken-"))
            .collect();
        assert_eq!(baks.len(), 1, "{baks:?}");
        assert_eq!(std::fs::read_to_string(d.join(&baks[0])).unwrap(), text, "壊れた中身がそのまま残る");
        assert_eq!(st.get().python, "python", "既定値で始まる");
        assert!(serde_json::from_str::<Settings>(&std::fs::read_to_string(d.join("settings.json")).unwrap()).is_ok());
        let _ = std::fs::remove_dir_all(&d);
    }

    /// 知らない送信方法は Mod にして他の項目は保ち、元のファイルを退避して知らせる
    #[test]
    fn unknown_delivery_is_backed_up() {
        let d = tmp("delivery");
        let text = r#"{"python":"py","delivery":"zzz"}"#;
        std::fs::write(d.join("settings.json"), text).unwrap();
        std::fs::write(d.join("voice_dict.json"), b"{}").unwrap();
        let st = SettingsStore::load_in(&d, &d.join("voice_dict.json"));
        let n = st.take_notices();
        assert!(n.iter().any(|x| x.error && x.text.contains("zzz") && x.text.contains("退避")), "{n:?}");
        assert_eq!((st.get().python.as_str(), st.get().delivery), ("py", Delivery::Mod));
        let bak = std::fs::read_dir(&d)
            .unwrap()
            .map(|e| e.unwrap().path())
            .find(|p| p.file_name().unwrap().to_string_lossy().starts_with("settings.json.broken-"))
            .expect("退避の写し");
        assert_eq!(std::fs::read_to_string(bak).unwrap(), text, "元の値が残る");
        let _ = std::fs::remove_dir_all(&d);
    }

    /// 読み込めた settings.json は壊れた扱いにしない（知らせも退避も無い）
    #[test]
    fn good_settings_is_untouched() {
        let d = tmp("good");
        std::fs::write(d.join("settings.json"), r#"{"python":"py"}"#).unwrap();
        std::fs::write(d.join("voice_dict.json"), b"{}").unwrap();
        let st = SettingsStore::load_in(&d, &d.join("voice_dict.json"));
        assert!(st.take_notices().is_empty());
        assert_eq!(st.get().python, "py");
        assert!(std::fs::read_dir(&d).unwrap().all(|e| !e.unwrap().file_name().to_string_lossy().contains("broken")));
        let _ = std::fs::remove_dir_all(&d);
    }
}
