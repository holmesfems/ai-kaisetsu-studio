//! 攻略動画の型（`studio/guide.json` と、画面・HTTP に返す形）。
//!
//! 読み書きとパートの組み立ては `src-tauri/src/guide/`。TypeScript の型は ts-rs で `ui/src/gen/` に書き出す。

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use ts_rs::TS;

/// アンカーの名前（素材の時刻の基準）。順番に並んでいなければならない
pub const ANCHOR_NAMES: [&str; 5] = ["formation_dark", "manual_cost0", "manual_end", "auto_cost0", "auto_end"];

/// 戦闘画面の素材の大きさの基準（HudCrop の src はこの大きさでの画素）
pub const REF_W: f64 = 2752.0;
pub const REF_H: f64 = 2064.0;

/// `studio/guide.json`（version 1）
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, JsonSchema, TS)]
#[serde(default)]
#[ts(export)]
pub struct Guide {
    pub version: u32,
    /// 通しの素材（回のフォルダからの相対。例 "QSMF5499.MP4"）
    pub src: String,
    /// formation_dark / manual_cost0 / manual_end / auto_cost0 / auto_end
    pub anchors: BTreeMap<String, Anchor>,
    /// この回だけ変えた固定値（None は設定の値）
    pub fixed: GuideFixedOpt,
    /// 手動パートのフェードカット（素材の時刻）
    pub cuts: Vec<Cut>,
    /// 戦闘画面のレイヤーの重ね方
    pub layout: GuideLayout,
    /// 手動操作解説の操作（素材の時刻 t の順）
    pub ops: Vec<GuideOp>,
    /// 編成（使う枠・補足）
    pub formation: GuideFormation,
    /// 補足素材の小窓（手動パートの間、操作のところに別の素材を小さく出す）
    pub subs: Vec<GuideSub>,
    /// 知らない項目（新しい版・Claude が足したもの）。保存し直しても消さない
    #[serde(flatten)]
    #[ts(skip)]
    #[schemars(skip)]
    pub extra: serde_json::Map<String, Value>,
}

impl Default for Guide {
    fn default() -> Self {
        Guide {
            version: 1,
            src: String::new(),
            anchors: BTreeMap::new(),
            fixed: GuideFixedOpt::default(),
            cuts: vec![],
            layout: GuideLayout::default(),
            ops: vec![],
            formation: GuideFormation::default(),
            subs: vec![],
            extra: Default::default(),
        }
    }
}

/// 補足素材の小窓 1 つ（手動パートの間、操作の出力の時刻に合わせて別の素材の区間を小さく出す）
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, JsonSchema, TS)]
#[serde(default)]
#[ts(export)]
pub struct GuideSub {
    /// 回のフォルダからの相対（例 "OPVF9634.MP4"）
    pub src: String,
    /// 補足素材の秒（この区間を出す）
    pub src_in: f64,
    pub src_out: f64,
    /// どの操作のところに出すか（操作の id）
    pub at_op: String,
    /// 出し始め = その操作の出力の時刻 + offset（秒。負なら前）
    pub offset: f64,
    /// 小窓の見出し
    pub label: String,
    /// 知らない項目。保存し直しても消さない
    #[serde(flatten)]
    #[ts(skip)]
    #[schemars(skip)]
    pub extra: serde_json::Map<String, Value>,
}

/// 手動操作解説の操作 1 つ
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, JsonSchema, TS)]
#[serde(default)]
#[ts(export)]
pub struct GuideOp {
    /// "op1"… 作るときに Rust が振る（消しても詰めない）
    pub id: String,
    /// 素材の時刻（秒）。保存の前にいちばん近いコマの時刻に合わせる
    pub t: f64,
    /// "deploy"（配置）/ "skill"（スキル）/ "retreat"（撤退）/ "trust"（信頼枠の配置。タイムラインの演出には載せない）
    pub kind: String,
    /// キャラ名
    pub op: String,
    /// 置き場所・向き（自由文。例 "R3C5 ↑"）
    pub r#where: String,
    /// 補足（自由文。例 "S2 は溜めてから"）。画面と動画の NOW/NEXT・操作の帯に出す
    pub info: String,
    /// フレーム単位で合わせる操作
    pub precise: bool,
    /// NOW を操作の瞬間より何秒前に出すか（0〜10 秒の個別値。None は固定値の汎用値。信頼枠は常に 0）
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub now_lead: Option<f64>,
    /// コストでのタイミングの指定（例 "Kill15 の敵が出た数字 +12"）
    pub cue_text: String,
    /// 注意書き
    pub note: String,
    /// 知らない項目（解析が足したコストのコマ番号など）。保存し直しても消さない
    #[serde(flatten)]
    #[ts(skip)]
    #[schemars(skip)]
    pub extra: serde_json::Map<String, Value>,
}

/// 操作の種類（`GuideOp::kind`）
pub const OP_KINDS: [&str; 4] = ["deploy", "skill", "retreat", "trust"];

/// 編成パートの見せ方
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, JsonSchema, TS)]
#[serde(default)]
#[ts(export)]
pub struct GuideFormation {
    /// 枠の矩形（素材の画素、2752x2064 での値）。空なら既定の 12 枠（`default_slots`）
    pub slots: Vec<[f64; 4]>,
    /// 使う枠の番号。空なら「全部使う」（暗くしない）
    pub used: Vec<usize>,
    /// 枠の番号 → 補足（レベル・凸・モジュールの要求値など）
    pub notes: BTreeMap<String, String>,
    /// 知らない項目。保存し直しても消さない
    #[serde(flatten)]
    #[ts(skip)]
    #[schemars(skip)]
    pub extra: serde_json::Map<String, Value>,
}

/// 編成画面の既定の 12 枠（17-5 の実測。2752x2064）。番号は上の段の左から 0〜5、下の段が 6〜11
pub fn default_slots() -> Vec<[f64; 4]> {
    let mut v = Vec::with_capacity(12);
    for (y0, y1) in [(426.0, 1010.0), (1046.0, 1630.0)] {
        for i in 0..6 {
            let x = 230.0 + 333.0 * i as f64;
            v.push([x, y0, x + 270.0, y1]);
        }
    }
    v
}

impl GuideFormation {
    /// 効く枠の矩形（slots が空なら既定の 12 枠）
    pub fn effective_slots(&self) -> Vec<[f64; 4]> {
        if self.slots.is_empty() { default_slots() } else { self.slots.clone() }
    }

    /// 枠 i を使うか（used が空なら全部使う）
    pub fn is_used(&self, i: usize) -> bool {
        self.used.is_empty() || self.used.contains(&i)
    }
}

/// アンカー（素材の秒）。使うのは t、無ければ auto（解析の候補）
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, JsonSchema, TS)]
#[serde(default)]
#[ts(export)]
pub struct Anchor {
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub t: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub auto: Option<f64>,
}

impl Anchor {
    /// 使う値（t、無ければ auto）
    pub fn value(&self) -> Option<f64> {
        self.t.or(self.auto)
    }
}

/// 手動パートのフェードカット（素材の時刻。src_in から src_out までを抜く）
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, JsonSchema, TS)]
#[serde(default)]
#[ts(export)]
pub struct Cut {
    pub src_in: f64,
    pub src_out: f64,
    /// 継ぎ目のクロスフェードの長さ（秒）
    pub fade: f64,
}

/// 固定値（秒）
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, JsonSchema, TS)]
#[serde(default)]
#[ts(export)]
pub struct GuideFixed {
    /// 出力の 0 秒から、編成パートが終わるまで（出力上で固定）
    pub formation_end: f64,
    /// 暗転の後に見せる長さ
    pub formation_tail: f64,
    /// 手動の最初のコストの前・作戦終了の帯の後
    pub manual_lead: f64,
    pub manual_tail: f64,
    /// 自動の同じもの
    pub auto_lead: f64,
    pub auto_tail: f64,
    /// リザルトの長さ
    pub result_len: f64,
    /// NOW を操作の瞬間より何秒前に出すか（信頼枠以外の汎用値。操作ごとの `GuideOp::now_lead` が優先）
    pub now_lead: f64,
}

impl Default for GuideFixed {
    fn default() -> Self {
        GuideFixed {
            formation_end: 8.0,
            formation_tail: 0.5,
            manual_lead: 2.0,
            manual_tail: 1.5,
            auto_lead: 2.0,
            auto_tail: 1.5,
            result_len: 8.0,
            now_lead: 1.0,
        }
    }
}

/// この回だけ変えた固定値（無い項目は設定の値）
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, JsonSchema, TS)]
#[serde(default)]
#[ts(export)]
pub struct GuideFixedOpt {
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub formation_end: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub formation_tail: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub manual_lead: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub manual_tail: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub auto_lead: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub auto_tail: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub result_len: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub now_lead: Option<f64>,
}

impl GuideFixedOpt {
    /// base の上に、この回だけ変えた値を重ねる（回 → 設定 → 既定の「回」の層）
    pub fn over(&self, base: &GuideFixed) -> GuideFixed {
        GuideFixed {
            formation_end: self.formation_end.unwrap_or(base.formation_end),
            formation_tail: self.formation_tail.unwrap_or(base.formation_tail),
            manual_lead: self.manual_lead.unwrap_or(base.manual_lead),
            manual_tail: self.manual_tail.unwrap_or(base.manual_tail),
            auto_lead: self.auto_lead.unwrap_or(base.auto_lead),
            auto_tail: self.auto_tail.unwrap_or(base.auto_tail),
            result_len: self.result_len.unwrap_or(base.result_len),
            now_lead: self.now_lead.unwrap_or(base.now_lead),
        }
    }
}

/// 戦闘画面のレイヤーの重ね方
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, JsonSchema, TS)]
#[serde(default)]
#[ts(export)]
pub struct GuideLayout {
    /// 素材を何倍にして中央に置くか（0.7）
    pub scale: f64,
    /// 手動パートでレイヤー全体を縮める倍率（0.75）
    pub manual_scale: f64,
    /// 手動パートでのレイヤーの左上の位置（1920x1080 の座標。0, 0 = 左上に寄せる）
    pub manual_x: f64,
    pub manual_y: f64,
    pub crops: Vec<HudCrop>,
    /// 出力の上に拡大して重ねる切り抜き（コストの拡大など）。手動の縮めの後、レイヤーの外に置く
    pub zooms: Vec<HudZoom>,
    /// 補足素材の小窓の枠（出力の x, y, 幅, 高さ。1920x1080 での値）。中に縦横比を保って収める
    pub sub_box: [f64; 4],
}

impl Default for GuideLayout {
    fn default() -> Self {
        let both = || vec!["manual".to_string(), "auto".to_string()];
        GuideLayout {
            scale: 0.7,
            manual_scale: 0.75,
            manual_x: 0.0,
            manual_y: 0.0,
            crops: vec![
                // 敵数・耐久。x は元の位置のまま、上端を y=30 に
                HudCrop { name: "top".into(), src: [980.0, 20.0, 1760.0, 110.0], to: [683.0, 30.0], parts: both() },
                // 2X・一時停止。x は元の位置のまま
                HudCrop { name: "speed".into(), src: [2280.0, 30.0, 2700.0, 200.0], to: [1593.0, 30.0], parts: both() },
                // キャラ待機欄。横全体（12 枠すべて）を切り抜く。置き方（大きさ・不透明度・左下寄せ）は設定の `GuideBench` が決める（to は使わない）
                HudCrop { name: "bench".into(), src: BENCH_SRC, to: [0.0, 888.0], parts: both() },
            ],
            zooms: default_zooms(),
            sub_box: default_sub_box(),
        }
    }
}

/// 待機欄の切り抜きの既定（素材の画素。横全体・12 枠すべて）
pub const BENCH_SRC: [f64; 4] = [0.0, 1790.0, 2752.0, 2064.0];
/// 前の既定（`upgrade_layout` が、この値のままの回を今の既定に読み替える）
pub const OLD_BENCH_SRC: [f64; 4] = [0.0, 1790.0, 2310.0, 2064.0];

/// コストの拡大の既定（17-5 の実測。菱形・数字・白いバーと、その下の「残り配置可能数」の箱（x 2280–2752・y 1690–1760）まで。472x205 → 448x195）
pub const COST_ZOOM_SRC: [f64; 4] = [2280.0, 1560.0, 2752.0, 1765.0];
pub const COST_ZOOM_TO: [f64; 2] = [1456.0, 600.0];
pub const COST_ZOOM_SCALE: f64 = 0.95;
/// 前の既定（282x130 → 395x182）
pub const OLD_COST_ZOOM_SRC: [f64; 4] = [2470.0, 1565.0, 2752.0, 1695.0];
pub const OLD_COST_ZOOM_TO: [f64; 2] = [1482.0, 612.0];
pub const OLD_COST_ZOOM_SCALE: f64 = 1.4;

/// 拡大の切り抜きの既定
pub fn default_zooms() -> Vec<HudZoom> {
    vec![HudZoom { name: "cost".into(), src: COST_ZOOM_SRC, to: COST_ZOOM_TO, scale: COST_ZOOM_SCALE, parts: vec!["manual".into()] }]
}

/// 補足素材の小窓の既定（戦闘画面の右上、倍速・一時停止のボタンの下。右の欄は NOW / NEXT のチップの列が使うので、小窓は戦闘画面の上に置く）
pub fn default_sub_box() -> [f64; 4] {
    [1060.0, 150.0, 360.0, 240.0]
}
/// 前の既定（`upgrade_layout` 用。右の欄の真ん中だった）
pub const PREV_SUB_BOX: [f64; 4] = [1500.0, 320.0, 360.0, 240.0];
/// 前々の既定（`upgrade_layout` 用）
pub const OLD_SUB_BOX: [f64; 4] = [1480.0, 300.0, 400.0, 300.0];

/// 攻略動画の操作の効果音（4 つのグループ）で、アプリの効果音の棚から選んだ音。全部の回で使うので設定（settings.json の
/// `guide_se`：種類 → この形）に持つ。回の `fx/se/bank.json` にその種類があれば回のほうが勝つ
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, JsonSchema, TS)]
#[serde(default)]
#[ts(export)]
pub struct GuideSe {
    /// 棚の `<種類>/<名前>.wav`（相対パス）か "synth"（合成の音）
    pub src: String,
    /// 標準の大きさからの音量（dB。-30〜+12）
    pub db: f64,
}

impl Default for GuideSe {
    fn default() -> Self {
        GuideSe { src: "synth".into(), db: 0.0 }
    }
}

/// 待機欄（下のキャラの待機欄の切り抜き）の置き方。全部の回で使うので設定（settings.json の `guide_bench`）に持つ
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, JsonSchema, TS)]
#[serde(default)]
#[ts(export)]
pub struct GuideBench {
    /// 素材の画素 → 出力の画素の倍率（0.5 なら 2752 の幅が 1376）
    pub scale: f64,
    /// 不透明度（0〜1）
    pub opacity: f64,
}

pub const BENCH_SCALE_RANGE: (f64, f64) = (0.2, 1.0);
pub const BENCH_OPACITY_RANGE: (f64, f64) = (0.1, 1.0);

impl Default for GuideBench {
    fn default() -> Self {
        GuideBench { scale: 0.5, opacity: 0.5 }
    }
}

impl GuideBench {
    /// 保存してよい値か（外れていれば和文の理由）
    pub fn check(&self) -> Result<(), String> {
        let (s0, s1) = BENCH_SCALE_RANGE;
        let (o0, o1) = BENCH_OPACITY_RANGE;
        if !self.scale.is_finite() || self.scale < s0 || self.scale > s1 {
            return Err(format!("待機欄の大きさは {}〜{}%（{}〜{} 倍）にしてください（今は {}）", s0 * 100.0, s1 * 100.0, s0, s1, self.scale));
        }
        if !self.opacity.is_finite() || self.opacity < o0 || self.opacity > o1 {
            return Err(format!("待機欄の不透明度は {}〜{}% にしてください（今は {}）", o0 * 100.0, o1 * 100.0, self.opacity));
        }
        Ok(())
    }

    /// 範囲に収めた値（settings.json を手で直して外れていても、描くときに巨大な絵や見えない絵にしない。数でなければ既定）
    pub fn clamped(&self) -> GuideBench {
        let d = GuideBench::default();
        let fit = |v: f64, (a, b): (f64, f64), dv: f64| if v.is_finite() { v.clamp(a, b) } else { dv };
        GuideBench { scale: fit(self.scale, BENCH_SCALE_RANGE, d.scale), opacity: fit(self.opacity, BENCH_OPACITY_RANGE, d.opacity) }
    }
}

/// 素材から切り抜いて、出力の上に拡大して重ねる部品（レイヤーの外。手動の縮めの後に置く）
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, JsonSchema, TS)]
#[serde(default)]
#[ts(export)]
pub struct HudZoom {
    /// "cost"
    pub name: String,
    /// 素材の画素（2752x2064 での値。大きさが違えば比で読み替える）x0, y0, x1, y1
    pub src: [f64; 4],
    /// 出力（1920x1080）での左上の位置
    pub to: [f64; 2],
    /// 素材の画素 → 出力の倍率
    pub scale: f64,
    /// 出すパート（"manual" など）
    pub parts: Vec<String>,
}

impl Default for HudZoom {
    fn default() -> Self {
        HudZoom { name: String::new(), src: [0.0; 4], to: [0.0; 2], scale: 1.0, parts: vec![] }
    }
}

/// 素材から切り抜いてレイヤーに重ねる部品
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, JsonSchema, TS)]
#[serde(default)]
#[ts(export)]
pub struct HudCrop {
    /// "top" / "speed" / "bench"
    pub name: String,
    /// 素材の画素（2752x2064 での値。大きさが違えば比で読み替える）x0, y0, x1, y1
    pub src: [f64; 4],
    /// レイヤー（1920x1080）での左上の位置。倍率は scale
    pub to: [f64; 2],
    /// 出すパート（"manual" / "auto"）
    pub parts: Vec<String>,
}

impl Default for HudCrop {
    fn default() -> Self {
        HudCrop { name: String::new(), src: [0.0; 4], to: [0.0; 2], parts: vec![] }
    }
}

/// 組み立てたパート 1 つ（出力の秒）。name は formation / manual / auto / result
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, JsonSchema, TS)]
#[ts(export)]
pub struct PlanPart {
    pub name: String,
    pub t0: f64,
    pub t1: f64,
}

/// 操作 1 つの、組み立てから分かる位置と、解析のそのコマの値（`GuideState::ops_out`。ops と同じ並び）
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, JsonSchema, TS)]
#[ts(export)]
pub struct OpOut {
    pub id: String,
    /// 手動パートでの出力の時刻（秒）。カットの中・手動の区間の外なら null
    pub t_out: Option<f64>,
    /// NOW が始まる出力の時刻（秒）。t_out から先出しを引き、前の操作の瞬間・手動パートの頭より前にしない。t_out が無ければ null
    pub t_now: Option<f64>,
    /// 素材のコマ番号（解析の pts の添字）
    pub frame: u32,
    /// そのコマのコスト（読めていなければ null）
    pub cost: Option<i32>,
    /// そのコマのゲーム内の時間（ゲームのフレーム数。60 で 1 秒。読めていなければ null）
    pub game: Option<f64>,
    /// カットの中にある
    pub in_cut: bool,
}

/// 解析の戦闘区間（guide_analysis.json の battles）
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, JsonSchema, TS)]
#[serde(default)]
#[ts(export)]
pub struct GuideBattle {
    pub start: i64,
    pub end: i64,
    pub cost0: Option<i64>,
    pub end_band: Option<i64>,
}

/// 解析の結果の要約（画面に出す分）
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, JsonSchema, TS)]
#[ts(export)]
pub struct GuideAnalysisInfo {
    /// studio/guide_analysis.json があって読めたか
    pub exists: bool,
    /// 解析したコマの数
    pub frames: u32,
    pub battles: Vec<GuideBattle>,
    pub warnings: Vec<String>,
    /// 解析が出したアンカーの候補（素材の秒）
    pub anchors: BTreeMap<String, f64>,
}

/// `guide_get` の戻り値
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, JsonSchema, TS)]
#[ts(export)]
pub struct GuideState {
    /// guide.json（無ければ null）
    pub guide: Option<Guide>,
    /// 効いている固定値（回 → 設定 → 既定）
    pub fixed_effective: GuideFixed,
    /// 設定の guide_fixed（全部の回で使う値）
    pub fixed_settings: GuideFixed,
    /// 効いている待機欄の置き方（設定の guide_bench。全部の回で使う）
    pub bench: GuideBench,
    /// 組み立てたパート（作れていなければ空）
    pub plan_parts: Vec<PlanPart>,
    /// アンカーが出力のどの時刻に来るか（秒。「ここへ」の行き先）。組み立てられていなければ空
    pub anchor_out: BTreeMap<String, f64>,
    /// ops と同じ並びの、出力の時刻とそのコマのコスト・ゲーム内の時間。組み立てられていなければ空
    pub ops_out: Vec<OpOut>,
    /// 各カットの継ぎ目の出力の時刻 [始まり, 終わり]（フェードが無ければ同じ）。カットは素材の時刻の順
    pub cuts_out: Vec<[f64; 2]>,
    pub analysis: GuideAnalysisInfo,
    /// 読めない・解析がまだ・組み立てられない理由（和文）。無ければ null
    pub error: Option<String>,
    /// 解析を走らせているか
    pub scanning: bool,
    /// 直したほうがよいこと（和文。補足素材の小窓が出せない理由など。組み立てのエラーではない）
    pub warnings: Vec<String>,
    /// guide.json を書く・読み直すたびに増える番号。画面は保存のとき今の値を送り（`guide_set` の `base_rev`）、
    /// 違えば本体は保存せずに断る（画面の編集と Claude の変更がぶつかって、片方が消えないように）
    pub rev: u64,
    /// 戻せる Claude の変更（いちばん新しいもの。戻せないとき null）
    pub claude_undo: Option<ClaudeUndo>,
    /// 素材の音の wav（`studio/guide_audio.wav`）を作り直した印（ミリ秒）。画面の再生はこの値で読み直す。
    /// まだ作れていない・素材に音が無い・作れなかったときは null
    #[serde(default)]
    pub audio_rev: Option<u64>,
    /// 素材の音を作れなかった理由（和文。作れたら・まだ作っていなければ null）。行と詳細に「作れませんでした」と出す
    #[serde(default)]
    pub audio_err: Option<String>,
}

/// Claude（MCP の `guide_update`）が直した変更のうち、画面から戻せるもの
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, JsonSchema, TS)]
#[ts(export)]
pub struct ClaudeUndo {
    /// 変更の番号（回を開いている間で一意。同じ秒の変更も見分ける。`guide_undo_claude` に渡す）
    pub id: u32,
    /// 直した時刻（ローカル時刻。例 "2026-10-21T21:03:00"）
    pub at: String,
    /// Claude が添えた理由
    pub reason: String,
}

/// `guide_src_at` の戻り値（出力の時刻に見えている素材のコマ）
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, JsonSchema, TS)]
#[ts(export)]
pub struct SrcAt {
    /// 素材のコマ番号（解析の pts の添字）
    pub frame: u32,
    /// そのコマの素材の時刻（秒）
    pub t_src: f64,
    /// パート名（formation / manual / auto / result）
    pub part: String,
}
