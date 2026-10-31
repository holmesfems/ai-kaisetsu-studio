//! 字幕。台本のセリフ（声の並び）を、Python（PIL、akfx_worker.py の subtitle）で絵にして重ねる。
//!
//! - モニター・完成版 mp4・確認画像（Claude 用）・字幕の透過 .mov が全部同じ絵を使う（見た目が揃う）
//! - 見た目は 3 段: 回の共通（doc.subtitle）→ 話者ごと（doc.subtitle.speakers[話者]）→ セリフごと（doc.subtitle_lines[鍵]）
//! - セリフごとの鍵は「話者 \u0001 字幕の文」。行の id は読み直すたびに変わるので使わない。
//!   台本の文が変わったら効かなくなる（孤立。画面の「この回の設定」で数が見え、掃除できる）
//! - 絵は (文, 見た目, 倍率) ごとに覚える。演出のコマのキャッシュ（cache.rs）とは別
//! - 字幕だけのワーカー（演出の読み込みは要らない）を数本、必要なときに起こして使う

use crate::voice::layout::VoiceItem;
use crate::render::engine::{merge_warnings, push_log, spawn_proc, Proc, Warning};
use crate::render::media::Rgba;
use crate::project::ProjectDoc;
use anyhow::{anyhow, bail, Result};
use parking_lot::{Condvar, Mutex};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::hash_map::DefaultHasher;
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use ts_rs::TS;

/// 声が終わってから字幕を残す長さ（秒）。今までのチャンネルの字幕と同じ
pub const TAIL: f64 = 0.3;

/// 字幕の見た目。既定は、プリセット（`presets/subtitle.json`。ビルドに埋め込む。旧#173）があればそれ、無ければ素の既定（白・黒の縁）。
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, TS)]
#[serde(default = "SubStyle::plain")]
#[ts(export)]
pub struct SubStyle {
    /// フォントのファイル（.ttc は「パス#番号」）
    pub font: String,
    /// 太さ（可変フォントのときだけ効く）
    pub weight: f64,
    pub size: f64,
    /// 文字の色・縁の色（#rrggbb / #rrggbbaa）
    pub color: String,
    pub stroke: String,
    pub stroke_w: f64,
    /// 影の濃さ（0 = 影なし）
    pub shadow: f64,
    pub shadow_color: String,
    pub shadow_x: f64,
    pub shadow_y: f64,
    pub shadow_blur: f64,
    /// 最後の行の下（1 行目の上 = y - line_h × 行数）
    pub y: f64,
    /// 行の間隔
    pub line_h: f64,
    /// 左右の余白（この内側に収める。折り返しの幅もこれ）
    pub margin_l: f64,
    pub margin_r: f64,
    /// center / left / right
    #[ts(type = "\"center\" | \"left\" | \"right\"")]
    pub align: String,
    /// 幅を超えたら自動で折り返す（手で改行した行は折り返さない）
    pub wrap: bool,
}

impl SubStyle {
    /// 素の既定（白い文字・黒の縁）。プリセットの有無に関わらず同じ
    pub fn plain() -> SubStyle {
        SubStyle {
            font: "C:/Windows/Fonts/NotoSansJP-VF.ttf".into(),
            weight: 900.0,
            size: 54.0,
            color: "#ffffff".into(),
            stroke: "#000000".into(),
            stroke_w: 6.0,
            shadow: 0.0,
            shadow_color: "#000000".into(),
            shadow_x: 4.0,
            shadow_y: 4.0,
            shadow_blur: 6.0,
            y: 1060.0,
            line_h: 70.0,
            margin_l: 210.0,
            margin_r: 210.0,
            align: "center".into(),
            wrap: true,
        }
    }

    /// 素の既定にプリセットの項目を重ねたもの。型違いの項目があれば Err
    pub fn from_preset(preset: &serde_json::Map<String, Value>) -> Result<SubStyle, serde_json::Error> {
        let mut m = match serde_json::to_value(SubStyle::plain()) {
            Ok(Value::Object(m)) => m,
            _ => serde_json::Map::new(),
        };
        for (k, v) in preset {
            m.insert(k.clone(), v.clone());
        }
        serde_json::from_value(Value::Object(m))
    }
}

impl Default for SubStyle {
    /// 優先の順は「回の設定 → ユーザーの設定 → プリセット → 素の既定」の下 2 つ（上 2 つは呼び出し側が重ねる）
    fn default() -> Self {
        static D: std::sync::OnceLock<SubStyle> = std::sync::OnceLock::new();
        D.get_or_init(|| {
            crate::settings::presets()
                .subtitle
                .as_ref()
                .and_then(|m| SubStyle::from_preset(m).ok())
                .unwrap_or_else(SubStyle::plain)
        })
        .clone()
    }
}

/// 1 枚の字幕を描くのに要るもの。
#[derive(Clone, Debug)]
pub struct SubJob {
    /// 描く文（手で改行したときは \n 入り）
    pub text: String,
    /// 改行は text のとおり（自動で折り返さない）
    pub manual: bool,
    /// 字幕だけ文を書き換えている（台本の文と違う）
    pub custom: bool,
    pub style: SubStyle,
}

impl SubJob {
    fn key(&self) -> u64 {
        let mut h = DefaultHasher::new();
        self.text.hash(&mut h);
        self.manual.hash(&mut h);
        serde_json::to_string(&self.style).unwrap_or_default().hash(&mut h);
        h.finish()
    }

    fn key_at(&self, scale: f64) -> u64 {
        let mut h = DefaultHasher::new();
        self.key().hash(&mut h);
        ((scale * 10000.0).round() as i64).hash(&mut h);
        h.finish()
    }

    fn req(&self) -> Value {
        // fallback_font: 読めないフォントの代わり（ワーカーは既定の見た目を持たず、ここで教える）
        json!({"text": self.text, "manual": self.manual, "style": self.style, "fallback_font": SubStyle::default().font})
    }
}

/// 字幕の行の位置（1920x1080 の座標）。はみ出しの判定と、モニターの目安線に使う。
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct SubInfo {
    /// 行ごとの [x0, y0, x1, y1]（文字の送り幅。縁は含まない）
    pub lines: Vec<[f64; 4]>,
    pub texts: Vec<String>,
    pub overflow: Vec<bool>,
    /// 収めるべき左右の線
    pub l: f64,
    pub r: f64,
}

impl SubInfo {
    pub fn any_overflow(&self) -> bool {
        self.overflow.iter().any(|x| *x)
    }
}

/// 描いた字幕（透明でない所だけ）。
pub struct SubBitmap {
    pub x: i64,
    pub y: i64,
    pub w: u32,
    pub h: u32,
    lz: Vec<u8>,
    raw_len: usize,
    pub info: Arc<SubInfo>,
}

impl SubBitmap {
    pub fn rgba(&self) -> Rgba {
        Rgba { w: self.w, h: self.h, data: lz4_flex::decompress(&self.lz, self.raw_len).unwrap_or_default() }
    }

    fn bytes(&self) -> usize {
        self.lz.len() + 128
    }

    pub fn is_empty(&self) -> bool {
        self.w == 0 || self.h == 0
    }
}

// ======================================================================= 見た目の決め方
/// セリフごとの設定の鍵（話者 + 字幕の文）。
pub fn line_key(sp: &str, text: &str) -> String {
    format!("{sp}\u{1}{text}")
}

/// patch の値を base に重ねる（型が既定と同じものだけ。変な値で全部が既定に戻らないように）。
fn merge(base: &mut serde_json::Map<String, Value>, patch: &Value, skip: &[&str]) {
    let Some(p) = patch.as_object() else { return };
    for (k, v) in p {
        if skip.contains(&k.as_str()) || v.is_null() {
            continue;
        }
        let ok = match base.get(k) {
            Some(Value::Number(_)) => v.is_number(),
            Some(Value::String(_)) => v.is_string(),
            Some(Value::Bool(_)) => v.is_boolean(),
            _ => false,
        };
        if ok {
            base.insert(k.clone(), v.clone());
        }
    }
}

/// 字幕のトラックを隠しているか。
pub fn hidden(doc: &ProjectDoc) -> bool {
    doc.subtitle.get("hidden").and_then(|v| v.as_bool()).unwrap_or(false)
}

/// 回の字幕が使うフォント（既定・共通・話者ごと・セリフごと）。字幕を非表示にしていれば空。
pub fn fonts_in_use(doc: &ProjectDoc) -> HashSet<String> {
    let mut out = HashSet::new();
    if hidden(doc) {
        return out;
    }
    let font = |v: &Value| v.get("font").and_then(|f| f.as_str()).map(str::to_string);
    out.insert(font(&doc.subtitle).unwrap_or_else(|| SubStyle::default().font));
    let nested = |v: Option<&Value>| v.and_then(|s| s.as_object()).map(|m| m.values().filter_map(font).collect::<Vec<_>>());
    out.extend(nested(doc.subtitle.get("speakers")).unwrap_or_default());
    out.extend(nested(Some(&doc.subtitle_lines)).unwrap_or_default());
    out
}

/// 既定に doc.subtitle（speakers・hidden 以外）を重ねた Map（見た目の項目だけ）。
fn track_map(doc: &ProjectDoc) -> serde_json::Map<String, Value> {
    let mut m = match serde_json::to_value(SubStyle::default()) {
        Ok(Value::Object(m)) => m,
        _ => serde_json::Map::new(),
    };
    merge(&mut m, &doc.subtitle, &["speakers", "hidden"]);
    m
}

/// 話者の上書きまで重ねた Map。
fn speaker_map(doc: &ProjectDoc, sp: &str) -> serde_json::Map<String, Value> {
    let mut m = track_map(doc);
    if let Some(o) = doc.subtitle.get("speakers").and_then(|s| s.get(sp)) {
        merge(&mut m, o, &[]);
    }
    m
}

fn style_of(m: serde_json::Map<String, Value>) -> SubStyle {
    serde_json::from_value(Value::Object(m)).unwrap_or_default()
}

/// 回の共通の見た目（既定に doc.subtitle を重ねたもの）。
pub fn track_style(doc: &ProjectDoc) -> SubStyle {
    style_of(track_map(doc))
}

/// 話者の見た目（共通に話者の上書きを重ねたもの）。
pub fn speaker_style(doc: &ProjectDoc, sp: &str) -> SubStyle {
    style_of(speaker_map(doc, sp))
}

/// 共通 → 話者 → セリフの順に重ねた見た目と、描く文。
pub fn resolve(doc: &ProjectDoc, it: &VoiceItem) -> SubJob {
    let mut m = speaker_map(doc, &it.sp);
    let lo = doc.subtitle_lines.get(line_key(&it.sp, &it.text));
    if let Some(lo) = lo {
        merge(&mut m, lo, &["text"]);
    }
    let style = style_of(m);
    let own = lo
        .and_then(|o| o.get("text"))
        .and_then(|v| v.as_str())
        .map(|s| s.replace("\r\n", "\n"))
        .filter(|s| !s.trim().is_empty());
    match own {
        Some(t) => SubJob { manual: t.contains('\n'), custom: t.replace('\n', "") != it.text, text: t, style },
        None => SubJob { text: it.text.clone(), manual: false, custom: false, style },
    }
}

/// t に出ている字幕のセリフ（重なっていたら後から始まった方）。
pub fn active(items: &[VoiceItem], t: f64) -> Option<&VoiceItem> {
    items
        .iter()
        .filter(|it| it.start <= t && t < it.start + it.dur + TAIL)
        .max_by(|a, b| a.start.partial_cmp(&b.start).unwrap_or(std::cmp::Ordering::Equal))
}

/// 字幕を出す区間（次のセリフの頭で切る）。items は開始の順。
pub fn spans(items: &[VoiceItem]) -> Vec<(f64, f64)> {
    items
        .iter()
        .enumerate()
        .map(|(i, it)| {
            let mut b = it.start + it.dur + TAIL;
            if let Some(n) = items[i + 1..].iter().find(|n| n.start > it.start) {
                b = b.min(n.start);
            }
            (it.start, b.max(it.start + 0.05))
        })
        .collect()
}

/// 孤立したセリフごとの設定（今の台本のどの行にも当たらない鍵）。
pub fn orphans(doc: &ProjectDoc, items: &[VoiceItem]) -> Vec<String> {
    let live: std::collections::HashSet<String> = items.iter().map(|it| line_key(&it.sp, &it.text)).collect();
    doc.subtitle_lines
        .as_object()
        .map(|o| o.keys().filter(|k| !live.contains(*k)).cloned().collect())
        .unwrap_or_default()
}

// ======================================================================= 画面に出す見た目と区間
/// 画面に出す字幕の見た目と区間（subs_view）。重ね方・区間・鍵はここでだけ決め、画面は結果を表示する。
#[derive(Serialize, Clone, Debug, PartialEq, TS)]
#[ts(export)]
pub struct SubView {
    /// 既定（チャンネルの標準）
    pub default: SubStyle,
    /// 回の共通（既定に doc.subtitle を重ねたもの）
    pub track: SubStyle,
    /// 話者ごと（台本に出る話者と doc.subtitle.speakers の話者）。共通に話者の上書きを重ねたもの
    pub speakers: BTreeMap<String, SubStyle>,
    /// 行の id → そのセリフの字幕
    pub lines: BTreeMap<String, SubLineView>,
    /// 最後の字幕が消える時刻（字幕が無ければ 0）
    pub end: f64,
    /// 孤立したセリフごとの設定の鍵（今の台本のどの行にも当たらないもの）
    pub orphans: Vec<String>,
}

/// 1 セリフの字幕（見た目と区間）。
#[derive(Serialize, Clone, Debug, PartialEq, TS)]
#[ts(export)]
pub struct SubLineView {
    /// セリフごとの設定の鍵（`line_key`）
    pub key: String,
    /// 共通 → 話者 → セリフを重ねた見た目
    pub style: SubStyle,
    /// 描く文（手で改行・字幕だけの書き換えを反映）
    pub text: String,
    /// 改行は text のとおり（自動で折り返さない）
    pub manual: bool,
    /// 字幕だけ文を書き換えている（台本の文と違う）
    pub custom: bool,
    /// 字幕を出す区間（秒）
    pub a: f64,
    pub b: f64,
}

/// 画面に出す字幕の見た目と区間。items は並びのまま渡してよい（ここで開始の順に並べる）。
pub fn view(doc: &ProjectDoc, items: &[VoiceItem]) -> SubView {
    let mut sorted: Vec<VoiceItem> = items.to_vec();
    sorted.sort_by(|a, b| a.start.partial_cmp(&b.start).unwrap_or(std::cmp::Ordering::Equal).then(a.n.cmp(&b.n)));
    let mut speakers = BTreeMap::new();
    let mut names: Vec<&str> = sorted.iter().map(|it| it.sp.as_str()).collect();
    if let Some(m) = doc.subtitle.get("speakers").and_then(|s| s.as_object()) {
        names.extend(m.keys().map(String::as_str));
    }
    for sp in names {
        speakers.entry(sp.to_string()).or_insert_with(|| speaker_style(doc, sp));
    }
    let mut lines = BTreeMap::new();
    let mut end: f64 = 0.0;
    for (it, (a, b)) in sorted.iter().zip(spans(&sorted)) {
        let job = resolve(doc, it);
        end = end.max(b);
        lines.insert(
            it.line_id.clone(),
            SubLineView { key: line_key(&it.sp, &it.text), style: job.style, text: job.text, manual: job.manual,
                          custom: job.custom, a, b },
        );
    }
    SubView { default: SubStyle::default(), track: track_style(doc), speakers, lines, end, orphans: orphans(doc, items) }
}

// ======================================================================= 描き手
struct Pool {
    idle: Vec<Proc>,
    live: usize,
}

pub struct SubRenderer {
    python: Mutex<String>,
    script: PathBuf,
    log: Arc<Mutex<VecDeque<String>>>,
    pool: Mutex<Pool>,
    cv: Condvar,
    max: usize,
    cache: Mutex<HashMap<u64, (Arc<SubBitmap>, u64)>>,
    tick: Mutex<u64>,
    bytes: Mutex<usize>,
    budget: usize,
    infos: Mutex<HashMap<u64, Arc<SubInfo>>>,
    fonts: Mutex<Option<Value>>,
    /// 字幕のフォントの警告（見つからない・太さを変えられない）。key は「font:<フォント>」「font_weight:<フォント>」。
    /// 絵はキャッシュに当たるとワーカーを呼ばないので消さずに覚えておき、見せるときに今使っているフォントの分だけに絞る
    warnings: Mutex<Vec<Warning>>,
    on_warn: Mutex<Option<Box<dyn Fn() + Send + Sync>>>,
    /// 描くのに失敗した字幕の鍵（`SubJob::key_at`。待ち続けないために覚える。1 万を超えたら空にする）
    failed: Mutex<HashSet<u64>>,
}

impl SubRenderer {
    pub fn new(python: &str, script: PathBuf, log: Arc<Mutex<VecDeque<String>>>) -> SubRenderer {
        SubRenderer {
            python: Mutex::new(python.to_string()),
            script,
            log,
            pool: Mutex::new(Pool { idle: vec![], live: 0 }),
            cv: Condvar::new(),
            max: 3,
            cache: Mutex::new(HashMap::new()),
            tick: Mutex::new(0),
            bytes: Mutex::new(0),
            budget: 256 << 20,
            infos: Mutex::new(HashMap::new()),
            fonts: Mutex::new(None),
            warnings: Mutex::new(vec![]),
            on_warn: Mutex::new(None),
            failed: Mutex::new(HashSet::new()),
        }
    }

    /// 字幕のフォントの警告が増えた・変わったときに呼ぶ（画面に知らせる）。
    pub fn set_on_warn(&self, f: impl Fn() + Send + Sync + 'static) {
        *self.on_warn.lock() = Some(Box::new(f));
    }

    /// fonts（今の回の字幕が使うフォント。fonts_in_use）の警告だけ。
    pub fn warnings(&self, fonts: &HashSet<String>) -> Vec<Warning> {
        self.warnings
            .lock()
            .iter()
            .filter(|w| w.key.split_once(':').is_some_and(|(_, spec)| fonts.contains(spec)))
            .cloned()
            .collect()
    }

    fn note_warnings(&self, h: &Value) {
        let fresh = merge_warnings(&mut self.warnings.lock(), h);
        if fresh.is_empty() {
            return;
        }
        for w in &fresh {
            push_log(&self.log, format!("[subs] {}", w.message));
        }
        if let Some(f) = self.on_warn.lock().as_ref() {
            f();
        }
    }

    pub fn workers(&self) -> usize {
        self.max
    }

    /// Python を変えたら、今いるワーカーは捨てて次から新しい方で起こす。
    pub fn set_python(&self, python: &str) {
        *self.python.lock() = python.to_string();
        self.shutdown();
    }

    pub fn shutdown(&self) {
        let mut p = self.pool.lock();
        let idle: Vec<Proc> = p.idle.drain(..).collect();
        p.live -= idle.len();
        for mut w in idle {
            w.kill();
        }
    }

    /// ワーカーを 1 本借りて依頼を出す。プロセスが落ちたら捨てる。
    fn call(&self, req: &Value) -> Result<(Value, Vec<u8>)> {
        let mut proc = {
            let mut g = self.pool.lock();
            let since = std::time::Instant::now();
            loop {
                // ワーカーが全部ふさがったまま空かない（固まっている）なら待ち続けない。固まったワーカーは見張りが止める
                if since.elapsed() > Duration::from_secs(90) {
                    bail!("字幕のワーカーが 90 秒たっても空きません");
                }
                if let Some(p) = g.idle.pop() {
                    break p;
                }
                if g.live < self.max {
                    g.live += 1;
                    drop(g);
                    let py = self.python.lock().clone();
                    match spawn_proc(&py, &self.script, self.log.clone(), "subs") {
                        Ok(p) => break p,
                        Err(e) => {
                            self.pool.lock().live -= 1;
                            self.cv.notify_one();
                            return Err(e);
                        }
                    }
                }
                self.cv.wait_for(&mut g, Duration::from_millis(500));
            }
        };
        match proc.call(req) {
            Ok((h, data)) => {
                self.pool.lock().idle.push(proc);
                self.cv.notify_one();
                if h.get("ok").and_then(|v| v.as_bool()) != Some(true) {
                    bail!("字幕を描けませんでした: {}", h.get("error").and_then(|v| v.as_str()).unwrap_or("原因不明"));
                }
                self.note_warnings(&h);
                Ok((h, data))
            }
            Err(e) => {
                proc.kill();
                self.pool.lock().live -= 1;
                self.cv.notify_one();
                Err(anyhow!("字幕のワーカーが止まりました: {e:#}"))
            }
        }
    }

    pub fn cached(&self, job: &SubJob, scale: f64) -> Option<Arc<SubBitmap>> {
        let k = job.key_at(scale);
        let mut c = self.cache.lock();
        let e = c.get_mut(&k)?;
        let mut t = self.tick.lock();
        *t += 1;
        e.1 = *t;
        Some(e.0.clone())
    }

    /// この字幕は描くのに失敗した（直せば文か見た目が変わって別の鍵になる）。
    pub fn is_failed(&self, job: &SubJob, scale: f64) -> bool {
        self.failed.lock().contains(&job.key_at(scale))
    }

    fn mark_failed(&self, k: u64) {
        let mut f = self.failed.lock();
        if f.len() >= 10000 {
            f.clear();
        }
        f.insert(k);
    }

    /// 字幕の絵（キャッシュつき）。失敗したら鍵を覚える。
    pub fn bitmap(&self, job: &SubJob, scale: f64) -> Result<Arc<SubBitmap>> {
        if let Some(b) = self.cached(job, scale) {
            return Ok(b);
        }
        let r = self.bitmap_uncached(job, scale);
        if r.is_err() {
            self.mark_failed(job.key_at(scale));
        }
        r
    }

    fn bitmap_uncached(&self, job: &SubJob, scale: f64) -> Result<Arc<SubBitmap>> {
        let mut req = job.req();
        req["op"] = json!("subtitle");
        req["scale"] = json!(scale);
        let (h, data) = self.call(&req)?;
        let info: SubInfo = serde_json::from_value(h.clone()).unwrap_or_default();
        let info = Arc::new(info);
        self.infos.lock().insert(job.key(), info.clone());
        let bb = h.get("rect").and_then(|b| b.as_array()).map(|a| {
            (a[0].as_i64().unwrap_or(0), a[1].as_i64().unwrap_or(0), a[2].as_u64().unwrap_or(0) as u32,
             a[3].as_u64().unwrap_or(0) as u32)
        });
        let (x, y, w, hh) = bb.unwrap_or((0, 0, 0, 0));
        if (w as usize) * (hh as usize) * 4 != data.len() {
            // 空の字幕（空白だけの行など）
            let b = Arc::new(SubBitmap { x: 0, y: 0, w: 0, h: 0, lz: vec![], raw_len: 0, info });
            self.put(job.key_at(scale), b.clone());
            return Ok(b);
        }
        let b = Arc::new(SubBitmap { x, y, w, h: hh, lz: lz4_flex::compress(&data), raw_len: data.len(), info });
        self.put(job.key_at(scale), b.clone());
        Ok(b)
    }

    fn put(&self, k: u64, v: Arc<SubBitmap>) {
        let t = {
            let mut t = self.tick.lock();
            *t += 1;
            *t
        };
        let mut c = self.cache.lock();
        let mut b = self.bytes.lock();
        *b += v.bytes();
        if let Some(old) = c.insert(k, (v, t)) {
            *b -= old.0.bytes();
        }
        if *b > self.budget {
            // 古い方から 3 割捨てる
            let mut v: Vec<(u64, u64)> = c.iter().map(|(k, (_, t))| (*k, *t)).collect();
            v.sort_by_key(|x| x.1);
            for (k, _) in v.iter().take(v.len() * 3 / 10 + 1) {
                if let Some(old) = c.remove(k) {
                    *b -= old.0.bytes();
                }
            }
        }
    }

    /// 行の分け方と位置だけ（絵は描かない。まとめて 1 回で頼む）。
    pub fn measure(&self, jobs: &[SubJob]) -> Result<Vec<Arc<SubInfo>>> {
        let keys: Vec<u64> = jobs.iter().map(|j| j.key()).collect();
        let miss: Vec<usize> = {
            let m = self.infos.lock();
            (0..jobs.len()).filter(|&i| !m.contains_key(&keys[i])).collect()
        };
        if !miss.is_empty() {
            let items: Vec<Value> = miss.iter().map(|&i| jobs[i].req()).collect();
            let (h, _) = self.call(&json!({"op": "subtitle_measure", "items": items}))?;
            let arr = h.get("items").and_then(|v| v.as_array()).cloned().unwrap_or_default();
            let mut m = self.infos.lock();
            if m.len() > 20000 {
                m.clear();
            }
            for (k, &i) in miss.iter().enumerate() {
                let info: SubInfo = arr.get(k).and_then(|v| serde_json::from_value(v.clone()).ok()).unwrap_or_default();
                m.insert(keys[i], Arc::new(info));
            }
        }
        let m = self.infos.lock();
        Ok(keys.iter().map(|k| m.get(k).cloned().unwrap_or_default()).collect())
    }

    /// 入っているフォントの一覧（調べるのに数秒かかるので、フォントのフォルダが変わらない限り覚えておく）。
    pub fn fonts(&self) -> Result<Value> {
        if let Some(v) = self.fonts.lock().clone() {
            return Ok(v);
        }
        let sig = fonts_signature();
        let p = crate::settings::data_dir().join("fonts.json");
        if let Some(v) = std::fs::read_to_string(&p).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok()) {
            if v.get("sig").and_then(|s| s.as_str()) == Some(sig.as_str()) {
                let f = v["fonts"].clone();
                *self.fonts.lock() = Some(f.clone());
                return Ok(f);
            }
        }
        let (h, _) = self.call(&json!({"op": "fonts"}))?;
        let f = h.get("fonts").cloned().unwrap_or_else(|| json!([]));
        let _ = std::fs::write(&p, serde_json::to_string(&json!({"sig": sig, "fonts": f}))?);
        *self.fonts.lock() = Some(f.clone());
        Ok(f)
    }
}

/// フォントのフォルダの中身が変わったかの目印（ファイルの数と、一番新しい更新時刻）。
fn fonts_signature() -> String {
    let mut dirs = vec![PathBuf::from("C:/Windows/Fonts")];
    if let Some(la) = dirs::data_local_dir() {
        dirs.push(la.join("Microsoft").join("Windows").join("Fonts"));
    }
    let mut n = 0u64;
    let mut newest = 0u64;
    for d in dirs {
        if let Ok(rd) = std::fs::read_dir(&d) {
            for e in rd.flatten() {
                n += 1;
                let mt = e
                    .metadata()
                    .ok()
                    .and_then(|m| m.modified().ok())
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_secs())
                    .unwrap_or(0);
                newest = newest.max(mt);
            }
        }
    }
    format!("{n}:{newest}")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 素の既定は白い文字・黒の縁。プリセットの有無に関わらず同じ
    #[test]
    fn plain_is_white_with_black_stroke() {
        let p = SubStyle::plain();
        assert_eq!((p.color.as_str(), p.stroke.as_str()), ("#ffffff", "#000000"));
        // プリセットが空でも素の既定
        assert_eq!(SubStyle::from_preset(&serde_json::Map::new()).unwrap(), p);
        // 一部だけのプリセットは素の既定に重なる
        let m: serde_json::Map<String, Value> = serde_json::from_str(r##"{"stroke": "#123456"}"##).unwrap();
        let s = SubStyle::from_preset(&m).unwrap();
        assert_eq!((s.stroke.as_str(), s.size), ("#123456", 54.0));
    }

    /// 今の手元のプリセット（presets/subtitle.json）は、旧#173 の前の既定（青い縁）と同じ値になる
    #[test]
    fn repo_preset_equals_previous_default() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("presets").join("subtitle.json");
        // 公開側の clone（プリセットを非公開に動かしたあと）には無いので飛ばす
        let Ok(text) = std::fs::read_to_string(&path) else { return };
        let m: serde_json::Map<String, Value> = serde_json::from_str(&text).unwrap();
        let before = SubStyle {
            font: "C:/Windows/Fonts/NotoSansJP-VF.ttf".into(), weight: 900.0, size: 54.0, color: "#ffffff".into(),
            stroke: "#1e50c8".into(), stroke_w: 6.0, shadow: 0.0, shadow_color: "#000000".into(), shadow_x: 4.0,
            shadow_y: 4.0, shadow_blur: 6.0, y: 1060.0, line_h: 70.0, margin_l: 210.0, margin_r: 210.0,
            align: "center".into(), wrap: true,
        };
        assert_eq!(SubStyle::from_preset(&m).unwrap(), before);
    }

    /// Default はビルドに埋め込んだプリセットがあればそれ、無ければ素の既定
    #[test]
    fn default_follows_embedded_preset() {
        let want = match &crate::settings::presets().subtitle {
            Some(m) => SubStyle::from_preset(m).unwrap(),
            None => SubStyle::plain(),
        };
        assert_eq!(SubStyle::default(), want);
        // 設定の読み込みで欠けた項目を足すとき（serde の既定）は素の既定
        let s: SubStyle = serde_json::from_str("{}").unwrap();
        assert_eq!(s, SubStyle::plain());
    }

    fn item(sp: &str, text: &str, start: f64, dur: f64) -> VoiceItem {
        VoiceItem { line_id: format!("l{start}"), n: 1, sp: sp.into(), text: text.into(), start, dur, audio: None,
                    block: String::new(), key: String::new() }
    }

    /// 字幕のフォントの警告は、今の回が使うフォント（既定・共通・話者・セリフ）の分だけ見せる。消さずに覚えておく
    #[test]
    fn font_warnings_follow_fonts_in_use() {
        let r = SubRenderer::new("python", PathBuf::from("x.py"), Arc::new(Mutex::new(VecDeque::new())));
        let calls = Arc::new(Mutex::new(0));
        let c2 = calls.clone();
        r.set_on_warn(move || *c2.lock() += 1);
        let w = |k: &str| json!({"key": k, "message": format!("{k} の警告")});
        r.note_warnings(&json!({"warnings": [w("font:A.ttf"), w("font_weight:B.ttf"), w("font:C.ttf#1")]}));
        r.note_warnings(&json!({"warnings": [w("font:A.ttf")]})); // 同じ警告では知らせない
        assert_eq!(*calls.lock(), 1);
        assert_eq!(r.log.lock().len(), 3);

        let mut doc = ProjectDoc::default();
        let keys = |doc: &ProjectDoc| {
            let mut k: Vec<String> = r.warnings(&fonts_in_use(doc)).into_iter().map(|w| w.key).collect();
            k.sort();
            k
        };
        // 何も指定していなければ既定のフォントだけ
        assert!(keys(&doc).is_empty());
        assert!(fonts_in_use(&doc).contains(&SubStyle::default().font));
        doc.subtitle = json!({"font": "A.ttf", "speakers": {"トーク2": {"font": "B.ttf"}}});
        doc.subtitle_lines = json!({ line_key("トーク1", "こんにちは"): {"font": "C.ttf#1"} });
        assert_eq!(keys(&doc), vec!["font:A.ttf", "font:C.ttf#1", "font_weight:B.ttf"]);
        // 直したら（使わなくなったら）出さない。開き直してキャッシュに当たっても、使っていれば出る
        doc.subtitle = json!({"font": "D.ttf"});
        assert_eq!(keys(&doc), vec!["font:C.ttf#1"]);
        // 字幕を非表示にしていれば出さない
        doc.subtitle = json!({"font": "A.ttf", "hidden": true});
        assert!(keys(&doc).is_empty());
    }

    #[test]
    fn resolve_layers() {
        let mut doc = ProjectDoc::default();
        // 古い形（size・color・stroke・y だけ）も読める
        doc.subtitle = json!({"size": 60, "color": "#ffffff", "stroke": "#1e50c8", "y": 1050,
                              "speakers": {"トーク2": {"stroke": "#c83250"}}});
        doc.subtitle_lines = json!({ line_key("トーク2", "こんにちは世界"): {"size": 48, "text": "こんにちは\n世界"} });
        let a = resolve(&doc, &item("トーク1", "やあ", 0.0, 1.0));
        assert_eq!(a.style.size, 60.0);
        assert_eq!(a.style.stroke, "#1e50c8");
        assert_eq!(a.style.weight, 900.0);
        assert!(!a.manual);
        let b = resolve(&doc, &item("トーク2", "こんにちは世界", 0.0, 1.0));
        assert_eq!(b.style.stroke, "#c83250");
        assert_eq!(b.style.size, 48.0);
        assert!(b.manual && !b.custom);
        assert_eq!(b.text, "こんにちは\n世界");
        // 文が変わったら、その行の設定は効かない（孤立）
        let c = resolve(&doc, &item("トーク2", "こんにちは地球", 0.0, 1.0));
        assert_eq!(c.style.size, 60.0);
        assert!(!c.manual);
        assert_eq!(orphans(&doc, &[item("トーク2", "こんにちは地球", 0.0, 1.0)]).len(), 1);
        // 型の違う値は無視（全部が既定に戻らない）
        doc.subtitle = json!({"size": "big", "y": 1000});
        let d = resolve(&doc, &item("トーク1", "やあ", 0.0, 1.0));
        assert_eq!(d.style.size, 54.0);
        assert_eq!(d.style.y, 1000.0);
    }

    /// 本物の Python ワーカーで描く（Python と PIL が要るので普段は飛ばす: cargo test -- --ignored）
    #[test]
    #[ignore]
    fn render_with_python() {
        let script = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../engine/akfx_worker.py");
        let r = SubRenderer::new(&crate::settings::resolve_python("python"), script, Arc::new(Mutex::new(VecDeque::new())));
        let doc = ProjectDoc::default();
        let text = "この章で一番効率がいいのは、実はイベントの周回ステージじゃなくて、交換所の素材なんだよね。しかも上限まで交換できる";
        let job = resolve(&doc, &item("トーク1", text, 0.0, 1.0));
        let b = r.bitmap(&job, 1.0).unwrap();
        // 既定（今までのチャンネルの字幕）: 1500 で折り返して 3 行、最後の行の下が 1060
        assert_eq!(b.info.lines.len(), 3);
        assert_eq!(b.info.lines[2][3], 1060.0);
        assert!(!b.info.any_overflow());
        assert_eq!((b.info.l, b.info.r), (210.0, 1710.0));
        assert_eq!(b.rgba().data.len(), (b.w * b.h * 4) as usize);
        // 半分の大きさでも、行の分け方は同じ（1920 で描いてから縮める）
        let h = r.bitmap(&job, 0.5).unwrap();
        assert_eq!(h.info.texts, b.info.texts);
        assert!((h.w as i64 - b.w as i64 / 2).abs() <= 2);
        // 手で改行して、片方の行を長くするとはみ出し
        let mut doc2 = ProjectDoc::default();
        let long = format!("{text}{text}");
        doc2.subtitle_lines = json!({ line_key("トーク1", &long): {"text": format!("この章で\n{long}")} });
        let j2 = resolve(&doc2, &item("トーク1", &long, 0.0, 1.0));
        assert!(j2.manual);
        let m = r.measure(&[j2.clone(), job.clone()]).unwrap();
        assert_eq!(m[0].overflow, vec![false, true]);
        assert!(!m[1].any_overflow());
        assert!(r.cached(&job, 1.0).is_some());
        // 合成の確認用（AK_SUBS_OUT に、モニターと同じ重ね方の 1920x1080 を書く）
        if let Ok(out) = std::env::var("AK_SUBS_OUT") {
            let mut fr = crate::render::media::Rgb { w: 1920, h: 1080, data: [12u8, 18, 28].repeat(1920 * 1080) };
            crate::render::media::blend(&mut fr, &b.rgba(), b.x, b.y, 1.0, 1.0);
            image::RgbImage::from_raw(1920, 1080, fr.data).unwrap().save(out).unwrap();
        }
        r.shutdown();
    }

    #[test]
    fn active_and_spans() {
        let items = vec![item("トーク1", "a", 0.0, 1.0), item("トーク2", "b", 1.1, 1.0)];
        assert_eq!(active(&items, 1.05).map(|i| i.text.as_str()), Some("a"));
        assert_eq!(active(&items, 1.15).map(|i| i.text.as_str()), Some("b"));
        assert_eq!(active(&items, 2.39).map(|i| i.text.as_str()), Some("b"));
        assert!(active(&items, 2.41).is_none());
        let sp = spans(&items);
        assert!((sp[0].1 - 1.1).abs() < 1e-9);
        assert!((sp[1].1 - 2.4).abs() < 1e-9);
    }

    /// view: 共通 → 話者 → セリフの重ね方・区間・鍵・孤立・end。lines の style は resolve と同じ
    #[test]
    fn view_layers_spans_keys() {
        let mut doc = ProjectDoc::default();
        doc.subtitle = json!({"size": 60, "hidden": true, "y": "bad",
                              "speakers": {"トーク2": {"stroke": "#c83250", "size": 50, "speakers": 1, "text": "x"}, "ゲスト": {"size": 40}}});
        doc.subtitle_lines = json!({
            line_key("トーク2", "こんにちは"): {"size": 48, "text": "こん\nにちは", "hidden": true},
            line_key("トーク1", "消えた文"): {"size": 20},
        });
        // 並びは開始の順でなくてもよい。同じ時刻に始まる 2 行（n の順）・最短 0.05 秒・次のセリフの頭で切れる
        let mut a = item("トーク1", "やあ", 0.0, 1.0);
        a.line_id = "a".into();
        a.n = 1;
        let mut b = item("トーク2", "こんにちは", 2.0, 0.0);
        b.line_id = "b".into();
        b.n = 3;
        let mut c = item("トーク1", "同時", 2.0, 1.0);
        c.line_id = "c".into();
        c.n = 2;
        let mut d = item("トーク1", "やあ", 1.1, 1.0);
        d.line_id = "d".into();
        d.n = 4;
        let items = vec![d.clone(), b.clone(), c.clone(), a.clone()];
        let v = view(&doc, &items);
        assert_eq!(v.default, SubStyle::default());
        assert_eq!(v.track.size, 60.0);
        assert_eq!(v.track.y, 1060.0); // 型が違う値は捨てる
        assert_eq!(v.track, track_style(&doc));
        // speakers: 台本に出る話者と doc の話者。見た目に speakers・hidden・text は入らない
        assert_eq!(v.speakers.keys().map(String::as_str).collect::<Vec<_>>(), vec!["ゲスト", "トーク1", "トーク2"]);
        assert_eq!(v.speakers["トーク1"], v.track);
        assert_eq!(v.speakers["トーク2"].stroke, "#c83250");
        assert_eq!(v.speakers["トーク2"].size, 50.0);
        assert_eq!(v.speakers["ゲスト"].size, 40.0);
        assert_eq!(v.speakers["トーク2"], speaker_style(&doc, "トーク2"));
        // 区間: 声の終わり + 0.3 秒、次のセリフの頭で切る
        assert!((v.lines["a"].a - 0.0).abs() < 1e-9 && (v.lines["a"].b - 1.1).abs() < 1e-9);
        assert!((v.lines["d"].a - 1.1).abs() < 1e-9 && (v.lines["d"].b - 2.0).abs() < 1e-9);
        // 同じ時刻に始まる c(n=2)・b(n=3): c は b の頭が同じなので切れず声の終わり + 0.3 秒、b は最短 0.05 秒
        assert!((v.lines["c"].b - 3.3).abs() < 1e-9);
        assert!((v.lines["b"].a - 2.0).abs() < 1e-9 && (v.lines["b"].b - 2.3).abs() < 1e-9);
        assert!((v.end - 3.3).abs() < 1e-9);
        // 鍵・セリフごとの設定・手で改行
        assert_eq!(v.lines["b"].key, "トーク2\u{1}こんにちは");
        assert_eq!(v.lines["b"].style.size, 48.0);
        assert_eq!(v.lines["b"].style.stroke, "#c83250");
        assert!(v.lines["b"].manual && !v.lines["b"].custom);
        assert_eq!(v.lines["b"].text, "こん\nにちは");
        assert_eq!(v.orphans, vec![line_key("トーク1", "消えた文")]);
        // モニターの絵（resolve）と画面の表示（view）が同じ見た目になる
        for it in &items {
            let job = resolve(&doc, it);
            let l = &v.lines[&it.line_id];
            assert_eq!(l.style, job.style);
            assert_eq!((l.text.as_str(), l.manual, l.custom), (job.text.as_str(), job.manual, job.custom));
        }
        // 字幕が無ければ end は 0
        assert_eq!(view(&doc, &[]).end, 0.0);
    }

    /// 依頼には、読めないフォントの代わり（既定のフォント）を付ける
    #[test]
    fn request_has_fallback_font() {
        let job = resolve(&ProjectDoc::default(), &item("トーク1", "やあ", 0.0, 1.0));
        assert_eq!(job.req()["fallback_font"], json!(SubStyle::default().font));
        assert_eq!(job.req()["style"]["size"], json!(54.0));
    }
}
