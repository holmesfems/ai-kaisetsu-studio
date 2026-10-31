//! 声の解析とセリフの並びを Rust が持つ。台本・間隔・解析が変わったら `refresh_voice` で並べ直し、画面に配る。
//! 計算そのものは `voice/` の関数（`analysis::lookup`・`layout::layout`・`layout::infer_gaps`）。ここは持ち物と調停だけ。

use super::Core;
use crate::app::dto::VoiceSnapshot;
use crate::script::Script;
use crate::voice::analysis::{self, Analyses, Analysis, VoiceDict};
use crate::voice::layout::{self, voiced_lines, AnalysisUpdate, LayoutState, VoiceItem};
use anyhow::{Result};
use serde_json::json;
use std::collections::HashMap;
use std::sync::atomic::Ordering;

/// 声の解析・今の並び・解析が一度そろったか・直前に知らせた解析の失敗
#[derive(Default)]
pub struct VoiceState {
    pub analyses: Analyses,
    pub items: Vec<VoiceItem>,
    pub total: f64,
    pub ready: bool,
    /// 直前に知らせた解析の失敗（同じ失敗を何度も出さない）
    last_err: String,
    /// 直前に知らせた辞書の読み込みの失敗（同じ失敗を続けて出さない。読めたら空に戻す）
    dict_err: String,
    /// 回を開くたびに増える。中継を待っていた前の回の並べ直しが、新しい回に書き込まないための印
    gen: u64,
}

impl VoiceState {
    /// 台本と今の解析で並びを作り直す
    pub fn recompute(&mut self, s: &Script, gap: f64, block_gap: f64) {
        let l = layout::layout(s, &self.analyses, gap, block_gap);
        self.items = l.items;
        self.total = l.total;
    }

    pub fn layout_state(&self) -> LayoutState {
        LayoutState { items: self.items.clone(), total: self.total, ready: self.ready }
    }

    pub fn snapshot(&self) -> VoiceSnapshot {
        VoiceSnapshot {
            layout: self.layout_state(),
            analysis: self.analyses.iter().map(|(k, a)| (k.clone(), a.clone())).collect(),
        }
    }

    /// 回を開いたとき: 解析も並びも空にする（辞書は回の外のものなので残す）
    pub fn reset(&mut self) {
        let mut analyses = Analyses::default();
        analyses.set_dict(self.analyses.dict().clone());
        *self = VoiceState { analyses, gen: self.gen + 1, ..Default::default() };
    }
}

impl Core {
    /// セリフの並びの写し
    pub fn layout(&self) -> Vec<VoiceItem> {
        self.vstate.read().items.clone()
    }

    /// セリフの並びを読む（写さずに、ロックを持ったまま見る。持っている間に長い仕事をしない）
    pub fn layout_guard(&self) -> parking_lot::MappedRwLockReadGuard<'_, Vec<VoiceItem>> {
        parking_lot::RwLockReadGuard::map(self.vstate.read(), |v| &v.items)
    }

    /// 画面の初期表示に要る、並びと解析の全部
    pub fn voice_snapshot(&self) -> VoiceSnapshot {
        self.vstate.read().snapshot()
    }

    /// 画面の初期表示のあとで、今の並びと解析を取り直す（`voice_state`）
    pub fn voice_state(&self) -> VoiceSnapshot {
        self.voice_snapshot()
    }

    /// 中継が起きた（か、起こし直した）: 解析を受け取り直して並べ直す
    /// root は中継を起こした回。そのあいだに別の回を開いていたら何もしない
    pub fn voice_relay_up(&self, root: &std::path::Path) {
        if self.root().as_deref() != Some(root) {
            return;
        }
        self.voice_up.store(true, Ordering::SeqCst);
        self.refresh_voice(true);
    }

    /// 声のエンジン（か VOICEVOX の URL）を変えた: 中継を止めて起こし直し、前のエンジンの解析を捨てて長さを取り直す。
    /// 回を開いていなければ、次に回を開くときの起動に任せる。結果は回を開いたときと同じ「voice」の知らせで配る
    pub fn restart_voice_for_engine(&self) {
        self.voice.stop();
        let Some(root) = self.root() else { return };
        // 前のエンジンの長さ・音素で並べたままにしない（sig はエンジンを見ないので、捨てないと取り直されない）
        self.vstate.write().reset();
        self.voice_up.store(false, Ordering::SeqCst);
        self.refresh_voice(false);
        match self.voice.ensure(&self.settings.get(), &root) {
            Ok(p) => {
                self.emit("voice", serde_json::json!({"ok": true, "info": p}));
                self.voice_relay_up(&root);
            }
            Err(e) => {
                let a = crate::app::error::api_error(&e);
                self.emit("voice", serde_json::json!({"ok": false, "error": a.message, "code": a.code, "detail": a.detail}));
            }
        }
    }

    /// 台本・間隔・解析が変わったので、並びを作り直して画面に配る。並べ直しは 1 つずつ（`voice_lock`）。
    /// lookup が true なら、今の手直しで作った音声がまだ手元に無い行を中継に問い合わせる。
    /// 中継への問い合わせの間、`st` と `vstate` のロックは持たない（台本の保存を止めない）
    pub fn refresh_voice(&self, lookup: bool) {
        let _one = self.voice_lock.lock();
        // 1. 台本と間隔を写す
        let (script, mut gap, mut block_gap, inferred, gen) = {
            let st = self.st.read();
            let Some(ps) = st.as_ref() else { return };
            let doc = &ps.project.doc;
            // 回を開くときは st を持ったまま gen を進める（open_project）ので、台本と gen は同じ回のもの
            let gen = self.vstate.read().gen;
            (ps.script.as_ref().map(|s| s.script.clone()).unwrap_or_default(), doc.gap, doc.block_gap, doc.gaps_inferred, gen)
        };

        // 2. 解析
        let mut changed: Vec<String> = vec![];
        if lookup {
            self.reload_voice_dict();
            let up = self.voice_up.load(Ordering::SeqCst);
            // まだそろっていない（回を開いた・中継が起きた）ときの問い合わせだけ、音声の無い行の数を知らせる。
            // 保存のたびの問い合わせで知らせると、打ったばかりの行（まだ作っていない）のたびに出てしまう
            let first = !self.vstate.read().ready;
            let mut work = self.vstate.read().analyses.clone();
            // 問い合わせる行が無ければ、中継が起きていなくても「そろった」
            let all_fresh = voiced_lines(&script).iter().all(|x| work.fresh(&script, x.l));
            if up || all_fresh {
                match analysis::lookup(&self.voice, &script, &mut work) {
                    Ok(r) => {
                        {
                            let mut v = self.vstate.write();
                            if v.gen != gen {
                                return; // 回が変わった
                            }
                            v.analyses.merge_from(&work, &r.changed);
                            v.ready = true;
                            v.last_err.clear();
                        }
                        changed = r.changed;
                        if first && r.missing > 0 {
                            self.toast_error(&format!("{} 行の音声の長さを声のエンジンから受け取れませんでした", r.missing), 9000);
                        }
                        if r.old_server {
                            self.toast_error(analysis::OLD_SERVER, 12000);
                        }
                    }
                    Err(e) => {
                        // 中継がまだ起きていない（起動の知らせが来たら読み直す）ときは黙る。起きているのに失敗したら知らせる
                        let msg = format!("{e:#}");
                        let same = {
                            let mut v = self.vstate.write();
                            if v.gen != gen {
                                return;
                            }
                            std::mem::replace(&mut v.last_err, msg.clone()) == msg
                        };
                        if up && !same {
                            self.toast_err("声のエンジンで台本を解析できませんでした", &e, 9000);
                        }
                    }
                }
            }
        }

        // 3. 間隔の見積もり（初めて開いた回。台本エディタの間隔の設定は回のファイルに残っていないため）
        let (ready, cur_gen) = {
            let v = self.vstate.read();
            (v.ready, v.gen)
        };
        if cur_gen != gen {
            return;
        }
        if ready && !inferred {
            let g = layout::infer_gaps(&script, &self.vstate.read().analyses);
            // 開始は書いてあるのに音声の長さがまだ 1 つも分からないときは、見積もりを後回しにする
            let any_start = voiced_lines(&script).iter().any(|x| x.l.start.is_some());
            if g.samples > 0 || !any_start {
                match self.apply_inferred_gaps(g.gap, g.block_gap, gen) {
                    Ok(Some(((g0, b0), (g1, b1)))) => {
                        gap = g1;
                        block_gap = b1;
                        if g.samples > 0 {
                            let mut parts = vec![];
                            if g.gap.is_some() && (g1 - g0).abs() > 0.001 {
                                parts.push(format!("セリフ {g0} → {g1} 秒"));
                            }
                            if g.block_gap.is_some() && (b1 - b0).abs() > 0.001 {
                                parts.push(format!("ブロック {b0} → {b1} 秒"));
                            }
                            if !parts.is_empty() {
                                self.emit(
                                    "toast",
                                    json!({"text": format!("間隔を台本に合わせました（{}）", parts.join("、")), "kind": "info", "ms": 7000}),
                                );
                            }
                        }
                    }
                    Ok(None) => return, // 回が変わった（か、閉じた）
                    Err(e) => self.toast_err("間隔を保存できませんでした", &e, 9000),
                }
            }
        }

        // 4. 並べて配る
        let (state, entries) = {
            let mut v = self.vstate.write();
            if v.gen != gen {
                return;
            }
            v.recompute(&script, gap, block_gap);
            let entries: HashMap<String, Analysis> =
                changed.iter().filter_map(|k| v.analyses.get(k).map(|a| (k.clone(), a.clone()))).collect();
            (v.layout_state(), entries)
        };
        if !entries.is_empty() {
            self.emit("analysis", AnalysisUpdate { entries });
        }
        self.emit("layout", state);
    }

    /// 読み上げ辞書を読み直して、解析の署名の元にする。読めなければ前の辞書のまま知らせる（同じ失敗は続けて出さない）。
    /// 辞書は小さいので毎回読む（見張りが取りこぼしても、次の保存で直る）
    pub fn reload_voice_dict(&self) -> VoiceDict {
        match VoiceDict::load(&crate::settings::voice_dict_path()) {
            Ok(d) => {
                let mut v = self.vstate.write();
                v.analyses.set_dict(d);
                v.dict_err.clear();
                v.analyses.dict().clone()
            }
            Err(e) => {
                let msg = format!("{e:#}");
                let (same, prev) = {
                    let mut v = self.vstate.write();
                    (std::mem::replace(&mut v.dict_err, msg.clone()) == msg, v.analyses.dict().clone())
                };
                if !same {
                    self.toast_err("読み上げ辞書を読み込めませんでした", &e, 9000);
                }
                prev
            }
        }
    }

    /// 見積もった間隔を doc に決まりとして書く（見本が無くても、既定のままで「見積もった」ことにする）。
    /// 変える前と後の (gap, block_gap) を返す。回が開いていなければ None。
    /// 画面の doc を丸ごと入れ替えないよう、間隔の 3 項目だけを直す（set_doc は並べ直しを呼ぶので使わない）
    fn apply_inferred_gaps(&self, gap: Option<f64>, block_gap: Option<f64>, gen: u64) -> Result<Option<((f64, f64), (f64, f64))>> {
        let (before, doc) = {
            let mut st = self.st.write();
            let Some(ps) = st.as_mut() else { return Ok(None) };
            // 回を開くのは st を持ったまま gen を進めるので、ここで同じなら st は gen の回のもの
            if self.vstate.read().gen != gen {
                return Ok(None);
            }
            let mut d = ps.project.doc.clone();
            let before = (d.gap, d.block_gap);
            if let Some(g) = gap {
                d.gap = g;
            }
            if let Some(g) = block_gap {
                d.block_gap = g;
            }
            d.gaps_inferred = true;
            // 保存できたときだけ反映する（保存に失敗した doc を残さない）
            let old = std::mem::replace(&mut ps.project.doc, d.clone());
            match ps.project.save() {
                Ok(n) => self.notice_external_save(n),
                Err(e) => {
                    ps.project.doc = old;
                    return Err(e);
                }
            }
            (before, d)
        };
        self.emit("doc", serde_json::to_value(&doc)?);
        Ok(Some((before, (doc.gap, doc.block_gap))))
    }

    fn toast_error(&self, text: &str, ms: u64) {
        self.emit("toast", json!({"text": text, "kind": "error", "ms": ms}));
    }

    /// 「{head}: 理由」の和文をトーストに、元の連鎖をログとツールチップに出す
    fn toast_err(&self, head: &str, e: &anyhow::Error, ms: u64) {
        let (text, detail) = crate::app::error::notice_text(head, e);
        self.engine.push_log(detail.clone());
        let mut v = json!({"text": text, "kind": "error", "ms": ms});
        if detail != text {
            v["detail"] = detail.into();
        }
        self.emit("toast", v);
    }

    /// 画面の保存前の台本で並べた結果（`voice_layout_preview`）。今の解析を使うだけで、Core の並びは変えない
    pub fn layout_preview(&self, s: &Script, gap: f64, block_gap: f64) -> LayoutState {
        let v = self.vstate.read();
        let l = layout::layout(s, &v.analyses, gap, block_gap);
        LayoutState { items: l.items, total: l.total, ready: v.ready }
    }

    /// 1 行の音声を作る（`voice_synth`）。送られた台本の値で作り、解析に入れて配る。台本の中に無い行は None
    pub fn synth_line(&self, s: &Script, line_id: &str) -> Result<Option<Analysis>> {
        let dict = self.reload_voice_dict();
        let Some(r) = analysis::synth(&self.voice, s, line_id, &dict)? else { return Ok(None) };
        self.vstate.write().analyses.insert(r.key.clone(), r.analysis.clone(), r.sig);
        let mut entries = HashMap::new();
        entries.insert(r.key, r.analysis.clone());
        self.emit("analysis", AnalysisUpdate { entries });
        self.refresh_voice(false);
        Ok(Some(r.analysis))
    }

    /// 解析から消す（辞書を変えたとき）
    pub fn forget_voice(&self, keys: &[String]) -> Result<()> {
        if self.st.read().is_none() {
            return Err(crate::app::error::no_project());
        }
        {
            let mut v = self.vstate.write();
            for k in keys {
                v.analyses.remove(k);
            }
        }
        self.refresh_voice(false);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::script::format::{Block, Line};
    use crate::voice::key::vkey;

    fn line(id: &str, text: &str) -> Line {
        Line { id: id.into(), sp: "トーク1".into(), text: text.into(), start: None, memo: String::new() }
    }

    fn script(lines: Vec<Line>) -> Script {
        Script {
            blocks: vec![Block { id: "b".into(), file: None, title: "A".into(), lines, notes: String::new() }],
            ..Default::default()
        }
    }

    fn an(id: &str, d: f64) -> Analysis {
        Analysis {
            id: id.into(),
            duration: d,
            unread: vec![],
            analyzed_text: String::new(),
            phonemes: None,
            phoneme_durations: None,
            speaker_params: None,
        }
    }

    /// 台本を保存した直後（中継なし）に、足した行・直した行が推定の長さ・音声なしで並びに入る
    #[test]
    fn recompute_follows_saved_script() {
        let s0 = script(vec![line("l1", "こんにちは")]);
        let mut v = VoiceState::default();
        let l1 = &s0.blocks[0].lines[0];
        v.analyses.insert(vkey(&l1.sp, &l1.text), an("a1", 2.0), analysis::sig(&s0, l1, v.analyses.dict()));
        v.recompute(&s0, 0.3, 1.0);
        assert_eq!(v.items.len(), 1);
        assert_eq!(v.items[0].audio.as_deref(), Some("a1"));
        assert_eq!(v.items[0].dur, 2.0);

        let s1 = script(vec![line("l1", "こんばんは"), line("l2", "あ")]);
        v.recompute(&s1, 0.3, 1.0);
        assert_eq!(v.items.len(), 2);
        assert_eq!(v.items[0].text, "こんばんは");
        assert!(v.items[0].audio.is_none());
        assert_eq!(v.items[0].dur, 0.8); // 5 文字 × 0.13 秒は下限の 0.8 秒に満たない
        assert_eq!(v.items[1].line_id, "l2");
        assert_eq!(v.items[1].dur, 0.8);
        assert!(v.items[1].audio.is_none());
        assert!(v.total > 0.0);
        // 状態の写しは並びと同じ
        let st = v.layout_state();
        assert_eq!(st.items.len(), 2);
        assert!(!st.ready);
    }

    /// 辞書を変えると、その語を含む行だけ音声なし（作り直す対象）になる
    #[test]
    fn dict_change_drops_audio_of_related_lines_only() {
        let s = script(vec![line("l1", "ブルーベリーの話"), line("l2", "こんにちは")]);
        let mut v = VoiceState::default();
        for (i, l) in s.blocks[0].lines.iter().enumerate() {
            let g = analysis::sig(&s, l, v.analyses.dict());
            v.analyses.insert(vkey(&l.sp, &l.text), an(&format!("a{i}"), 1.0), g);
        }
        v.recompute(&s, 0.3, 1.0);
        assert!(v.items.iter().all(|x| x.audio.is_some()));
        let d = VoiceDict { entries: vec![json!({"surface": "ブルーベリー", "reading": "ブルーベリー"})] };
        assert!(v.analyses.set_dict(d));
        v.recompute(&s, 0.3, 1.0);
        assert!(v.items[0].audio.is_none());
        assert_eq!(v.items[1].audio.as_deref(), Some("a1"));
        // 回を開き直しても辞書は残る
        v.reset();
        assert_eq!(v.analyses.dict().entries.len(), 1);
    }

    #[test]
    fn reset_clears_everything() {
        let mut v = VoiceState::default();
        v.ready = true;
        v.analyses.insert("k".into(), an("a", 1.0), String::new());
        v.reset();
        assert!(!v.ready);
        assert!(v.analyses.is_empty());
        assert!(v.items.is_empty());
    }
}
