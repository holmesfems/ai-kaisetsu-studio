//! 攻略動画（ゲームの攻略解説動画）の編集のしくみ。
//!
//! - `studio/guide.json` … 正の置き場所（型は `ak_proto::guide`）。アンカー・固定値・カット・戦闘画面の重ね方
//! - `studio/guide_analysis.json` … 素材の解析（`fxkit/guide_scan.py` が書く）。各コマの時刻とアンカーの候補
//! - `studio/guide_map.json` … 組み立てたパートの対応表（出力の t → 素材のコマ）。演出の Python と Claude が読む
//!
//! ここは純粋な部分（解析の読み取り・パートの組み立て・レイヤーの座標・解析の出力の読み方）。
//! Core との結びつき（読み書き・見張り・合成・解析の実行）は `core/guide.rs`。

pub mod audio;
pub mod layer;
pub mod map;
pub mod patch;
pub mod plan;
pub mod read;
pub mod still;
pub mod undo;

use ak_proto::guide::{GuideAnalysisInfo, GuideBattle};
use ak_proto::{Guide, GuideFixed};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub use plan::GuidePlan;

pub const GUIDE_FILE: &str = "guide.json";
pub const ANALYSIS_FILE: &str = "guide_analysis.json";
pub const MAP_FILE: &str = "guide_map.json";

/// 回の `fx/guide_fx.py`（攻略動画の演出の入口）の中身。決まった演出は fxkit の `guide_template.py` が描く
pub const FX_STUB: &str = include_str!("fx_stub.py");
/// 入口のファイル（回のフォルダからの相対）
pub const FX_STUB_FILE: &str = "fx/guide_fx.py";

/// 演出の入口 `fx/guide_fx.py` を作る（原子的に書く）。もうあれば Err（中身は触らない）。作ったファイルのパスを返す
pub fn create_fx_stub(root: &Path) -> Result<std::path::PathBuf, String> {
    let p = root.join(FX_STUB_FILE);
    if p.exists() {
        return Err("fx/guide_fx.py はもうあります".into());
    }
    if let Some(d) = p.parent() {
        std::fs::create_dir_all(d).map_err(|e| format!("fx フォルダを作れませんでした: {e}"))?;
    }
    crate::settings::write_atomic(&p, FX_STUB).map_err(|e| format!("fx/guide_fx.py を作れませんでした: {e}"))?;
    Ok(p)
}

/// guide.json を作るとき、演出の入口も作るか（fx/ に演出のタイムラインがまだ無いとき）
pub fn fx_stub_needed(root: &Path) -> bool {
    crate::project::find_timeline(root).is_none() && !root.join(FX_STUB_FILE).exists()
}

/// guide_analysis.json（使う項目だけ。コストや fill の列は画面と演出の Python が使うのでここでは読まない）
#[derive(Debug, Clone, Default)]
pub struct GuideAnalysis {
    pub src: String,
    pub size: [u32; 2],
    /// コマごとの素材の時刻（秒）
    pub pts: Arc<Vec<f64>>,
    pub battles: Vec<GuideBattle>,
    /// 解析のアンカーの候補（素材の秒）。値が無い（null）ものは入れない
    pub anchors: BTreeMap<String, f64>,
    pub warnings: Vec<String>,
    /// コマごとのコスト（読めなかったコマは None）。pts と同じ並び
    pub cost: Vec<Option<i32>>,
    /// コマごとのゲーム内の時間（ゲームのフレーム数。コスト 1 = 30、60 で 1 秒。戦闘の外・読めないコマは None）。pts と同じ並び
    pub game: Vec<Option<f64>>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct RawAnalysis {
    src: String,
    size: Vec<u32>,
    pts: Vec<f64>,
    battles: Vec<GuideBattle>,
    anchors: BTreeMap<String, Option<f64>>,
    warnings: Vec<String>,
    cost: Vec<Option<i64>>,
    game: Vec<Option<f64>>,
}

pub fn parse_analysis(text: &str) -> Result<GuideAnalysis, String> {
    let r: RawAnalysis = serde_json::from_str(text).map_err(|e| e.to_string())?;
    if r.pts.is_empty() {
        return Err("コマの時刻（pts）がありません".into());
    }
    Ok(GuideAnalysis {
        src: r.src,
        size: [r.size.first().copied().unwrap_or(0), r.size.get(1).copied().unwrap_or(0)],
        pts: Arc::new(r.pts),
        battles: r.battles,
        anchors: r.anchors.into_iter().filter_map(|(k, v)| v.filter(|x| x.is_finite()).map(|v| (k, v))).collect(),
        warnings: r.warnings,
        cost: r.cost.into_iter().map(|c| c.and_then(|c| i32::try_from(c).ok())).collect(),
        game: r.game.into_iter().map(|g| g.filter(|x| x.is_finite())).collect(),
    })
}

impl GuideAnalysis {
    pub fn info(&self) -> GuideAnalysisInfo {
        GuideAnalysisInfo {
            exists: true,
            frames: self.pts.len() as u32,
            battles: self.battles.clone(),
            warnings: self.warnings.clone(),
            anchors: self.anchors.clone(),
        }
    }
}

/// 画面の保存が食い違ったときの理由の頭（画面は取り直してもう 1 回だけやり直す。`ui/src/logic/guide.ts` の `isRevConflict` と同じ文）
pub const REV_CONFLICT: &str = "ほかで変わりました";

/// 画面が持っている番号 base（`GuideState::rev`）が今の番号 now と違えば Err（保存しない）。base が無ければ確かめない
pub fn check_rev(base: Option<u64>, now: u64) -> Result<(), String> {
    match base {
        Some(b) if b != now => Err(format!("{REV_CONFLICT}（guide.json が、画面に出ている内容から変わっています。最新の内容で直してください）")),
        _ => Ok(()),
    }
}

/// 古い既定のままの重ね方を、今の既定に読み替える（guide.json は重ね方を丸ごと保存しているので、既定を変えても今ある回に届かない）。
/// **前の既定とまったく同じ値**のものだけ置き換える（回ごとに直した値は触らない）。置き換えた項目の名前を返す
/// （置き換えたら次の保存で書き直される）
pub fn upgrade_layout(l: &mut ak_proto::GuideLayout) -> Vec<String> {
    use ak_proto::guide::{
        default_sub_box, BENCH_SRC, COST_ZOOM_SCALE, COST_ZOOM_SRC, COST_ZOOM_TO, OLD_BENCH_SRC, OLD_COST_ZOOM_SCALE, OLD_COST_ZOOM_SRC,
        OLD_COST_ZOOM_TO, OLD_SUB_BOX, PREV_SUB_BOX,
    };
    let mut done = vec![];
    if let Some(c) = l.crops.iter_mut().find(|c| c.name == "bench") {
        if c.src == OLD_BENCH_SRC {
            c.src = BENCH_SRC;
            done.push("待機欄の切り抜き".to_string());
        }
    }
    if let Some(z) = l.zooms.iter_mut().find(|z| z.name == "cost") {
        if z.src == OLD_COST_ZOOM_SRC && z.to == OLD_COST_ZOOM_TO && z.scale == OLD_COST_ZOOM_SCALE {
            z.src = COST_ZOOM_SRC;
            z.to = COST_ZOOM_TO;
            z.scale = COST_ZOOM_SCALE;
            done.push("コストの拡大".to_string());
        }
    }
    if l.sub_box == OLD_SUB_BOX || l.sub_box == PREV_SUB_BOX {
        l.sub_box = default_sub_box();
        done.push("補足素材の小窓の枠".to_string());
    }
    done
}

/// guide.json の文字列を読む。段2 までは `"ops": []`・`"formation": null` と書いていたので、null の項目は無いものとして読む
pub fn parse_guide(text: &str) -> Result<Guide, serde_json::Error> {
    let mut v: serde_json::Value = serde_json::from_str(text)?;
    if let Some(o) = v.as_object_mut() {
        for k in ["ops", "formation"] {
            if o.get(k).is_some_and(|x| x.is_null()) {
                o.remove(k);
            }
        }
    }
    let mut g: Guide = serde_json::from_value(v)?;
    upgrade_layout(&mut g.layout);
    Ok(g)
}

/// guide.json を読む。無ければ Ok(None)。読めない・形式が違うときは Err（ファイルは触らない）
pub fn read_guide(path: &Path) -> Result<Option<Guide>, String> {
    read_guide_text(path).map(|r| r.map(|(g, _)| g))
}

/// guide.json を読む。読んだ中身（文字列）も返す（Claude の変更を戻せるかを、実際に読んだ中身の指紋で決めるため）
pub fn read_guide_text(path: &Path) -> Result<Option<(Guide, String)>, String> {
    match std::fs::read_to_string(path) {
        Ok(t) => parse_guide(&t)
            .map(|g| Some((g, t)))
            .map_err(|e| format!("studio/guide.json を読み込めませんでした（形式が違います）: {e}")),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("studio/guide.json を読めませんでした: {e}")),
    }
}

/// guide_analysis.json を読む。無ければ Ok(None)
pub fn read_analysis(path: &Path) -> Result<Option<GuideAnalysis>, String> {
    match std::fs::read_to_string(path) {
        Ok(t) => parse_analysis(&t)
            .map(Some)
            .map_err(|e| format!("studio/guide_analysis.json を読み込めませんでした（{e}）。もう一度解析してください")),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("studio/guide_analysis.json を読めませんでした: {e}")),
    }
}

/// 読んだ guide.json・解析・設定からパートを組み立てる。guide が無ければ Ok(None)。
/// 解析がまだ・組み立てられないときは Err（画面とモニターに出す和文）
pub fn build_plan(guide: Option<&Guide>, analysis: Option<&GuideAnalysis>, settings: &GuideFixed)
                  -> Result<Option<GuidePlan>, String> {
    let Some(g) = guide else { return Ok(None) };
    if g.src.trim().is_empty() {
        return Err("攻略動画の素材が選ばれていません".into());
    }
    let Some(a) = analysis else {
        return Err("素材の解析がまだです。攻略タブの「解析」を押してください".into());
    };
    // 別の素材を選び直したのに、前の素材の解析のまま組み立てると、違う動画のコマを引いてしまう
    let name = |s: &str| s.replace('\\', "/").rsplit('/').next().unwrap_or("").to_lowercase();
    if !a.src.is_empty() && name(&a.src) != name(&g.src) {
        return Err(format!(
            "解析した素材（{}）と、選んでいる素材（{}）が違います。もう一度解析してください",
            a.src, g.src
        ));
    }
    plan::plan(g, &plan::effective_fixed(g, settings), &a.pts).map(Some)
}

/// 解析のアンカーの候補を guide.json の auto に書く（使う値 t は触らない）。変わったら true
pub fn apply_auto_anchors(g: &mut Guide, anchors: &BTreeMap<String, f64>) -> bool {
    let mut changed = false;
    // 解析し直して見つからなくなった候補は消す（前の素材の候補を残さない）
    for name in ak_proto::guide::ANCHOR_NAMES {
        let v = anchors.get(name).copied();
        let cur = g.anchors.get(name).and_then(|a| a.auto);
        if cur != v {
            g.anchors.entry(name.to_string()).or_default().auto = v;
            changed = true;
        }
    }
    changed
}

/// 回のフォルダの中の素材を、guide.json の src（回のフォルダからの相対・/ 区切り）にする。
/// 絶対パスでも相対でもよい。回のフォルダの外・「..」を含むものは Err
pub fn rel_src(root: &Path, src: &str) -> Result<String, String> {
    let norm = |s: &str| s.replace('\\', "/");
    let s = norm(src.trim());
    if s.is_empty() {
        return Err("素材が選ばれていません".into());
    }
    if s.split('/').any(|p| p == "..") {
        return Err("素材は回のフォルダの中のものを選んでください（「..」は使えません）".into());
    }
    let r = norm(&root.to_string_lossy());
    let r = r.trim_end_matches('/');
    let is_abs = s.starts_with('/') || s.as_bytes().get(1) == Some(&b':');
    if !is_abs {
        return Ok(s.trim_start_matches("./").to_string());
    }
    // Windows のパスは大文字小文字を区別しない
    let (sl, rl) = (s.to_lowercase(), r.to_lowercase());
    match sl.strip_prefix(&format!("{rl}/")) {
        Some(_) => Ok(s[r.len() + 1..].to_string()),
        None => Err(format!("素材は回のフォルダ（{r}）の中のものを選んでください")),
    }
}

/// 解析の標準出力の 1 行
#[derive(Debug, Clone, PartialEq)]
pub enum ScanLine {
    /// 進み具合 0〜1
    Progress(f64),
    /// 終わり（出力のパス）
    Done(String),
    Other,
}

pub fn parse_scan_line(line: &str) -> ScanLine {
    let l = line.trim();
    if let Some(x) = l.strip_prefix("progress ") {
        if let Ok(v) = x.trim().parse::<f64>() {
            if v.is_finite() {
                return ScanLine::Progress(v.clamp(0.0, 1.0));
            }
        }
    }
    if let Some(p) = l.strip_prefix("done ") {
        return ScanLine::Done(p.trim().to_string());
    }
    ScanLine::Other
}

/// 標準エラーの終わりの n 行（失敗の知らせに添える）。空行は除く
pub fn stderr_tail(lines: &[String], n: usize) -> String {
    let v: Vec<&str> = lines.iter().map(|l| l.trim()).filter(|l| !l.is_empty()).collect();
    v[v.len().saturating_sub(n)..].join("\n")
}

/// 攻略動画の回で、決まった演出（kit の guide_template）が無いときの警告。kit が分からない（ワーカーが古い・未読み込み）ときは出さない
pub fn template_warning(kit: Option<&crate::render::engine::KitInfo>) -> Option<crate::render::engine::Warning> {
    let kit = kit?;
    if kit.features.iter().any(|f| f == "guide_template") {
        return None;
    }
    Some(crate::render::engine::Warning {
        key: "guide-template".into(),
        message: "攻略動画の決まった演出がありません（演出の kit に guide_template がありません）".into(),
        level: None,
    })
}

/// 解析の Python（`guide_scan.py`）を動かして終わるまで待つ。標準出力の進み具合は on_progress（0〜1）、
/// 標準エラーの行は終わってから on_log に渡す。失敗（起動できない・終了コードが 0 でない・終わりの知らせが無い）は和文の Err
pub fn run_scan_process(python: &str, kit: &Path, video: &Path, out: &Path, on_progress: impl Fn(f64),
                        on_log: impl Fn(&str)) -> Result<(), String> {
    use std::io::{BufRead, BufReader};
    use std::process::Stdio;
    // 公開版（kit が無い）では guide_scan.py が無い。python の「ファイルが無い」ではなく、分かる文で断る
    if !kit.join("guide_scan.py").is_file() {
        return Err("攻略動画の素材を解析できませんでした: 演出の kit に guide_scan.py がありません".to_string());
    }
    let mut cmd = crate::settings::hidden(std::process::Command::new(python));
    cmd.arg("-u")
        .arg(kit.join("guide_scan.py"))
        .arg(video)
        .arg("-o")
        .arg(out)
        .current_dir(kit)
        .env("PYTHONUTF8", "1")
        .env("PYTHONIOENCODING", "utf-8")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn().map_err(|e| format!("python を起動できませんでした（{python}）: {e}"))?;
    let stdout = child.stdout.take().ok_or("解析の出力を受け取れませんでした")?;
    let stderr = child.stderr.take().ok_or("解析のエラー出力を受け取れませんでした")?;
    let errs: Arc<parking_lot::Mutex<Vec<String>>> = Default::default();
    let e2 = errs.clone();
    let reader = std::thread::spawn(move || {
        for l in BufReader::new(stderr).lines().map_while(|l| l.ok()) {
            e2.lock().push(l);
        }
    });
    let mut done = false;
    let mut last = -1.0f64;
    for line in BufReader::new(stdout).lines().map_while(|l| l.ok()) {
        match parse_scan_line(&line) {
            ScanLine::Progress(x) if x - last >= 0.004 || x >= 1.0 => {
                last = x;
                on_progress(x);
            }
            ScanLine::Done(_) => done = true,
            _ => {}
        }
    }
    let status = child.wait().map_err(|e| format!("解析の終わりを待てませんでした: {e}"))?;
    let _ = reader.join();
    let lines = errs.lock().clone();
    for l in &lines {
        on_log(l);
    }
    if !status.success() {
        let tail = stderr_tail(&lines, 6);
        let why = if tail.is_empty() { format!("終了コード {}", status.code().unwrap_or(-1)) } else { tail };
        return Err(format!("解析できませんでした: {why}"));
    }
    if !done {
        return Err(format!("解析の終わりの知らせがありませんでした: {}", stderr_tail(&lines, 6)));
    }
    Ok(())
}

/// Core が持つ攻略動画の状態
#[derive(Default)]
pub struct GuideRt {
    pub guide: Option<Arc<Guide>>,
    pub analysis: Option<Arc<GuideAnalysis>>,
    pub plan: Option<Arc<GuidePlan>>,
    /// guide.json・解析が読めない理由（読めないファイルは上書きしない）
    pub load_error: Option<String>,
    /// そのうち guide.json が読めない理由（解析を始める・アンカーの候補を書く前に見る。解析が読めないだけなら解析し直せる）
    pub guide_error: Option<String>,
    /// 解析がまだ・組み立てられない理由
    pub plan_error: Option<String>,
    pub scanning: bool,
    /// 直近の組み立てで、補足素材の大きさを読めなかった理由（和文。警告に出す）
    pub sub_probe_errors: Vec<String>,
    /// 回を開くたびに増える番号。開き直す前に始めた解析の結果を、今の回に書かないための目印
    pub gen: u64,
    /// この状態を読んだ回のフォルダ（`gen` と同じ間に入れる。裏のスレッドが guide・plan・gen と食い違わない root を使うため）
    pub root: Option<PathBuf>,
    /// guide.json を書く・読み直すたびに増える番号（画面の保存の食い違いを見つける。`GuideState::rev`）。回を開き直しても減らさない
    pub rev: u64,
    /// Claude の変更の積み重ね（古いものが先。回を開き直したら空。控えのファイルは残る）
    pub claude: Vec<undo::ClaudeChange>,
    /// 次の Claude の変更の番号
    pub claude_seq: u32,
    /// 今の guide.json の中身の指紋（最後に書いた・読んだ文字列。読めない・無いときは None）。
    /// Claude の変更を戻せるのは、いちばん上の変更の保存の後の指紋と同じときだけ
    pub fp: Option<u64>,
    /// 素材の音の wav（studio/guide_audio.wav）を今の組み立てで作った印（ミリ秒。`GuideState::audio_rev`）。無いときは None
    pub audio_rev: Option<u64>,
    /// 素材の音について直したほうがよいこと（素材に音が無いなど。警告に出す）
    pub audio_warn: Option<String>,
    /// 直近に知らせた、素材の音を作れなかった理由（同じ理由を操作のたびに通知しないため）
    pub audio_err: Option<String>,
    /// 効果音を最後に作ったときの操作の指紋（`guide::audio::se_signature`）。None は読んでいない・まだ作っていない
    pub se_built: Option<u64>,
    /// `se_built` をファイルから読んだか
    pub se_built_loaded: bool,
}

impl GuideRt {
    /// 画面とモニターに出す理由（読めない → 組み立てられない）
    pub fn error(&self) -> Option<String> {
        self.load_error.clone().or_else(|| self.plan_error.clone())
    }

    /// 戻せる Claude の変更（いちばん上が、今の guide.json の保存の後と同じときだけ）
    pub fn undoable(&self) -> Option<&undo::ClaudeChange> {
        self.claude.last().filter(|c| Some(c.after) == self.fp)
    }

    /// 人が（画面・外のエディタで）直して、いちばん上の変更と合わなくなったら、積み重ね全部を捨てる
    /// （下の変更も、その上の今の中身から戻せない）
    pub fn drop_stale_claude(&mut self) {
        if self.claude.last().is_some_and(|c| Some(c.after) != self.fp) {
            self.claude.clear();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 戻せるのは、いちばん上の変更の保存の後と今の指紋が同じときだけ。人が直したら積み重ねごと捨てる
    #[test]
    fn claude_undo_only_while_fingerprint_matches() {
        let ch = |t: &str| undo::ClaudeChange { id: 1, backup: "b".into(), reason: "r".into(), at: "a".into(), after: undo::fingerprint(t) };
        let mut rt = GuideRt { claude: vec![ch("A"), ch("B")], fp: Some(undo::fingerprint("B")), ..Default::default() };
        assert!(rt.undoable().is_some());
        rt.drop_stale_claude();
        assert_eq!(rt.claude.len(), 2);
        // 人が直した（指紋が変わった）: 戻せないし、下の変更も戻せない
        rt.fp = Some(undo::fingerprint("B+human"));
        assert!(rt.undoable().is_none());
        rt.drop_stale_claude();
        assert!(rt.claude.is_empty());
        // 読めない・無いとき（指紋なし）も同じ
        let mut rt = GuideRt { claude: vec![ch("A")], fp: None, ..Default::default() };
        assert!(rt.undoable().is_none());
        rt.drop_stale_claude();
        assert!(rt.claude.is_empty());
    }
    use std::path::PathBuf;

    /// 前の既定の重ね方（段4 の手直しの前に作った guide.json が丸ごと保存していた値）
    fn old_layout() -> ak_proto::GuideLayout {
        use ak_proto::guide::{OLD_BENCH_SRC, OLD_COST_ZOOM_SCALE, OLD_COST_ZOOM_SRC, OLD_COST_ZOOM_TO, OLD_SUB_BOX};
        let mut l = ak_proto::GuideLayout::default();
        l.crops.iter_mut().find(|c| c.name == "bench").unwrap().src = OLD_BENCH_SRC;
        let z = &mut l.zooms[0];
        (z.src, z.to, z.scale) = (OLD_COST_ZOOM_SRC, OLD_COST_ZOOM_TO, OLD_COST_ZOOM_SCALE);
        l.sub_box = OLD_SUB_BOX;
        l
    }

    #[test]
    fn upgrade_layout_replaces_only_the_old_defaults() {
        let mut l = old_layout();
        let done = upgrade_layout(&mut l);
        assert_eq!(done.len(), 3, "{done:?}");
        assert_eq!(l, ak_proto::GuideLayout::default());
        assert_eq!(l.crops.iter().find(|c| c.name == "bench").unwrap().src, [0.0, 1790.0, 2752.0, 2064.0]);
        assert_eq!(l.zooms[0].src, [2280.0, 1560.0, 2752.0, 1765.0]);
        assert_eq!((l.zooms[0].to, l.zooms[0].scale), ([1456.0, 600.0], 0.95));
        assert_eq!(l.sub_box, [1060.0, 150.0, 360.0, 240.0]);
        // 前の既定（[1500, 320, 360, 240]）も新しい既定に読み替える
        let mut l2 = ak_proto::GuideLayout::default();
        l2.sub_box = ak_proto::guide::PREV_SUB_BOX;
        assert_eq!(upgrade_layout(&mut l2), ["補足素材の小窓の枠"]);
        assert_eq!(l2.sub_box, [1060.0, 150.0, 360.0, 240.0]);
        // 今の既定は何も置き換えない
        assert!(upgrade_layout(&mut l).is_empty());
        // 回ごとに直した値は残す
        let mut l = old_layout();
        l.sub_box = [1400.0, 200.0, 400.0, 300.0];
        l.crops.iter_mut().find(|c| c.name == "bench").unwrap().src = [0.0, 1800.0, 2310.0, 2064.0];
        let done = upgrade_layout(&mut l);
        assert_eq!(done, ["コストの拡大"]);
        assert_eq!(l.sub_box, [1400.0, 200.0, 400.0, 300.0]);
        assert_eq!(l.crops.iter().find(|c| c.name == "bench").unwrap().src, [0.0, 1800.0, 2310.0, 2064.0]);
        // コストの拡大は、src・to・scale の 3 つとも前の既定のときだけ（1 つ違えば残す）
        for which in 0..3 {
            let mut l = old_layout();
            match which {
                0 => l.zooms[0].src[0] = 2400.0,
                1 => l.zooms[0].to[1] = 620.0,
                _ => l.zooms[0].scale = 1.5,
            }
            let before = l.zooms[0].clone();
            let done = upgrade_layout(&mut l);
            assert!(!done.contains(&"コストの拡大".to_string()), "{which}");
            assert_eq!(l.zooms[0], before, "{which}");
        }
    }

    #[test]
    fn parse_guide_upgrades_an_old_saved_layout() {
        let old = Guide { src: "A.MP4".into(), layout: old_layout(), ..Default::default() };
        let text = serde_json::to_string(&old).unwrap();
        let g = parse_guide(&text).unwrap();
        assert_eq!(g.layout, ak_proto::GuideLayout::default());
        // layout の無い guide.json も今の既定
        assert_eq!(parse_guide(r#"{"src":"A.MP4"}"#).unwrap().layout, ak_proto::GuideLayout::default());
    }

    /// 画面の番号が今と違えば、保存を断る（食い違いの文は画面が頭の文字で見分ける）
    #[test]
    fn rev_mismatch_is_refused() {
        assert!(check_rev(Some(5), 5).is_ok());
        assert!(check_rev(None, 5).is_ok());
        let e = check_rev(Some(4), 5).unwrap_err();
        assert!(e.starts_with(REV_CONFLICT), "{e}");
        assert!(check_rev(Some(6), 5).is_err());
    }

    #[test]
    fn analysis_keeps_pts_battles_and_non_null_anchors() {
        let t = r#"{"version":1,"src":"A.MP4","size":[2752,2064],"pts":[0.0,0.017,0.034],
                    "fill":[null,0.5,0.6],"battles":[{"start":1,"end":2,"cost0":1,"end_band":null}],
                    "anchors":{"formation_dark":3.4,"manual_cost0":null},"warnings":["x"]}"#;
        let a = parse_analysis(t).unwrap();
        assert_eq!(a.pts.len(), 3);
        assert_eq!(a.size, [2752, 2064]);
        assert_eq!(a.battles[0].cost0, Some(1));
        assert_eq!(a.battles[0].end_band, None);
        assert_eq!(a.anchors.len(), 1);
        assert_eq!(a.anchors["formation_dark"], 3.4);
        // cost・game が無い解析（古い）は空の列
        assert!(a.cost.is_empty() && a.game.is_empty());
        let b = parse_analysis(r#"{"pts":[0.0,0.1,0.2],"cost":[null,10,11],"game":[null,0.0,1.042]}"#).unwrap();
        assert_eq!(b.cost, vec![None, Some(10), Some(11)]);
        assert_eq!(b.game, vec![None, Some(0.0), Some(1.042)]);
        let info = a.info();
        assert!(info.exists && info.frames == 3 && info.warnings == vec!["x".to_string()]);
        assert!(parse_analysis("{}").unwrap_err().contains("pts"));
        assert!(parse_analysis("{壊れた").is_err());
    }

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("ak-guide-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn old_guide_json_with_null_formation_still_reads() {
        let g = parse_guide(r#"{"src":"a.mp4","ops":null,"formation":null}"#).unwrap();
        assert!(g.ops.is_empty());
        assert_eq!(g.formation, ak_proto::GuideFormation::default());
        let g = parse_guide(r#"{"ops":[],"formation":null}"#).unwrap();
        assert!(g.ops.is_empty());
        assert!(parse_guide("[]").is_err());
    }

    #[test]
    fn read_guide_distinguishes_missing_from_broken() {
        let d = tmp("read");
        let p = d.join("guide.json");
        assert_eq!(read_guide(&p).unwrap(), None);
        std::fs::write(&p, "{ 壊れた").unwrap();
        let e = read_guide(&p).unwrap_err();
        assert!(e.contains("guide.json") && e.contains("読み込めませんでした"), "{e}");
        // 壊れたファイルには触らない
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "{ 壊れた");
        std::fs::write(&p, r#"{"src":"a.mp4"}"#).unwrap();
        assert_eq!(read_guide(&p).unwrap().unwrap().src, "a.mp4");
        let q = d.join("guide_analysis.json");
        assert!(read_analysis(&q).unwrap().is_none());
        std::fs::write(&q, "{}").unwrap();
        assert!(read_analysis(&q).unwrap_err().contains("もう一度解析"));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn build_plan_reports_each_missing_piece_in_japanese() {
        let s = GuideFixed::default();
        assert_eq!(build_plan(None, None, &s).unwrap(), None);
        let mut g = Guide::default();
        assert!(build_plan(Some(&g), None, &s).unwrap_err().contains("選ばれていません"));
        g.src = "a.mp4".into();
        assert!(build_plan(Some(&g), None, &s).unwrap_err().contains("解析がまだ"));
        let a = GuideAnalysis { pts: Arc::new(vec![0.0, 1.0]), ..Default::default() };
        assert!(build_plan(Some(&g), Some(&a), &s).unwrap_err().contains("アンカー"));
        // 解析した素材と選んでいる素材が違えば、組み立てない（フォルダの違い・大文字小文字は無視）
        let a = GuideAnalysis { src: "B.MP4".into(), pts: Arc::new(vec![0.0, 1.0]), ..Default::default() };
        assert!(build_plan(Some(&g), Some(&a), &s).unwrap_err().contains("違います"));
        let a = GuideAnalysis { src: "rec\\A.MP4".into(), pts: Arc::new(vec![0.0, 1.0]), ..Default::default() };
        assert!(build_plan(Some(&g), Some(&a), &s).unwrap_err().contains("アンカー"));
    }

    #[test]
    fn create_fx_stub_writes_once_and_refuses_when_it_exists() {
        let d = tmp("fxstub");
        // タイムラインが無ければ要る
        assert!(fx_stub_needed(&d));
        let p = create_fx_stub(&d).unwrap();
        assert_eq!(p, d.join("fx").join("guide_fx.py"));
        let text = std::fs::read_to_string(&p).unwrap();
        assert_eq!(text, FX_STUB);
        assert!(text.contains("def build(") && text.contains("GuideFx(ROOT)") && text.contains("guide_template"));
        // 作った入口は演出のタイムラインとして見つかる（回を開いたとき・見張りが読み込む）
        assert_eq!(crate::project::find_timeline(&d), Some(p.clone()));
        assert!(!fx_stub_needed(&d));
        // あれば断って、中身は触らない
        std::fs::write(&p, "# 手で直した
").unwrap();
        let e = create_fx_stub(&d).unwrap_err();
        assert!(e.contains("guide_fx.py") && e.contains("もうあります"), "{e}");
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "# 手で直した
");
        // 一時ファイルが残らない
        let names: Vec<_> = std::fs::read_dir(d.join("fx")).unwrap().flatten().map(|e| e.file_name().to_string_lossy().to_string()).collect();
        assert_eq!(names, vec!["guide_fx.py".to_string()]);
        let _ = std::fs::remove_dir_all(&d);
        // ほかの演出（foo_fx.py）がもうあれば、guide_create では作らない
        let d = tmp("fxstub2");
        std::fs::create_dir_all(d.join("fx")).unwrap();
        std::fs::write(d.join("fx").join("foo_fx.py"), "def build():
    pass
").unwrap();
        assert!(!fx_stub_needed(&d));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn auto_anchors_never_touch_t() {
        let mut g = Guide::default();
        g.anchors.insert("manual_end".into(), ak_proto::Anchor { t: Some(170.0), auto: Some(1.0) });
        let new: BTreeMap<String, f64> = [("manual_end".to_string(), 181.0), ("auto_end".to_string(), 300.0)].into();
        assert!(apply_auto_anchors(&mut g, &new));
        assert_eq!(g.anchors["manual_end"].t, Some(170.0));
        assert_eq!(g.anchors["manual_end"].auto, Some(181.0));
        assert_eq!(g.anchors["auto_end"].t, None);
        assert_eq!(g.anchors["auto_end"].auto, Some(300.0));
        // 同じ候補ならもう書かない
        assert!(!apply_auto_anchors(&mut g, &new));
        // 解析し直して見つからなくなった候補は消す（t はそのまま）
        let fewer: BTreeMap<String, f64> = [("auto_end".to_string(), 300.0)].into();
        assert!(apply_auto_anchors(&mut g, &fewer));
        assert_eq!(g.anchors["manual_end"].auto, None);
        assert_eq!(g.anchors["manual_end"].t, Some(170.0));
    }

    #[test]
    fn rel_src_accepts_inside_only() {
        let root = Path::new("/tmp/an/an_MAIN_17-5");
        assert_eq!(rel_src(root, "QSMF5499.MP4").unwrap(), "QSMF5499.MP4");
        assert_eq!(rel_src(root, "./rec/A.MP4").unwrap(), "rec/A.MP4");
        assert_eq!(rel_src(root, "\\tmp\\an\\an_MAIN_17-5\\QSMF5499.MP4").unwrap(), "QSMF5499.MP4");
        assert_eq!(rel_src(root, "/TMP/an/an_main_17-5/rec/A.mp4").unwrap(), "rec/A.mp4");
        assert!(rel_src(root, "/tmp/an/other/A.mp4").unwrap_err().contains("中の"));
        // 回のフォルダ名が前方一致するだけの別のフォルダは外
        assert!(rel_src(root, "/tmp/an/an_MAIN_17-5_copy/A.mp4").is_err());
        assert!(rel_src(root, "../A.mp4").unwrap_err().contains(".."));
        assert!(rel_src(root, "  ").is_err());
        // 末尾の / つきの root でも同じ
        assert_eq!(rel_src(Path::new("E:/x/"), "E:/x/a.mp4").unwrap(), "a.mp4");
    }

    fn script(lines: &[&str]) -> String {
        lines.join("\n") + "\n"
    }

    /// 偽の guide_scan.py を置いたフォルダで、解析の動かし方（進み具合・失敗の知らせ）を確かめる。python が無ければ飛ばす
    fn fake_kit(name: &str, script: &str) -> Option<PathBuf> {
        let ok = std::process::Command::new("python").arg("--version").output().map(|o| o.status.success()).unwrap_or(false);
        if !ok {
            eprintln!("python が無いので skip");
            return None;
        }
        let d = tmp(name);
        std::fs::write(d.join("guide_scan.py"), script).unwrap();
        Some(d)
    }

    #[test]
    fn run_scan_process_reports_progress_and_done() {
        let Some(kit) = fake_kit(
            "scan-ok",
            &script(&["import sys", "print('progress 0.100', flush=True)", "print('progress 0.500', flush=True)", "print('警告: 読めないコマ', file=sys.stderr)", "print('progress 1.000', flush=True)", "print('done ' + sys.argv[3], flush=True)"]),
        ) else { return };
        let prog = std::cell::RefCell::new(vec![]);
        let logs = std::cell::RefCell::new(vec![]);
        let r = run_scan_process("python", &kit, Path::new("v.mp4"), Path::new("out.json"), |x| prog.borrow_mut().push(x),
                                 |l| logs.borrow_mut().push(l.to_string()));
        assert_eq!(r, Ok(()));
        assert_eq!(*prog.borrow(), vec![0.1, 0.5, 1.0]);
        assert_eq!(*logs.borrow(), vec!["警告: 読めないコマ".to_string()]);
        let _ = std::fs::remove_dir_all(&kit);
    }

    #[test]
    fn kit_に_guide_scan_が無ければ動かさずに断る() {
        let kit = std::env::temp_dir().join(format!("ak-noscan-{}", std::process::id()));
        std::fs::create_dir_all(&kit).unwrap();
        let e = run_scan_process("python-that-does-not-exist", &kit, Path::new("v.mp4"), Path::new("o.json"), |_| {}, |_| {}).unwrap_err();
        assert_eq!(e, "攻略動画の素材を解析できませんでした: 演出の kit に guide_scan.py がありません");
        let _ = std::fs::remove_dir_all(&kit);
    }

    #[test]
    fn guide_template_の警告() {
        use crate::render::engine::KitInfo;
        let k = |f: &[&str]| KitInfo { name: "x".into(), api: Some(1), features: f.iter().map(|s| s.to_string()).collect(), path: None };
        assert!(template_warning(None).is_none());
        assert!(template_warning(Some(&k(&["guide_template", "guide_scan"]))).is_none());
        let w = template_warning(Some(&k(&["guide_scan"]))).unwrap();
        assert!(w.message.contains("guide_template がありません") && w.level.is_none());
        assert!(template_warning(Some(&k(&[]))).is_some());
    }

    #[test]
    fn run_scan_process_failure_carries_stderr_tail() {
        let Some(kit) = fake_kit(
            "scan-ng",
            &script(&["import sys", "print('progress 0.2', flush=True)", "print('解析できませんでした: ffmpeg が落ちました', file=sys.stderr)", "sys.exit(1)"]),
        ) else { return };
        let e = run_scan_process("python", &kit, Path::new("v.mp4"), Path::new("o.json"), |_| {}, |_| {}).unwrap_err();
        assert!(e.starts_with("解析できませんでした: ") && e.contains("ffmpeg が落ちました"), "{e}");
        // 終了コードだけで標準エラーが空なら、コードを出す
        std::fs::write(kit.join("guide_scan.py"), script(&["import sys", "sys.exit(3)"])).unwrap();
        let e = run_scan_process("python", &kit, Path::new("v.mp4"), Path::new("o.json"), |_| {}, |_| {}).unwrap_err();
        assert!(e.contains("終了コード 3"), "{e}");
        // 0 で終わっても done が無ければ失敗
        std::fs::write(kit.join("guide_scan.py"), script(&["print('progress 1.0')"])).unwrap();
        let e = run_scan_process("python", &kit, Path::new("v.mp4"), Path::new("o.json"), |_| {}, |_| {}).unwrap_err();
        assert!(e.contains("終わりの知らせ"), "{e}");
        // python が無い
        let e = run_scan_process("python-not-found-xyz", &kit, Path::new("v.mp4"), Path::new("o.json"), |_| {}, |_| {}).unwrap_err();
        assert!(e.contains("python を起動できませんでした"), "{e}");
        let _ = std::fs::remove_dir_all(&kit);
    }

    #[test]
    fn scan_lines_and_stderr_tail() {
        assert_eq!(parse_scan_line("progress 0.250"), ScanLine::Progress(0.25));
        assert_eq!(parse_scan_line("progress 1.7"), ScanLine::Progress(1.0));
        assert_eq!(parse_scan_line("progress abc"), ScanLine::Other);
        assert_eq!(parse_scan_line("progress nan"), ScanLine::Other);
        assert_eq!(parse_scan_line("done E:/a/studio/guide_analysis.json\r"), ScanLine::Done("E:/a/studio/guide_analysis.json".into()));
        assert_eq!(parse_scan_line("警告: x"), ScanLine::Other);
        let lines: Vec<String> = ["a", "", "b", "c", "d"].iter().map(|s| s.to_string()).collect();
        assert_eq!(stderr_tail(&lines, 2), "c\nd");
        assert_eq!(stderr_tail(&lines, 10), "a\nb\nc\nd");
        assert_eq!(stderr_tail(&[], 3), "");
    }
}
