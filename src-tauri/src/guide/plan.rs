//! 攻略動画のパートの組み立て（純粋な関数）。
//!
//! 台本（声）の無い回なので、時間の基準は cue ではなく「パート」と「基準の瞬間（アンカー）」。
//! アンカーの位置合わせ・カットから、出力（60fps）のコマ k ごとに「見せる素材のコマ番号」を決める。
//! 素材は可変 fps なので、素材の時刻は pts（秒）で持ち、コマは「その時刻に表示中のコマ」（`pts[i] <= 時刻` の最大の i）を使う。

use ak_proto::guide::{OpOut, PlanPart, ANCHOR_NAMES};
use ak_proto::{Cut, Guide, GuideFixed, GuideOp};
use std::sync::Arc;

/// 出力の fps
/// `studio/guide_map.json` の形の版（演出の Python は同じ版しか読まない）。項目を足す・意味を変えたら上げる
pub const MAP_VERSION: u32 = 2;
pub const FPS: f64 = 60.0;
/// 操作ごとの NOW の先出しの上限（秒）
pub const NOW_LEAD_MAX: f64 = 10.0;

/// パートの名前（出力の順）
pub const PART_NAMES: [&str; 4] = ["formation", "manual", "auto", "result"];

/// 画面・モニターのシーンの帯に出すパートの名前
pub fn part_label(name: &str) -> &str {
    match name {
        "formation" => "編成",
        "manual" => "手動",
        "auto" => "自動",
        "result" => "リザルト",
        other => other,
    }
}

/// 出力の 1 コマ。part は PART_NAMES の添字、src は素材のコマ番号（pts の添字）、
/// mix は (B のコマ, B の重み)。カットの継ぎ目のクロスフェードのときだけ入る
#[derive(Clone, Debug, PartialEq)]
pub struct PlanFrame {
    pub part: u8,
    pub src: u32,
    pub mix: Option<(u32, f32)>,
}

/// 素材の音の 1 区間。出力の [out0, out1)（秒）に、素材の時刻 src0 から並べる。
/// fade_in・fade_out はカットのクロスフェードの長さ（秒。直線で上げる・下げる。無ければ 0）。
/// src0 が 0 より前（編成の頭）の部分は無音
#[derive(Clone, Debug, PartialEq)]
pub struct AudioSeg {
    pub out0: f64,
    pub out1: f64,
    pub src0: f64,
    pub fade_in: f64,
    pub fade_out: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct GuidePlan {
    pub fps: f64,
    pub parts: Vec<PlanPart>,
    pub frames: Vec<PlanFrame>,
    /// アンカーが出力に来る時刻（秒）
    pub anchor_out: Vec<(String, f64)>,
    /// アンカーの素材の時刻（いちばん近いコマに合わせたもの）
    pub anchor_src: Vec<(String, f64)>,
    /// 解析のコマごとの素材の時刻（出力の時刻 ↔ 素材の時刻の行き来に使う）
    pub pts: Arc<Vec<f64>>,
    /// 手動パートの素材と出力の対応（カットで分かれた断片）と、パートの頭の出力の時刻
    manual: ManualMap,
    manual_t0: f64,
    /// 各カットの継ぎ目の出力の時刻 [始まり, 終わり]（素材の時刻の順。フェードが無ければ同じ）
    pub seams: Vec<[f64; 2]>,
    /// NOW の先出しの汎用値（効く固定値の `now_lead`。組み立てには効かず、`ops_out` が NOW の開始を決めるのに使う）
    pub now_lead: f64,
    /// 素材の音の区間（映像と同じ対応。`audio_segments`）
    audio: Vec<AudioSeg>,
}

impl GuidePlan {
    /// 出力の終わり（秒）
    pub fn end(&self) -> f64 {
        self.frames.len() as f64 / self.fps
    }

    /// 時刻 t のコマ（範囲外は None）
    pub fn frame_at(&self, t: f64) -> Option<&PlanFrame> {
        if !t.is_finite() || t < 0.0 {
            return None;
        }
        self.frames.get((t * self.fps).round() as usize)
    }

    pub fn part_name(&self, f: &PlanFrame) -> &'static str {
        PART_NAMES[f.part as usize]
    }

    /// 出力の時刻 t に見えている素材のコマ (コマ番号, そのコマの素材の時刻, パート名)。クロスフェード中は A のコマ。範囲外は None
    pub fn src_at(&self, t: f64) -> Option<(u32, f64, &'static str)> {
        let f = self.frame_at(t)?;
        let ts = *self.pts.get(f.src as usize)?;
        Some((f.src, ts, self.part_name(f)))
    }

    /// 素材の時刻 t_src が手動パートの出力のどの時刻に来るか。カットの中・手動の区間の外は None
    pub fn manual_out_of(&self, t_src: f64) -> Option<f64> {
        self.manual.try_out_of(t_src).map(|u| self.manual_t0 + u)
    }

    /// 素材の時刻 t_src（コマに合わせたもの）が、出力のどのパートに出ているか。どこにも出ていなければ None（最初に出る所）
    pub fn part_of_src(&self, t_src: f64) -> Option<&'static str> {
        self.frames
            .iter()
            .find(|f| self.pts.get(f.src as usize).is_some_and(|&p| (p - t_src).abs() < 1e-9))
            .map(|f| self.part_name(f))
    }

    /// 素材の音の区間（出力の時刻順）。映像のコマと同じ対応で、出力の頭は各パートの t0。
    /// 編成・手動の各区間（カットの前後はフェードの長さだけ重ねる）と、自動＋リザルト（続いた 1 区間）
    pub fn audio_segments(&self) -> Vec<AudioSeg> {
        self.audio.clone()
    }

    /// アンカーの素材の時刻
    pub fn anchor_src_of(&self, name: &str) -> Option<f64> {
        self.anchor_src.iter().find(|(k, _)| k == name).map(|(_, v)| *v)
    }

    /// `studio/guide_map.json`（演出の Python と Claude が読む出力の t ↔ 素材のコマの対応表）
    pub fn to_map_json(&self) -> serde_json::Value {
        let mut mix = serde_json::Map::new();
        for (k, f) in self.frames.iter().enumerate() {
            if let Some((b, w)) = f.mix {
                mix.insert(k.to_string(), serde_json::json!([b, (w as f64 * 1e4).round() / 1e4]));
            }
        }
        serde_json::json!({
            "version": MAP_VERSION,
            "fps": self.fps,
            "parts": self.parts,
            "src": self.frames.iter().map(|f| f.src).collect::<Vec<_>>(),
            "mix": mix,
        })
    }
}

/// 効く固定値: 回（guide.json の fixed）→ 設定（guide_fixed）→ 既定
pub fn effective_fixed(guide: &Guide, settings: &GuideFixed) -> GuideFixed {
    guide.fixed.over(settings)
}

/// 素材の時刻 t に表示中のコマ（`pts[i] <= t` の最大の i。先頭より前は 0）。
/// アンカー − lead + lead のような足し引きの誤差（9.208 → 9.20799…）で 1 つ前のコマにならないよう、少しだけ甘く比べる
pub fn frame_at_time(pts: &[f64], t: f64) -> u32 {
    let n = pts.partition_point(|&p| p <= t + 1e-6);
    n.saturating_sub(1) as u32
}

/// 素材の時刻 t にいちばん近いコマの時刻
pub fn snap_to_frame(pts: &[f64], t: f64) -> f64 {
    let i = pts.partition_point(|&p| p < t);
    match (i.checked_sub(1).map(|j| pts[j]), pts.get(i).copied()) {
        (Some(a), Some(b)) => if t - a <= b - t { a } else { b },
        (Some(a), None) => a,
        (None, Some(b)) => b,
        (None, None) => t,
    }
}

fn anchor(guide: &Guide, name: &str) -> Result<f64, String> {
    let v = guide.anchors.get(name).and_then(|a| a.value()).ok_or_else(|| {
        format!("アンカー「{name}」がありません。攻略タブで素材を解析するか、値を入れてください")
    })?;
    if !v.is_finite() {
        return Err(format!("アンカー「{name}」の値が数ではありません"));
    }
    Ok(v)
}

pub fn check_fixed(f: &GuideFixed) -> Result<(), String> {
    let items = [
        ("formation_end", f.formation_end),
        ("formation_tail", f.formation_tail),
        ("manual_lead", f.manual_lead),
        ("manual_tail", f.manual_tail),
        ("auto_lead", f.auto_lead),
        ("auto_tail", f.auto_tail),
        ("result_len", f.result_len),
        ("now_lead", f.now_lead),
    ];
    for (name, v) in items {
        if !v.is_finite() || v < 0.0 {
            return Err(format!("固定値「{name}」は 0 以上の数にしてください（今は {v}）"));
        }
    }
    Ok(())
}

/// 素材の時刻 t のコマの添字（いちばん近いコマ。同じ時刻が続くときはあとのコマ）
fn frame_index(pts: &[f64], t: f64) -> usize {
    frame_at_time(pts, snap_to_frame(pts, t)) as usize
}

/// いちばん近いコマから delta コマ進めた（負なら戻した）コマの時刻。先頭と末尾で止まる。
/// 同じ時刻のコマが続くときは、見た目の変わる次の（前の）時刻まで進める
pub fn step_frame(pts: &[f64], t_src: f64, delta: i32) -> f64 {
    if pts.is_empty() {
        return t_src;
    }
    let mut i = frame_index(pts, t_src);
    for _ in 0..delta.unsigned_abs() {
        if delta > 0 {
            i = pts.partition_point(|&p| p <= pts[i]).min(pts.len() - 1);
        } else {
            i = pts.partition_point(|&p| p < pts[i]).saturating_sub(1);
        }
    }
    pts[i]
}

/// 手動パートの素材の区間（カットで分かれた断片）と、断片どうしのフェード
#[derive(Clone, Debug, PartialEq)]
struct Seg {
    /// 素材の時刻
    a: f64,
    b: f64,
    /// パートの頭からの出力の時刻
    out0: f64,
}

#[derive(Clone, Debug, PartialEq)]
struct ManualMap {
    segs: Vec<Seg>,
    /// segs[i] と segs[i+1] の継ぎ目のフェード（秒）
    fades: Vec<f64>,
    len: f64,
}

/// 手動パートの素材 [s0, s1] からカットを抜いて詰める。カットが区間の外・重なる・フェードが入らないときは Err
fn manual_map(s0: f64, s1: f64, cuts: &[Cut]) -> Result<ManualMap, String> {
    let mut cuts: Vec<&Cut> = cuts.iter().collect();
    for c in &cuts {
        if !c.src_in.is_finite() || !c.src_out.is_finite() || !c.fade.is_finite() {
            return Err("カットの値が数ではありません".into());
        }
        if c.src_out <= c.src_in {
            return Err(format!("カットの終わり（{:.3} 秒）が始まり（{:.3} 秒）より前です", c.src_out, c.src_in));
        }
        if c.fade < 0.0 {
            return Err(format!("カット（{:.3}〜{:.3} 秒）のフェードは 0 以上にしてください", c.src_in, c.src_out));
        }
        if c.src_in <= s0 || c.src_out >= s1 {
            return Err(format!(
                "カット（{:.3}〜{:.3} 秒）が手動パートの素材の区間（{:.3}〜{:.3} 秒）の外にあります",
                c.src_in, c.src_out, s0, s1
            ));
        }
    }
    cuts.sort_by(|a, b| a.src_in.total_cmp(&b.src_in));
    for w in cuts.windows(2) {
        if w[1].src_in <= w[0].src_out {
            return Err(format!(
                "カット（{:.3}〜{:.3} 秒）と（{:.3}〜{:.3} 秒）が重なっています",
                w[0].src_in, w[0].src_out, w[1].src_in, w[1].src_out
            ));
        }
    }
    let mut bounds = vec![];
    let mut a = s0;
    for c in &cuts {
        bounds.push((a, c.src_in));
        a = c.src_out;
    }
    bounds.push((a, s1));
    let fades: Vec<f64> = cuts.iter().map(|c| c.fade).collect();
    let mut segs = vec![];
    let mut out0 = 0.0;
    for (i, &(a, b)) in bounds.iter().enumerate() {
        let before = if i > 0 { fades[i - 1] } else { 0.0 };
        let after = fades.get(i).copied().unwrap_or(0.0);
        if b - a < before + after - 1e-9 {
            return Err(format!(
                "カットの間（{:.3}〜{:.3} 秒）が短すぎて、前後のフェード（{:.2} 秒と {:.2} 秒）が入りません",
                a, b, before, after
            ));
        }
        segs.push(Seg { a, b, out0 });
        out0 += (b - a) - after;
    }
    let last = segs.last().expect("断片は 1 つ以上ある");
    let len = last.out0 + (last.b - last.a);
    Ok(ManualMap { segs, fades, len })
}

impl ManualMap {
    /// 素材の時刻 src が、パートの頭から何秒の出力に来るか（カットの中なら、その手前の断片の終わり）
    fn out_of(&self, src: f64) -> f64 {
        let k = self.segs.iter().rposition(|s| s.a <= src + 1e-9).unwrap_or(0);
        let s = &self.segs[k];
        s.out0 + (src.min(s.b) - s.a).max(0.0)
    }

    /// 素材の時刻 src の、パートの頭からの出力の時刻。カットの中・区間の外は None
    fn try_out_of(&self, src: f64) -> Option<f64> {
        let k = self.segs.iter().rposition(|s| s.a <= src + 1e-9)?;
        let s = &self.segs[k];
        let last = k + 1 == self.segs.len();
        let inside = src < s.b - 1e-9 || (last && src <= s.b + 1e-9);
        inside.then(|| s.out0 + (src - s.a).max(0.0))
    }

    /// パートの頭から u 秒の出力で見せる素材の時刻 (A, 継ぎ目なら (B の時刻, B の重み))
    fn at(&self, u: f64) -> (f64, Option<(f64, f64)>) {
        let k = self.segs.iter().rposition(|s| s.out0 <= u + 1e-9).unwrap_or(0);
        let s = &self.segs[k];
        if k >= 1 {
            let f = self.fades[k - 1];
            if f > 0.0 && u < s.out0 + f - 1e-9 {
                let p = &self.segs[k - 1];
                let w = ((u - s.out0) / f).clamp(0.0, 1.0);
                return (p.a + (u - p.out0), Some((s.a + (u - s.out0), w)));
            }
        }
        (s.a + (u - s.out0), None)
    }
}

/// パートを組み立てる。pts は guide_analysis.json の各コマの素材の時刻
pub fn plan(guide: &Guide, fixed: &GuideFixed, pts: &[f64]) -> Result<GuidePlan, String> {
    if pts.is_empty() {
        return Err("解析の結果にコマの時刻がありません。もう一度解析してください".into());
    }
    if pts.windows(2).any(|w| w[1] < w[0]) {
        return Err("解析の結果のコマの時刻が増えていきません。もう一度解析してください".into());
    }
    check_fixed(fixed)?;
    let mut a = [0.0; 5];
    for (i, name) in ANCHOR_NAMES.iter().enumerate() {
        // アンカーは「そのコマ」を指す。丸めた秒（解析は 3 桁・手入力も）だとコマの時刻の少し前になり、
        // 1 つ前のコマが出てしまうので、いちばん近いコマの時刻に合わせる
        a[i] = snap_to_frame(pts, anchor(guide, name)?);
    }
    for i in 1..5 {
        if a[i] <= a[i - 1] {
            return Err(format!(
                "アンカーの順番が合いません: 「{}」（{:.3} 秒）は「{}」（{:.3} 秒）より後でなければなりません",
                ANCHOR_NAMES[i], a[i], ANCHOR_NAMES[i - 1], a[i - 1]
            ));
        }
    }
    let [dark, m_cost0, m_end, a_cost0, a_end] = a;
    let n = |sec: f64| (sec * FPS).round() as usize;

    // 手動パートの素材の区間とカット
    let (m0, m1) = (m_cost0 - fixed.manual_lead, m_end + fixed.manual_tail);
    let mmap = manual_map(m0, m1, &guide.cuts)?;
    // 自動パートの素材の区間
    let (a0, a1) = (a_cost0 - fixed.auto_lead, a_end + fixed.auto_tail);
    if a1 <= a0 {
        return Err("自動パートの素材の区間が空です".into());
    }
    // 編成パートの素材は「暗転 + tail」が formation_end に来る位置から（素材の 0 秒より前は先頭のコマで止まる）
    let f0 = dark + fixed.formation_tail - fixed.formation_end;

    let counts = [n(fixed.formation_end), n(mmap.len), n(a1 - a0), n(fixed.result_len)];
    let mut parts = vec![];
    let mut frames: Vec<PlanFrame> = Vec::with_capacity(counts.iter().sum());
    let mut start = 0usize;
    for (pi, &cnt) in counts.iter().enumerate() {
        parts.push(PlanPart { name: PART_NAMES[pi].into(), t0: start as f64 / FPS, t1: (start + cnt) as f64 / FPS });
        for j in 0..cnt {
            let u = j as f64 / FPS;
            let part = pi as u8;
            let fr = match pi {
                0 => PlanFrame { part, src: frame_at_time(pts, f0 + u), mix: None },
                1 => {
                    let (ta, mix) = mmap.at(u);
                    PlanFrame {
                        part,
                        src: frame_at_time(pts, ta),
                        mix: mix.map(|(tb, w)| (frame_at_time(pts, tb), w as f32)),
                    }
                }
                2 => PlanFrame { part, src: frame_at_time(pts, a0 + u), mix: None },
                // リザルト: 自動パートの素材の続きをそのまま流す
                _ => PlanFrame { part, src: frame_at_time(pts, a1 + u), mix: None },
            };
            frames.push(fr);
        }
        start += cnt;
    }
    let part_t0 = |i: usize| parts[i].t0;
    let anchor_out = vec![
        (ANCHOR_NAMES[0].to_string(), fixed.formation_end - fixed.formation_tail),
        (ANCHOR_NAMES[1].to_string(), part_t0(1) + mmap.out_of(m_cost0)),
        (ANCHOR_NAMES[2].to_string(), part_t0(1) + mmap.out_of(m_end)),
        (ANCHOR_NAMES[3].to_string(), part_t0(2) + (a_cost0 - a0)),
        (ANCHOR_NAMES[4].to_string(), part_t0(2) + (a_end - a0)),
    ];
    let anchor_src = ANCHOR_NAMES.iter().zip(a).map(|(k, v)| (k.to_string(), v)).collect();
    let manual_t0 = part_t0(1);
    // カットの継ぎ目: B の断片の頭から、フェードの長さのあいだ
    let seams = (1..mmap.segs.len())
        .map(|k| {
            let t = manual_t0 + mmap.segs[k].out0;
            [t, t + mmap.fades[k - 1]]
        })
        .collect();
    // 素材の音: 映像と同じ「出力 → 素材」の対応で並べる（素材の音は連続なので、映像のようにコマには丸めない）
    let mut audio = vec![AudioSeg { out0: parts[0].t0, out1: parts[0].t1, src0: f0, fade_in: 0.0, fade_out: 0.0 }];
    for (k, sg) in mmap.segs.iter().enumerate() {
        let out0 = manual_t0 + sg.out0;
        audio.push(AudioSeg {
            out0,
            // 手動パートの終わり（コマに丸めた値）より先へは出さない
            out1: (out0 + (sg.b - sg.a)).min(parts[1].t1),
            src0: sg.a,
            fade_in: if k > 0 { mmap.fades[k - 1] } else { 0.0 },
            fade_out: mmap.fades.get(k).copied().unwrap_or(0.0),
        });
    }
    // 自動とリザルトは素材の続き（映像の a1 + u と同じ）
    audio.push(AudioSeg { out0: parts[2].t0, out1: parts[3].t1, src0: a0, fade_in: 0.0, fade_out: 0.0 });
    Ok(GuidePlan {
        fps: FPS,
        parts,
        frames,
        anchor_out,
        anchor_src,
        pts: Arc::new(pts.to_vec()),
        manual: mmap,
        manual_t0,
        seams,
        now_lead: fixed.now_lead,
        audio,
    })
}

/// 次の操作の id（"op" + 今ある番号の最大 + 1。消しても詰めない）
pub fn next_op_id(ops: &[GuideOp]) -> String {
    let max = ops.iter().filter_map(|o| o.id.strip_prefix("op").and_then(|n| n.parse::<u32>().ok())).max().unwrap_or(0);
    format!("op{}", max + 1)
}

/// 出力の時刻 t_out（手動パートの中）に操作を足す。kind は "deploy"。足した操作の id を返す。
/// 手動パートの外・範囲外は Err（和文）。ops は素材の時刻の順に保つ
pub fn add_op(g: &mut Guide, plan: &GuidePlan, t_out: f64) -> Result<String, String> {
    let Some((_, t_src, part)) = plan.src_at(t_out) else {
        return Err("今の位置は動画の外です。手動パートの中に再生位置を置いてください".into());
    };
    if part != "manual" {
        return Err(format!(
            "操作は手動パートの中に足してください（今の位置は「{}」パートです）",
            part_label(part)
        ));
    }
    let id = next_op_id(&g.ops);
    g.ops.push(GuideOp { id: id.clone(), t: t_src, kind: "deploy".into(), ..Default::default() });
    g.ops.sort_by(|a, b| a.t.total_cmp(&b.t));
    Ok(id)
}

/// 保存の前に整える: ops の t・アンカーの t・カットの両端をいちばん近いコマの時刻に合わせ、
/// ops・cuts を時刻の順に並べ、id の無い（重なる）操作に id を振る。数でない値は Err
pub fn normalize_guide(g: &mut Guide, pts: &[f64]) -> Result<(), String> {
    for o in &g.ops {
        if !o.t.is_finite() {
            return Err(format!("操作「{}」の時刻が数ではありません", o.id));
        }
        if o.now_lead.is_some_and(|v| !v.is_finite() || !(0.0..=NOW_LEAD_MAX).contains(&v)) {
            return Err(format!("操作「{}」の NOW の先出しは 0〜10 秒で入れてください", o.id));
        }
    }
    for c in &g.cuts {
        if !c.src_in.is_finite() || !c.src_out.is_finite() || !c.fade.is_finite() {
            return Err("カットの値が数ではありません".into());
        }
    }
    for (i, s) in g.subs.iter().enumerate() {
        if !s.src_in.is_finite() || !s.src_out.is_finite() || !s.offset.is_finite() {
            return Err(format!("補足素材 {} の値が数ではありません", i + 1));
        }
    }
    // id: 空・重なりに新しい番号を振る（消しても詰めない）
    let mut seen = std::collections::HashSet::new();
    let mut max = g.ops.iter().filter_map(|o| o.id.strip_prefix("op").and_then(|n| n.parse::<u32>().ok())).max().unwrap_or(0);
    for o in g.ops.iter_mut() {
        if o.id.is_empty() || !seen.insert(o.id.clone()) {
            max += 1;
            o.id = format!("op{max}");
            seen.insert(o.id.clone());
        }
    }
    if !pts.is_empty() {
        for a in g.anchors.values_mut() {
            if let Some(t) = a.t.filter(|t| t.is_finite()) {
                a.t = Some(snap_to_frame(pts, t));
            }
        }
        for c in g.cuts.iter_mut() {
            c.src_in = snap_to_frame(pts, c.src_in);
            c.src_out = snap_to_frame(pts, c.src_out);
        }
        for o in g.ops.iter_mut() {
            o.t = snap_to_frame(pts, o.t);
        }
    }
    g.ops.sort_by(|a, b| a.t.total_cmp(&b.t));
    // カットも素材の時刻の順（`GuideState::cuts_out` と同じ並び）
    g.cuts.sort_by(|a, b| a.src_in.total_cmp(&b.src_in));
    Ok(())
}

/// 操作ごとの出力の時刻・NOW の開始・そのコマのコストとゲーム内の時間（ops と同じ並び）。
/// cost・game は解析のコマごとの列（コマ番号で引く。短ければ None）
pub fn ops_out(guide: &Guide, plan: &GuidePlan, cost: &[Option<i32>], game: &[Option<f64>]) -> Vec<OpOut> {
    let mut outs: Vec<OpOut> = guide
        .ops
        .iter()
        .map(|o| {
            let t = if o.t.is_finite() { snap_to_frame(&plan.pts, o.t) } else { 0.0 };
            let frame = frame_at_time(&plan.pts, t);
            let in_cut = guide.cuts.iter().any(|c| c.src_in <= t + 1e-9 && t < c.src_out - 1e-9);
            OpOut {
                id: o.id.clone(),
                t_out: if in_cut { None } else { plan.manual_out_of(t) },
                t_now: None,
                frame,
                cost: cost.get(frame as usize).copied().flatten(),
                game: game.get(frame as usize).copied().flatten(),
                in_cut,
            }
        })
        .collect();
    let t_out: Vec<Option<f64>> = outs.iter().map(|u| u.t_out).collect();
    for (u, t) in outs.iter_mut().zip(now_starts(&guide.ops, &t_out, plan.manual_t0, plan.now_lead)) {
        u.t_now = t;
    }
    outs
}

/// 操作 o の NOW の先出し（秒）: 信頼枠は 0、それ以外は個別値、無ければ汎用値 generic。0 未満・数でない値は 0
pub fn op_now_lead(o: &GuideOp, generic: f64) -> f64 {
    if o.kind == "trust" {
        return 0.0;
    }
    let v = o.now_lead.unwrap_or(generic);
    if v.is_finite() { v.max(0.0) } else { 0.0 }
}

/// 操作ごとの NOW の開始（出力の秒。ops・t_out と同じ並び）。
/// `t_out − 先出し` を、手動パートの頭 manual_t0 と前の操作（t_out がこの操作より小さい操作のうち最も遅いもの。信頼枠も含む）の
/// 瞬間より前にしない。t_out が無い操作は None。同じ t_out の操作どうしは縛り合わない
pub fn now_starts(ops: &[GuideOp], t_out: &[Option<f64>], manual_t0: f64, generic: f64) -> Vec<Option<f64>> {
    ops.iter()
        .zip(t_out)
        .map(|(o, &t)| {
            let t = t?;
            let mut start = (t - op_now_lead(o, generic)).max(manual_t0);
            if let Some(prev) = t_out.iter().flatten().copied().filter(|&p| p < t).fold(None, |m: Option<f64>, p| Some(m.map_or(p, |m| m.max(p)))) {
                start = start.max(prev);
            }
            Some(start.min(t))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ak_proto::guide::Anchor;
    use ak_proto::GuideFixedOpt;

    /// 一定の間隔の pts（秒）
    fn pts_const(n: usize, fps: f64) -> Vec<f64> {
        (0..n).map(|i| i as f64 / fps).collect()
    }

    fn guide(anchors: [f64; 5]) -> Guide {
        let mut g = Guide::default();
        for (name, v) in ANCHOR_NAMES.iter().zip(anchors) {
            g.anchors.insert(name.to_string(), Anchor { t: Some(v), auto: None });
        }
        g
    }

    /// 17-5 の実測に近いアンカー
    fn g175() -> Guide {
        guide([3.475, 9.208, 181.043, 202.393, 300.945])
    }

    fn part<'a>(p: &'a GuidePlan, name: &str) -> &'a PlanPart {
        p.parts.iter().find(|x| x.name == name).unwrap()
    }

    #[test]
    fn part_lengths_follow_anchors_and_fixed() {
        let f = GuideFixed::default();
        let pts = pts_const(20000, 60.0);
        let p = plan(&g175(), &f, &pts).unwrap();
        let len = |n: &str| part(&p, n).t1 - part(&p, n).t0;
        // 編成は出力上で固定（8 秒）
        assert_eq!((part(&p, "formation").t0, len("formation")), (0.0, 8.0));
        // 手動: (181.043 + 1.5) − (9.208 − 2.0)
        assert!((len("manual") - (182.543 - 7.208)).abs() < 1.0 / 60.0, "{}", len("manual"));
        // 自動: (300.945 + 1.5) − (202.393 − 2.0)
        assert!((len("auto") - (302.445 - 200.393)).abs() < 1.0 / 60.0);
        assert_eq!(len("result"), 8.0);
        // パートは隙間なく並ぶ
        for w in p.parts.windows(2) {
            assert_eq!(w[0].t1, w[1].t0);
        }
        assert_eq!(p.frames.len(), (p.parts[3].t1 * 60.0).round() as usize);
        assert!((p.end() - p.parts[3].t1).abs() < 1e-9);
    }

    #[test]
    fn formation_ends_at_dark_plus_tail_and_holds_first_frame_before_zero() {
        let f = GuideFixed::default();
        let pts = pts_const(20000, 60.0);
        let p = plan(&g175(), &f, &pts).unwrap();
        // 素材の 0 秒より前（出力の 0〜4.025 秒）は先頭のコマで止まる
        for k in [0usize, 100, 240] {
            assert_eq!(p.frames[k].src, 0, "k={k}");
        }
        // 出力 8 秒の直前のコマ = 暗転 3.475 + 0.5 の素材の時刻
        let last = p.frames[(8.0f64 * 60.0) as usize - 1].src;
        let want = frame_at_time(&pts, 3.975 - 1.0 / 60.0);
        assert_eq!(last, want);
        // 固定値を変えても、編成の終わりは出力の 8 秒のまま
        let f2 = GuideFixed { formation_tail: 1.0, ..f };
        let p2 = plan(&g175(), &f2, &pts).unwrap();
        assert_eq!(part(&p2, "formation").t1, 8.0);
        assert_eq!(p2.frames[479].src, frame_at_time(&pts, 4.475 - 1.0 / 60.0));
    }

    #[test]
    fn manual_cost0_lands_at_formation_end_plus_lead() {
        let f = GuideFixed::default();
        let pts = pts_const(20000, 60.0);
        let p = plan(&g175(), &f, &pts).unwrap();
        // 出力の 8 + 2.0 秒 = 最初のコストのコマ（素材 9.208 秒）
        let k = ((8.0 + 2.0) * 60.0) as usize;
        assert_eq!(p.frames[k].src, frame_at_time(&pts, snap_to_frame(&pts, 9.208)));
        assert_eq!(p.frames[k].part, 1);
        // 自動も同じ（パートの頭から auto_lead 秒の位置）
        let k = ((part(&p, "auto").t0 + 2.0) * 60.0).round() as usize;
        assert_eq!(p.frames[k].src, frame_at_time(&pts, snap_to_frame(&pts, 202.393)));
        assert_eq!(p.frames[k].part, 2);
    }

    #[test]
    fn rounded_anchor_snaps_to_its_frame() {
        // 17-5 の実測: 解析の pts は 4 桁（9.1917, 9.2083）、アンカーは 3 桁（9.208）。9.208 は 9.2083 のコマを指す
        let mut pts = pts_const(20000, 60.0);
        pts[552] = 9.1917;
        pts[553] = 9.2083;
        assert_eq!(snap_to_frame(&pts, 9.208), 9.2083);
        assert_eq!(snap_to_frame(&pts, -1.0), 0.0);
        assert_eq!(snap_to_frame(&pts, 1e9), *pts.last().unwrap());
        let p = plan(&g175(), &GuideFixed::default(), &pts).unwrap();
        let k = ((8.0 + 2.0) * 60.0) as usize;
        assert_eq!(p.frames[k].src, 553, "出力 10 秒 = 最初のコストのコマ");
        assert!(p.frames[k - 1].src < 553, "その前のコマは最初のコストより前");
    }

    #[test]
    fn anchor_frame_is_not_lost_to_float_error() {
        // 解析の pts は 4 桁に丸めてある。17-5 の実測で (6.6667 − 2.1) + 126/60 < 6.6667 になる組み合わせ
        let pts: Vec<f64> = (0..20000).map(|i| ((i as f64) / 60.0 * 1e4).round() / 1e4).collect();
        let i0 = 400usize;
        assert_eq!(pts[i0], 6.6667);
        assert!((pts[i0] - 2.1) + 126.0 / 60.0 < pts[i0], "誤差の出る組み合わせであること");
        let mut g = g175();
        g.anchors.get_mut("manual_cost0").unwrap().t = Some(pts[i0]);
        let f = GuideFixed { manual_lead: 2.1, ..GuideFixed::default() };
        let p = plan(&g, &f, &pts).unwrap();
        let k = ((part(&p, "manual").t0 + 2.1) * 60.0).round() as usize;
        assert_eq!(p.frames[k].src as usize, i0, "手動の最初のコストのコマ");
    }

    #[test]
    fn variable_fps_picks_the_displayed_frame() {
        // 重複・飛びのある pts: 0.1 と 0.5 の間は 0.1 のコマが出ている
        let pts = [0.0, 0.1, 0.5, 0.6, 1.0];
        assert_eq!(frame_at_time(&pts, -3.0), 0);
        assert_eq!(frame_at_time(&pts, 0.0), 0);
        assert_eq!(frame_at_time(&pts, 0.099), 0);
        assert_eq!(frame_at_time(&pts, 0.1), 1);
        assert_eq!(frame_at_time(&pts, 0.49), 1);
        assert_eq!(frame_at_time(&pts, 0.5), 2);
        assert_eq!(frame_at_time(&pts, 0.99), 3);
        assert_eq!(frame_at_time(&pts, 99.0), 4);
        // 同じ時刻が 2 回ならあとのコマ
        assert_eq!(frame_at_time(&[0.0, 0.2, 0.2, 0.4], 0.2), 2);
    }

    #[test]
    fn variable_fps_in_plan() {
        // 58.5fps 平均の揺れた pts でも、出力のコマが見せる素材の時刻は pts[src] <= 時刻 < pts[src+1]
        let mut pts = vec![];
        let mut t = 0.0;
        for i in 0..20000 {
            pts.push(t);
            t += if i % 7 == 0 { 1.0 / 30.0 } else { 1.0 / 61.0 };
        }
        let f = GuideFixed::default();
        let p = plan(&g175(), &f, &pts).unwrap();
        let k = 10 * 60;
        let want_t = 9.208;
        let s = p.frames[k].src as usize;
        assert!(pts[s] <= want_t && want_t < pts[s + 1]);
    }

    #[test]
    fn cut_shortens_by_cut_length_plus_fade_and_crossfades() {
        let f = GuideFixed::default();
        let pts = pts_const(20000, 60.0);
        let base = plan(&g175(), &f, &pts).unwrap();
        let mut g = g175();
        g.cuts.push(Cut { src_in: 40.0, src_out: 52.0, fade: 0.5 });
        let p = plan(&g, &f, &pts).unwrap();
        let len = |p: &GuidePlan| part(p, "manual").t1 - part(p, "manual").t0;
        // 詰まるのはカットの長さ（12 秒）+ フェードの重なり（0.5 秒）
        assert!(((len(&base) - len(&p)) - 12.5).abs() < 2.0 / 60.0, "{} {}", len(&base), len(&p));
        // 継ぎ目より前は元と同じ、フェードの間は mix つき、後は B のコマだけ
        let t0 = part(&p, "manual").t0;
        let u_cut = 40.0 - (9.208 - 2.0); // パートの頭から継ぎ目の始まりまで
        let at = |u: f64| &p.frames[((t0 + u) * 60.0).round() as usize];
        assert!(at(u_cut - 1.0).mix.is_none());
        assert_eq!(at(u_cut - 1.0).src, frame_at_time(&pts, 40.0 - 1.0));
        // 継ぎ目の頭 u_cut − 0.5 から 0.5 秒かけて B の重みが 0 → 1
        let first = at(u_cut - 0.5);
        let (b, w) = first.mix.expect("フェードの頭は mix");
        assert!(w < 0.05);
        assert_eq!(b, frame_at_time(&pts, 52.0));
        assert_eq!(first.src, frame_at_time(&pts, 39.5));
        let mid = at(u_cut - 0.25).mix.unwrap();
        assert!((mid.1 - 0.5).abs() < 0.05, "{mid:?}");
        // フェードのあと（A が「元の 40.0 秒」を過ぎたところ）は B の素材だけ
        let after = at(u_cut + 1.0);
        assert!(after.mix.is_none());
        assert_eq!(after.src, frame_at_time(&pts, 52.0 + 1.5));
    }

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1.0 / 60.0 + 1e-9
    }

    /// 素材の音の区間: 編成は素材の 0 秒より前から（頭は無音になる）、手動はパートの頭から、自動＋リザルトは 1 区間
    #[test]
    fn audio_segments_follow_parts_and_start_at_part_t0() {
        let f = GuideFixed::default();
        let pts = pts_const(20000, 60.0);
        let p = plan(&g175(), &f, &pts).unwrap();
        let a = p.audio_segments();
        assert_eq!(a.len(), 3, "編成・手動（カット無し）・自動+リザルト");
        // 編成: 出力 0〜8 秒、素材は 暗転(3.475 → コマに合わせて 3.4667) + 0.5 − 8 秒から。0 より前は無音になる
        assert_eq!((a[0].out0, a[0].out1), (0.0, 8.0));
        assert!(a[0].src0 < 0.0 && close(a[0].src0, 3.475 + 0.5 - 8.0), "{}", a[0].src0);
        assert_eq!((a[0].fade_in, a[0].fade_out), (0.0, 0.0));
        // 手動: 頭は parts[1].t0（8 秒）、素材は最初のコスト − 2 秒から。終わりはパートの終わり
        assert_eq!(a[1].out0, part(&p, "manual").t0);
        assert!(close(a[1].src0, 9.208 - 2.0), "{}", a[1].src0);
        assert!(close(a[1].out1, part(&p, "manual").t1), "{} {}", a[1].out1, part(&p, "manual").t1);
        // 自動とリザルトは続いた 1 区間。頭は自動パートの t0、終わりはリザルトの終わり
        assert_eq!(a[2].out0, part(&p, "auto").t0);
        assert_eq!(a[2].out1, part(&p, "result").t1);
        assert!(close(a[2].src0, 202.393 - 2.0), "{}", a[2].src0);
        // 映像と同じ対応: 区間の中の出力の時刻 t の素材の時刻（src0 + t − out0）は、コマが見せる素材の時刻と 1 コマ以内で合う
        for seg in &a {
            let t = (seg.out0 + seg.out1) / 2.0;
            let want = pts[p.frame_at(t).unwrap().src as usize];
            let got = seg.src0 + (t - seg.out0);
            if got >= 0.0 {
                assert!((got - want).abs() < 2.0 / 60.0, "t={t} got={got} want={want}");
            }
        }
    }

    /// カットのクロスフェード: 後ろの区間の頭と前の区間の終わりが、フェードの長さだけ重なる。映像（mix）の重なりと同じ
    #[test]
    fn audio_segments_overlap_by_cut_fade() {
        let f = GuideFixed::default();
        let pts = pts_const(20000, 60.0);
        let mut g = g175();
        g.cuts.push(Cut { src_in: 40.0, src_out: 52.0, fade: 0.5 });
        let p = plan(&g, &f, &pts).unwrap();
        let a = p.audio_segments();
        assert_eq!(a.len(), 4);
        let (x, y) = (&a[1], &a[2]);
        // 前の区間は素材の 40.0 秒で終わり、後ろは 52.0 秒から
        assert!(close(x.src0 + (x.out1 - x.out0), 40.0), "{x:?}");
        assert_eq!(y.src0, 52.0);
        assert_eq!((x.fade_in, x.fade_out), (0.0, 0.5));
        assert_eq!((y.fade_in, y.fade_out), (0.5, 0.0));
        // 重なりはフェードの長さ（0.5 秒）。継ぎ目（seams）の始まり = 後ろの区間の頭
        assert!(close(x.out1 - y.out0, 0.5), "{x:?} {y:?}");
        assert!(close(y.out0, p.seams[0][0]) && close(x.out1, p.seams[0][1]), "{:?}", p.seams);
        // 後ろの区間は手動パートの終わりで終わる
        assert!(close(y.out1, part(&p, "manual").t1));
    }

    /// フェードの無いカットは、前の終わりと後ろの頭がちょうど続く（重ならない・隙間も無い）
    #[test]
    fn audio_segments_hard_cut_is_contiguous() {
        let f = GuideFixed::default();
        let pts = pts_const(20000, 60.0);
        let mut g = g175();
        g.cuts.push(Cut { src_in: 40.0, src_out: 52.0, fade: 0.0 });
        let a = plan(&g, &f, &pts).unwrap().audio_segments();
        assert_eq!(a.len(), 4);
        assert!((a[1].out1 - a[2].out0).abs() < 1e-9);
        assert_eq!((a[1].fade_out, a[2].fade_in), (0.0, 0.0));
    }

    #[test]
    fn zero_fade_cut_is_a_hard_cut() {
        let f = GuideFixed::default();
        let pts = pts_const(20000, 60.0);
        let mut g = g175();
        g.cuts.push(Cut { src_in: 40.0, src_out: 52.0, fade: 0.0 });
        let p = plan(&g, &f, &pts).unwrap();
        assert!(p.frames.iter().all(|f| f.mix.is_none()));
        let base = plan(&g175(), &f, &pts).unwrap();
        let len = |p: &GuidePlan| part(p, "manual").t1 - part(p, "manual").t0;
        assert!(((len(&base) - len(&p)) - 12.0).abs() < 2.0 / 60.0);
    }

    #[test]
    fn bad_cuts_are_errors() {
        let f = GuideFixed::default();
        let pts = pts_const(20000, 60.0);
        let with = |cuts: Vec<Cut>| {
            let mut g = g175();
            g.cuts = cuts;
            plan(&g, &f, &pts)
        };
        let c = |a, b, fade| Cut { src_in: a, src_out: b, fade };
        // 区間の外（手動の素材は 7.208〜182.543 秒）
        assert!(with(vec![c(2.0, 5.0, 0.2)]).unwrap_err().contains("外"));
        assert!(with(vec![c(100.0, 190.0, 0.2)]).unwrap_err().contains("外"));
        // 重なる
        assert!(with(vec![c(40.0, 52.0, 0.2), c(50.0, 60.0, 0.2)]).unwrap_err().contains("重なって"));
        // 向きが逆・フェードが負
        assert!(with(vec![c(52.0, 40.0, 0.2)]).unwrap_err().contains("より前"));
        assert!(with(vec![c(40.0, 52.0, -1.0)]).unwrap_err().contains("0 以上"));
        // 断片が短くてフェードが入らない
        assert!(with(vec![c(40.0, 52.0, 1.0), c(52.5, 60.0, 1.0)]).unwrap_err().contains("短すぎ"));
        // 重ならなければ順番が逆に書かれていてもよい
        assert!(with(vec![c(60.0, 62.0, 0.2), c(40.0, 52.0, 0.2)]).is_ok());
    }

    #[test]
    fn missing_or_misordered_anchors_are_errors() {
        let f = GuideFixed::default();
        let pts = pts_const(20000, 60.0);
        let mut g = g175();
        g.anchors.remove("auto_cost0");
        let e = plan(&g, &f, &pts).unwrap_err();
        assert!(e.contains("auto_cost0") && e.contains("ありません"), "{e}");
        // 値が無いアンカー（t も auto も無い）も不足
        g.anchors.insert("auto_cost0".into(), Anchor::default());
        assert!(plan(&g, &f, &pts).unwrap_err().contains("ありません"));
        // 順番が逆
        let e = plan(&guide([3.0, 100.0, 50.0, 202.0, 300.0]), &f, &pts).unwrap_err();
        assert!(e.contains("順番") && e.contains("manual_end") && e.contains("manual_cost0"), "{e}");
        // auto だけでも使える（t が無ければ auto）
        let mut g = g175();
        for a in g.anchors.values_mut() {
            a.auto = a.t.take();
        }
        assert!(plan(&g, &f, &pts).is_ok());
        // t は auto より優先
        g.anchors.get_mut("manual_end").unwrap().t = Some(170.0);
        let p = plan(&g, &f, &pts).unwrap();
        let l = part(&p, "manual").t1 - part(&p, "manual").t0;
        assert!((l - (171.5 - 7.208)).abs() < 1.0 / 30.0);
    }

    #[test]
    fn bad_pts_and_fixed_are_errors() {
        let f = GuideFixed::default();
        assert!(plan(&g175(), &f, &[]).unwrap_err().contains("コマの時刻"));
        assert!(plan(&g175(), &f, &[0.0, 1.0, 0.5]).unwrap_err().contains("増えていきません"));
        let bad = GuideFixed { manual_lead: -1.0, ..f.clone() };
        assert!(plan(&g175(), &bad, &pts_const(100, 60.0)).unwrap_err().contains("manual_lead"));
        let bad = GuideFixed { result_len: f64::NAN, ..f };
        assert!(plan(&g175(), &bad, &pts_const(100, 60.0)).unwrap_err().contains("result_len"));
    }

    #[test]
    fn effective_fixed_layers_episode_over_settings_over_default() {
        let mut g = g175();
        let settings = GuideFixed { manual_lead: 3.0, result_len: 5.0, ..Default::default() };
        // 回に何も書かなければ設定の値
        let e = effective_fixed(&g, &settings);
        assert_eq!((e.manual_lead, e.result_len, e.auto_lead), (3.0, 5.0, 2.0));
        // 回の値が優先
        g.fixed = GuideFixedOpt { manual_lead: Some(4.5), ..Default::default() };
        let e = effective_fixed(&g, &settings);
        assert_eq!((e.manual_lead, e.result_len), (4.5, 5.0));
        // 設定も既定のままなら既定値
        assert_eq!(effective_fixed(&Guide::default(), &GuideFixed::default()), GuideFixed::default());
    }

    #[test]
    fn manual_lead_change_changes_part_length() {
        let pts = pts_const(20000, 60.0);
        let a = plan(&g175(), &GuideFixed::default(), &pts).unwrap();
        let b = plan(&g175(), &GuideFixed { manual_lead: 3.0, ..Default::default() }, &pts).unwrap();
        let len = |p: &GuidePlan| part(p, "manual").t1 - part(p, "manual").t0;
        assert!(((len(&b) - len(&a)) - 1.0).abs() < 1.0 / 30.0);
    }

    #[test]
    fn anchors_know_their_output_time() {
        let f = GuideFixed::default();
        let pts = pts_const(20000, 60.0);
        let out = |p: &GuidePlan, n: &str| p.anchor_out.iter().find(|(k, _)| k == n).unwrap().1;
        let p = plan(&g175(), &f, &pts).unwrap();
        let sn = |t: f64| snap_to_frame(&pts, t);
        // 暗転は編成の終わりの tail 手前、最初のコストは手動の頭から lead 秒（アンカーはコマの時刻に合わせてから）
        assert_eq!(out(&p, "formation_dark"), 7.5);
        assert!((out(&p, "manual_cost0") - 10.0).abs() < 1e-9);
        assert!((out(&p, "manual_end") - (8.0 + sn(181.043) - (sn(9.208) - 2.0))).abs() < 1e-9);
        let a0 = part(&p, "auto").t0;
        assert!((out(&p, "auto_cost0") - (a0 + 2.0)).abs() < 1e-9);
        assert!((out(&p, "auto_end") - (a0 + sn(300.945) - (sn(202.393) - 2.0))).abs() < 1e-9);
        // カットがあれば、後ろのアンカーはカットのぶん手前に来る（長さ + フェード）
        let mut g = g175();
        g.cuts.push(Cut { src_in: 40.0, src_out: 52.0, fade: 0.5 });
        let q = plan(&g, &f, &pts).unwrap();
        assert!((out(&q, "manual_end") - (out(&p, "manual_end") - 12.5)).abs() < 1e-9);
        assert!((out(&q, "manual_cost0") - 10.0).abs() < 1e-9);
        // その時刻の出力のコマは、そのアンカーの素材のコマ
        let k = (out(&q, "manual_end") * 60.0).round() as usize;
        assert_eq!(q.frames[k].src, frame_at_time(&pts, sn(181.043)));
    }

    #[test]
    fn map_json_shape() {
        let f = GuideFixed::default();
        let pts = pts_const(20000, 60.0);
        let mut g = g175();
        g.cuts.push(Cut { src_in: 40.0, src_out: 52.0, fade: 0.5 });
        let p = plan(&g, &f, &pts).unwrap();
        let m = p.to_map_json();
        assert_eq!(m["version"], MAP_VERSION);
        assert_eq!(MAP_VERSION, 2);
        assert_eq!(m["fps"], 60.0);
        assert_eq!(m["parts"].as_array().unwrap().len(), 4);
        assert_eq!(m["parts"][0]["name"], "formation");
        assert_eq!(m["src"].as_array().unwrap().len(), p.frames.len());
        let mix = m["mix"].as_object().unwrap();
        assert!(!mix.is_empty());
        let (k, v) = mix.iter().next().unwrap();
        let k: usize = k.parse().unwrap();
        assert_eq!(v[0].as_u64().unwrap() as u32, p.frames[k].mix.unwrap().0);
    }

    #[test]
    fn frame_at_is_none_out_of_range() {
        let pts = pts_const(20000, 60.0);
        let p = plan(&g175(), &GuideFixed::default(), &pts).unwrap();
        assert!(p.frame_at(-0.1).is_none());
        assert!(p.frame_at(p.end() + 1.0).is_none());
        assert!(p.frame_at(f64::NAN).is_none());
        assert_eq!(p.frame_at(0.0).unwrap().part, 0);
        assert_eq!(p.part_name(p.frame_at(100.0).unwrap()), "manual");
        assert_eq!(part_label("manual"), "手動");
    }

    // ---- 段3: 時刻の行き来・操作・保存の前の整え

    fn plan_with_cut() -> (GuidePlan, Vec<f64>) {
        let pts = pts_const(20000, 60.0);
        let mut g = g175();
        g.cuts.push(Cut { src_in: 40.0, src_out: 52.0, fade: 0.5 });
        (plan(&g, &GuideFixed::default(), &pts).unwrap(), pts)
    }

    #[test]
    fn src_at_returns_the_visible_source_frame_and_part() {
        let pts = pts_const(20000, 60.0);
        let p = plan(&g175(), &GuideFixed::default(), &pts).unwrap();
        // 出力 10.0 秒 = 手動の最初のコスト（素材 9.208 に近いコマ）
        let (frame, t, part) = p.src_at(10.0).unwrap();
        assert_eq!(part, "manual");
        assert_eq!(frame, p.frames[600].src);
        assert_eq!(t, pts[frame as usize]);
        assert!((t - 9.208).abs() < 1.0 / 60.0);
        assert_eq!(p.src_at(1.0).unwrap().2, "formation");
        assert!(p.src_at(-1.0).is_none() && p.src_at(1e6).is_none());
        // クロスフェード中は A のコマ
        let (q, _) = plan_with_cut();
        let [seam0, _] = q.seams[0];
        let k = (seam0 * 60.0).round() as usize;
        assert_eq!(q.src_at(seam0).unwrap().0, q.frames[k].src);
        assert!(q.frames[k].mix.is_some() || q.frames[k + 1].mix.is_some());
    }

    #[test]
    fn manual_out_of_is_none_in_cut_or_outside_the_manual_range() {
        let (p, pts) = plan_with_cut();
        let t0 = part(&p, "manual").t0;
        let m0 = snap_to_frame(&pts, 9.208) - 2.0;
        // カットの前: パートの頭 + (素材 − 区間の頭)
        assert!((p.manual_out_of(30.0).unwrap() - (t0 + 30.0 - m0)).abs() < 1e-9);
        // カットの中: 抜かれている
        for t in [40.0, 45.0, 51.99] {
            assert!(p.manual_out_of(t).is_none(), "{t}");
        }
        // カットの後: 断片の頭は継ぎ目の始まり（フェードの重なりのぶん手前）
        let after = p.manual_out_of(52.0).unwrap();
        assert!((after - (t0 + (40.0 - m0) - 0.5)).abs() < 1e-9, "{after}");
        assert!((p.manual_out_of(60.0).unwrap() - (after + 8.0)).abs() < 1e-9);
        // 手動の区間の外（前と後）
        assert!(p.manual_out_of(5.0).is_none());
        assert!(p.manual_out_of(190.0).is_none());
        // 区間の両端は中
        assert!((p.manual_out_of(m0).unwrap() - t0).abs() < 1e-9);
        assert!(p.manual_out_of(snap_to_frame(&pts, 181.043) + 1.5).is_some());
        // その時刻の出力のコマは、その素材のコマ
        let t = p.manual_out_of(100.0).unwrap();
        assert_eq!(p.frames[(t * 60.0).round() as usize].src, frame_at_time(&pts, 100.0));
        // 継ぎ目の印: 始まり → 終わりがフェードの長さ
        assert_eq!(p.seams.len(), 1);
        assert!((p.seams[0][0] - after).abs() < 1e-9);
        assert!((p.seams[0][1] - p.seams[0][0] - 0.5).abs() < 1e-9);
    }

    #[test]
    fn step_frame_moves_by_source_frames_and_stops_at_ends() {
        // 17-5 の実測（解析では pts[422]）: 9.2083 が最初のコスト、次は 9.225（ここでは添字 553 に置く）
        let mut pts = pts_const(20000, 60.0);
        pts[552] = 9.1917;
        pts[553] = 9.2083;
        pts[554] = 9.225;
        assert_eq!(step_frame(&pts, 9.208, 1), 9.225);
        assert_eq!(step_frame(&pts, 9.2083, -1), 9.1917);
        assert_eq!(step_frame(&pts, 9.2083, 0), 9.2083);
        assert_eq!(step_frame(&pts, 9.2083, 10), pts[563]);
        assert_eq!(step_frame(&pts, 9.2083, -10), pts[543]);
        // 両端で止まる
        assert_eq!(step_frame(&pts, 0.0, -1), 0.0);
        assert_eq!(step_frame(&pts, 0.0, -50), 0.0);
        assert_eq!(step_frame(&pts, 1e9, 1), *pts.last().unwrap());
        assert_eq!(step_frame(&[], 3.0, 1), 3.0);
        // 可変 fps: コマの間隔が揃っていなくても、1 コマ = 1 つ隣
        let v = [0.0, 0.1, 0.5, 0.6, 1.0];
        assert_eq!(step_frame(&v, 0.1, 1), 0.5);
        assert_eq!(step_frame(&v, 0.5, -1), 0.1);
        assert_eq!(step_frame(&v, 0.45, 1), 0.6, "いちばん近いコマ（0.5）から 1 つ進む");
        // 同じ時刻が 2 コマ続くところは 1 つに数える
        let d = [0.0, 0.2, 0.2, 0.4];
        assert_eq!(step_frame(&d, 0.0, 1), 0.2);
        assert_eq!(step_frame(&d, 0.2, 1), 0.4);
        assert_eq!(step_frame(&d, 0.4, -1), 0.2);
        assert_eq!(step_frame(&d, 0.2, -1), 0.0);
    }

    #[test]
    fn add_op_assigns_ids_without_renumbering_and_refuses_outside_manual() {
        let pts = pts_const(20000, 60.0);
        let p = plan(&g175(), &GuideFixed::default(), &pts).unwrap();
        let mut g = g175();
        assert_eq!(add_op(&mut g, &p, 20.0).unwrap(), "op1");
        assert_eq!(add_op(&mut g, &p, 12.0).unwrap(), "op2");
        assert_eq!(add_op(&mut g, &p, 30.0).unwrap(), "op3");
        // 時刻の順に並ぶ（op2 → op1 → op3）。kind は配置
        assert_eq!(g.ops.iter().map(|o| o.id.as_str()).collect::<Vec<_>>(), ["op2", "op1", "op3"]);
        assert!(g.ops.iter().all(|o| o.kind == "deploy"));
        // 素材の時刻は出力のコマの素材の時刻
        assert_eq!(g.ops[1].t, pts[p.frame_at(20.0).unwrap().src as usize]);
        // 消しても詰めない
        g.ops.retain(|o| o.id != "op1");
        assert_eq!(add_op(&mut g, &p, 40.0).unwrap(), "op4");
        // 手動パートの外（編成・自動・リザルト・範囲外）は断る
        let e = add_op(&mut g, &p, 1.0).unwrap_err();
        assert!(e.contains("手動パート") && e.contains("編成"), "{e}");
        let a0 = part(&p, "auto").t0 + 5.0;
        assert!(add_op(&mut g, &p, a0).unwrap_err().contains("自動"));
        assert!(add_op(&mut g, &p, 1e6).unwrap_err().contains("動画の外"));
        assert_eq!(g.ops.len(), 3);
        assert_eq!(next_op_id(&[]), "op1");
        // 手で書かれた id（数字でない）は番号の計算に入れない
        let odd = [GuideOp { id: "x".into(), ..Default::default() }];
        assert_eq!(next_op_id(&odd), "op1");
    }

    #[test]
    fn normalize_snaps_to_frames_sorts_ops_and_fixes_ids() {
        let mut pts = pts_const(20000, 60.0);
        pts[552] = 9.1917;
        pts[553] = 9.2083;
        let mut g = g175();
        g.anchors.get_mut("manual_cost0").unwrap().t = Some(9.208);
        g.anchors.insert("auto_end".into(), Anchor { t: None, auto: Some(300.0) });
        g.cuts.push(Cut { src_in: 60.0, src_out: 62.0, fade: 0.1 });
        g.cuts.push(Cut { src_in: 40.004, src_out: 52.001, fade: 0.2 });
        let op = |id: &str, t: f64| GuideOp { id: id.into(), t, ..Default::default() };
        g.ops = vec![op("op5", 50.0), op("", 9.208), op("op5", 20.0), op("op2", 30.0)];
        normalize_guide(&mut g, &pts).unwrap();
        // 丸めた秒はそのコマの時刻に
        assert_eq!(g.anchors["manual_cost0"].t, Some(9.2083));
        // t の無い（候補だけの）アンカーは触らない
        assert_eq!(g.anchors["auto_end"], Anchor { t: None, auto: Some(300.0) });
        // カットは素材の時刻の順（書いた順ではなく）
        assert_eq!((g.cuts[0].src_in, g.cuts[0].src_out), (snap_to_frame(&pts, 40.004), snap_to_frame(&pts, 52.001)));
        assert_eq!(g.cuts[1].fade, 0.1);
        // t の順。空の id・重なる id には新しい番号（最大 5 の次から）
        assert_eq!(g.ops.iter().map(|o| o.t).collect::<Vec<_>>(), [9.2083, 20.0, 30.0, 50.0]);
        let ids: Vec<&str> = g.ops.iter().map(|o| o.id.as_str()).collect();
        assert_eq!(ids, ["op6", "op7", "op2", "op5"]);
        // 解析がまだ（pts が空）でも、並べて id を振る
        let mut g = g175();
        g.ops = vec![op("", 5.0), op("", 1.0)];
        normalize_guide(&mut g, &[]).unwrap();
        assert_eq!(g.ops.iter().map(|o| (o.id.as_str(), o.t)).collect::<Vec<_>>(), [("op2", 1.0), ("op1", 5.0)]);
        // 数でない時刻は Err
        g.ops[0].t = f64::NAN;
        assert!(normalize_guide(&mut g, &pts).unwrap_err().contains("数ではありません"));
    }

    fn now_op(id: &str, kind: &str, lead: Option<f64>) -> GuideOp {
        GuideOp { id: id.into(), kind: kind.into(), now_lead: lead, ..Default::default() }
    }

    #[test]
    fn now_starts_generic_individual_trust_and_previous_op() {
        let ops = vec![now_op("a", "deploy", None), now_op("b", "trust", None), now_op("c", "skill", Some(3.0)),
                       now_op("d", "deploy", Some(0.0)), now_op("e", "deploy", None)];
        // 汎用 1.0 秒。a は 10 秒（頭 8 秒より後）→ 9.0。b は信頼枠なので 0 → 20.0。
        // c は 24 秒の 3 秒前の 21.0 だが、前の操作は t_out が最も遅い e（22.5）なので 22.5。d は 0 秒 → 22.0。
        // e は 22.5 の 1 秒前の 21.5 だが、前の操作 d（22.0）で遅れて 22.0
        let t = [Some(10.0), Some(20.0), Some(24.0), Some(22.0), Some(22.5)];
        let s = now_starts(&ops, &t, 8.0, 1.0);
        assert_eq!(s, vec![Some(9.0), Some(20.0), Some(22.5), Some(22.0), Some(22.0)]);
        // 時刻の順に見ると減らない・t_out 以下
        let mut v: Vec<(f64, f64)> = t.iter().flatten().copied().zip(s.iter().flatten().copied()).collect();
        v.sort_by(|a, b| a.0.total_cmp(&b.0));
        assert!(v.windows(2).all(|w| w[0].1 <= w[1].1) && v.iter().all(|(o, n)| n <= o));
        // 汎用値を変えると、個別値のない（信頼枠でない）操作だけ動く
        let s2 = now_starts(&ops, &t, 8.0, 2.0);
        assert_eq!(s2[0], Some(8.0));
        assert_eq!(s2[2], Some(22.5));
    }

    #[test]
    fn now_starts_stops_at_part_head_equal_times_and_missing() {
        // パートの頭より前にしない（頭 8.0、操作 8.5、先出し 5 秒でも 8.0）
        let ops = vec![now_op("a", "deploy", Some(5.0)), now_op("b", "deploy", None), now_op("c", "deploy", None), now_op("d", "deploy", None)];
        let s = now_starts(&ops, &[Some(8.5), Some(30.0), Some(30.0), None], 8.0, 1.0);
        assert_eq!(s[0], Some(8.0));
        // 同じ t_out の 2 つは互いに縛らない（どちらも t_out − 1。前の操作 a の 8.5 より後）
        assert_eq!((s[1], s[2]), (Some(29.0), Some(29.0)));
        // t_out が無ければ None
        assert_eq!(s[3], None);
        // 数でない・負の個別値は 0 として扱う
        let o = vec![now_op("x", "deploy", Some(f64::NAN)), now_op("y", "deploy", Some(-1.0))];
        assert_eq!(now_starts(&o, &[Some(20.0), Some(30.0)], 8.0, 1.0), vec![Some(20.0), Some(30.0)]);
    }

    #[test]
    fn ops_out_has_t_now_from_plan_generic_lead() {
        let (p, pts) = plan_with_cut();
        let mut g = g175();
        let op = |id: &str, t: f64, kind: &str, lead: Option<f64>| GuideOp { id: id.into(), t, kind: kind.into(), now_lead: lead, ..Default::default() };
        g.ops = vec![op("a", 30.0, "deploy", None), op("b", 31.0, "trust", None), op("c", 60.0, "skill", Some(4.0)), op("d", 5.0, "deploy", None)];
        normalize_guide(&mut g, &pts).unwrap();
        let o = ops_out(&g, &p, &[], &[]);
        let by = |id: &str| o.iter().find(|u| u.id == id).unwrap();
        let (a, b, c, d) = (by("a"), by("b"), by("c"), by("d"));
        // 汎用値は plan が持つ固定値（既定 1.0 秒）
        assert!((a.t_now.unwrap() - (a.t_out.unwrap() - 1.0)).abs() < 1e-9);
        assert_eq!(b.t_now, b.t_out);
        assert!((c.t_now.unwrap() - (c.t_out.unwrap() - 4.0)).abs() < 1e-9);
        // 手動の区間の外は t_out も t_now も無い
        assert!(d.t_out.is_none() && d.t_now.is_none());
        // 固定値の now_lead を変えて組み立て直すと a の t_now が動く
        let p2 = plan(&g175(), &GuideFixed { now_lead: 0.5, ..Default::default() }, &pts).unwrap();
        let a2 = ops_out(&g, &p2, &[], &[]).into_iter().find(|u| u.id == "a").unwrap();
        assert!((a2.t_now.unwrap() - (a2.t_out.unwrap() - 0.5)).abs() < 1e-9);
    }

    #[test]
    fn normalize_refuses_bad_now_lead_and_fixed_refuses_negative() {
        let pts = pts_const(100, 60.0);
        for bad in [-0.1, 10.5, f64::NAN, f64::INFINITY] {
            let mut g = g175();
            g.ops = vec![now_op("a", "deploy", Some(bad))];
            assert_eq!(normalize_guide(&mut g, &pts).unwrap_err(), "操作「a」の NOW の先出しは 0〜10 秒で入れてください", "{bad}");
        }
        for ok in [0.0, 1.5, 10.0] {
            let mut g = g175();
            g.ops = vec![now_op("a", "deploy", Some(ok))];
            normalize_guide(&mut g, &pts).unwrap();
        }
        let bad = GuideFixed { now_lead: -1.0, ..Default::default() };
        assert!(plan(&g175(), &bad, &pts_const(100, 60.0)).unwrap_err().contains("now_lead"));
    }

    #[test]
    fn ops_out_reads_cost_and_game_and_flags_cut_and_outside() {
        let (p, pts) = plan_with_cut();
        let n = pts.len();
        let mut cost: Vec<Option<i32>> = vec![None; n];
        let mut game: Vec<Option<f64>> = vec![None; n];
        let at = |t: f64| frame_at_time(&pts, snap_to_frame(&pts, t)) as usize;
        cost[at(30.0)] = Some(27);
        game[at(30.0)] = Some(125.0);
        let mut g = g175();
        g.cuts = vec![Cut { src_in: 40.0, src_out: 52.0, fade: 0.5 }];
        let op = |id: &str, t: f64| GuideOp { id: id.into(), t, ..Default::default() };
        g.ops = vec![op("a", 30.0), op("b", 45.0), op("c", 5.0), op("d", 100.0)];
        let o = ops_out(&g, &p, &cost, &game);
        assert_eq!(o.len(), 4);
        // 通常: 出力の時刻・コスト・ゲーム内の時間
        assert_eq!(o[0].id, "a");
        assert_eq!((o[0].cost, o[0].game, o[0].in_cut), (Some(27), Some(125.0), false));
        assert_eq!(o[0].frame as usize, at(30.0));
        assert!((o[0].t_out.unwrap() - p.manual_out_of(30.0).unwrap()).abs() < 1e-9);
        // カットの中: 出力に無い
        assert!(o[1].in_cut && o[1].t_out.is_none());
        // 手動の区間の外: カットではないが出力に無い
        assert!(!o[2].in_cut && o[2].t_out.is_none());
        // 解析に値が無いコマ
        assert_eq!((o[3].cost, o[3].game), (None, None));
        assert!(o[3].t_out.is_some());
        // 解析の列が短くても落ちない
        let short = ops_out(&g, &p, &[], &[]);
        assert_eq!(short[0].cost, None);
    }

    /// 実際の解析（17-5）で値を確かめる（手で走らせる）:
    /// AK_TEST_GUIDE_ANALYSIS=<guide_analysis.json> cargo test -p ak-studio real_step_and_ops -- --ignored
    #[test]
    #[ignore = "実際の解析の結果が要る（AK_TEST_GUIDE_ANALYSIS）"]
    fn real_step_and_ops() {
        let Ok(path) = std::env::var("AK_TEST_GUIDE_ANALYSIS") else { panic!("AK_TEST_GUIDE_ANALYSIS を指定してください") };
        let a = crate::guide::parse_analysis(&std::fs::read_to_string(path).unwrap()).unwrap();
        let mut g = Guide { src: "x.mp4".into(), ..Default::default() };
        for (k, v) in &a.anchors {
            g.anchors.insert(k.clone(), Anchor { t: None, auto: Some(*v) });
        }
        let p = plan(&g, &GuideFixed::default(), &a.pts).unwrap();
        // 手動の最初のコスト: 出力 10.0 秒 = 解析のコマ 422（9.2083）。次のコマは 9.225
        assert_eq!(p.frames[600].src, 422);
        assert_eq!(a.pts[422], 9.2083);
        assert_eq!(step_frame(&a.pts, 9.208, 1), 9.225);
        assert_eq!(step_frame(&a.pts, 9.2083, -1), a.pts[421]);
        assert_eq!(p.src_at(10.0).unwrap().0, 422);
        // 次のコマの操作: 出力の 10.0 + (9.225 − 9.2083) 付近、コストは解析のその値
        let mut g2 = g.clone();
        g2.ops.push(GuideOp { id: "op1".into(), t: 9.225, ..Default::default() });
        normalize_guide(&mut g2, &a.pts).unwrap();
        let o = ops_out(&g2, &p, &a.cost, &a.game);
        assert_eq!(o[0].frame, 423);
        assert_eq!(o[0].cost, a.cost[423]);
        assert_eq!(o[0].game, a.game[423]);
        let t = o[0].t_out.unwrap();
        assert!((t - 10.0 - (9.225 - 9.2083)).abs() < 1e-6, "{t}");
        eprintln!("op@9.225 → 出力 {t:.4} 秒 / コスト {:?} / ゲーム {:?}", o[0].cost, o[0].game);
    }
}
