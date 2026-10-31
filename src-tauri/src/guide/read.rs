//! Claude に返す攻略動画の状態（`guide_read`）と、素材の画素 ↔ 出力の画素の変換（`guide_coords`）。純粋な関数。
//!
//! 座標・時刻は Rust の組み立て（`plan.rs`・`layer::geometry`）が決めたものをそのまま渡す（Claude の側で計算し直さない）。

use super::map::{sub_slots, valid_used, xform};
use super::plan::{part_label, GuidePlan, PART_NAMES};
use ak_proto::guide::{GuideState, ANCHOR_NAMES, REF_H, REF_W};
use ak_proto::{Guide, GuideLayout};
use serde_json::{json, Value};

/// ゲーム内のフレーム（60 で 1 秒）→ `時:分:秒:フレーム`（各 2 桁。fxkit の `fmt_timer` と同じ書き方）
pub fn game_text(game: f64) -> String {
    let f = if game.is_finite() { game.floor().max(0.0) as i64 } else { 0 };
    format!("{:02}:{:02}:{:02}:{:02}", f / 216000, f / 3600 % 60, f / 60 % 60, f % 60)
}

/// 点を素材の画素 ↔ 出力の画素に変換する。to は "out"（素材 → 出力）か "src"（出力 → 素材）。
/// `visible` は、出力の点がレイヤーの範囲（clip）の中で、素材の点が素材の中（0〜REF）にあるか。
/// 切り抜きで重ねた HUD（上の敵数・倍速・待機欄）の上は考えない（素材の本体の位置）。
/// (sw, sh) は素材の大きさ
pub fn coords(layout: &GuideLayout, part: &str, size: (u32, u32), points: &[[f64; 2]], to: &str)
              -> Result<(Vec<[f64; 2]>, Vec<bool>), String> {
    if !PART_NAMES.contains(&part) {
        return Err(format!("パート「{part}」はありません（{} のどれか）", PART_NAMES.join(" / ")));
    }
    if to != "out" && to != "src" {
        return Err(format!("to「{to}」は使えません（out か src）"));
    }
    let x = xform(layout, part, size.0, size.1);
    let mut out = Vec::with_capacity(points.len());
    let mut vis = Vec::with_capacity(points.len());
    for (i, p) in points.iter().enumerate() {
        if !p[0].is_finite() || !p[1].is_finite() {
            return Err(format!("points[{i}] が数ではありません"));
        }
        let (src, dst) = if to == "out" {
            ([p[0], p[1]], [x.o[0] as f64 + p[0] * x.k[0], x.o[1] as f64 + p[1] * x.k[1]])
        } else {
            ([(p[0] - x.o[0] as f64) / x.k[0], (p[1] - x.o[1] as f64) / x.k[1]], [p[0], p[1]])
        };
        let in_clip = dst[0] >= x.clip[0] as f64 && dst[0] < x.clip[2] as f64 && dst[1] >= x.clip[1] as f64 && dst[1] < x.clip[3] as f64;
        let in_src = src[0] >= 0.0 && src[0] <= REF_W && src[1] >= 0.0 && src[1] <= REF_H;
        out.push(if to == "out" { dst } else { src });
        vis.push(in_clip && in_src);
    }
    Ok((out, vis))
}

/// `guide_read` の中身。st は `Core::guide_state()`、plan は今の組み立て、src_size は解析した素材の大きさ、
/// clear は自動パートのタイマーの最後の値（クリア時間のフレーム数。`map::clear_time_frames`）
pub fn read_json(g: &Guide, st: &GuideState, plan: Option<&GuidePlan>, src_size: Option<[u32; 2]>, clear: Option<i64>) -> Value {
    let size = src_size.filter(|s| s[0] > 0 && s[1] > 0).unwrap_or([REF_W as u32, REF_H as u32]);

    let parts: Vec<Value> = st
        .plan_parts
        .iter()
        .map(|p| json!({"name": p.name, "label": part_label(&p.name), "t0": p.t0, "t1": p.t1}))
        .collect();

    let anchors: serde_json::Map<String, Value> = ANCHOR_NAMES
        .iter()
        .map(|name| {
            let a = g.anchors.get(*name);
            let t = plan.and_then(|p| p.anchor_src_of(name)).or_else(|| a.and_then(|a| a.value()));
            (name.to_string(), json!({"t": t, "auto": a.and_then(|a| a.auto), "out": st.anchor_out.get(*name)}))
        })
        .collect();

    let cuts: Vec<Value> = g
        .cuts
        .iter()
        .enumerate()
        .map(|(i, c)| json!({"src_in": c.src_in, "src_out": c.src_out, "fade": c.fade, "out": st.cuts_out.get(i)}))
        .collect();

    let have_out = st.ops_out.len() == g.ops.len();
    let ops: Vec<Value> = g
        .ops
        .iter()
        .enumerate()
        .map(|(i, o)| {
            let u = have_out.then(|| &st.ops_out[i]);
            json!({
                "id": o.id, "t": o.t, "t_out": u.and_then(|u| u.t_out), "t_now": u.and_then(|u| u.t_now), "now_lead": o.now_lead, "frame": u.map(|u| u.frame),
                "cost": u.and_then(|u| u.cost), "game": u.and_then(|u| u.game),
                "game_text": u.and_then(|u| u.game).map(game_text),
                "in_cut": u.map(|u| u.in_cut),
                "kind": o.kind, "op": o.op, "where": o.r#where, "info": o.info, "precise": o.precise, "cue_text": o.cue_text, "note": o.note,
            })
        })
        .collect();

    let f = &g.formation;
    let (used, _) = valid_used(f);
    let formation = json!({"slots": f.effective_slots(), "used": used, "notes": f.notes});

    let slots = plan.map(|p| sub_slots(g, p, &st.ops_out).0).unwrap_or_default();
    let subs: Vec<Value> = g
        .subs
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let slot = slots.iter().find(|x| x.index == i);
            json!({
                "src": s.src, "src_in": s.src_in, "src_out": s.src_out, "at_op": s.at_op, "offset": s.offset, "label": s.label,
                "t0": slot.map(|x| x.t0), "t1": slot.map(|x| x.t1),
            })
        })
        .collect();

    let mut geo = serde_json::Map::new();
    for part in PART_NAMES {
        let x = xform(&g.layout, part, size[0], size[1]);
        geo.insert(part.into(), json!({"k": x.k, "o": x.o, "clip": x.clip}));
    }

    json!({
        "src": g.src,
        "src_size": src_size,
        "fps": plan.map(|p| p.fps),
        "parts": parts,
        "anchors": anchors,
        "fixed": st.fixed_effective,
        "cuts": cuts,
        "ops": ops,
        "formation": formation,
        "subs": subs,
        "geo": geo,
        // 自動パートのタイマーが止まって見えている値（サムネなどに使うクリア時間）。読めなければ null
        "clear_time": clear.map(|f| json!({"frames": f, "text": game_text(f as f64)})),
        "analysis": {"exists": st.analysis.exists, "battles": st.analysis.battles, "warnings": st.analysis.warnings},
        "warnings": st.warnings,
        "error": st.error,
        "claude_undo": st.claude_undo,
        "rev": st.rev,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::guide::layer::geometry;
    use crate::guide::plan::plan;
    use ak_proto::guide::{ClaudeUndo, OpOut, PlanPart};
    use ak_proto::{Anchor, GuideFixed, GuideOp};

    #[test]
    fn game_text_matches_fmt_timer_samples() {
        // 見本 tests/fixtures/guide_samples.json（kit の fmt_timer のテストも同じ表を読む）
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/guide_samples.json");
        let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        let rows = v["fmt_timer"].as_array().unwrap();
        assert!(!rows.is_empty());
        for r in rows {
            assert_eq!(game_text(r[0].as_f64().unwrap()), r[1].as_str().unwrap(), "{r}");
        }
        // 負・数でないものは 0
        assert_eq!(game_text(-5.0), "00:00:00:00");
        assert_eq!(game_text(f64::NAN), "00:00:00:00");
    }

    #[test]
    fn coords_use_geometry_and_round_trip() {
        let l = GuideLayout::default();
        // 手動パート: 素材の中心 (1376, 1032) は、レイヤー（75% で左上）の中心 (720, 405)
        let (o, vis) = coords(&l, "manual", (2752, 2064), &[[1376.0, 1032.0]], "out").unwrap();
        assert!((o[0][0] - 720.0).abs() < 1.0 && (o[0][1] - 405.0).abs() < 1.0, "{o:?}");
        assert!(vis[0]);
        // geometry（倍率 1）の式と同じ: 出力 = base + 素材 × (read / REF)
        let g = geometry(&l, &Default::default(), "manual", 2752, 2064, 1.0);
        let want = g.base.0 as f64 + 100.0 * g.read.0 as f64 / REF_W;
        let (o, _) = coords(&l, "manual", (2752, 2064), &[[100.0, 200.0]], "out").unwrap();
        assert!((o[0][0] - want).abs() < 1e-9);
        // 逆変換で戻る
        let pts = [[100.0, 200.0], [1376.0, 1032.0], [2700.0, 2000.0]];
        let (out, _) = coords(&l, "manual", (2752, 2064), &pts, "out").unwrap();
        let (back, _) = coords(&l, "manual", (2752, 2064), &out, "src").unwrap();
        for (a, b) in pts.iter().zip(&back) {
            assert!((a[0] - b[0]).abs() < 1e-6 && (a[1] - b[1]).abs() < 1e-6, "{a:?} {b:?}");
        }
    }

    #[test]
    fn coords_visible_follows_clip() {
        let l = GuideLayout::default();
        // 編成パートは素材を 70% で中央に置く（1926x1445）ので上下が切れる。素材の上端 (y=0) は出力の外
        let (o, vis) = coords(&l, "formation", (2752, 2064), &[[1376.0, 0.0], [1376.0, 1032.0], [1376.0, 2064.0]], "out").unwrap();
        assert!(o[0][1] < 0.0 && !vis[0]);
        assert!(vis[1]);
        assert!(o[2][1] > 1080.0 && !vis[2]);
        // 手動パートは左上の 75% の中に収まる（clip は 0,0,1440,810）
        let (_, vis) = coords(&l, "manual", (2752, 2064), &[[500.0, 1000.0]], "out").unwrap();
        assert!(vis[0]);
        // src 指定で、出力の外（x=1900）は手動パートの clip の外
        let (_, vis) = coords(&l, "manual", (2752, 2064), &[[1900.0, 400.0], [500.0, 400.0]], "src").unwrap();
        assert_eq!(vis, vec![false, true]);
        // 素材の外の点も見えない
        let (_, vis) = coords(&l, "formation", (2752, 2064), &[[3000.0, 1000.0]], "out").unwrap();
        assert!(!vis[0]);
    }

    #[test]
    fn coords_errors_are_japanese() {
        let l = GuideLayout::default();
        assert!(coords(&l, "intro", (2752, 2064), &[], "out").unwrap_err().contains("パート「intro」"));
        assert!(coords(&l, "manual", (2752, 2064), &[], "x").unwrap_err().contains("out か src"));
        assert!(coords(&l, "manual", (2752, 2064), &[[f64::NAN, 0.0]], "out").unwrap_err().contains("points[0]"));
    }

    #[test]
    fn read_json_has_the_documented_shape() {
        let mut g = Guide { src: "A.MP4".into(), ..Default::default() };
        for (k, t) in [("formation_dark", 5.0), ("manual_cost0", 10.0), ("manual_end", 100.0), ("auto_cost0", 200.0), ("auto_end", 300.0)] {
            g.anchors.insert(k.into(), Anchor { t: Some(t), auto: Some(t + 0.001) });
        }
        g.ops = vec![GuideOp { id: "op1".into(), t: 20.0, kind: "deploy".into(), op: "ウタゲ".into(), precise: true, ..Default::default() }];
        let pts: Vec<f64> = (0..24000).map(|i| i as f64 / 60.0).collect();
        let p = plan(&g, &GuideFixed::default(), &pts).unwrap();
        let st = GuideState {
            guide: Some(g.clone()),
            plan_parts: p.parts.clone(),
            ops_out: vec![OpOut { id: "op1".into(), t_out: Some(30.0), t_now: Some(29.0), frame: 1200, cost: Some(12), game: Some(5141.0), in_cut: false }],
            anchor_out: p.anchor_out.iter().cloned().collect(),
            cuts_out: vec![],
            rev: 7,
            claude_undo: Some(ClaudeUndo { id: 1, at: "2026-10-21T21:03:00".into(), reason: "r".into() }),
            ..Default::default()
        };
        let v = read_json(&g, &st, Some(&p), Some([2752, 2064]), Some(5141));
        assert_eq!(v["src"], "A.MP4");
        assert_eq!(v["src_size"], json!([2752, 2064]));
        assert_eq!(v["fps"], 60.0);
        assert_eq!(v["parts"][1]["name"], "manual");
        assert_eq!(v["parts"][1]["label"], "手動");
        assert_eq!(v["anchors"]["manual_cost0"]["t"], 10.0);
        assert_eq!(v["anchors"]["manual_cost0"]["auto"], 10.001);
        assert!(v["anchors"]["manual_cost0"]["out"].is_number());
        assert_eq!(v["ops"][0]["in_cut"], false);
        assert_eq!(v["ops"][0]["game_text"], "00:01:25:41");
        assert_eq!(v["ops"][0]["t_out"], 30.0);
        assert_eq!(v["ops"][0]["t_now"], 29.0);
        assert!(v["ops"][0]["now_lead"].is_null());
        assert_eq!(v["ops"][0]["where"], "");
        assert_eq!(v["ops"][0]["info"], "");
        assert_eq!(v["formation"]["slots"].as_array().unwrap().len(), 12);
        assert_eq!(v["formation"]["used"].as_array().unwrap().len(), 12);
        assert_eq!(v["geo"]["manual"]["k"].as_array().unwrap().len(), 2);
        assert_eq!(v["rev"], 7);
        assert_eq!(v["claude_undo"]["reason"], "r");
        // クリア時間（タイマーの最後の値）
        assert_eq!(v["clear_time"], json!({"frames": 5141, "text": "00:01:25:41"}));
        // 画素の巨大な配列（コマごとの src・game・timer・pts）は入れない
        assert!(v.get("pts").is_none() && v.get("src_frames").is_none() && v.get("game").is_none() && v.get("timer").is_none());
        // 組み立てが無いときは、時刻・コマは null（落ちない）
        let st = GuideState { guide: Some(g.clone()), plan_parts: vec![PlanPart { name: "manual".into(), t0: 0.0, t1: 1.0 }], ..Default::default() };
        let v = read_json(&g, &st, None, None, None);
        assert!(v["ops"][0]["in_cut"].is_null() && v["clear_time"].is_null());
        assert!(v["ops"][0]["t_out"].is_null() && v["ops"][0]["frame"].is_null() && v["fps"].is_null() && v["src_size"].is_null());
    }
}
