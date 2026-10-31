//! 効果音（SE）の設定。演出の動きから自動で付く効果音を、種類（kind）ごとに「どの音を鳴らすか」「どの大きさか」を決める。
//!
//! 中身は回の `fx/se/bank.json`（fxsound.load_bank と同じ形に、`_studio` の候補・音量を足したもの）。
//! 読み書きと試聴の音づくりは演出ワーカー（akfx_worker.py の se_bank / se_bank_set / se_audition）がやる。
//! 画面（効果音の設定）と Claude（HTTP API → MCP の se_list / se_set）が同じ道を通る。

use anyhow::Context;
use crate::app::error::{bad_input, coded, partly_saved};
use ak_proto::{ErrorCode, GuideSe, SeBank, SeBankUpdate};
use std::collections::BTreeMap;
use crate::core::Core;
use anyhow::{anyhow, bail, Result};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

/// アプリの効果音の棚に置く種類（攻略動画の操作の 4 つのグループ。fxsound.SHELF_KINDS と同じ）。
/// 音は `%LOCALAPPDATA%\ak-studio\se_library\<種類>\*.wav`、どれを鳴らすか・音量は settings.json の `guide_se`
pub const SHELF_KINDS: [&str; 4] = ["guide_pre", "guide_done", "guide_precise_pre", "guide_precise_done"];

pub fn is_shelf_kind(kind: &str) -> bool {
    SHELF_KINDS.contains(&kind)
}

/// 演出ワーカーに渡す棚の場所と選び（se・se_bank・se_bank_set・se_audition の要求に足す）
pub fn shelf_params(core: &Core) -> Value {
    json!({"lib_dir": core.settings.se_library_dir(), "guide_se": core.settings.get().guide_se})
}

/// 要求に棚の場所と選びを足す
pub fn with_shelf(core: &Core, mut req: Value) -> Value {
    if let (Some(m), Value::Object(p)) = (req.as_object_mut(), shelf_params(core)) {
        m.extend(p);
    }
    req
}

const DB_RANGE: (f64, f64) = (-30.0, 24.0);

/// 棚の候補の名前か（"synth" か `<種類>/<名前>.wav`。棚の外・サブフォルダは指せない）
fn shelf_src_ok(kind: &str, src: &str) -> bool {
    if src == "synth" {
        return true;
    }
    let Some(name) = src.strip_prefix(&format!("{kind}/")) else { return false };
    !name.is_empty() && !name.contains(['/', '\\']) && name != ".." && name.to_ascii_lowercase().ends_with(".wav")
}

/// 棚の種類 1 つへの変更（se_bank_set の `{use, db, add, remove}`）を、今の選びに当てる。戻り値は新しい選び（None は合成の音・0 dB＝設定から外す）。
/// 棚の種類は 1 つの音だけ選べる。音量は選んでいる音の分だけ持つ（選び直すと 0 dB から）。add は棚の音が自動で候補に出るので何もしない。
/// remove は選んでいる音を外すと合成の音に戻す（棚のファイルは消さない）。誤りは何も変えずに和文の Err
pub fn shelf_patch(cur: Option<&GuideSe>, kind: &str, patch: &Value, lib_dir: &Path) -> Result<Option<GuideSe>> {
    let patch = match patch {
        Value::Null => return Ok(cur.cloned()),
        Value::Object(m) => m,
        _ => return Err(bad_input(format!("{kind} の設定は {{use, db, add, remove}} の形で指定してください"))),
    };
    let mut now = cur.cloned().unwrap_or_default();
    if let Some(u) = patch.get("use").filter(|v| !v.is_null()) {
        let list: Vec<&str> = u.as_array().ok_or_else(|| bad_input(format!("{kind}.use は候補の並びで指定してください")))?
            .iter().filter_map(|v| v.as_str()).collect();
        if list.len() > 1 {
            return Err(bad_input(format!("{kind} は棚の音を 1 つだけ選べます（今は {} 個指定されています）", list.len())));
        }
        let src = list.first().copied().unwrap_or("synth");
        if !shelf_src_ok(kind, src) {
            return Err(bad_input(format!("候補は \"synth\" か効果音の棚の {kind}/ の下の .wav にしてください: {src}")));
        }
        if src != "synth" {
            let f = lib_dir.join(src);
            if !f.is_file() {
                return Err(bad_input(format!("効果音の棚にファイルがありません: {src}")));
            }
            // 16bit PCM の空でない wav だけ（壊れた wav を選ぶと鳴らせず、黙って合成の音になってしまう）
            check_wav(&f).map_err(|e| bad_input(format!("{src} は効果音に使えません: {e}")))?;
        }
        if src != now.src {
            now = GuideSe { src: src.to_string(), db: 0.0 };
        }
    }
    if let Some(rm) = patch.get("remove").and_then(|v| v.as_array()) {
        if now.src != "synth" && rm.iter().any(|v| v.as_str() == Some(now.src.as_str())) {
            now = GuideSe::default();
        }
    }
    match patch.get("db") {
        None | Some(Value::Null) => {}
        Some(Value::Object(m)) => {
            for (s, v) in m {
                if !shelf_src_ok(kind, s) {
                    return Err(bad_input(format!("候補は \"synth\" か効果音の棚の {kind}/ の下の .wav にしてください: {s}")));
                }
                // 選んでいない音の音量は持たない（黙って捨てると画面の下書きが直らないので断る）
                if *s != now.src {
                    return Err(bad_input(format!("{kind} は選んでいる音の音量だけ変えられます（{s} は選ばれていません）")));
                }
                now.db = match v {
                    Value::Null => 0.0,
                    Value::Number(n) => {
                        let d = n.as_f64().unwrap_or(f64::NAN);
                        if !d.is_finite() {
                            return Err(bad_input(format!("{kind}.db の {s} は数（dB）か null で指定してください: {v}")));
                        }
                        (d.clamp(DB_RANGE.0, DB_RANGE.1) * 100.0).round() / 100.0
                    }
                    _ => return Err(bad_input(format!("{kind}.db の {s} は数（dB）か null で指定してください: {v}"))),
                };
            }
        }
        Some(_) => return Err(bad_input(format!("{kind}.db は {{候補: dB}} の形で指定してください"))),
    }
    Ok(if now.src == "synth" && now.db.abs() < 1e-6 { None } else { Some(now) })
}

/// 回の fx/se/bank.json がその種類の候補の並びを持っているか（持っていれば回のほうが勝ち、棚には行かない。ワーカーの `_is_shelf` と同じ）
fn episode_lists(fx: &Path, kind: &str) -> bool {
    let Ok(text) = std::fs::read_to_string(fx.join("se").join("bank.json")) else { return false };
    serde_json::from_str::<Value>(&text).map(|v| v.get(kind).is_some_and(Value::is_array)).unwrap_or(false)
}

/// その種類を棚（設定の guide_se・アプリの棚）で扱うか。棚の種類でも、回の bank.json に候補の並びがあれば回のほうが勝つ
fn is_shelf_for(fx: Option<&Path>, kind: &str) -> bool {
    is_shelf_kind(kind) && !fx.is_some_and(|d| episode_lists(d, kind))
}

/// 作り直しの警告（export_se の戻りの warnings）
pub fn warnings_of(r: &Value) -> Vec<String> {
    r["warnings"].as_array().map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect()).unwrap_or_default()
}

fn fx_dir(core: &Core) -> Result<PathBuf> {
    let desc = core.desc().ok_or_else(|| anyhow!(crate::msg::NO_FX))?;
    Ok(Path::new(&desc.path).parent().ok_or_else(|| anyhow!("演出のフォルダが分かりません"))?.to_path_buf())
}

/// ワーカーの返事（`se_bank`・`se_bank_set` の後の `se_bank_info`）を型で読む。形が違えば和文の Err
fn parse_bank(h: Value) -> Result<SeBank> {
    serde_json::from_value(strip(h)).map_err(|e| anyhow!("効果音の設定を読めませんでした: {e}"))
}

/// 種類ごとの候補・鳴らす音・音量・この回で鳴る時刻。
pub fn get(core: &Core) -> Result<SeBank> {
    let dir = fx_dir(core)?;
    let (h, _) = core.engine.raw(with_shelf(core, json!({"op": "se_bank", "fx_dir": dir})), Duration::from_secs(60))?;
    parse_bank(h)
}

/// 設定を変えて保存する。kinds: {種類: {use?, db?, add?, remove?}}。render なら効果音も作り直す（再生・書き出しに反映）。
pub fn set(core: &Arc<Core>, kinds: &Value, render: bool) -> Result<SeBankUpdate> {
    let Some(all) = kinds.as_object() else {
        return Err(bad_input(format!("kinds は {{種類: {{use, db, add, remove}}}} の形で指定してください")));
    };
    let dir = fx_dir(core)?;
    // 棚の種類（攻略動画の操作の 4 つのグループ）は回の bank.json ではなく設定（全部の回で使う）へ。先に全部を確かめてから書く
    let lib = core.settings.se_library_dir();
    let mut guide_se: BTreeMap<String, GuideSe> = core.settings.get().guide_se;
    let mut shelf_changed = false;
    let mut rest = serde_json::Map::new();
    for (k, patch) in all {
        if !is_shelf_for(Some(&dir), k) {
            rest.insert(k.clone(), patch.clone());
            continue;
        }
        let new = shelf_patch(guide_se.get(k), k, patch, &lib)?;
        if new.as_ref() != guide_se.get(k) {
            shelf_changed = true;
            match new {
                Some(g) => guide_se.insert(k.clone(), g),
                None => guide_se.remove(k),
            };
        }
    }
    // 回の bank.json の分を先に（誤りがあれば設定は変えない）
    let req = if rest.is_empty() {
        json!({"op": "se_bank", "fx_dir": dir})
    } else {
        json!({"op": "se_bank_set", "fx_dir": dir, "kinds": rest})
    };
    if !rest.is_empty() {
        core.engine.raw(with_shelf(core, req.clone()), Duration::from_secs(60))?;
    }
    if shelf_changed {
        if let Err(e) = core.settings.try_update(|s| s.guide_se = guide_se) {
            return Err(if rest.is_empty() {
                e.context("効果音の棚の選びを設定に保存できませんでした")
            } else {
                // 回の bank.json はもう書き換わっている。何が保存できて何ができなかったかを伝える
                partly_saved("この回の効果音の設定（bank.json）は保存しましたが、棚の種類の選びは設定に保存できませんでした", &e)
            });
        }
    }
    let (h, _) = core.engine.raw(with_shelf(core, json!({"op": "se_bank", "fx_dir": dir})), Duration::from_secs(60))?;
    let info = parse_bank(h)?;
    // 作り直した印（画面の再生はこの値で効果音の wav を読み直す。画面の呼び出しと知らせで同じ値になる）
    let rev = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0);
    let (mut wav, mut count, mut warnings) = (None, None, Vec::new());
    if render {
        let r = match crate::export::audio::export_se(core) {
            Ok(r) => r,
            Err(e) => {
                // 設定は保存できている。画面の設定は新しくして、作り直しの失敗だけを返す
                core.emit("se_bank", SeBankUpdate { bank: info, rendered: false, rev, wav: None, count: None, warnings: vec![] });
                return Err(partly_saved("効果音の設定は保存しましたが、効果音を作り直せませんでした", &e));
            }
        };
        wav = r["wav"].as_str().map(String::from);
        count = r["count"].as_u64().map(|n| n as u32);
        warnings = warnings_of(&r);
    }
    // 画面の呼び出しの戻り値と知らせ（se_bank）は同じ形・同じ値
    let update = SeBankUpdate { bank: info, rendered: render, rev, wav, count, warnings };
    core.emit("se_bank", &update);
    Ok(update)
}

static AUDITION_N: AtomicU64 = AtomicU64::new(0);

/// 試聴の wav を置く所（画面の /file から読めるように、lib.rs の allowed もここを通す）
pub fn audition_dir() -> PathBuf {
    std::env::temp_dir().join("ak-studio-se")
}

/// 候補 1 つを標準の大きさ（0 dB）で鳴らした短い wav を作る。音量は再生側でかける。
pub fn audition(core: &Core, kind: &str, src: &str) -> Result<Value> {
    let dir = fx_dir(core)?;
    let out_dir = audition_dir();
    std::fs::create_dir_all(&out_dir)?;
    let n = AUDITION_N.fetch_add(1, Ordering::Relaxed);
    if n == 0 {
        // 前に起動したときの試聴の音を片付ける（ほかに起動している AK Studio の分は 1 日は残す）
        let pre = format!("{}_", std::process::id());
        if let Ok(rd) = std::fs::read_dir(&out_dir) {
            for e in rd.flatten() {
                let name = e.file_name().to_string_lossy().to_string();
                let old = e.metadata().and_then(|m| m.modified()).ok()
                    .and_then(|t| t.elapsed().ok())
                    .map(|d| d > Duration::from_secs(24 * 3600))
                    .unwrap_or(false);
                if name.ends_with(".wav") && !name.starts_with(&pre) && old {
                    let _ = std::fs::remove_file(e.path());
                }
            }
        }
    }
    let out = out_dir.join(format!("{}_{n}.wav", std::process::id()));
    let (h, _) = core.engine.raw(with_shelf(core, json!({"op": "se_audition", "fx_dir": dir, "kind": kind, "src": src, "out": out})),
                                 Duration::from_secs(60))?;
    Ok(strip(h))
}

/// 取り込める拡張子（大文字小文字は無視）
const IMPORT_EXTS: [&str; 5] = ["wav", "mp3", "ogg", "flac", "m4a"];

fn ext_of(p: &Path) -> String {
    p.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default()
}

/// 取り込める拡張子か
fn is_importable_ext(ext: &str) -> bool {
    IMPORT_EXTS.contains(&ext.to_ascii_lowercase().as_str())
}

/// ffmpeg で変換が要るか。16bit PCM の wav はそのまま写し、それ以外（mp3 など・24bit や float の wav）は変換する。
fn needs_convert(ext: &str, wav_ok: bool) -> bool {
    !(ext.eq_ignore_ascii_case("wav") && wav_ok)
}

/// ffmpeg で 16bit PCM・48kHz・2ch の wav にする（fxsound の SR=48000）。失敗したら書きかけの dst は消す。
fn convert_to_wav(ffmpeg: &str, src: &Path, dst: &Path) -> Result<()> {
    if ffmpeg.trim().is_empty() {
        return Err(coded(ErrorCode::NeedFfmpeg, "ffmpeg が見つかりません"));
    }
    let mut c = Command::new(ffmpeg);
    c.args(["-hide_banner", "-loglevel", "error", "-y", "-i"]).arg(src)
        .args(["-vn", "-map_metadata", "-1", "-ac", "2", "-ar", "48000", "-c:a", "pcm_s16le"]).arg(dst);
    let out = crate::settings::hidden(c).output().context("ffmpeg を起動できませんでした")?;
    if !out.status.success() {
        let _ = std::fs::remove_file(dst);
        let err = String::from_utf8_lossy(&out.stderr);
        let tail: Vec<&str> = err.lines().filter(|l| !l.trim().is_empty()).collect();
        let tail = tail[tail.len().saturating_sub(3)..].join(" / ");
        bail!("音声を変換できませんでした: {} ({tail})", src.display());
    }
    if let Err(e) = check_wav(dst) {
        let _ = std::fs::remove_file(dst);
        return Err(e);
    }
    Ok(())
}

/// 取り込んだ音を置くフォルダ。棚の種類は棚の `<種類>/`（回が開いていなくても取り込める）、それ以外は回の `fx/se/<種類>/`
fn import_dir(lib: &Path, kind: &str, episode_fx: impl FnOnce() -> Result<PathBuf>) -> Result<PathBuf> {
    // 回が開いていて、その回の bank.json がその種類の候補を持っていれば回のほうへ（持っていなければ棚へ。回が開いていなくても棚には取り込める）
    let fx = episode_fx().ok();
    if is_shelf_for(fx.as_deref(), kind) {
        Ok(lib.join(kind))
    } else {
        Ok(fx.ok_or_else(|| anyhow!(crate::msg::NO_FX))?.join("se").join(kind))
    }
}

/// 手元の音声ファイル（wav・mp3・ogg・flac・m4a）を `fx/se/<種類>/` に置いて、その種類の候補にする。
/// 16bit PCM の wav はそのまま写し、それ以外は ffmpeg で 16bit の wav に変換する。
pub fn import(core: &Core, kind: &str, path: &str) -> Result<Value> {
    if kind.is_empty() || !kind.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        bail!("不明な効果音の種類です: {kind}");
    }
    let src = Path::new(path);
    let ext = ext_of(src);
    if !is_importable_ext(&ext) {
        bail!("使えるのは wav・mp3・ogg・flac・m4a です: {path}");
    }
    let convert = needs_convert(&ext, ext == "wav" && check_wav(src).is_ok());
    // 棚の種類は回ではなくアプリの棚（se_library/<種類>/。全部の回で使う）に置く
    let dst_dir = import_dir(&core.settings.se_library_dir(), kind, || fx_dir(core))?;
    std::fs::create_dir_all(&dst_dir)?;
    let stem = src.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "se".into());
    let mut dst = dst_dir.join(format!("{stem}.wav"));
    let mut i = 2;
    while dst.exists() {
        dst = dst_dir.join(format!("{stem}_{i}.wav"));
        i += 1;
    }
    if convert {
        convert_to_wav(&core.ffmpeg(), src, &dst)?;
    } else {
        std::fs::copy(src, &dst)?;
    }
    let rel = format!("{kind}/{}", dst.file_name().unwrap().to_string_lossy());
    Ok(json!({"src": rel, "path": dst, "converted": convert}))
}

/// 効果音の素材に使える wav か（fxsound.read_wav は 16bit PCM だけ読める）
fn check_wav(p: &Path) -> Result<()> {
    let r = hound::WavReader::open(p).map_err(|e| anyhow!("wav として読めません: {} ({e})", p.display()))?;
    let spec = r.spec();
    if spec.sample_format != hound::SampleFormat::Int || spec.bits_per_sample != 16 {
        let fmt = if spec.sample_format == hound::SampleFormat::Float { "浮動小数点" } else { "整数" };
        bail!("使えるのは 16bit PCM の wav だけです（これは {}bit の{fmt}）: {}", spec.bits_per_sample, p.display());
    }
    if r.duration() == 0 {
        bail!("音が入っていない wav です: {}", p.display());
    }
    Ok(())
}

fn strip(mut h: Value) -> Value {
    if let Some(m) = h.as_object_mut() {
        m.remove("id");
        m.remove("ok");
    }
    h
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lib_with(kind: &str, names: &[&str]) -> PathBuf {
        // テストは並行に走るので、呼ぶたびに別のフォルダ（同じ種類の棚を消し合わない）
        static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let d = std::env::temp_dir().join(format!("ak-shelf-test-{}-{n}-{kind}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join(kind)).unwrap();
        for n in names {
            write_tone(&d.join(kind).join(n));
        }
        d
    }

    fn write_tone(p: &Path) {
        let spec = hound::WavSpec { channels: 1, sample_rate: 48000, bits_per_sample: 16, sample_format: hound::SampleFormat::Int };
        let mut w = hound::WavWriter::create(p, spec).unwrap();
        for i in 0..480 {
            w.write_sample((i * 50) as i16).unwrap();
        }
        w.finalize().unwrap();
    }

    #[test]
    fn shelf_kinds_are_the_four_groups() {
        assert_eq!(SHELF_KINDS.len(), 4);
        assert!(is_shelf_kind("guide_precise_done") && !is_shelf_kind("blip") && !is_shelf_kind(""));
    }

    #[test]
    fn shelf_src_check() {
        assert!(shelf_src_ok("guide_done", "synth"));
        assert!(shelf_src_ok("guide_done", "guide_done/a b.WAV"));
        for bad in ["guide_pre/a.wav", "guide_done/../a.wav", "guide_done/x/a.wav", "guide_done/a.mp3", "guide_done/", "a.wav", r"guide_done\a.wav"] {
            assert!(!shelf_src_ok("guide_done", bad), "{bad}");
        }
    }

    #[test]
    fn shelf_patch_selects_and_sets_db() {
        let lib = lib_with("guide_done", &["a.wav", "b.wav"]);
        // 選ぶ（音量は 0 から）→ 音量を決める
        let a = shelf_patch(None, "guide_done", &json!({"use": ["guide_done/a.wav"]}), &lib).unwrap().unwrap();
        assert_eq!(a, GuideSe { src: "guide_done/a.wav".into(), db: 0.0 });
        let a2 = shelf_patch(Some(&a), "guide_done", &json!({"db": {"guide_done/a.wav": -3.456}}), &lib).unwrap().unwrap();
        assert_eq!(a2.db, -3.46);
        // 範囲に丸める・null で 0 に戻す
        let hi = shelf_patch(Some(&a), "guide_done", &json!({"db": {"guide_done/a.wav": 99}}), &lib).unwrap().unwrap();
        assert_eq!(hi.db, 24.0);
        let z = shelf_patch(Some(&a2), "guide_done", &json!({"db": {"guide_done/a.wav": null}}), &lib).unwrap().unwrap();
        assert_eq!(z.db, 0.0);
        // 選び直すと音量は 0 から。同じ要求の db は新しい音に効く。選んでいない音の db は持たない
        let b = shelf_patch(Some(&a2), "guide_done", &json!({"use": ["guide_done/b.wav"]}), &lib).unwrap().unwrap();
        assert_eq!(b, GuideSe { src: "guide_done/b.wav".into(), db: 0.0 });
        let b2 = shelf_patch(Some(&a2), "guide_done", &json!({"use": ["guide_done/b.wav"], "db": {"guide_done/b.wav": 2}}), &lib).unwrap().unwrap();
        assert_eq!(b2, GuideSe { src: "guide_done/b.wav".into(), db: 2.0 });
        // 選んでいない音の音量は黙って捨てずに断る
        let e = shelf_patch(Some(&a2), "guide_done", &json!({"db": {"guide_done/b.wav": 2}}), &lib).unwrap_err().to_string();
        assert!(e.contains("選んでいる音の音量だけ"), "{e}");
        let e = shelf_patch(Some(&a2), "guide_done", &json!({"use": ["guide_done/b.wav"], "db": {"guide_done/a.wav": -9}}), &lib).unwrap_err().to_string();
        assert!(e.contains("選ばれていません"), "{e}");
        // 合成の音に戻す（音量が 0 なら設定から外す。0 でなければ合成の音の音量として残す）
        assert_eq!(shelf_patch(Some(&a2), "guide_done", &json!({"use": ["synth"]}), &lib).unwrap(), None);
        assert_eq!(shelf_patch(Some(&a2), "guide_done", &json!({"use": []}), &lib).unwrap(), None);
        let s = shelf_patch(None, "guide_done", &json!({"db": {"synth": 3}}), &lib).unwrap().unwrap();
        assert_eq!(s, GuideSe { src: "synth".into(), db: 3.0 });
        // remove は選んでいる音だけ外す。add・null は何もしない
        assert_eq!(shelf_patch(Some(&a2), "guide_done", &json!({"remove": ["guide_done/a.wav"]}), &lib).unwrap(), None);
        assert_eq!(shelf_patch(Some(&a2), "guide_done", &json!({"remove": ["guide_done/b.wav"]}), &lib).unwrap(), Some(a2.clone()));
        assert_eq!(shelf_patch(Some(&a2), "guide_done", &json!({"add": ["guide_done/b.wav"]}), &lib).unwrap(), Some(a2.clone()));
        assert_eq!(shelf_patch(Some(&a2), "guide_done", &Value::Null, &lib).unwrap(), Some(a2.clone()));
        let _ = std::fs::remove_dir_all(&lib);
    }

    #[test]
    fn shelf_patch_rejects_bad_input() {
        let lib = lib_with("guide_pre", &["a.wav"]);
        let err = |p: Value| shelf_patch(None, "guide_pre", &p, &lib).unwrap_err().to_string();
        assert!(err(json!({"use": ["guide_pre/none.wav"]})).contains("ファイルがありません"));
        assert!(err(json!({"use": ["guide_done/a.wav"]})).contains("棚"));
        assert!(err(json!({"use": ["guide_pre/a.wav", "synth"]})).contains("1 つだけ"));
        assert!(err(json!({"use": "x"})).contains("候補の並び"));
        assert!(err(json!({"db": 3})).contains("{候補: dB}"));
        assert!(err(json!({"use": ["guide_pre/a.wav"], "db": {"guide_pre/a.wav": "loud"}})).contains("数"));
        assert!(err(json!("x")).contains("形"));
        let _ = std::fs::remove_dir_all(&lib);
    }

    #[test]
    fn shelf_patch_rejects_broken_wav() {
        let lib = lib_with("guide_pre", &["ok.wav"]);
        std::fs::write(lib.join("guide_pre").join("junk.wav"), b"not a wav").unwrap();
        // 16bit でない wav（32bit float）
        let spec = hound::WavSpec { channels: 1, sample_rate: 48000, bits_per_sample: 32, sample_format: hound::SampleFormat::Float };
        let mut w = hound::WavWriter::create(lib.join("guide_pre").join("f32.wav"), spec).unwrap();
        w.write_sample(0.5f32).unwrap();
        w.finalize().unwrap();
        // 空の wav
        let spec = hound::WavSpec { channels: 1, sample_rate: 48000, bits_per_sample: 16, sample_format: hound::SampleFormat::Int };
        hound::WavWriter::create(lib.join("guide_pre").join("empty.wav"), spec).unwrap().finalize().unwrap();
        for n in ["junk", "f32", "empty"] {
            let e = shelf_patch(None, "guide_pre", &json!({"use": [format!("guide_pre/{n}.wav")]}), &lib).unwrap_err().to_string();
            assert!(e.contains("効果音に使えません"), "{n}: {e}");
        }
        assert!(shelf_patch(None, "guide_pre", &json!({"use": ["guide_pre/ok.wav"]}), &lib).is_ok());
        let _ = std::fs::remove_dir_all(&lib);
    }

    #[test]
    fn episode_bank_wins_over_shelf() {
        let d = std::env::temp_dir().join(format!("ak-shelf-ep-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let fx = d.join("fx");
        std::fs::create_dir_all(fx.join("se")).unwrap();
        // bank.json が無い・その種類が無い → 棚
        assert!(is_shelf_for(Some(&fx), "guide_done"));
        std::fs::write(fx.join("se").join("bank.json"), r#"{"guide_done": ["guide_done/x.wav"], "guide_pre": "no"}"#).unwrap();
        assert!(!is_shelf_for(Some(&fx), "guide_done"), "回の bank.json に並びがあれば回が勝つ");
        assert!(is_shelf_for(Some(&fx), "guide_pre"), "並びでなければ棚");
        assert!(is_shelf_for(None, "guide_done"), "回が開いていなければ棚");
        assert!(!is_shelf_for(Some(&fx), "slam"));
        // 壊れた bank.json は棚として扱う（ワーカーが読めない理由を知らせる）
        std::fs::write(fx.join("se").join("bank.json"), b"{broken").unwrap();
        assert!(is_shelf_for(Some(&fx), "guide_done"));
        // 取り込み先も同じ
        std::fs::write(fx.join("se").join("bank.json"), r#"{"guide_done": []}"#).unwrap();
        let lib = Path::new("lib");
        assert_eq!(import_dir(lib, "guide_done", || Ok(fx.clone())).unwrap(), fx.join("se").join("guide_done"));
        assert_eq!(import_dir(lib, "guide_pre", || Ok(fx.clone())).unwrap(), lib.join("guide_pre"));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn parse_bank_reads_fixture_and_names_the_failure() {
        let mut v: Value = serde_json::from_str(include_str!("../tests/fixtures/se_bank.json")).unwrap();
        // ワーカーの返事には id・ok が付く（読むときに外す）
        v["id"] = json!(3);
        v["ok"] = json!(true);
        let b = parse_bank(v.clone()).unwrap();
        assert_eq!(b.kinds.len(), 3);
        assert_eq!(b.kinds[0].use_, ["synth", "slam/a.wav"]);
        assert!(b.kinds[2].shelf && b.db_range == [-30.0, 24.0]);
        v["kinds"][1]["bankable"] = json!("yes");
        let e = parse_bank(v).unwrap_err().to_string();
        assert!(e.starts_with("効果音の設定を読めませんでした: "), "{e}");
    }

    #[test]
    fn warnings_of_reads_strings() {
        assert_eq!(warnings_of(&json!({"warnings": ["a", 1, "b"]})), vec!["a".to_string(), "b".to_string()]);
        assert!(warnings_of(&json!({})).is_empty());
    }

    #[test]
    fn import_goes_to_shelf_for_guide_kinds() {
        let lib = Path::new("lib");
        // 棚の種類は回が開いていなくても取り込める（回を見に行かない）
        let d = import_dir(lib, "guide_pre", || Err(anyhow!("回が開いていません"))).unwrap();
        assert_eq!(d, Path::new("lib").join("guide_pre"));
        // それ以外は回の fx/se/<種類>
        let d = import_dir(lib, "slam", || Ok(PathBuf::from("ep/fx"))).unwrap();
        assert_eq!(d, Path::new("ep/fx").join("se").join("slam"));
        assert!(import_dir(lib, "slam", || Err(anyhow!("回が開いていません"))).is_err());
    }

    #[test]
    fn ext_check() {
        for e in ["wav", "MP3", "ogg", "Flac", "m4a"] {
            assert!(is_importable_ext(e), "{e}");
        }
        for e in ["", "txt", "aac", "mp4"] {
            assert!(!is_importable_ext(e), "{e}");
        }
        assert_eq!(ext_of(Path::new("a/B.MP3")), "mp3");
        assert_eq!(ext_of(Path::new("noext")), "");
    }

    #[test]
    fn convert_decision() {
        assert!(!needs_convert("wav", true));
        assert!(!needs_convert("WAV", true));
        assert!(needs_convert("wav", false)); // 24bit / float の wav
        assert!(needs_convert("mp3", true));
        assert!(needs_convert("flac", false));
    }

    #[test]
    fn missing_ffmpeg_is_error() {
        let e = convert_to_wav("", Path::new("a.mp3"), Path::new("b.wav")).unwrap_err();
        assert!(e.to_string().contains("ffmpeg が見つかりません"));
    }

    #[test]
    fn real_convert_if_ffmpeg() {
        let ok = Command::new("ffmpeg").arg("-version").output().map(|o| o.status.success()).unwrap_or(false);
        if !ok {
            eprintln!("ffmpeg が無いので skip");
            return;
        }
        let dir = std::env::temp_dir().join(format!("ak-sebank-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("t.mp3");
        let g = Command::new("ffmpeg")
            .args(["-hide_banner", "-loglevel", "error", "-y", "-f", "lavfi", "-i", "sine=frequency=440:duration=0.3"])
            .arg(&src).output().unwrap();
        if !g.status.success() {
            eprintln!("mp3 を作れないので skip");
            let _ = std::fs::remove_dir_all(&dir);
            return;
        }
        let dst = dir.join("t.wav");
        convert_to_wav("ffmpeg", &src, &dst).unwrap();
        let r = hound::WavReader::open(&dst).unwrap();
        assert_eq!(r.spec().sample_rate, 48000);
        assert_eq!(r.spec().channels, 2);
        assert_eq!(r.spec().bits_per_sample, 16);
        // 壊れた入力は失敗して dst を残さない
        let bad = dir.join("bad.mp3");
        std::fs::write(&bad, b"not audio").unwrap();
        let dst2 = dir.join("bad.wav");
        assert!(convert_to_wav("ffmpeg", &bad, &dst2).is_err());
        assert!(!dst2.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
