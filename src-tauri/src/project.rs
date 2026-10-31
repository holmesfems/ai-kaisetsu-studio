//! 回（エピソード）のフォルダと、AK Studio の編集データ。
//!
//! - `<回>/studio/project.json` … 映像・画像・音声のトラック（背景・録画・立ち絵・BGM）と設定
//! - `<回>/fx/studio_overrides.json` … 演出クリップの手動の微調整（Python 側で読む）
//! - `<回>/fx/studio_clips.json` … 画面から足した簡単な演出（テロップ・注釈・ハンコ・画像）
//! - `<回>/studio/chat.jsonl` … Claude とのやり取り

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use ts_rs::TS;

#[derive(Serialize, Deserialize, Clone, Debug, TS)]
#[serde(default)]
#[ts(export)]
pub struct MediaClip {
    pub id: String,
    pub name: String,
    pub src: String,
    /// タイムライン上の開始（秒）
    pub start: f64,
    /// 長さ（秒）
    pub dur: f64,
    /// 素材のどこから使うか（秒）
    pub src_in: f64,
    /// 再生速度（1.0 = 等速。0.1〜10。ネストの素材だけで使う）
    #[serde(default = "one", skip_serializing_if = "is_one")]
    pub speed: f64,
    /// 素材の長さより長いときに繰り返す（背景ループ）
    #[serde(rename = "loop")]
    pub looped: bool,
    /// 画面の中心の位置（1920x1080 の座標、Premiere の「位置」と同じ）
    pub x: f64,
    pub y: f64,
    /// 倍率（1.0 = 素材の大きさのまま）
    pub scale: f64,
    pub opacity: f64,
    pub fade_in: f64,
    pub fade_out: f64,
    pub volume_db: f64,
    /// 立ち絵の「話している人を明るく」（話者名。空なら何もしない）
    pub speaker: String,
    pub flip: bool,
    /// クロップ（画面に映っている素材の、左・上・右・下を何 % 切り落とすか。Premiere の「クロップ」と同じ）
    pub crop_l: f64,
    pub crop_t: f64,
    pub crop_r: f64,
    pub crop_b: f64,
    /// クロップした辺のぼかし幅（1920x1080 の画素）
    pub feather: f64,
    /// 入り・抜けのトランジション（種類が空なら無し。fade_in / fade_out とは別に掛かる）
    #[serde(default, skip_serializing_if = "Trans::is_none", deserialize_with = "trans_or_null")]
    pub trans_in: Trans,
    #[serde(default, skip_serializing_if = "Trans::is_none", deserialize_with = "trans_or_null")]
    pub trans_out: Trans,
}

/// speed の既定（等速）。ts-rs に「省かれることがある」と伝えるための field 単位の default
fn one() -> f64 {
    1.0
}

fn is_one(v: &f64) -> bool {
    (*v - 1.0).abs() < 1e-9
}

/// 素材クリップの入り・抜けのトランジション（中身の計算は trans.rs）。
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, TS)]
#[serde(default)]
#[ts(export)]
pub struct Trans {
    /// 種類: fade / slide / pop / slam / wipe / glitch（空 = 無し）
    pub kind: String,
    /// 長さ（秒）。0 以下なら既定（0.35 秒）
    pub dur: f64,
    /// 向き: left / right / up / down。slide は入りなら来る側・抜けなら去る側、wipe は走査線が走り出す側。空なら left
    pub dir: String,
}

/// null は「無し」、文字列は種類だけの省略形（"wipe"。MCP の media_update と同じ）として読む
fn trans_or_null<'de, D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Trans, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum T {
        Kind(String),
        Full(Trans),
    }
    Ok(match Option::<T>::deserialize(d)? {
        None => Trans::default(),
        Some(T::Kind(kind)) => Trans { kind, ..Default::default() },
        Some(T::Full(t)) => t,
    })
}

/// オブジェクトの中の null の項目を取り除く（null は「既定の値」として読む。JS の NaN も null になって来る）
fn strip_nulls(v: &mut Value) {
    match v {
        Value::Object(m) => {
            m.retain(|_, x| !x.is_null());
            m.values_mut().for_each(strip_nulls);
        }
        Value::Array(a) => a.iter_mut().for_each(strip_nulls),
        _ => {}
    }
}

/// project.json の中身を読む。null の項目は既定の値にする
pub fn parse_doc(text: &str) -> Result<ProjectDoc> {
    let mut v: Value = serde_json::from_str(text)?;
    strip_nulls(&mut v);
    Ok(serde_json::from_value(v)?)
}

impl Trans {
    pub fn is_none(&self) -> bool {
        self.kind.is_empty() || self.kind == "none"
    }
}

impl Default for MediaClip {
    fn default() -> Self {
        MediaClip {
            id: String::new(),
            name: String::new(),
            src: String::new(),
            start: 0.0,
            dur: 5.0,
            src_in: 0.0,
            speed: 1.0,
            looped: false,
            x: 960.0,
            y: 540.0,
            scale: 1.0,
            opacity: 1.0,
            fade_in: 0.0,
            fade_out: 0.0,
            volume_db: 0.0,
            speaker: String::new(),
            flip: false,
            crop_l: 0.0,
            crop_t: 0.0,
            crop_r: 0.0,
            crop_b: 0.0,
            feather: 0.0,
            trans_in: Trans::default(),
            trans_out: Trans::default(),
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, TS)]
#[serde(default)]
#[ts(export)]
pub struct Track {
    pub id: String,
    /// video / image / fx / audio
    #[ts(type = "\"video\" | \"image\" | \"fx\" | \"audio\"")]
    pub kind: String,
    pub name: String,
    pub clips: Vec<MediaClip>,
    pub hidden: bool,
    pub muted: bool,
    pub locked: bool,
    pub volume_db: f64,
}

impl Default for Track {
    fn default() -> Self {
        Track {
            id: String::new(),
            kind: "video".into(),
            name: String::new(),
            clips: vec![],
            hidden: false,
            muted: false,
            locked: false,
            volume_db: 0.0,
        }
    }
}

/// RecWindow（録画の窓）に差し込む録画のまとまり（Premiere のネストに当たる）。
/// 自分のキャンバス（既定 1920x1080）の上にトラックで素材を並べ、キャンバスの外は捨てて、窓（box）にはめる。
/// クリップの start / dur は本体と同じ絶対時刻（秒）、x / y / scale はキャンバスの座標（左上が 0, 0）。
#[derive(Serialize, Deserialize, Clone, Debug, TS)]
#[serde(default)]
#[ts(export)]
pub struct Nest {
    /// 窓（RecWindow の box、1920x1080 の座標 [x0, y0, x1, y1]）。演出が読み込まれていればそちらの値を優先する
    #[serde(rename = "box")]
    pub bx: [f64; 4],
    /// キャンバスの大きさ
    pub w: f64,
    pub h: f64,
    /// キャンバスを窓にはめる方法: contain（縦横比を保って全体を収める）/ stretch（窓いっぱいに引き伸ばす）/ cover（縦横比を保って窓を埋める）
    #[ts(type = "\"contain\" | \"stretch\" | \"cover\"")]
    pub fit: String,
    /// 下から上の順（映像・画像）
    pub tracks: Vec<Track>,
}

impl Default for Nest {
    fn default() -> Self {
        Nest { bx: [600.0, 100.0, 1880.0, 820.0], w: 1920.0, h: 1080.0, fit: "contain".into(), tracks: vec![] }
    }
}

impl Nest {
    /// キャンバス → 画面（1920x1080）の対応。(横の倍率, 縦の倍率, キャンバスの左上 x, y)。画面の nestMap と見本 ui_rules.json で縛っている
    pub fn map(&self, bx: [f64; 4]) -> (f64, f64, f64, f64) {
        let (bw, bh) = (bx[2] - bx[0], bx[3] - bx[1]);
        let (cw, ch) = (self.w.max(1.0), self.h.max(1.0));
        let (sx, sy) = match self.fit.as_str() {
            "stretch" => (bw / cw, bh / ch),
            "cover" => {
                let s = (bw / cw).max(bh / ch);
                (s, s)
            }
            _ => {
                let s = (bw / cw).min(bh / ch);
                (s, s)
            }
        };
        (sx, sy, bx[0] + (bw - cw * sx) / 2.0, bx[1] + (bh - ch * sy) / 2.0)
    }

    /// 素材が置かれている区間（隠したトラックは除く。重なりはまとめる）
    pub fn covered(&self) -> Vec<[f64; 2]> {
        let mut v: Vec<[f64; 2]> = self
            .tracks
            .iter()
            .filter(|t| !t.hidden)
            .flat_map(|t| t.clips.iter())
            .filter(|c| !c.src.is_empty() && c.dur > 0.0)
            .map(|c| [c.start, c.start + c.dur])
            .collect();
        v.sort_by(|a, b| a[0].partial_cmp(&b[0]).unwrap_or(std::cmp::Ordering::Equal));
        let mut out: Vec<[f64; 2]> = vec![];
        for r in v {
            match out.last_mut() {
                Some(l) if r[0] <= l[1] + 1.0 / 120.0 => l[1] = l[1].max(r[1]),
                _ => out.push(r),
            }
        }
        out
    }

    pub fn clip_count(&self) -> usize {
        self.tracks.iter().map(|t| t.clips.len()).sum()
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, TS)]
#[serde(default)]
#[ts(export)]
pub struct ProjectDoc {
    pub version: u32,
    /// 下から上の順（映像）。音声トラックは別扱いでも同じ列に入れる
    pub tracks: Vec<Track>,
    /// fx/ の中のタイムライン（相対パス）
    pub fx_timeline: Option<String>,
    /// 声の音量（dB）、効果音の音量（dB）
    pub voice_db: f64,
    pub se_db: f64,
    pub se_muted: bool,
    pub voice_muted: bool,
    /// 攻略動画の回の「素材の音」（通しの素材の音）の音量（dB）と消音。モニターと書き出しの両方に効く
    pub guide_audio_db: f64,
    pub guide_audio_muted: bool,
    /// セリフの間隔（秒）とブロックの間隔（秒）。台本エディタのトラックビューと同じ。
    /// 開始の無い行を置くときと「詰め直す」で使う（開始が書いてある行はこの値では動かさない）
    pub gap: f64,
    pub block_gap: f64,
    /// gap・block_gap を台本の開始と音声の長さから見積もった（か、手で決めた）か。
    /// false のうちは、画面が音声の長さを読み終えたところで 1 回だけ見積もって true にする（issue 旧#4）
    pub gaps_inferred: bool,
    /// 録画・スクショの仮表示を出すか
    pub placeholder: bool,
    /// Premiere でオーバーレイを置く位置（イントロのぶん）
    pub premiere_t0: f64,
    pub premiere_fps: f64,
    #[ts(type = "any[]")]
    pub markers: Vec<Value>,
    /// 字幕の見た目（回の共通。subs::SubStyle の項目＋ hidden（字幕を出さない）＋ speakers（話者ごとの上書き））
    #[ts(type = "import(\"../types.ts\").SubTrack")]
    pub subtitle: Value,
    /// RecWindow に差し込んだ録画（キーは演出クリップのキー。RecWindow@c70#1 など）
    pub nests: BTreeMap<String, Nest>,
    /// セリフごとの字幕の設定。鍵は「話者 \u0001 字幕の文」（subs::line_key）。
    /// {"font": …, "size": …, "text": "手で改行した\n字幕"}。台本の文が変わると効かなくなる
    #[ts(type = "Record<string, import(\"../types.ts\").SubLine>")]
    pub subtitle_lines: Value,
}

impl Default for ProjectDoc {
    fn default() -> Self {
        ProjectDoc {
            version: 1,
            tracks: vec![],
            fx_timeline: None,
            voice_db: 0.0,
            se_db: -2.0,
            se_muted: false,
            voice_muted: false,
            guide_audio_db: 0.0,
            guide_audio_muted: false,
            gap: 0.3,
            // 台本エディタの既定は 1.0 だったが、実際の回（an387〜389 の script.srt）はブロックの間も 0.3
            block_gap: 0.3,
            gaps_inferred: false,
            placeholder: true,
            premiere_t0: 0.0,
            premiere_fps: 60.0,
            markers: vec![],
            subtitle: serde_json::to_value(crate::render::subs::SubStyle::default()).unwrap_or_default(),
            subtitle_lines: serde_json::json!({}),
            nests: BTreeMap::new(),
        }
    }
}

pub struct Project {
    pub root: PathBuf,
    pub name: String,
    pub doc: ProjectDoc,
    /// 開くときに知らせること（project.json が読めなかったので退避した、など）
    pub warning: Option<String>,
    /// warning の元の失敗の文（serde の英語を含む。ログ・ツールチップ用）
    pub warning_detail: Option<String>,
    /// 最後に読んだ・書いた project.json の指紋（無いときは None）。save で外の変更に気づくのに使う
    seen: Option<u64>,
}

impl Project {
    pub fn open(root: &Path) -> Result<Project> {
        let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
        let root = PathBuf::from(root.to_string_lossy().trim_start_matches(r"\\?\").to_string());
        let name = root.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
        let p = root.join("studio").join("project.json");
        // 読めない project.json を既定の値で上書きすると素材・ネスト・字幕の設定が消えるので、退避してから始める
        let mut warning = None;
        let mut warning_detail = None;
        let mut seen = None;
        let mut doc: ProjectDoc = match std::fs::read_to_string(&p) {
            Ok(t) => match parse_doc(&t) {
                Ok(d) => {
                    seen = Some(fingerprint(t.as_bytes()));
                    d
                }
                Err(e) => {
                    warning_detail = Some(format!("project.json: {e}"));
                    let e = crate::app::error::reason(e);
                    let bak = p.with_file_name(format!("project.json.broken-{}", chrono::Local::now().format("%Y%m%d-%H%M%S")));
                    std::fs::rename(&p, &bak)
                        .with_context(|| format!("読めない project.json を退避できませんでした（{e}）"))?;
                    warning = Some(format!(
                        "studio/project.json が読めないので {} に退避して、新しく作りました（{e}）",
                        bak.file_name().unwrap_or_default().to_string_lossy()
                    ));
                    ProjectDoc::default()
                }
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => ProjectDoc::default(),
            Err(e) => return Err(anyhow::Error::new(e).context(format!("{} を読めません", p.display()))),
        };
        let fresh = doc.tracks.is_empty();
        if fresh {
            doc.tracks = default_tracks(&root);
        }
        migrate(&mut doc);
        if !doc.tracks.iter().any(|t| t.kind == "fx") {
            doc.tracks.push(Track { id: "fx".into(), kind: "fx".into(), name: "演出".into(), ..Default::default() });
        }
        let mut pr = Project { root, name, doc, warning, warning_detail, seen };
        pr.rediscover_timeline();
        if fresh {
            if let Some(w) = pr.save()? {
                pr.warning = Some(w);
            }
        }
        Ok(pr)
    }

    /// タイムラインが未設定、または指すファイルが無いときだけ fx/ から探し直す（見つかったら true）。
    /// 今のタイムラインが生きていれば切り替えない。
    pub fn rediscover_timeline(&mut self) -> bool {
        if self.doc.fx_timeline.as_ref().is_some_and(|r| self.root.join(r).exists()) {
            return false;
        }
        self.doc.fx_timeline = find_timeline(&self.root).map(|p| {
            p.strip_prefix(&self.root).unwrap_or(&p).to_string_lossy().replace('\\', "/")
        });
        self.doc.fx_timeline.is_some()
    }

    pub fn studio_dir(&self) -> PathBuf {
        let d = self.root.join("studio");
        let _ = std::fs::create_dir_all(&d);
        d
    }

    /// project.json に書く。読んだ後に外で書き換えられていたら（指紋が違えば）、外の版を
    /// `project.json.external-<日時>` に退避してから書き、画面に出す知らせを返す（読み直し・マージはしない）。
    pub fn save(&mut self) -> Result<Option<String>> {
        let p = self.studio_dir().join("project.json");
        let text = serde_json::to_string_pretty(&self.doc)?;
        let mut notice = None;
        match std::fs::read(&p) {
            Ok(now) => {
                if Some(fingerprint(&now)) != self.seen && now != text.as_bytes() {
                    let bak = p.with_file_name(format!("project.json.external-{}", chrono::Local::now().format("%Y%m%d-%H%M%S%.3f")));
                    crate::settings::write_atomic(&bak, &now)
                        .with_context(|| format!("外で変更された project.json の控え（{}）を書けなかったので、保存しませんでした", bak.display()))?;
                    notice = Some(format!(
                        "studio/project.json が外で変更されていたので、控え（{}）を残して上書きしました。外での変更を戻すときは控えから写してください",
                        bak.file_name().unwrap_or_default().to_string_lossy()
                    ));
                }
            }
            // 外で消されていたら控えるものは無い
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(anyhow::Error::new(e).context(format!("{} を読めません", p.display()))),
        }
        crate::settings::write_atomic(&p, &text)?;
        self.seen = Some(fingerprint(text.as_bytes()));
        Ok(notice)
    }

    pub fn script_dir(&self) -> PathBuf {
        self.root.join("script")
    }

    pub fn fx_timeline(&self) -> Option<PathBuf> {
        self.doc.fx_timeline.as_ref().map(|r| self.root.join(r))
    }

    pub fn fx_dir(&self) -> PathBuf {
        self.fx_timeline().and_then(|p| p.parent().map(|x| x.to_path_buf())).unwrap_or_else(|| self.root.join("fx"))
    }

    pub fn overrides_path(&self) -> PathBuf {
        self.fx_dir().join("studio_overrides.json")
    }

    pub fn clips_path(&self) -> PathBuf {
        self.fx_dir().join("studio_clips.json")
    }

    /// 演出の調整を読む。読めなければ退避して空で始め、知らせる文を返す（無いだけなら何も言わない）
    pub fn load_overrides(&self) -> (Value, Option<String>) {
        load_json_or_keep(&self.overrides_path(), "演出の調整", serde_json::json!({}))
    }

    pub fn save_overrides(&self, v: &Value) -> Result<()> {
        let p = self.overrides_path();
        if let Some(d) = p.parent() {
            std::fs::create_dir_all(d)?;
        }
        crate::settings::write_atomic(&p, serde_json::to_string_pretty(v)?)?;
        Ok(())
    }

    /// 画面で追加した演出を読む（読めないときは load_overrides と同じ）
    pub fn load_gui_clips(&self) -> (Value, Option<String>) {
        load_json_or_keep(&self.clips_path(), "画面で追加した演出", serde_json::json!([]))
    }

    pub fn save_gui_clips(&self, v: &Value) -> Result<()> {
        let p = self.clips_path();
        if let Some(d) = p.parent() {
            std::fs::create_dir_all(d)?;
        }
        crate::settings::write_atomic(&p, serde_json::to_string_pretty(v)?)?;
        Ok(())
    }

    pub fn voice_cache(&self) -> PathBuf {
        self.root.join("voice_cache")
    }
}

/// ファイルの中身の指紋（同じプロセスの中で比べるだけなので DefaultHasher でよい）
fn fingerprint(b: &[u8]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    b.hash(&mut h);
    h.finish()
}

/// fx/ の JSON（演出の調整・画面で追加した演出）を読む。
/// 読めないファイルを空のまま次の保存で上書きすると手の調整がすべて消えるので、
/// `<名前>.broken-<日時>` に退避してから `empty` で始め、画面に出す文を返す。ファイルが無いだけなら何も言わない。
pub fn load_json_or_keep(p: &Path, what: &str, empty: Value) -> (Value, Option<String>) {
    let file = p.file_name().unwrap_or_default().to_string_lossy().to_string();
    let err = match std::fs::read_to_string(p) {
        Ok(t) => match serde_json::from_str::<Value>(&t) {
            Ok(v) if v.is_object() == empty.is_object() && v.is_array() == empty.is_array() => return (v, None),
            Ok(_) => format!("{}ではありません", if empty.is_array() { "JSON の配列" } else { "JSON のオブジェクト" }),
            Err(e) => crate::app::error::reason(e),
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return (empty, None),
        Err(e) => crate::app::error::reason(e),
    };
    let bak = p.with_file_name(format!("{file}.broken-{}", chrono::Local::now().format("%Y%m%d-%H%M%S")));
    let kept = match std::fs::rename(p, &bak) {
        Ok(()) => format!("前の内容は {} に保存しました。", bak.file_name().unwrap_or_default().to_string_lossy()),
        Err(e) => format!("前の内容を退避できませんでした（{}）。", crate::app::error::reason(e)),
    };
    (empty, Some(format!("{what}（{file}）を読み込めませんでした: {err}。{kept}")))
}

/// project.json の版の更新（読み込むたびに呼ぶ。何度呼んでも同じ結果になる）
pub fn migrate(doc: &mut ProjectDoc) {
    if doc.version < 2 {
        // 版 2（issue 旧#11）: トラックのロックをプレビューの選択に効かせるようになった。
        // それまでは背景（v1）をプレビューで選ばないよう決め打ちで外していたので、背景のトラックをロックしておく
        if let Some(t) = doc.tracks.iter_mut().find(|t| t.id == "v1") {
            t.locked = true;
        }
        doc.version = 2;
    }
}

/// 新しい回の背景ループ。既定はプリセット（`presets/project.json` の bg_loop。旧#173）の指す mp4 で、
/// 同じ名前のファイルが回のフォルダにあればそちらを使う。プリセットが無い・ファイルが無ければ背景なし
pub(crate) fn default_bg(root: &Path, bg_loop: Option<&str>) -> Option<PathBuf> {
    let p = PathBuf::from(bg_loop?);
    let local = p.file_name().map(|n| root.join(n));
    local.into_iter().chain([p]).find(|c| c.exists())
}

/// 背景のトラック（v1）に素材が 1 つも無いか（背景のトラックが無い回は false）
pub fn no_background(doc: &ProjectDoc) -> bool {
    doc.tracks.iter().find(|t| t.id == "v1").is_some_and(|t| t.clips.is_empty())
}

fn default_tracks(root: &Path) -> Vec<Track> {
    let bg = default_bg(root, crate::settings::presets().bg_loop.as_deref());
    // 背景はプレビューのクリックで選ばないよう、初めからロックしておく（issue 旧#11）
    let mut v1 = Track { id: "v1".into(), kind: "video".into(), name: "背景".into(), locked: true, ..Default::default() };
    if let Some(bg) = bg {
        v1.clips.push(MediaClip {
            id: "bg".into(),
            name: "背景ループ".into(),
            src: bg.to_string_lossy().replace('\\', "/"),
            start: 0.0,
            dur: 600.0,
            looped: true,
            ..Default::default()
        });
    }
    vec![
        v1,
        Track { id: "v2".into(), kind: "video".into(), name: "録画".into(), ..Default::default() },
        Track { id: "v3".into(), kind: "image".into(), name: "立ち絵".into(), ..Default::default() },
        Track { id: "fx".into(), kind: "fx".into(), name: "演出".into(), ..Default::default() },
        Track { id: "a1".into(), kind: "audio".into(), name: "BGM".into(), volume_db: -18.0, ..Default::default() },
    ]
}

/// fx/ の中から、一番新しく書き換えたタイムライン（*_fx.py で build() があるもの）を探す。
pub fn find_timeline(root: &Path) -> Option<PathBuf> {
    let fx = root.join("fx");
    let mut best: Option<(std::time::SystemTime, PathBuf)> = None;
    for e in std::fs::read_dir(&fx).ok()?.flatten() {
        let p = e.path();
        let n = p.file_name()?.to_string_lossy().to_string();
        if !n.ends_with(".py") || n.starts_with("fx") || n.contains("template") {
            continue;
        }
        let ok_name = n.ends_with("_fx.py") || n == "risei_fx.py" || n == "kakin_fx.py";
        if !ok_name {
            continue;
        }
        let Ok(t) = std::fs::read_to_string(&p) else { continue };
        if !t.contains("def build(") {
            continue;
        }
        let mt = e.metadata().ok()?.modified().ok()?;
        if best.as_ref().map(|(b, _)| mt > *b).unwrap_or(true) {
            best = Some((mt, p));
        }
    }
    best.map(|(_, p)| p)
}

/// fx/ の中のタイムラインの一覧（選び直す用）。
pub fn list_timelines(root: &Path) -> Vec<String> {
    let mut out = vec![];
    if let Ok(rd) = std::fs::read_dir(root.join("fx")) {
        for e in rd.flatten() {
            let n = e.file_name().to_string_lossy().to_string();
            // *_template.py（fxfreeze が写したテンプレートの部品）は回のタイムラインではない（find_timeline と同じ）
            if n.ends_with(".py") && !n.starts_with("fx") && !n.contains("template") {
                if let Ok(t) = std::fs::read_to_string(e.path()) {
                    if t.contains("def build(") {
                        out.push(format!("fx/{n}"));
                    }
                }
            }
        }
    }
    out.sort();
    out
}

/// an_root の下の回の一覧（新しい順）。script/ か fx/ があるもの。
/// template_dir（設定の「ひな形のフォルダ」）は、その中身が回に見えても一覧に出さない（空なら除外しない）
pub fn list_projects(an_root: &Path, template_dir: &str) -> Vec<Value> {
    let mut v = vec![];
    let tpl = template_dir.trim();
    let same = |p: &Path| -> bool {
        if tpl.is_empty() {
            return false;
        }
        let norm = |s: &str| s.replace('\\', "/").trim_end_matches('/').to_lowercase();
        let t = std::path::Path::new(tpl);
        match (p.canonicalize(), t.canonicalize()) {
            (Ok(a), Ok(b)) => a == b,
            _ => norm(&p.to_string_lossy()) == norm(tpl),
        }
    };
    if an_root.as_os_str().is_empty() {
        return v;
    }
    if let Ok(rd) = std::fs::read_dir(an_root) {
        for e in rd.flatten() {
            let p = e.path();
            if !p.is_dir() {
                continue;
            }
            let has_script = p.join("script").is_dir();
            let has_fx = p.join("fx").is_dir();
            if !(has_script || has_fx) {
                continue;
            }
            if same(&p) {
                continue;
            }
            let n = e.file_name().to_string_lossy().to_string();
            let mt = e.metadata().ok().and_then(|m| m.modified().ok())
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs()).unwrap_or(0);
            v.push(serde_json::json!({"name": n, "path": p.to_string_lossy().replace('\\', "/"),
                                       "script": has_script, "fx": has_fx, "mtime": mt}));
        }
    }
    v.sort_by(|a, b| b["name"].as_str().cmp(&a["name"].as_str()));
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn old_project_json_loads_without_nests() {
        let d: ProjectDoc = serde_json::from_str(r#"{"version": 1, "tracks": []}"#).unwrap();
        assert!(d.nests.is_empty());
    }

    /// 素材の音の項目が無い古い project.json はそのまま読め、既定は 0 dB・消音なし
    #[test]
    fn old_project_json_loads_without_guide_audio() {
        let d: ProjectDoc = serde_json::from_str(r#"{"version": 1, "tracks": [], "se_db": -3.0}"#).unwrap();
        assert_eq!((d.guide_audio_db, d.guide_audio_muted), (0.0, false));
        assert_eq!(d.se_db, -3.0);
        let d: ProjectDoc = serde_json::from_str(r#"{"guide_audio_db": -6.5, "guide_audio_muted": true}"#).unwrap();
        assert_eq!((d.guide_audio_db, d.guide_audio_muted), (-6.5, true));
    }

    #[test]
    fn clip_transitions_roundtrip() {
        // 昔のクリップ（trans_* 無し）と null は「無し」、書き出しでは無しのものは出さない
        let c: MediaClip = serde_json::from_str(r#"{"id": "m1", "trans_out": null}"#).unwrap();
        assert!(c.trans_in.is_none() && c.trans_out.is_none());
        let v = serde_json::to_value(&c).unwrap();
        assert!(v.get("trans_in").is_none() && v.get("trans_out").is_none());
        let c: MediaClip = serde_json::from_str(r#"{"trans_in": {"kind": "wipe", "dur": 0.4}}"#).unwrap();
        assert_eq!(c.trans_in, Trans { kind: "wipe".into(), dur: 0.4, dir: String::new() });
        assert_eq!(serde_json::to_value(&c).unwrap()["trans_in"]["kind"], "wipe");
    }

    #[test]
    fn doc_reads_shorthand_and_nulls() {
        let d = parse_doc(
            r#"{"tracks": [{"id": "v3", "kind": "image", "clips": [{"id": "m1", "trans_in": "wipe", "x": null}]}]}"#,
        )
        .unwrap();
        let c = &d.tracks[0].clips[0];
        assert_eq!(c.trans_in.kind, "wipe");
        assert_eq!(c.x, 960.0);
    }

    #[test]
    fn list_timelines_skips_template_files() {
        let dir = std::env::temp_dir().join(format!("aks-listtl-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("fx")).unwrap();
        std::fs::write(dir.join("fx").join("guide_fx.py"), "def build():
    pass
").unwrap();
        // fxfreeze が写したテンプレート（文章に def build( が入っていても出さない）
        std::fs::write(dir.join("fx").join("guide_template.py"), "\"\"\"def build(\"\"\"
").unwrap();
        assert_eq!(list_timelines(&dir), vec!["fx/guide_fx.py".to_string()]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rediscover_finds_timeline_made_after_open() {
        let dir = std::env::temp_dir().join(format!("aks-rediscover-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut pr = Project::open(&dir).unwrap();
        assert!(pr.doc.fx_timeline.is_none());
        assert!(!pr.rediscover_timeline());
        std::fs::create_dir_all(dir.join("fx")).unwrap();
        std::fs::write(dir.join("fx/foo_fx.py"), "def build(ctx):\n    pass\n").unwrap();
        assert!(pr.rediscover_timeline());
        assert_eq!(pr.doc.fx_timeline.as_deref(), Some("fx/foo_fx.py"));
        // 今のタイムラインがあるうちは、別の *_fx.py が増えても切り替えない
        std::fs::write(dir.join("fx/bar_fx.py"), "def build(ctx):\n    pass\n").unwrap();
        assert!(!pr.rediscover_timeline());
        assert_eq!(pr.doc.fx_timeline.as_deref(), Some("fx/foo_fx.py"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn broken_project_json_is_kept() {
        let dir = std::env::temp_dir().join(format!("aks-broken-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("studio")).unwrap();
        std::fs::write(dir.join("studio/project.json"), "{\"tracks\": [{\"clips\": 3}]}").unwrap();
        let pr = Project::open(&dir).unwrap();
        let w = pr.warning.as_deref().unwrap();
        assert!(w.contains("中身の形が合いません"), "{w}");
        assert!(!w.contains("invalid type"), "{w}");
        // 元の serde の文はログ・ツールチップ用に残る
        assert!(pr.warning_detail.as_deref().unwrap().contains("invalid type"));
        let kept = std::fs::read_dir(dir.join("studio")).unwrap().flatten()
            .any(|e| e.file_name().to_string_lossy().starts_with("project.json.broken-"));
        assert!(kept);
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn external_backups(dir: &Path) -> Vec<PathBuf> {
        std::fs::read_dir(dir.join("studio")).unwrap().flatten()
            .filter(|e| e.file_name().to_string_lossy().starts_with("project.json.external-"))
            .map(|e| e.path())
            .collect()
    }

    #[test]
    fn external_change_is_kept_before_save() {
        let dir = std::env::temp_dir().join(format!("aks-external-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut pr = Project::open(&dir).unwrap();
        let p = dir.join("studio/project.json");
        // 外で書き換えられた後の save: 外の版を控えに残し、本体は新しい doc
        let outside = std::fs::read_to_string(&p).unwrap().replacen("\"gap\"", "\"x_outside\": 1, \"gap\"", 1);
        assert!(outside.contains("x_outside"));
        std::fs::write(&p, &outside).unwrap();
        pr.doc.gap = 1.25;
        let n = pr.save().unwrap();
        assert!(n.as_deref().is_some_and(|s| s.contains("project.json.external-")), "{n:?}");
        let baks = external_backups(&dir);
        assert_eq!(baks.len(), 1);
        assert_eq!(std::fs::read_to_string(&baks[0]).unwrap(), outside);
        let now = std::fs::read_to_string(&p).unwrap();
        assert!(!now.contains("x_outside"));
        assert_eq!(parse_doc(&now).unwrap().gap, 1.25);
        // 自分の save の直後にもう一度 save しても控えを作らない
        pr.doc.gap = 0.5;
        assert_eq!(pr.save().unwrap(), None);
        assert_eq!(pr.save().unwrap(), None);
        assert_eq!(external_backups(&dir).len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn unchanged_file_saves_without_backup() {
        let dir = std::env::temp_dir().join(format!("aks-unchanged-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        drop(Project::open(&dir).unwrap());
        // 開き直して（読んだ中身の指紋で）save しても、外で変わっていなければ控えなし
        let mut pr = Project::open(&dir).unwrap();
        pr.doc.gap = 2.0;
        assert_eq!(pr.save().unwrap(), None);
        assert!(external_backups(&dir).is_empty());
        // 外で消されていたら、控えずにそのまま書く
        std::fs::remove_file(dir.join("studio/project.json")).unwrap();
        assert_eq!(pr.save().unwrap(), None);
        assert!(dir.join("studio/project.json").exists());
        assert!(external_backups(&dir).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn broken_fx_json_is_kept() {
        let dir = std::env::temp_dir().join(format!("aks-broken-fx-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let ov = dir.join("studio_overrides.json");
        // 無いだけなら空で、何も言わない
        assert_eq!(load_json_or_keep(&ov, "演出の調整", serde_json::json!({})), (serde_json::json!({}), None));
        // 読めるものはそのまま
        std::fs::write(&ov, r#"{"Telop@c1#1": {"dx": 5}}"#).unwrap();
        let (v, w) = load_json_or_keep(&ov, "演出の調整", serde_json::json!({}));
        assert_eq!(v["Telop@c1#1"]["dx"], 5);
        assert!(w.is_none());
        // 壊れていたら退避して空で始め、退避先を知らせる
        let broken = r#"{"Telop@c1#1": {"dx": 5},"#;
        std::fs::write(&ov, broken).unwrap();
        let (v, w) = load_json_or_keep(&ov, "演出の調整", serde_json::json!({}));
        assert_eq!(v, serde_json::json!({}));
        let w = w.unwrap();
        assert!(w.starts_with("演出の調整（studio_overrides.json）を読み込めませんでした: 書式が正しくありません（行 1、"), "{w}");
        assert!(!w.contains("EOF") && !w.contains("at line"), "{w}");
        assert!(!ov.exists());
        let bak: Vec<_> = std::fs::read_dir(&dir).unwrap().flatten()
            .filter(|e| e.file_name().to_string_lossy().starts_with("studio_overrides.json.broken-"))
            .collect();
        assert_eq!(bak.len(), 1);
        assert!(w.contains(&*bak[0].file_name().to_string_lossy()), "{w}");
        assert_eq!(std::fs::read_to_string(bak[0].path()).unwrap(), broken);
        // 型が違うもの（配列のはずがオブジェクト）も読めないものとして扱う
        let clips = dir.join("studio_clips.json");
        std::fs::write(&clips, "{}").unwrap();
        let (v, w) = load_json_or_keep(&clips, "画面で追加した演出", serde_json::json!([]));
        assert_eq!(v, serde_json::json!([]));
        assert!(w.unwrap().contains("JSON の配列ではありません"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn migrate_locks_background_once() {
        let mut d: ProjectDoc = serde_json::from_str(
            r#"{"version": 1, "tracks": [{"id": "v1", "kind": "video"}, {"id": "v2", "kind": "video"}]}"#,
        )
        .unwrap();
        assert!(!d.tracks[0].locked);
        migrate(&mut d);
        assert_eq!(d.version, 2);
        assert!(d.tracks[0].locked && !d.tracks[1].locked);
        // 版 2 以降は、ユーザーが外したロックをそのままにする
        d.tracks[0].locked = false;
        migrate(&mut d);
        assert!(!d.tracks[0].locked);
    }

    #[test]
    fn nest_fit_modes() {
        let bx = [600.0, 100.0, 1880.0, 820.0];
        // 1920x1080 → 1280x720 はどれでも 66.667%（Premiere の 位置 (1240, 460)・スケール 66.667% と同じ）
        for fit in ["contain", "stretch", "cover"] {
            let n = Nest { fit: fit.into(), ..Default::default() };
            let (sx, sy, ox, oy) = n.map(bx);
            assert!((sx - 2.0 / 3.0).abs() < 1e-9 && (sy - 2.0 / 3.0).abs() < 1e-9);
            assert!((ox - 600.0).abs() < 1e-9 && (oy - 100.0).abs() < 1e-9);
        }
        // 1920x768: contain は 1280x512 を上下中央に、stretch は窓いっぱい
        let n = Nest { h: 768.0, ..Default::default() };
        let (sx, sy, ox, oy) = n.map(bx);
        assert!((sx - sy).abs() < 1e-9 && (sx - 2.0 / 3.0).abs() < 1e-9);
        assert!((ox - 600.0).abs() < 1e-9 && (oy - (100.0 + (720.0 - 512.0) / 2.0)).abs() < 1e-9);
        let n = Nest { h: 768.0, fit: "stretch".into(), ..Default::default() };
        let (sx, sy, _, oy) = n.map(bx);
        assert!((sx - 2.0 / 3.0).abs() < 1e-9 && (sy - 720.0 / 768.0).abs() < 1e-9 && (oy - 100.0).abs() < 1e-9);
    }

    #[test]
    fn nest_covered_merges() {
        let mut n = Nest::default();
        let c = |s: f64, d: f64| MediaClip { src: "a.mp4".into(), start: s, dur: d, ..Default::default() };
        n.tracks.push(Track { clips: vec![c(10.0, 2.0), c(11.0, 3.0), c(20.0, 1.0)], ..Default::default() });
        n.tracks.push(Track { clips: vec![c(30.0, 1.0)], hidden: true, ..Default::default() });
        assert_eq!(n.covered(), vec![[10.0, 14.0], [20.0, 21.0]]);
    }

    fn tmpdir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("ak-project-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// 背景の既定: プリセットが無ければ背景なし。あればその mp4（回のフォルダに同じ名前があればそちら）。ファイルが無ければ背景なし
    #[test]
    fn default_bg_follows_preset() {
        let d = tmpdir("bg");
        let root = d.join("ep");
        std::fs::create_dir_all(&root).unwrap();
        let shared = d.join("bg").join("loop.mp4");
        std::fs::create_dir_all(shared.parent().unwrap()).unwrap();
        std::fs::write(&shared, b"x").unwrap();
        let s = shared.to_string_lossy().to_string();
        assert_eq!(default_bg(&root, None), None);
        assert_eq!(default_bg(&root, Some(&s)), Some(shared.clone()));
        assert_eq!(default_bg(&root, Some(&d.join("nothing.mp4").to_string_lossy())), None);
        std::fs::write(root.join("loop.mp4"), b"y").unwrap();
        assert_eq!(default_bg(&root, Some(&s)), Some(root.join("loop.mp4")));
        let _ = std::fs::remove_dir_all(&d);
    }

    /// 回の一覧: 回のフォルダが空なら空。ひな形のフォルダは一覧に出さない（空なら除外しない）
    #[test]
    fn list_projects_skips_template_dir() {
        let d = tmpdir("list");
        for n in ["an1", "tpl"] {
            std::fs::create_dir_all(d.join(n).join("fx")).unwrap();
        }
        let names = |v: Vec<Value>| v.iter().map(|x| x["name"].as_str().unwrap().to_string()).collect::<Vec<_>>();
        assert_eq!(names(list_projects(&d, "")), ["tpl", "an1"]);
        assert_eq!(names(list_projects(&d, &d.join("tpl").to_string_lossy())), ["an1"]);
        assert_eq!(names(list_projects(&d, &format!("{}/tpl/", d.to_string_lossy().replace('\\', "/")))), ["an1"]);
        assert!(list_projects(Path::new(""), "").is_empty());
        let _ = std::fs::remove_dir_all(&d);
    }
}
