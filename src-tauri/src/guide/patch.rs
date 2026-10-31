//! Claude（MCP の `guide_update`）が guide.json に当てる直し（純粋な関数）。
//!
//! 直せるのは操作（足す・直す・消す）・編成（使う枠・補足）・補足素材の小窓（`subs`）だけ。
//! アンカー・カット・固定値・重ね方は画面でユーザーが直す。適用は全部か無しか（1 つでもエラーなら何も変えない）。

use super::plan::{next_op_id, part_label, snap_to_frame, GuidePlan};
use ak_proto::guide::OP_KINDS;
use ak_proto::http::{GuidePatch, OpAdd, OpUpdate};
use ak_proto::{Cut, Guide, GuideOp};

/// 操作の種類の和名（操作の文・画面と同じ）
pub fn kind_label(kind: &str) -> &str {
    match kind {
        "deploy" => "配置",
        "skill" => "スキル",
        "retreat" => "撤退",
        "trust" => "信頼枠",
        other => other,
    }
}

fn check_kind(kind: &str) -> Result<(), String> {
    if OP_KINDS.contains(&kind) {
        Ok(())
    } else {
        Err(format!("kind「{kind}」は使えません（{} のどれか）", OP_KINDS.join(" / ")))
    }
}

/// 出力の時刻 t_out（手動パートの中）が見せている素材の時刻。手動パートの外・範囲外は Err（和文。`add_op` と同じ判断）
pub fn manual_src_of(plan: &GuidePlan, t_out: f64) -> Result<f64, String> {
    let Some((_, t_src, part)) = plan.src_at(t_out) else {
        return Err("出力の時刻が動画の外です。手動パートの中の時刻にしてください".into());
    };
    if part != "manual" {
        return Err(format!("操作は手動パートの中に置いてください（出力の {t_out:.2} 秒は「{}」パートです）", part_label(part)));
    }
    Ok(t_src)
}

/// 素材の時刻 t（秒）がカットの中か
pub fn in_cut(cuts: &[Cut], t: f64) -> bool {
    cuts.iter().any(|c| c.src_in <= t + 1e-9 && t < c.src_out - 1e-9)
}

/// 時刻の指定（t・t_out・frame のどれか 1 つ）を素材の時刻にする。1 つも無ければ None、2 つ以上は Err。
/// 素材のコマの時刻に合わせ、カットの中・手動パートの外は Err（足した操作が出力に出ないので）。
/// 組み立てが無いときは、手動パートの外かは分からないので、負の時刻だけ断る
pub fn resolve_time(t: Option<f64>, t_out: Option<f64>, frame: Option<u32>, pts: &[f64], plan: Option<&GuidePlan>, cuts: &[Cut])
                    -> Result<Option<f64>, String> {
    let n = t.is_some() as u8 + t_out.is_some() as u8 + frame.is_some() as u8;
    if n > 1 {
        return Err("時刻は t・t_out・frame のどれか 1 つだけ渡してください".into());
    }
    let raw = if let Some(t) = t {
        if !t.is_finite() {
            return Err("t が数ではありません".into());
        }
        t
    } else if let Some(f) = frame {
        match pts.get(f as usize) {
            Some(&v) => v,
            None if pts.is_empty() => return Err("素材の解析がまだなので frame は使えません。t か t_out を使ってください".into()),
            None => return Err(format!("frame {f} は素材のコマの数（{}）の外です", pts.len())),
        }
    } else if let Some(to) = t_out {
        if !to.is_finite() {
            return Err("t_out が数ではありません".into());
        }
        let Some(plan) = plan else {
            return Err("パートを組み立てられていないので t_out は使えません。素材を解析して、アンカーを確かめてください".into());
        };
        manual_src_of(plan, to)?
    } else {
        return Ok(None);
    };
    // コマに合わせる前に見る（合わせると最初のコマに丸められて、負の時刻が通ってしまう）
    if raw < 0.0 {
        return Err(format!("素材の {raw:.3} 秒は動画の外です（0 秒以上にしてください）"));
    }
    let t = if pts.is_empty() { raw } else { snap_to_frame(pts, raw) };
    if in_cut(cuts, t) {
        return Err(format!("素材の {t:.3} 秒はカットの中です（出力に出ません）"));
    }
    if let Some(plan) = plan {
        if plan.manual_out_of(t).is_none() {
            return Err(match plan.part_of_src(t) {
                Some(part) => format!("操作は手動パートの中に置いてください（素材の {t:.3} 秒は「{}」パートです）", part_label(part)),
                None => format!("操作は手動パートの中に置いてください（素材の {t:.3} 秒は出力に出ません）"),
            });
        }
    }
    Ok(Some(t))
}

/// 操作の説明（「ウタゲ 配置」）
fn op_title(o: &GuideOp) -> String {
    format!("{} {}", o.op, kind_label(&o.kind)).trim().to_string()
}

fn apply_add(g: &mut Guide, a: &OpAdd, next_n: &mut u32, pts: &[f64], plan: Option<&GuidePlan>) -> Result<String, String> {
    check_kind(&a.kind)?;
    let t = resolve_time(a.t, a.t_out, a.frame, pts, plan, &g.cuts)?.ok_or("操作の時刻（t・t_out・frame のどれか）が要ります")?;
    *next_n += 1;
    let o = GuideOp {
        id: format!("op{next_n}"),
        t,
        kind: a.kind.clone(),
        op: a.op.clone(),
        r#where: a.r#where.clone(),
        info: a.info.clone(),
        precise: a.precise,
        now_lead: a.now_lead,
        cue_text: a.cue_text.clone(),
        note: a.note.clone(),
        ..Default::default()
    };
    let msg = format!("操作 {}（{}）を足しました", o.id, op_title(&o));
    g.ops.push(o);
    Ok(msg)
}

fn apply_update(g: &mut Guide, u: &OpUpdate, pts: &[f64], plan: Option<&GuidePlan>) -> Result<Option<String>, String> {
    let Some(i) = g.ops.iter().position(|o| o.id == u.id) else {
        return Err(format!("操作「{}」がありません（guide_read の ops[].id を使ってください）", u.id));
    };
    let new_t = resolve_time(u.t, u.t_out, u.frame, pts, plan, &g.cuts)?;
    if let Some(k) = &u.kind {
        check_kind(k)?;
    }
    let o = &mut g.ops[i];
    let mut fields: Vec<&str> = vec![];
    macro_rules! set {
        ($field:ident, $name:expr, $new:expr) => {
            if let Some(v) = $new {
                if o.$field != *v {
                    o.$field = v.clone();
                    fields.push($name);
                }
            }
        };
    }
    if let Some(t) = new_t {
        if (o.t - t).abs() > 1e-9 {
            o.t = t;
            fields.push("t");
        }
    }
    set!(kind, "kind", u.kind.as_ref());
    set!(op, "op", u.op.as_ref());
    set!(r#where, "where", u.r#where.as_ref());
    set!(info, "info", u.info.as_ref());
    set!(precise, "precise", u.precise.as_ref());
    // now_lead: 数で個別値、null で汎用値に戻す（None の Option は「触らない」）
    if let Some(v) = u.now_lead {
        if o.now_lead != v {
            o.now_lead = v;
            fields.push(if v.is_some() { "now_lead" } else { "now_lead（汎用値に戻す）" });
        }
    }
    set!(cue_text, "cue_text", u.cue_text.as_ref());
    set!(note, "note", u.note.as_ref());
    Ok((!fields.is_empty()).then(|| format!("{} の {} を変えました", u.id, fields.join("・"))))
}

/// patch を g に当てた新しい guide と、変えたことの和文の一覧。何も変わらなければ一覧は空（保存しなくてよい）。
/// pts・plan は時刻の指定（frame・t_out）に使う（解析・組み立てが無ければ、それらを使う指定は Err）
pub fn apply_patch(g: &Guide, plan: Option<&GuidePlan>, pts: &[f64], p: &GuidePatch) -> Result<(Guide, Vec<String>), String> {
    if p.reason.trim().is_empty() {
        return Err("reason（直す理由）を書いてください。画面の「Claude の変更を戻す」に出ます".into());
    }
    let mut new = g.clone();
    let mut changed = vec![];

    if let Some(ops) = &p.ops {
        // id は今ある最大の番号から振る（消しても詰めない。同じ patch で消した番号も使い回さない）
        let mut next_n: u32 = next_op_id(&g.ops).trim_start_matches("op").parse::<u32>().map(|n| n - 1).unwrap_or(0);
        for (i, u) in ops.update.iter().enumerate() {
            if let Some(m) = apply_update(&mut new, u, pts, plan).map_err(|e| format!("ops.update[{i}]: {e}"))? {
                changed.push(m);
            }
        }
        for id in &ops.remove {
            let Some(i) = new.ops.iter().position(|o| o.id == *id) else {
                return Err(format!("ops.remove: 操作「{id}」がありません（guide_read の ops[].id を使ってください）"));
            };
            let o = new.ops.remove(i);
            changed.push(format!("操作 {}（{}）を消しました", o.id, op_title(&o)));
        }
        for (i, a) in ops.add.iter().enumerate() {
            let m = apply_add(&mut new, a, &mut next_n, pts, plan).map_err(|e| format!("ops.add[{i}]: {e}"))?;
            changed.push(m);
        }
    }

    if let Some(f) = &p.formation {
        let n = new.formation.effective_slots().len();
        if let Some(used) = &f.used {
            let mut v: Vec<usize> = used.clone().unwrap_or_default();
            if let Some(&bad) = v.iter().find(|&&i| i >= n) {
                return Err(format!("formation.used: 枠 {bad} はありません（0〜{} の番号）", n - 1));
            }
            v.sort_unstable();
            v.dedup();
            // 全部の枠を挙げたら「全部使う」と同じ
            if v.len() == n {
                v.clear();
            }
            if v != new.formation.used {
                changed.push(if v.is_empty() {
                    "編成の使う枠を全部にしました".to_string()
                } else {
                    format!("編成の使う枠を {} にしました", v.iter().map(|i| i.to_string()).collect::<Vec<_>>().join("・"))
                });
                new.formation.used = v;
            }
        }
        for (k, text) in &f.notes {
            let idx: usize = k.trim().parse().map_err(|_| format!("formation.notes: 枠の番号「{k}」が数ではありません"))?;
            if idx >= n {
                return Err(format!("formation.notes: 枠 {idx} はありません（0〜{} の番号）", n - 1));
            }
            let key = idx.to_string();
            match text.as_deref().map(str::trim).filter(|t| !t.is_empty()) {
                Some(t) => {
                    if new.formation.notes.get(&key).map(String::as_str) != Some(t) {
                        new.formation.notes.insert(key, t.to_string());
                        changed.push(format!("枠 {idx} の補足を「{t}」にしました"));
                    }
                }
                None => {
                    if new.formation.notes.remove(&key).is_some() {
                        changed.push(format!("枠 {idx} の補足を消しました"));
                    }
                }
            }
        }
    }

    if let Some(subs) = &p.subs {
        for (i, s) in subs.iter().enumerate() {
            if !(s.src_in.is_finite() && s.src_out.is_finite() && s.offset.is_finite()) {
                return Err(format!("subs[{i}]: 秒が数ではありません"));
            }
        }
        if *subs != new.subs {
            changed.push(if subs.is_empty() {
                "補足素材の小窓を全部外しました".to_string()
            } else {
                format!("補足素材の小窓を {} 件に置き換えました", subs.len())
            });
            new.subs = subs.clone();
        }
    }
    Ok((new, changed))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::guide::plan::plan;
    use ak_proto::http::{FormationPatch, OpsPatch};
    use ak_proto::{Anchor, GuideFixed, GuideSub};

    /// 60fps の等間隔 pts（0〜400 秒）
    fn pts() -> Vec<f64> {
        (0..24000).map(|i| i as f64 / 60.0).collect()
    }

    fn anchors(g: &mut Guide) {
        for (k, t) in [("formation_dark", 5.0), ("manual_cost0", 10.0), ("manual_end", 100.0), ("auto_cost0", 200.0), ("auto_end", 300.0)] {
            g.anchors.insert(k.into(), Anchor { t: Some(t), auto: None });
        }
    }

    fn base() -> Guide {
        let mut g = Guide { src: "A.MP4".into(), ..Default::default() };
        anchors(&mut g);
        g.ops = vec![
            GuideOp { id: "op1".into(), t: 20.0, kind: "deploy".into(), op: "ウタゲ".into(), ..Default::default() },
            GuideOp { id: "op2".into(), t: 30.0, kind: "trust".into(), ..Default::default() },
        ];
        g
    }

    fn pl(g: &Guide) -> GuidePlan {
        plan(g, &GuideFixed::default(), &pts()).unwrap()
    }

    fn patch(ops: OpsPatch) -> GuidePatch {
        GuidePatch { reason: "テスト".into(), ops: Some(ops), ..Default::default() }
    }

    fn add_at(t: Option<f64>, t_out: Option<f64>, frame: Option<u32>) -> OpAdd {
        OpAdd { t, t_out, frame, kind: "deploy".into(), op: "X".into(), ..Default::default() }
    }

    #[test]
    fn add_accepts_t_t_out_and_frame() {
        let g = base();
        let p = pl(&g);
        let ps = pts();
        let ops = OpsPatch {
            add: vec![add_at(Some(40.0), None, None), add_at(None, None, Some(60 * 50)), add_at(None, Some(p.parts[1].t0 + 10.0), None)],
            ..Default::default()
        };
        let (n, ch) = apply_patch(&g, Some(&p), &ps, &patch(ops)).unwrap();
        assert_eq!(n.ops.len(), 5);
        let by = |id: &str| n.ops.iter().find(|o| o.id == id).unwrap();
        assert_eq!(by("op3").t, 40.0);
        assert_eq!(by("op4").t, 50.0);
        // 手動パートの頭から 10 秒 = 素材の manual_cost0 − manual_lead (10 − 2) + 10 秒
        assert!((by("op5").t - 18.0).abs() < 0.02, "{}", by("op5").t);
        assert_eq!(ch[0], "操作 op3（X 配置）を足しました");
        assert_eq!(ch.len(), 3);
    }

    #[test]
    fn two_time_specs_is_an_error() {
        let g = base();
        let ops = OpsPatch { add: vec![add_at(Some(40.0), None, Some(3))], ..Default::default() };
        let e = apply_patch(&g, None, &pts(), &patch(ops)).unwrap_err();
        assert!(e.contains("どれか 1 つだけ") && e.starts_with("ops.add[0]"), "{e}");
        // add は時刻が必須
        let ops = OpsPatch { add: vec![add_at(None, None, None)], ..Default::default() };
        assert!(apply_patch(&g, None, &pts(), &patch(ops)).unwrap_err().contains("時刻"));
        // update は 2 つ渡すとエラー・無ければ時刻は変えない
        let u = OpUpdate { id: "op1".into(), t: Some(1.0), frame: Some(2), ..Default::default() };
        assert!(apply_patch(&g, None, &pts(), &patch(OpsPatch { update: vec![u], ..Default::default() })).is_err());
    }

    #[test]
    fn t_out_outside_manual_or_without_plan_is_an_error() {
        let g = base();
        let p = pl(&g);
        // 編成パートの中（0.5 秒）
        let ops = OpsPatch { add: vec![add_at(None, Some(0.5), None)], ..Default::default() };
        let e = apply_patch(&g, Some(&p), &pts(), &patch(ops)).unwrap_err();
        assert!(e.contains("手動パートの中") && e.contains("編成"), "{e}");
        // 自動パートの中
        let t_auto = p.parts[2].t0 + 1.0;
        let ops = OpsPatch { add: vec![add_at(None, Some(t_auto), None)], ..Default::default() };
        assert!(apply_patch(&g, Some(&p), &pts(), &patch(ops)).unwrap_err().contains("自動"));
        // 動画の外
        let ops = OpsPatch { add: vec![add_at(None, Some(9999.0), None)], ..Default::default() };
        assert!(apply_patch(&g, Some(&p), &pts(), &patch(ops)).unwrap_err().contains("動画の外"));
        // 組み立てが無い
        let ops = OpsPatch { add: vec![add_at(None, Some(30.0), None)], ..Default::default() };
        assert!(apply_patch(&g, None, &pts(), &patch(ops)).unwrap_err().contains("組み立てられていない"));
    }

    /// 組み立てがあるとき、t・frame も手動パートの中だけ（編成・自動・動画の外は、どのパートかを添えて断る）。組み立てが無ければ負の時刻だけ断る
    #[test]
    fn t_and_frame_outside_manual_are_errors() {
        let g = base();
        let p = pl(&g);
        let ps = pts();
        // 編成パート（暗転の前 = 素材の 2 秒）・自動パート（素材の 250 秒）・リザルト後の動画の端
        for a in [add_at(Some(2.0), None, None), add_at(None, None, Some(60 * 250)), add_at(Some(250.0), None, None)] {
            let e = apply_patch(&g, Some(&p), &ps, &patch(OpsPatch { add: vec![a], ..Default::default() })).unwrap_err();
            assert!(e.starts_with("ops.add[0]") && e.contains("手動パートの中"), "{e}");
        }
        let e = apply_patch(&g, Some(&p), &ps, &patch(OpsPatch { add: vec![add_at(Some(250.0), None, None)], ..Default::default() })).unwrap_err();
        assert!(e.contains("「自動」パート"), "{e}");
        let e = apply_patch(&g, Some(&p), &ps, &patch(OpsPatch { add: vec![add_at(Some(2.0), None, None)], ..Default::default() })).unwrap_err();
        assert!(e.contains("「編成」パート"), "{e}");
        // update も同じ
        let u = OpUpdate { id: "op1".into(), t: Some(250.0), ..Default::default() };
        assert!(apply_patch(&g, Some(&p), &ps, &patch(OpsPatch { update: vec![u], ..Default::default() })).unwrap_err().contains("手動パートの中"));
        // 手動パートの中は通る
        assert!(apply_patch(&g, Some(&p), &ps, &patch(OpsPatch { add: vec![add_at(Some(50.0), None, None)], ..Default::default() })).is_ok());
        // 組み立てが無い: 負・数でないものは断り、ほかは通す
        let e = apply_patch(&g, None, &ps, &patch(OpsPatch { add: vec![add_at(Some(-1.0), None, None)], ..Default::default() })).unwrap_err();
        assert!(e.contains("動画の外"), "{e}");
        assert!(apply_patch(&g, None, &ps, &patch(OpsPatch { add: vec![add_at(Some(f64::NAN), None, None)], ..Default::default() })).unwrap_err().contains("数ではありません"));
        assert!(apply_patch(&g, None, &ps, &patch(OpsPatch { add: vec![add_at(Some(250.0), None, None)], ..Default::default() })).is_ok());
    }

    #[test]
    fn time_inside_a_cut_is_an_error() {
        let mut g = base();
        g.cuts.push(Cut { src_in: 50.0, src_out: 60.0, fade: 0.25 });
        let p = pl(&g);
        for a in [add_at(Some(55.0), None, None), add_at(None, None, Some(60 * 55))] {
            let e = apply_patch(&g, Some(&p), &pts(), &patch(OpsPatch { add: vec![a], ..Default::default() })).unwrap_err();
            assert!(e.contains("カットの中"), "{e}");
        }
        // 出力の時刻からは、カットの中には来ない（カットの後ろの素材になる）
        let ops = OpsPatch { add: vec![add_at(None, Some(p.parts[1].t0 + 40.0), None)], ..Default::default() };
        let (n, _) = apply_patch(&g, Some(&p), &pts(), &patch(ops)).unwrap();
        assert!(n.ops.iter().all(|o| !in_cut(&n.cuts, o.t)));
    }

    #[test]
    fn update_changes_only_given_fields() {
        let g = base();
        let u = OpUpdate { id: "op1".into(), cue_text: Some("Kill15 +12".into()), precise: Some(true), ..Default::default() };
        let (n, ch) = apply_patch(&g, None, &pts(), &patch(OpsPatch { update: vec![u], ..Default::default() })).unwrap();
        let o = &n.ops[0];
        assert_eq!((o.cue_text.as_str(), o.precise, o.op.as_str(), o.t, o.kind.as_str()), ("Kill15 +12", true, "ウタゲ", 20.0, "deploy"));
        assert_eq!(ch, vec!["op1 の precise・cue_text を変えました".to_string()]);
        // 同じ値なら変えたことにしない
        let u = OpUpdate { id: "op1".into(), op: Some("ウタゲ".into()), ..Default::default() };
        let (_, ch) = apply_patch(&g, None, &pts(), &patch(OpsPatch { update: vec![u], ..Default::default() })).unwrap();
        assert!(ch.is_empty());
        // 時刻の更新
        let u = OpUpdate { id: "op2".into(), frame: Some(60 * 33), ..Default::default() };
        let (n, ch) = apply_patch(&g, None, &pts(), &patch(OpsPatch { update: vec![u], ..Default::default() })).unwrap();
        assert_eq!(n.ops[1].t, 33.0);
        assert_eq!(ch, vec!["op2 の t を変えました".to_string()]);
    }

    #[test]
    fn unknown_ids_and_bad_kind_error_and_nothing_applies() {
        let g = base();
        let u = OpUpdate { id: "op1".into(), note: Some("x".into()), ..Default::default() };
        // 先に正しい直しがあっても、あとの remove が無い id なら全部断る（g は触らない）
        let ops = OpsPatch { update: vec![u], remove: vec!["op99".into()], ..Default::default() };
        let e = apply_patch(&g, None, &pts(), &patch(ops)).unwrap_err();
        assert!(e.contains("op99") && e.contains("ありません"), "{e}");
        assert_eq!(g, base());
        let u = OpUpdate { id: "op9".into(), ..Default::default() };
        assert!(apply_patch(&g, None, &pts(), &patch(OpsPatch { update: vec![u], ..Default::default() })).unwrap_err().contains("op9"));
        let mut a = add_at(Some(40.0), None, None);
        a.kind = "dance".into();
        assert!(apply_patch(&g, None, &pts(), &patch(OpsPatch { add: vec![a], ..Default::default() })).unwrap_err().contains("kind"));
    }

    #[test]
    fn info_is_added_and_updated() {
        let g = base();
        let mut a = add_at(Some(40.0), None, None);
        a.info = "S2 は溜めてから".into();
        let (n, _) = apply_patch(&g, None, &pts(), &patch(OpsPatch { add: vec![a], ..Default::default() })).unwrap();
        assert_eq!(n.ops[2].info, "S2 は溜めてから");
        let u = OpUpdate { id: "op1".into(), info: Some("先に置く".into()), ..Default::default() };
        let (n, ch) = apply_patch(&g, None, &pts(), &patch(OpsPatch { update: vec![u.clone()], ..Default::default() })).unwrap();
        assert_eq!(n.ops[0].info, "先に置く");
        assert_eq!(ch, vec!["op1 の info を変えました".to_string()]);
        // 同じ値なら変更なし
        let (_, ch) = apply_patch(&n, None, &pts(), &patch(OpsPatch { update: vec![u], ..Default::default() })).unwrap();
        assert!(ch.is_empty());
    }

    #[test]
    fn now_lead_is_added_set_and_cleared_by_null() {
        let g = base();
        let mut a = add_at(Some(40.0), None, None);
        a.now_lead = Some(2.0);
        let (n, _) = apply_patch(&g, None, &pts(), &patch(OpsPatch { add: vec![a], ..Default::default() })).unwrap();
        assert_eq!(n.ops[2].now_lead, Some(2.0));
        // 数で個別値を入れる
        let set: OpsPatch = serde_json::from_str(r#"{"update":[{"id":"op1","now_lead":3}]}"#).unwrap();
        let (n, ch) = apply_patch(&g, None, &pts(), &patch(set.clone())).unwrap();
        assert_eq!(n.ops[0].now_lead, Some(3.0));
        assert_eq!(ch, vec!["op1 の now_lead を変えました".to_string()]);

        // 同じ値なら変更なし
        let (_, ch) = apply_patch(&n, None, &pts(), &patch(set)).unwrap();
        assert!(ch.is_empty());
        // 項目が無ければ触らない
        let keep: OpsPatch = serde_json::from_str(r#"{"update":[{"id":"op1","info":"x"}]}"#).unwrap();
        let (k, _) = apply_patch(&n, None, &pts(), &patch(keep)).unwrap();
        assert_eq!(k.ops[0].now_lead, Some(3.0));
        // null で個別値を消す（汎用値に戻る）
        let clear: OpsPatch = serde_json::from_str(r#"{"update":[{"id":"op1","now_lead":null}]}"#).unwrap();
        let (c, ch) = apply_patch(&n, None, &pts(), &patch(clear.clone())).unwrap();
        assert_eq!(c.ops[0].now_lead, None);
        assert_eq!(ch, vec!["op1 の now_lead（汎用値に戻す） を変えました".to_string()]);
        // 元から無ければ変更なし
        let (_, ch) = apply_patch(&c, None, &pts(), &patch(clear)).unwrap();
        assert!(ch.is_empty());
    }

    #[test]
    fn remove_does_not_renumber_and_ids_are_not_reused_in_one_patch() {
        let g = base();
        // 最大の op2 を消して同じ patch で足しても op2 は使い回さない
        let ops = OpsPatch { remove: vec!["op2".into()], add: vec![add_at(Some(40.0), None, None)], ..Default::default() };
        let (n, ch) = apply_patch(&g, None, &pts(), &patch(ops)).unwrap();
        assert_eq!(n.ops.iter().map(|o| o.id.as_str()).collect::<Vec<_>>(), ["op1", "op3"]);
        assert_eq!(ch[0], "操作 op2（信頼枠）を消しました");
    }

    #[test]
    fn formation_used_null_means_all_and_notes_null_removes() {
        let mut g = base();
        g.formation.used = vec![0, 2];
        g.formation.notes.insert("2".into(), "Lv90".into());
        let mut f = FormationPatch::default();
        f.used = Some(None);
        f.notes.insert("2".into(), None);
        f.notes.insert("5".into(), Some("潜在5".into()));
        let (n, ch) = apply_patch(&g, None, &pts(), &GuidePatch { reason: "r".into(), formation: Some(f), ..Default::default() }).unwrap();
        assert!(n.formation.used.is_empty());
        assert_eq!(n.formation.notes.get("5").map(String::as_str), Some("潜在5"));
        assert!(!n.formation.notes.contains_key("2"));
        assert_eq!(ch, vec!["編成の使う枠を全部にしました", "枠 2 の補足を消しました", "枠 5 の補足を「潜在5」にしました"]);
        // 渡さなければ今のまま
        let (n, ch) = apply_patch(&g, None, &pts(), &GuidePatch { reason: "r".into(), formation: Some(FormationPatch::default()), ..Default::default() }).unwrap();
        assert_eq!(n, g);
        assert!(ch.is_empty());
        // 範囲外の枠
        let mut f = FormationPatch::default();
        f.used = Some(Some(vec![0, 12]));
        assert!(apply_patch(&g, None, &pts(), &GuidePatch { reason: "r".into(), formation: Some(f), ..Default::default() }).is_err());
        let mut f = FormationPatch::default();
        f.notes.insert("12".into(), Some("x".into()));
        assert!(apply_patch(&g, None, &pts(), &GuidePatch { reason: "r".into(), formation: Some(f), ..Default::default() }).is_err());
        // 全部の枠を挙げたら全部使うと同じ
        let mut f = FormationPatch::default();
        f.used = Some(Some((0..12).collect()));
        let (n, _) = apply_patch(&g, None, &pts(), &GuidePatch { reason: "r".into(), formation: Some(f), ..Default::default() }).unwrap();
        assert!(n.formation.used.is_empty());
    }

    #[test]
    fn subs_replace_whole_and_empty_clears() {
        let mut g = base();
        g.subs = vec![GuideSub { src: "B.MP4".into(), src_in: 1.0, src_out: 3.0, at_op: "op1".into(), ..Default::default() }];
        let new = vec![
            GuideSub { src: "C.MP4".into(), src_in: 0.0, src_out: 2.0, at_op: "op1".into(), offset: -2.0, ..Default::default() },
            GuideSub { src: "D.MP4".into(), src_in: 0.0, src_out: 2.0, at_op: "op2".into(), ..Default::default() },
        ];
        let (n, ch) = apply_patch(&g, None, &pts(), &GuidePatch { reason: "r".into(), subs: Some(new.clone()), ..Default::default() }).unwrap();
        assert_eq!(n.subs, new);
        assert_eq!(ch, vec!["補足素材の小窓を 2 件に置き換えました"]);
        let (n, ch) = apply_patch(&g, None, &pts(), &GuidePatch { reason: "r".into(), subs: Some(vec![]), ..Default::default() }).unwrap();
        assert!(n.subs.is_empty());
        assert_eq!(ch, vec!["補足素材の小窓を全部外しました"]);
        // 同じなら変わらない
        let (_, ch) = apply_patch(&g, None, &pts(), &GuidePatch { reason: "r".into(), subs: Some(g.subs.clone()), ..Default::default() }).unwrap();
        assert!(ch.is_empty());
    }

    #[test]
    fn reason_is_required() {
        let g = base();
        let e = apply_patch(&g, None, &pts(), &GuidePatch { reason: "  ".into(), ..Default::default() }).unwrap_err();
        assert!(e.contains("reason"), "{e}");
    }

    #[test]
    fn patch_rejects_unknown_fields() {
        let e = serde_json::from_str::<GuidePatch>(r#"{"reason":"r","cuts":[]}"#).unwrap_err().to_string();
        assert!(e.contains("cuts"), "{e}");
        let e = serde_json::from_str::<GuidePatch>(r#"{"reason":"r","ops":{"add":[{"kind":"deploy","t":1,"wher":"x"}]}}"#).unwrap_err().to_string();
        assert!(e.contains("wher"), "{e}");
        // formation.used は null と無いを分ける
        let p: GuidePatch = serde_json::from_str(r#"{"reason":"r","formation":{"used":null}}"#).unwrap();
        assert_eq!(p.formation.unwrap().used, Some(None));
        let p: GuidePatch = serde_json::from_str(r#"{"reason":"r","formation":{}}"#).unwrap();
        assert_eq!(p.formation.unwrap().used, None);
    }

    /// 実際の回（写し）の guide.json・解析で、guide_update の判断と guide_read・guide_coords を確かめる（読むだけ。手で走らせる）:
    /// AK_TEST_GUIDE_DIR=E:/tmp/an_MAIN_17-5_copy/studio cargo test -p ak-studio real_patch_and_read -- --ignored
    #[test]
    #[ignore = "実際の guide.json・解析が要る（AK_TEST_GUIDE_DIR）"]
    fn real_patch_and_read() {
        use crate::guide::read::{coords, read_json};
        let Ok(dir) = std::env::var("AK_TEST_GUIDE_DIR") else { panic!("AK_TEST_GUIDE_DIR（studio フォルダ）を指定してください") };
        let dir = std::path::PathBuf::from(dir);
        let g = crate::guide::read_guide(&dir.join("guide.json")).unwrap().expect("guide.json がある");
        let a = crate::guide::read_analysis(&dir.join("guide_analysis.json")).unwrap().expect("解析がある");
        let p = crate::guide::build_plan(Some(&g), Some(&a), &GuideFixed::default()).unwrap().expect("組み立てられる");
        let man = &p.parts[1];
        // 手動パートの中ほどに足す（t_out）→ 素材の時刻に直って、ops_out で同じ出力の時刻に戻る
        let t_out = (man.t0 + man.t1) / 2.0;
        let ops = OpsPatch { add: vec![add_at(None, Some(t_out), None)], ..Default::default() };
        let (n, ch) = apply_patch(&g, Some(&p), &a.pts, &patch(ops)).unwrap();
        eprintln!("{ch:?}");
        let mut n = n;
        crate::guide::plan::normalize_guide(&mut n, &a.pts).unwrap();
        let outs = crate::guide::plan::ops_out(&n, &p, &a.cost, &a.game);
        let id = ch[0].trim_start_matches("操作 ").split('（').next().unwrap().to_string();
        let i = n.ops.iter().position(|o| o.id == id).unwrap();
        assert!((outs[i].t_out.unwrap() - t_out).abs() < 0.02, "{:?} {t_out}", outs[i].t_out);
        // 編成パートの時刻は断られる
        let ops = OpsPatch { add: vec![add_at(None, Some(1.0), None)], ..Default::default() };
        assert!(apply_patch(&g, Some(&p), &a.pts, &patch(ops)).is_err());
        // guide_read・guide_coords
        let st = ak_proto::guide::GuideState { guide: Some(g.clone()), plan_parts: p.parts.clone(), ..Default::default() };
        let v = read_json(&g, &st, Some(&p), Some(a.size), None);
        eprintln!("parts={} ops={}", v["parts"], v["ops"].as_array().unwrap().len());
        let (o, vis) = coords(&g.layout, "manual", (a.size[0], a.size[1]), &[[1376.0, 1032.0]], "out").unwrap();
        eprintln!("manual (1376,1032) → {o:?} {vis:?}");
        assert!((o[0][0] - 720.0).abs() < 2.0 && (o[0][1] - 405.0).abs() < 2.0);
    }
}
