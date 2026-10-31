//! 対応表 `studio/guide_map.json` に足す項目と、補足素材の小窓の区間（どちらも純粋な関数）。
//!
//! 時刻と座標は Rust がここで決めて、演出の Python（`fxkit/guide_template.py`）は読むだけにする。
//! 座標の式は `layer::geometry` に 1 か所だけ（ここでは書き写さず、呼んで使う）。

use super::layer::{geometry, holds, mini_boxes, valid_zooms, zoom_box, DIM_RELEASE};
use super::plan::{ops_out, GuidePlan, PART_NAMES};
use super::GuideAnalysis;
use ak_proto::guide::{OpOut, REF_H, REF_W};
use ak_proto::{Guide, GuideFormation};
use serde_json::{json, Value};

/// 出せる補足素材の小窓 1 つ（出力の時刻 [t0, t1) に、補足素材の src_start 秒から）
#[derive(Clone, Debug, PartialEq)]
pub struct SubSlot {
    /// guide.subs の何番目か
    pub index: usize,
    pub src: String,
    pub at_op: String,
    pub label: String,
    /// 出力の時刻（手動パートの中に収めたもの）
    pub t0: f64,
    pub t1: f64,
    /// t0 に出す補足素材の秒（手動パートの頭で切ったときは、そのぶん進める）
    pub src_start: f64,
}

impl SubSlot {
    /// 出力の時刻 t に出す補足素材の秒。区間の外は None
    pub fn src_time(&self, t: f64) -> Option<f64> {
        (t >= self.t0 && t < self.t1).then(|| self.src_start + (t - self.t0))
    }
}

/// guide.subs から、出せる小窓の区間と、出せないものの理由（警告。和文）を作る。
/// 操作の出力の時刻（`ops_out`）に offset を足して出し始めとし、長さは `src_out − src_in`。手動パートの外へはみ出す分は切る。
/// 区間が全部手動パートの外・`at_op` が無い・操作がカットの中・`src_out <= src_in`・素材なしは警告にして、その sub だけ出さない。
/// 手動パートが組み立てに無ければ、何も出さず警告も出さない（組み立てのエラーのほうが先に出る）
pub fn sub_slots(guide: &Guide, plan: &GuidePlan, outs: &[OpOut]) -> (Vec<SubSlot>, Vec<String>) {
    let mut slots = vec![];
    let mut warns = vec![];
    let Some(man) = plan.parts.iter().find(|p| p.name == "manual") else {
        return (slots, warns);
    };
    for (i, s) in guide.subs.iter().enumerate() {
        let name = format!("補足素材 {}（{}）", i + 1, if s.src.is_empty() { "素材なし" } else { &s.src });
        if s.src.trim().is_empty() {
            warns.push(format!("{name}: 素材が選ばれていません"));
            continue;
        }
        if !(s.src_in.is_finite() && s.src_out.is_finite() && s.offset.is_finite()) || !(s.src_out > s.src_in) {
            warns.push(format!("{name}: 補足素材の区間（{}〜{} 秒）が空です。終わりを始めより後にしてください", s.src_in, s.src_out));
            continue;
        }
        let Some(op) = guide.ops.iter().position(|o| o.id == s.at_op) else {
            warns.push(format!("{name}: 操作「{}」がありません", s.at_op));
            continue;
        };
        let Some(t_out) = outs.get(op).and_then(|o| o.t_out) else {
            warns.push(format!("{name}: 操作「{}」の出力の時刻がありません（カットの中か、手動パートの外です）", s.at_op));
            continue;
        };
        let t0 = t_out + s.offset;
        let t1 = t0 + (s.src_out - s.src_in);
        let (c0, c1) = (t0.max(man.t0), t1.min(man.t1));
        if c1 <= c0 {
            warns.push(format!(
                "{name}: 出す区間（{t0:.2}〜{t1:.2} 秒）が手動パート（{:.2}〜{:.2} 秒）の外です",
                man.t0, man.t1
            ));
            continue;
        }
        slots.push(SubSlot {
            index: i,
            src: s.src.clone(),
            at_op: s.at_op.clone(),
            label: s.label.clone(),
            t0: c0,
            t1: c1,
            src_start: s.src_in + (c0 - t0),
        });
    }
    (slots, warns)
}

/// 自動パートのタイマーを、ゲーム内のフレームに何フレーム足して出すか。
/// 最初にコストバーが動いたコマ（cost0。解析のゲーム内の時間は 0）が 1F、その 1 つ前のコマが 0F になる
pub const TIMER_LEAD: f64 = 1.0;

/// 自動の戦闘の cost0（解析が最初にコストバーが動いたと見たコマ）の添字。
/// 自動のアンカー（auto_cost0）にいちばん近い cost0 を持つ戦闘のもの。解析に無ければ None
pub fn auto_cost0_frame(plan: &GuidePlan, analysis: &GuideAnalysis) -> Option<u32> {
    let at = plan.anchor_src_of("auto_cost0")?;
    analysis
        .battles
        .iter()
        .filter_map(|b| b.cost0.and_then(|c| u32::try_from(c).ok()))
        .filter_map(|c| analysis.pts.get(c as usize).map(|&t| (c, (t - at).abs())))
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(c, _)| c)
}

/// 出力のコマごとのタイマーの値（ゲーム内のフレーム。自動パートの間だけ。ほかは None）。
/// 見せている素材のコマにゲーム内の時間があれば `game + TIMER_LEAD`。無くて、cost0 を最初に見せるコマ
/// （出力は 60fps・素材は可変 fps なので、cost0 の 1 つ前の素材のコマが出力に出ないことがある）の 1 つ前の出力のコマなら 0。ほかは None
pub fn timer_values(plan: &GuidePlan, analysis: Option<&GuideAnalysis>) -> Vec<Option<f64>> {
    let Some(a) = analysis else { return vec![None; plan.frames.len()] };
    // 自動パートで、素材が cost0 に届いた最初の出力のコマ。その 1 つ前が 0F
    let pre = auto_cost0_frame(plan, a)
        .and_then(|c0| plan.frames.iter().position(|f| plan.part_name(f) == "auto" && f.src >= c0))
        .and_then(|k| k.checked_sub(1));
    plan.frames
        .iter()
        .enumerate()
        .map(|(k, f)| {
            if plan.part_name(f) != "auto" {
                return None;
            }
            match a.game.get(f.src as usize).copied().flatten() {
                Some(g) => Some(g + TIMER_LEAD),
                None => (Some(k) == pre).then_some(0.0),
            }
        })
        .collect()
}

/// クリア時間: タイマーが止まって見えている値（最後の None でない値）のフレーム数。無ければ None
pub fn clear_time_frames(timer: &[Option<f64>]) -> Option<i64> {
    timer.iter().rev().find_map(|t| t.filter(|x| x.is_finite())).map(|x| x.floor().max(0.0) as i64)
}

/// 素材の画素（REF の大きさでの値）→ 出力の画素の変換（倍率 1。出力 = o + 素材 × k）。式は `layer::geometry` の 1 か所
#[derive(Clone, Debug, PartialEq)]
pub struct Xform {
    pub k: [f64; 2],
    pub o: [i64; 2],
    /// レイヤーの範囲 [x0, y0, x1, y1]（出力の画素）。この外は素材が見えない
    pub clip: [i64; 4],
}

/// part の変換。(sw, sh) は素材の大きさ
pub fn xform(layout: &ak_proto::GuideLayout, part: &str, sw: u32, sh: u32) -> Xform {
    // k・o・clip は本体（レイヤー）の位置だけで、待機欄の置き方には左右されない
    let g = geometry(layout, &Default::default(), part, sw, sh, 1.0);
    Xform {
        k: [g.read.0 as f64 / REF_W, g.read.1 as f64 / REF_H],
        o: [g.base.0, g.base.1],
        clip: g.clip,
    }
}

/// 編成の使う枠の番号のうち、枠の数の中にあるもの（used が空なら全部の枠）と、外のものの警告（和文）
pub fn valid_used(f: &GuideFormation) -> (Vec<usize>, Vec<String>) {
    let n = f.effective_slots().len();
    if f.used.is_empty() {
        return ((0..n).collect(), vec![]);
    }
    let warns = f.used.iter().filter(|&&i| i >= n).map(|i| format!("編成の使う枠 {i} は枠の数（{n}）の外です。この枠は無いものとして扱います")).collect();
    (f.used.iter().copied().filter(|&i| i < n).collect(), warns)
}

/// 画面に出す警告（和文）: 使えない拡大・編成の枠の番号・補足素材の小窓。
/// plan が無ければ小窓は見ない（組み立てのエラーのほうが先に出る）。outs は plan の `ops_out`
pub fn guide_warnings(guide: &Guide, plan: Option<&GuidePlan>, outs: &[OpOut]) -> Vec<String> {
    let mut w = valid_zooms(&guide.layout).1;
    w.extend(valid_used(&guide.formation).1);
    if let Some(p) = plan {
        w.extend(sub_slots(guide, p, outs).1);
    }
    w
}

/// 出力の時刻 t に出す補足素材の小窓と、そのときの補足素材の秒（`guide.subs` の順。補足素材が無ければ空）
pub fn subs_at(guide: &Guide, plan: &GuidePlan, t: f64) -> Vec<(SubSlot, f64)> {
    if guide.subs.is_empty() {
        return vec![];
    }
    let (slots, _) = sub_slots(guide, plan, &ops_out(guide, plan, &[], &[]));
    slots.into_iter().filter_map(|s| s.src_time(t).map(|st| (s, st))).collect()
}

/// 画面の状態に出す警告（和文）: `guide_warnings` → 補足素材の大きさの警告 → ほか（素材の音の警告など。extra の順）。
/// guide が無ければ空。解析が無ければ cost・game は空、組み立てが無ければ操作の出力は空
pub fn state_warnings(guide: Option<&Guide>, plan: Option<&GuidePlan>, analysis: Option<&GuideAnalysis>, extra: &[&[String]]) -> Vec<String> {
    let Some(g) = guide else { return vec![] };
    let (cost, game) = analysis.map(|a| (a.cost.as_slice(), a.game.as_slice())).unwrap_or((&[], &[]));
    let outs = plan.map(|p| ops_out(g, p, cost, game)).unwrap_or_default();
    let mut w = guide_warnings(g, plan, &outs);
    for e in extra {
        w.extend(e.iter().cloned());
    }
    w
}

/// 箱 [x, y, 幅, 高さ] の中に、縦横比を保って収めて中央に置いた矩形 [x, y, 幅, 高さ]。大きさが分からなければ箱のまま
pub fn fit_box(b: [f64; 4], w: u32, h: u32) -> [f64; 4] {
    if w == 0 || h == 0 || b[2] <= 0.0 || b[3] <= 0.0 {
        return b;
    }
    let k = (b[2] / w as f64).min(b[3] / h as f64);
    let (fw, fh) = (w as f64 * k, h as f64 * k);
    [b[0] + (b[2] - fw) / 2.0, b[1] + (b[3] - fh) / 2.0, fw, fh]
}

/// 対応表（`studio/guide_map.json`）。`GuidePlan::to_map_json` の項目に、演出が使う項目を足す（version は `MAP_VERSION` = 2。Python は 2 しか読まない）。
/// analysis は素材の大きさ・コスト・ゲーム内の時間を引くため（無ければ REF の大きさ・null）。
/// sub_dims は補足素材の絵の大きさ（分からなければ None。小窓の箱を `sub_box` のままにする）
pub fn map_json(plan: &GuidePlan, guide: &Guide, analysis: Option<&GuideAnalysis>,
                sub_dims: &dyn Fn(&str) -> Option<(u32, u32)>) -> Value {
    let mut m = plan.to_map_json();
    let (cost, game) = analysis.map(|a| (a.cost.as_slice(), a.game.as_slice())).unwrap_or((&[], &[]));
    let outs = ops_out(guide, plan, cost, game);
    let o = m.as_object_mut().expect("to_map_json は object");

    o.insert("game".into(), json!(plan.frames.iter().map(|f| game.get(f.src as usize).copied().flatten()).collect::<Vec<_>>()));
    // 自動パートのタイマー（出力のコマごと。自動パートの間だけ値）。Rust が決めて、演出は読むだけ
    o.insert("timer".into(), json!(timer_values(plan, analysis)));
    o.insert("anchors_out".into(), json!(plan.anchor_out.iter().cloned().collect::<std::collections::BTreeMap<_, _>>()));
    o.insert("seams".into(), json!(plan.seams));
    o.insert(
        "ops".into(),
        json!(guide.ops.iter().zip(&outs).map(|(g, u)| json!({
            "id": g.id, "t_out": u.t_out, "t_now": u.t_now, "now_lead": g.now_lead, "kind": g.kind, "op": g.op, "where": g.r#where, "info": g.info, "precise": g.precise,
            "cue_text": g.cue_text, "note": g.note, "cost": u.cost, "frame": u.frame,
        })).collect::<Vec<_>>()),
    );

    // 素材の画素 (x, y)（REF の大きさでの値）→ 出力 = (o.x + x·kx, o.y + y·ky)。式は layer::geometry
    let (sw, sh) = analysis.map(|a| (a.size[0], a.size[1])).filter(|s| s.0 > 0 && s.1 > 0).unwrap_or((REF_W as u32, REF_H as u32));
    let mut geo = serde_json::Map::new();
    for part in PART_NAMES {
        let x = xform(&guide.layout, part, sw, sh);
        geo.insert(part.into(), json!({"k": x.k, "o": x.o, "clip": x.clip}));
    }
    o.insert("geo".into(), Value::Object(geo));

    let f = &guide.formation;
    let (used, _) = valid_used(f);
    // 使わない枠の暗さを戻し始める時刻（出力）。暗転のアンカーが無ければ null（演出は編成パートの終わりまで出す）
    let t_release = plan.anchor_out.iter().find(|(k, _)| k == "formation_dark").map(|(_, t)| t - DIM_RELEASE);
    o.insert(
        "formation".into(),
        json!({"slots": f.effective_slots(), "used": used, "notes": f.notes, "t_release": t_release}),
    );

    // コストの拡大（名前が cost）は、普段の 1 行の箱 mini と、そこに置く切り抜きの箱 mini_img も（演出が枠と見出しを合わせる）
    o.insert(
        "zooms".into(),
        json!(valid_zooms(&guide.layout).0.iter().map(|z| {
            let mut e = json!({"name": z.name, "box": zoom_box(z), "parts": z.parts});
            if z.name == "cost" {
                let (row, img) = mini_boxes(z);
                e["mini"] = json!(row);
                e["mini_img"] = json!(img);
            }
            e
        }).collect::<Vec<_>>()),
    );
    // コストの拡大を出す区間（精密な操作ごと）と、出力のコマごとの今のコスト。度合い k(t) は演出が式（layer::cost_k）で出す
    let hs = holds(&guide.ops, &outs, plan, cost);
    o.insert(
        "holds".into(),
        json!(hs.iter().map(|h| json!({
            "op": h.op, "t0": h.t0, "t1": h.t1, "frame": h.frame, "cost_from": h.cost_from, "cost_to": h.cost_to,
        })).collect::<Vec<_>>()),
    );
    o.insert("cost".into(), json!(plan.frames.iter().map(|f| cost.get(f.src as usize).copied().flatten()).collect::<Vec<_>>()));

    let (slots, _) = sub_slots(guide, plan, &outs);
    o.insert(
        "subs".into(),
        json!(slots.iter().map(|s| {
            let b = match sub_dims(&s.src) {
                Some((w, h)) => fit_box(guide.layout.sub_box, w, h),
                None => guide.layout.sub_box,
            };
            json!({"t0": s.t0, "t1": s.t1, "box": b, "label": s.label, "at_op": s.at_op})
        }).collect::<Vec<_>>()),
    );
    m
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::guide::layer::slot_rect;
    use crate::guide::plan::plan;
    use ak_proto::guide::{Anchor, ANCHOR_NAMES};
    use ak_proto::{Cut, GuideFixed, GuideOp, GuideSub};
    use std::sync::Arc;

    fn pts_const(n: usize, fps: f64) -> Vec<f64> {
        (0..n).map(|i| i as f64 / fps).collect()
    }

    /// 17-5 に近いアンカー、操作 3 つ（op2 は信頼枠）
    fn guide() -> Guide {
        let mut g = Guide::default();
        for (name, v) in ANCHOR_NAMES.iter().zip([3.475, 9.208, 181.043, 202.393, 300.945]) {
            g.anchors.insert(name.to_string(), Anchor { t: Some(v), auto: None });
        }
        let op = |id: &str, t: f64, kind: &str, precise: bool| GuideOp {
            id: id.into(), t, kind: kind.into(), op: "ウタゲ".into(), r#where: "R3C5 ↑".into(), precise,
            cue_text: "Kill15".into(), note: "注意".into(), info: "補足".into(), ..Default::default()
        };
        g.ops = vec![op("op1", 20.0, "deploy", true), op("op2", 30.0, "trust", false), op("op3", 60.0, "skill", false)];
        g
    }

    fn analysis(n: usize) -> GuideAnalysis {
        GuideAnalysis {
            src: "a.mp4".into(),
            size: [2752, 2064],
            pts: Arc::new(pts_const(n, 60.0)),
            cost: (0..n).map(|i| Some(i as i32 % 50)).collect(),
            game: (0..n).map(|i| (i % 7 != 0).then_some(i as f64)).collect(),
            ..Default::default()
        }
    }

    fn built(g: &Guide) -> (GuidePlan, GuideAnalysis) {
        let a = analysis(20000);
        (plan(g, &GuideFixed::default(), &a.pts).unwrap(), a)
    }

    #[test]
    fn map_has_game_ops_anchors_seams_and_formation() {
        let mut g = guide();
        g.cuts.push(Cut { src_in: 40.0, src_out: 52.0, fade: 0.5 });
        let (p, a) = built(&g);
        let m = map_json(&p, &g, Some(&a), &|_| None);
        // 既存の項目はそのまま
        assert_eq!(m["version"], 2);
        assert_eq!(m["src"].as_array().unwrap().len(), p.frames.len());
        // game は出力のコマごと（src と同じ長さ）。見せている素材のコマの値
        let game = m["game"].as_array().unwrap();
        assert_eq!(game.len(), m["src"].as_array().unwrap().len());
        let k = 700usize;
        let src = m["src"][k].as_u64().unwrap() as usize;
        assert_eq!(game[k], json!(a.game[src]));
        let null_k = (0..p.frames.len()).find(|&k| a.game[p.frames[k].src as usize].is_none()).unwrap();
        assert!(game[null_k].is_null());
        // 解析の game が無ければ全部 null で、長さは同じ
        let m2 = map_json(&p, &g, None, &|_| None);
        assert_eq!(m2["game"].as_array().unwrap().len(), p.frames.len());
        assert!(m2["game"].as_array().unwrap().iter().all(|x| x.is_null()));
        // アンカーの出力の時刻・継ぎ目
        assert_eq!(m["anchors_out"]["manual_cost0"].as_f64().unwrap(), p.anchor_out.iter().find(|(k, _)| k == "manual_cost0").unwrap().1);
        assert_eq!(m["anchors_out"].as_object().unwrap().len(), 5);
        assert_eq!(m["seams"], json!(p.seams));
        assert_eq!(m["seams"].as_array().unwrap().len(), 1);
        // 操作: t_out は ops_out と同じ、cost・frame も入る。信頼枠も入る
        let ops = m["ops"].as_array().unwrap();
        assert_eq!(ops.len(), 3);
        assert_eq!(ops[0]["id"], "op1");
        assert_eq!(ops[0]["where"], "R3C5 ↑");
        assert_eq!(ops[0]["info"], "補足");
        assert_eq!(ops[0]["precise"], true);
        assert_eq!(ops[1]["kind"], "trust");
        let outs = ops_out(&g, &p, &a.cost, &a.game);
        assert_eq!(ops[0]["t_out"].as_f64(), outs[0].t_out);
        // NOW の開始（先出し）: ops_out と同じ。個別値が無ければ now_lead は null
        assert_eq!(ops[0]["t_now"].as_f64(), outs[0].t_now);
        assert!(ops[0]["t_now"].as_f64().unwrap() < ops[0]["t_out"].as_f64().unwrap());
        assert!(ops[0]["now_lead"].is_null());
        // 信頼枠は先出し無し（t_now = t_out）
        assert_eq!(ops[1]["t_now"], ops[1]["t_out"]);
        assert!(ops[0]["t_out"].as_f64().unwrap() > 8.0);
        assert_eq!(ops[2]["cost"], json!(outs[2].cost));
        assert_eq!(ops[2]["frame"].as_u64().unwrap() as u32, outs[2].frame);
        // カットの中の操作は t_out が null のまま入る
        g.ops.push(GuideOp { id: "op4".into(), t: 45.0, kind: "deploy".into(), ..Default::default() });
        g.ops.sort_by(|a, b| a.t.total_cmp(&b.t));
        let m3 = map_json(&p, &g, Some(&a), &|_| None);
        let o4 = m3["ops"].as_array().unwrap().iter().find(|o| o["id"] == "op4").unwrap();
        assert!(o4["t_out"].is_null());
    }

    /// 手動の戦闘（cost0 = 552）と自動の戦闘（cost0 = 12144）が解析にある分析。ゲーム内の時間は cost0 から 1 コマ 1 ずつ
    fn analysis_with_battles(n: usize) -> GuideAnalysis {
        let mut a = analysis(n);
        let bs = [(100i64, 11000i64, 552i64), (12000, 18000, 12144)];
        a.game = vec![None; n];
        for (s, e, c) in bs {
            for k in c..=e {
                a.game[k as usize] = Some((k - c) as f64);
            }
            a.battles.push(ak_proto::guide::GuideBattle { start: s, end: e, cost0: Some(c), end_band: None });
        }
        a
    }

    #[test]
    fn timer_starts_at_one_on_cost0_and_zero_the_frame_before() {
        let g = guide();
        let a = analysis_with_battles(20000);
        let p = plan(&g, &GuideFixed::default(), &a.pts).unwrap();
        assert_eq!(auto_cost0_frame(&p, &a), Some(12144));
        let m = map_json(&p, &g, Some(&a), &|_| None);
        let timer = m["timer"].as_array().unwrap();
        assert_eq!(timer.len(), p.frames.len());
        let at = |src: u32, part: &str| {
            let k = p.frames.iter().position(|f| f.src == src && p.part_name(f) == part).unwrap();
            timer[k].as_f64()
        };
        assert_eq!(at(12144, "auto"), Some(1.0), "cost0 が 1F");
        assert_eq!(at(12143, "auto"), Some(0.0), "その 1 つ前が 0F");
        assert_eq!(at(12142, "auto"), None, "それより前は出さない");
        assert_eq!(at(12145, "auto"), Some(2.0));
        assert_eq!(at(12144 + 60, "auto"), Some(61.0));
        // 自動の外（手動パートの戦闘中・自動の cost0 の前の手動の cost0-1）は game があっても null
        let km = p.frames.iter().position(|f| f.src == 700 && p.part_name(f) == "manual").unwrap();
        assert!(timer[km].is_null());
        let km0 = p.frames.iter().position(|f| f.src == 551 && p.part_name(f) == "manual").unwrap();
        assert!(timer[km0].is_null(), "手動の cost0 の前は 0 にしない");
        // game が読めないコマ（戦闘が終わった後）は null
        assert_eq!(at(18001, "auto"), None);
        // 解析が無ければ全部 null で長さは同じ
        let m2 = map_json(&p, &g, None, &|_| None);
        assert_eq!(m2["timer"].as_array().unwrap().len(), p.frames.len());
        assert!(m2["timer"].as_array().unwrap().iter().all(|x| x.is_null()));
        // game はそのまま（手動パートの game_text・画面のゲーム内の時刻は今のまま）
        assert_eq!(m["game"][km], json!(a.game[700]));
    }

    #[test]
    fn timer_zero_frame_is_the_output_frame_before_cost0_even_if_the_source_frame_is_skipped() {
        let g = guide();
        let a = analysis_with_battles(20000);
        let mut p = plan(&g, &GuideFixed::default(), &a.pts).unwrap();
        // 可変 fps の素材: cost0 の 1 つ前の素材のコマ（12143）が出力に出ない（12142 → 12144 と飛ぶ）
        let k = p.frames.iter().position(|f| f.src == 12143 && p.part_name(f) == "auto").unwrap();
        p.frames.remove(k);
        let t = timer_values(&p, Some(&a));
        assert_eq!(p.frames[k].src, 12144);
        assert_eq!(t[k], Some(1.0));
        assert_eq!(t[k - 1], Some(0.0), "出力で 1 つ前のコマが 0F");
        assert_eq!(t[k - 2], None);
        // cost0 を同じ素材のコマで 2 コマ見せても、最初のコマが 1F・その前が 0F（0 は 1 つだけ）
        let mut p = plan(&g, &GuideFixed::default(), &a.pts).unwrap();
        let k = p.frames.iter().position(|f| f.src == 12144 && p.part_name(f) == "auto").unwrap();
        let dup = p.frames[k].clone();
        p.frames.insert(k, dup);
        let t = timer_values(&p, Some(&a));
        assert_eq!((t[k - 1], t[k], t[k + 1]), (Some(0.0), Some(1.0), Some(1.0)));
        assert_eq!(t.iter().filter(|x| **x == Some(0.0)).count(), 1);
    }

    #[test]
    fn timer_without_cost0_battle_has_no_zero_frame() {
        let g = guide();
        let mut a = analysis_with_battles(20000);
        a.battles.clear();
        let p = plan(&g, &GuideFixed::default(), &a.pts).unwrap();
        assert_eq!(auto_cost0_frame(&p, &a), None);
        let t = timer_values(&p, Some(&a));
        let k = p.frames.iter().position(|f| f.src == 12143 && p.part_name(f) == "auto").unwrap();
        assert_eq!(t[k], None);
    }

    #[test]
    fn clear_time_is_the_last_timer_value() {
        let g = guide();
        let a = analysis_with_battles(20000);
        let p = plan(&g, &GuideFixed::default(), &a.pts).unwrap();
        let t = timer_values(&p, Some(&a));
        // 自動の戦闘の最後のコマ 18000 の値 5856 + 1
        assert_eq!(clear_time_frames(&t), Some(5857));
        assert_eq!(clear_time_frames(&[None, None]), None);
        assert_eq!(clear_time_frames(&[Some(3.0), Some(5.9), None]), Some(5));
        assert_eq!(clear_time_frames(&[]), None);
    }

    #[test]
    fn map_geo_matches_geometry_on_sample_points() {
        let g = guide();
        let (p, a) = built(&g);
        let m = map_json(&p, &g, Some(&a), &|_| None);
        for part in PART_NAMES {
            let geo = geometry(&g.layout, &Default::default(), part, 2752, 2064, 1.0);
            let e = &m["geo"][part];
            assert_eq!(e["clip"], json!(geo.clip), "{part}");
            let (kx, ky) = (e["k"][0].as_f64().unwrap(), e["k"][1].as_f64().unwrap());
            let (ox, oy) = (e["o"][0].as_f64().unwrap(), e["o"][1].as_f64().unwrap());
            // 素材の画素の点 → 出力。slot_rect（描くときの変換）と 1 画素以内で一致する
            for (x, y) in [(0.0, 0.0), (230.0, 426.0), (1376.0, 1032.0), (2752.0, 2064.0)] {
                let r = slot_rect(&geo, [x, y, x, y]);
                let (px, py) = (ox + x * kx, oy + y * ky);
                assert!((px - r[0] as f64).abs() <= 0.5 + 1e-9 && (py - r[1] as f64).abs() <= 0.5 + 1e-9, "{part} {x},{y}: {px},{py} vs {r:?}");
            }
        }
        // 手動は 75% で左上、ほかは全面
        assert_eq!(m["geo"]["manual"]["clip"], json!([0, 0, 1440, 810]));
        assert_eq!(m["geo"]["auto"]["clip"], json!([0, 0, 1920, 1080]));
        // 解析が無ければ REF の大きさで同じ
        let m2 = map_json(&p, &g, None, &|_| None);
        assert_eq!(m2["geo"], m["geo"]);
    }

    #[test]
    fn map_formation_used_empty_means_all_slots() {
        let mut g = guide();
        let (p, a) = built(&g);
        let m = map_json(&p, &g, Some(&a), &|_| None);
        assert_eq!(m["formation"]["slots"].as_array().unwrap().len(), 12);
        assert_eq!(m["formation"]["used"], json!((0..12).collect::<Vec<_>>()));
        assert_eq!(m["formation"]["slots"][0], json!([230.0, 426.0, 500.0, 1010.0]));
        g.formation.used = vec![0, 2];
        g.formation.notes.insert("2".into(), "Lv90".into());
        let m = map_json(&p, &g, Some(&a), &|_| None);
        assert_eq!(m["formation"]["used"], json!([0, 2]));
        assert_eq!(m["formation"]["notes"]["2"], "Lv90");
    }

    #[test]
    fn map_zooms_and_subs() {
        let mut g = guide();
        let (p, a) = built(&g);
        let m = map_json(&p, &g, Some(&a), &|_| None);
        // 既定のコストの拡大: 472x205 の 0.95 倍 → 448.4x194.75（出力の箱）
        let z = &m["zooms"][0];
        assert_eq!(z["name"], "cost");
        assert_eq!(z["parts"], json!(["manual"]));
        let b = z["box"].as_array().unwrap().iter().map(|x| x.as_f64().unwrap()).collect::<Vec<_>>();
        assert_eq!((b[0], b[1]), (1456.0, 600.0));
        assert!((b[2] - 448.4).abs() < 1e-9 && (b[3] - 194.75).abs() < 1e-9, "{b:?}");
        // sub が無ければ空
        assert!(m["subs"].as_array().unwrap().is_empty());
        // sub: op1 の出力の時刻 − 2 秒から、補足素材の区間の長さだけ。4:3 の絵は sub_box にそのまま
        g.subs.push(GuideSub { src: "OPVF9634.MP4".into(), src_in: 10.0, src_out: 16.0, at_op: "op1".into(), offset: -2.0, label: "別の動き".into(), ..Default::default() });
        let outs = ops_out(&g, &p, &a.cost, &a.game);
        let t1 = outs[0].t_out.unwrap();
        let m = map_json(&p, &g, Some(&a), &|_| Some((640, 480)));
        let s = &m["subs"][0];
        assert!((s["t0"].as_f64().unwrap() - (t1 - 2.0)).abs() < 1e-9);
        assert!((s["t1"].as_f64().unwrap() - (t1 + 4.0)).abs() < 1e-9);
        assert_eq!(s["box"], json!([1080.0, 150.0, 320.0, 240.0]));
        assert_eq!((s["label"].as_str(), s["at_op"].as_str()), (Some("別の動き"), Some("op1")));
        // 16:9 の絵は縦を余らせて中央に（360x202.5、y = 150 + 18.75）
        let m = map_json(&p, &g, Some(&a), &|_| Some((1920, 1080)));
        assert_eq!(m["subs"][0]["box"], json!([1060.0, 168.75, 360.0, 202.5]));
        // 出せない sub は入らない
        g.subs[0].at_op = "op99".into();
        let m = map_json(&p, &g, Some(&a), &|_| None);
        assert!(m["subs"].as_array().unwrap().is_empty());
    }

    #[test]
    fn map_has_holds_cost_and_mini_boxes() {
        let g = guide();
        let (p, a) = built(&g);
        let m = map_json(&p, &g, Some(&a), &|_| None);
        let outs = ops_out(&g, &p, &a.cost, &a.game);
        // 区間: 精密な op1 だけ（op2 は信頼枠・op3 は精密でない）。t0 = t_now、t1 = t_out + 0.5、止めるコマは frame − 1
        let hs = m["holds"].as_array().unwrap();
        assert_eq!(hs.len(), 1);
        let h = &hs[0];
        assert_eq!(h["op"], "op1");
        assert_eq!(h["t0"].as_f64(), outs[0].t_now);
        assert!((h["t1"].as_f64().unwrap() - (outs[0].t_out.unwrap() + 0.5)).abs() < 1e-9);
        assert_eq!(h["frame"].as_u64().unwrap() as u32, outs[0].frame - 1);
        // 目標のコストは止めるコマのコスト、始まりのコストは t0 のコマのコスト
        assert_eq!(h["cost_to"], json!(a.cost[(outs[0].frame - 1) as usize]));
        let f0 = p.frame_at(outs[0].t_now.unwrap()).unwrap().src as usize;
        assert_eq!(h["cost_from"], json!(a.cost[f0]));
        // 出力のコマごとの今のコスト（game と同じ並び）
        let cost = m["cost"].as_array().unwrap();
        assert_eq!(cost.len(), p.frames.len());
        assert_eq!(cost[700], json!(a.cost[p.frames[700].src as usize]));
        // 解析が無ければ全部 null・区間の cost は null
        let m2 = map_json(&p, &g, None, &|_| None);
        assert_eq!(m2["cost"].as_array().unwrap().len(), p.frames.len());
        assert!(m2["cost"].as_array().unwrap().iter().all(|x| x.is_null()));
        assert!(m2["holds"][0]["cost_to"].is_null() && m2["holds"][0]["cost_from"].is_null());
        // コストの拡大の項目に mini（1 行の箱）・mini_img（切り抜きの箱）
        let z = &m["zooms"][0];
        let mini = z["mini"].as_array().unwrap().iter().map(|x| x.as_f64().unwrap()).collect::<Vec<_>>();
        assert_eq!((mini[0], mini[3]), (1456.0, 56.0));
        assert!((mini[1] - (600.0 + 194.75 - 56.0)).abs() < 1e-9 && (mini[2] - 448.4).abs() < 1e-9, "{mini:?}");
        let img = z["mini_img"].as_array().unwrap().iter().map(|x| x.as_f64().unwrap()).collect::<Vec<_>>();
        assert_eq!(img[3], 48.0);
        assert!(img[0] >= 1456.0 + 96.0 && img[0] + img[2] <= 1456.0 + 448.4);
        // 先出し 0 の精密な操作は区間に入らない
        let mut g0 = guide();
        g0.ops[0].now_lead = Some(0.0);
        let (p0, a0) = built(&g0);
        assert!(map_json(&p0, &g0, Some(&a0), &|_| None)["holds"].as_array().unwrap().is_empty());
    }

    #[test]
    fn sub_slots_interval_offset_and_warnings() {
        let mut g = guide();
        let (p, a) = built(&g);
        let outs = ops_out(&g, &p, &a.cost, &a.game);
        let man = p.parts.iter().find(|x| x.name == "manual").unwrap().clone();
        let t_op1 = outs[0].t_out.unwrap();
        let sub = |at: &str, off: f64, i: f64, o: f64| GuideSub {
            src: "S.MP4".into(), src_in: i, src_out: o, at_op: at.into(), offset: off, ..Default::default()
        };
        // 普通: t0 = 操作 + offset、長さ = src_out − src_in
        g.subs = vec![sub("op1", -2.0, 5.0, 9.0)];
        let (s, w) = sub_slots(&g, &p, &outs);
        assert!(w.is_empty(), "{w:?}");
        assert!((s[0].t0 - (t_op1 - 2.0)).abs() < 1e-9 && (s[0].t1 - (t_op1 + 2.0)).abs() < 1e-9);
        assert_eq!(s[0].src_start, 5.0);
        assert_eq!(s[0].src_time(s[0].t0), Some(5.0));
        assert!((s[0].src_time(s[0].t0 + 1.5).unwrap() - 6.5).abs() < 1e-9);
        assert_eq!(s[0].src_time(s[0].t1), None, "終わりは含まない");
        assert_eq!(s[0].src_time(s[0].t0 - 0.01), None);
        // 手動パートの終わりで切る
        g.subs = vec![sub("op3", 0.0, 0.0, 1000.0)];
        let (s, w) = sub_slots(&g, &p, &outs);
        assert!(w.is_empty());
        assert_eq!(s[0].t1, man.t1);
        // 手動パートの頭より前から始まるなら頭で切り、補足素材もそのぶん進める
        g.subs = vec![sub("op1", -1000.0, 0.0, 1000.0 + 5.0)];
        let (s, _) = sub_slots(&g, &p, &outs);
        assert_eq!(s[0].t0, man.t0);
        assert!((s[0].src_start - (man.t0 - (t_op1 - 1000.0))).abs() < 1e-9);
        // 警告: 区間が全部手動パートの外・at_op が無い・src_out <= src_in・素材なし・数でない値
        g.subs = vec![
            sub("op1", 1000.0, 0.0, 3.0),
            sub("op99", 0.0, 0.0, 3.0),
            sub("op1", 0.0, 5.0, 5.0),
            GuideSub { at_op: "op1".into(), src_out: 3.0, ..Default::default() },
            sub("op1", 0.0, f64::NAN, 3.0),
        ];
        let (s, w) = sub_slots(&g, &p, &outs);
        assert!(s.is_empty());
        assert_eq!(w.len(), 5, "{w:?}");
        assert!(w[0].contains("外") && w[0].contains("補足素材 1"));
        assert!(w[1].contains("op99") && w[1].contains("ありません"));
        assert!(w[2].contains("空"));
        assert!(w[3].contains("素材が選ばれていません"));
        assert!(w[4].contains("空"));
        // カットの中の操作（t_out が無い）
        let mut g2 = guide();
        g2.cuts.push(Cut { src_in: 40.0, src_out: 52.0, fade: 0.5 });
        g2.ops.push(GuideOp { id: "op4".into(), t: 45.0, kind: "deploy".into(), ..Default::default() });
        g2.ops.sort_by(|a, b| a.t.total_cmp(&b.t));
        let (p2, a2) = built(&g2);
        let outs2 = ops_out(&g2, &p2, &a2.cost, &a2.game);
        g2.subs = vec![sub("op4", 0.0, 0.0, 3.0)];
        let (s, w) = sub_slots(&g2, &p2, &outs2);
        assert!(s.is_empty() && w.len() == 1 && w[0].contains("カットの中"), "{w:?}");
    }

    #[test]
    fn subs_at_picks_the_slots_covering_t_in_order() {
        let mut g = guide();
        let (p, a) = built(&g);
        let outs = ops_out(&g, &p, &a.cost, &a.game);
        let t_op1 = outs[0].t_out.unwrap();
        let sub = |at: &str, off: f64, i: f64, o: f64| GuideSub {
            src: "S.MP4".into(), src_in: i, src_out: o, at_op: at.into(), offset: off, ..Default::default()
        };
        // 補足素材が無ければ空
        assert!(subs_at(&g, &p, t_op1).is_empty());
        // 1 つ: [t_op1, t_op1 + 4)、補足素材の 5 秒から
        g.subs = vec![sub("op1", 0.0, 5.0, 9.0)];
        assert!(subs_at(&g, &p, t_op1 - 0.01).is_empty(), "前");
        let v = subs_at(&g, &p, t_op1 + 1.5);
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].0.index, 0);
        assert!((v[0].1 - 6.5).abs() < 1e-9);
        assert_eq!(subs_at(&g, &p, t_op1).len(), 1, "始まりちょうどは出る");
        assert!(subs_at(&g, &p, t_op1 + 4.0).is_empty(), "終わりちょうどは出ない");
        assert!(subs_at(&g, &p, t_op1 + 4.5).is_empty(), "後");
        // 重なる 2 つは guide.subs の順に両方
        g.subs = vec![sub("op1", 0.0, 5.0, 9.0), sub("op1", 1.0, 0.0, 10.0)];
        let v = subs_at(&g, &p, t_op1 + 2.0);
        assert_eq!(v.iter().map(|(s, _)| s.index).collect::<Vec<_>>(), vec![0, 1]);
        assert!((v[0].1 - 7.0).abs() < 1e-9 && (v[1].1 - 1.0).abs() < 1e-9);
        // 片方だけの区間
        let v = subs_at(&g, &p, t_op1 + 0.5);
        assert_eq!(v.iter().map(|(s, _)| s.index).collect::<Vec<_>>(), vec![0]);
    }

    #[test]
    fn state_warnings_orders_guide_sub_and_extra() {
        let mut g = guide();
        let (p, a) = built(&g);
        // guide が無ければ空（extra も付けない）
        assert!(state_warnings(None, Some(&p), Some(&a), &[&["x".to_string()]]).is_empty());
        // 警告が無くても extra は順番どおり後ろに付く
        let (e1, e2) = (vec!["a".to_string(), "b".to_string()], vec!["c".to_string()]);
        assert_eq!(state_warnings(Some(&g), Some(&p), Some(&a), &[&e1, &e2]), vec!["a", "b", "c"]);
        // 解析が無くても組み立ての警告（補足素材の小窓）は出る。guide_warnings の後ろに extra
        g.formation.used = vec![99];
        g.subs = vec![GuideSub { at_op: "op99".into(), src: "S.MP4".into(), src_out: 3.0, ..Default::default() }];
        let w = state_warnings(Some(&g), Some(&p), None, &[&e2]);
        assert_eq!(w.len(), 3, "{w:?}");
        assert!(w[0].contains("編成の使う枠"));
        assert!(w[1].contains("op99"));
        assert_eq!(w[2], "c");
        // 組み立てが無ければ小窓の警告は無い
        let w = state_warnings(Some(&g), None, None, &[]);
        assert_eq!(w.len(), 1);
        assert!(w[0].contains("編成の使う枠"));
    }

    #[test]
    fn map_formation_t_release_and_used_validation() {
        let mut g = guide();
        let (p, a) = built(&g);
        let m = map_json(&p, &g, Some(&a), &|_| None);
        let dark = p.anchor_out.iter().find(|(k, _)| k == "formation_dark").unwrap().1;
        assert!((m["formation"]["t_release"].as_f64().unwrap() - (dark - DIM_RELEASE)).abs() < 1e-9);
        assert_eq!(m["anchors_out"]["formation_dark"].as_f64().unwrap(), dark);
        // 枠の数の外の番号は map に入れず、警告にする
        g.formation.used = vec![0, 2, 12, 99];
        let m = map_json(&p, &g, Some(&a), &|_| None);
        assert_eq!(m["formation"]["used"], json!([0, 2]));
        let (u, w) = valid_used(&g.formation);
        assert_eq!(u, vec![0, 2]);
        assert_eq!(w.len(), 2);
        assert!(w[0].contains("12") && w[0].contains("外"));
        assert!(guide_warnings(&g, Some(&p), &[]).iter().any(|x| x.contains("99")));
        // used が空なら全部の枠・警告なし
        g.formation.used = vec![];
        assert_eq!(valid_used(&g.formation), ((0..12).collect(), vec![]));
        // 暗転のアンカーが無ければ null
        let mut p2 = p.clone();
        p2.anchor_out.retain(|(k, _)| k != "formation_dark");
        let m = map_json(&p2, &g, Some(&a), &|_| None);
        assert!(m["formation"]["t_release"].is_null());
    }

    #[test]
    fn map_zooms_skip_invalid_ones_with_warnings() {
        let mut g = guide();
        let (p, a) = built(&g);
        let good = g.layout.zooms[0].clone();
        let mut bad_empty = good.clone();
        bad_empty.name = "empty".into();
        bad_empty.src = [10.0, 10.0, 10.0, 50.0];
        let mut bad_scale = good.clone();
        bad_scale.name = "scale".into();
        bad_scale.scale = 0.0;
        let mut bad_nan = good.clone();
        bad_nan.name = "nan".into();
        bad_nan.to = [f64::NAN, 0.0];
        let mut bad_out = good.clone();
        bad_out.name = "out".into();
        bad_out.src = [2000.0, 1500.0, 3000.0, 1600.0];
        g.layout.zooms = vec![bad_empty, good, bad_scale, bad_nan, bad_out];
        let m = map_json(&p, &g, Some(&a), &|_| None);
        let zs = m["zooms"].as_array().unwrap();
        assert_eq!(zs.len(), 1);
        assert_eq!(zs[0]["name"], "cost");
        let w = guide_warnings(&g, Some(&p), &[]);
        assert_eq!(w.len(), 4, "{w:?}");
        assert!(w[0].contains("empty") && w[0].contains("空"));
        assert!(w[1].contains("scale") && w[1].contains("倍率"));
        assert!(w[2].contains("nan") && w[2].contains("数でない"));
        assert!(w[3].contains("out") && w[3].contains("外"));
        // 描く側（zoom_ops）も同じものを使う: 使える拡大だけが残る
        let geo = geometry(&g.layout, &Default::default(), "manual", 2752, 2064, 1.0);
        let ops = crate::guide::layer::zoom_ops(&g.layout, "manual", &geo, 1.0);
        assert_eq!(ops.len(), 1);
        assert_eq!(ops[0].name, "cost");
    }

    #[test]
    fn fit_box_keeps_aspect_and_centers() {
        let b = [1480.0, 300.0, 400.0, 300.0];
        assert_eq!(fit_box(b, 640, 480), b);
        assert_eq!(fit_box(b, 1920, 1080), [1480.0, 337.5, 400.0, 225.0]);
        // 縦長は横を余らせる
        assert_eq!(fit_box(b, 300, 600), [1605.0, 300.0, 150.0, 300.0]);
        assert_eq!(fit_box(b, 0, 0), b);
    }

    /// 実際の回（写し）の解析で、自動パートのタイマーとクリア時間を確かめる（読むだけ。手で走らせる）:
    /// AK_TEST_GUIDE_DIR=E:/tmp/an_MAIN_17-5_copy/studio cargo test -p ak-studio real_timer -- --ignored --nocapture
    #[test]
    #[ignore = "実際の guide.json・解析が要る（AK_TEST_GUIDE_DIR）"]
    fn real_timer_and_clear_time() {
        let Ok(dir) = std::env::var("AK_TEST_GUIDE_DIR") else { panic!("AK_TEST_GUIDE_DIR（studio フォルダ）を指定してください") };
        let dir = std::path::PathBuf::from(dir);
        let g = crate::guide::read_guide(&dir.join("guide.json")).unwrap().expect("guide.json がある");
        let a = crate::guide::read_analysis(&dir.join("guide_analysis.json")).unwrap().expect("解析がある");
        let p = crate::guide::build_plan(Some(&g), Some(&a), &GuideFixed::default()).unwrap().expect("組み立てられる");
        let c0 = auto_cost0_frame(&p, &a).expect("自動の cost0 がある");
        let timer = timer_values(&p, Some(&a));
        let k0 = p.frames.iter().position(|f| f.src == c0 && p.part_name(f) == "auto").unwrap();
        eprintln!("cost0 付近の出力のコマ: {:?}", (k0 - 3..k0 + 4).map(|k| (k, p.frames[k].src, timer[k])).collect::<Vec<_>>());
        let clear = clear_time_frames(&timer).expect("クリア時間がある");
        eprintln!("自動の cost0 = コマ {c0}（{:.3} 秒）", a.pts[c0 as usize]);
        eprintln!("timer: cost0 の 1 つ前の出力のコマ = {:?}, cost0 = {:?}, その次 = {:?}", timer[k0 - 1], timer[k0], timer[k0 + 2]);
        eprintln!("clear_time = {clear} フレーム（{}）", crate::guide::read::game_text(clear as f64));
        assert_eq!(timer[k0 - 1], Some(0.0), "cost0 を見せる最初のコマの 1 つ前が 0F");
        assert_eq!(timer[k0], Some(1.0));
    }
}
