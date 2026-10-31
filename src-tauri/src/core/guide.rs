//! 攻略動画（`studio/guide.json`）の読み書き・見張り・合成・解析の実行。純粋な部分は `crate::guide`。

use super::Core;
use crate::app::error::{bad_input, coded, no_project};
use crate::guide::layer::{even_scaled, frame_inputs, render_plan_frame, slot_crop_px, sub_target, FrameInputs};
use crate::guide::patch::apply_patch;
use crate::guide::read::{coords, game_text, read_json};
use crate::guide::still::{check_size, crop_and_scale, draw_grid, parse_target, fit_long, pick_frame, resolve_crop, resolve_scale, still_stem, Target};
use crate::guide::undo::{fingerprint, prune_dir, undo_target, write_backup, ClaudeChange, BACKUP_DIR, BACKUP_KEEP};
use crate::guide::map::{clear_time_frames, map_json, state_warnings, subs_at, timer_values};
use crate::guide::plan::{add_op, effective_fixed, normalize_guide, ops_out, part_label, step_frame, GuidePlan};
use crate::guide::{
    apply_auto_anchors, build_plan, create_fx_stub, fx_stub_needed, read_analysis, read_guide_text, rel_src, ANALYSIS_FILE,
    FX_STUB_FILE, GUIDE_FILE, MAP_FILE,
};
use crate::render::media::Rgb;
use crate::settings::write_atomic;
use ak_proto::guide::{ClaudeUndo, GuideState, SrcAt};
use ak_proto::http::{GuideCoordsReq, GuidePatch, GuideStillReq};
use ak_proto::{ErrorCode, Guide, GuideBench, GuideFixed};
use anyhow::{anyhow, bail, Context, Result};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::Arc;

/// 演出の入口のパス（見張りが見るのと同じ区切りの形）
fn fx_stub_path(root: &std::path::Path) -> PathBuf {
    std::path::Path::new(FX_STUB_FILE).components().fold(root.to_path_buf(), |a, c| a.join(c))
}

impl Core {
    fn studio_path(&self, name: &str) -> Option<PathBuf> {
        self.root().map(|r| r.join("studio").join(name))
    }

    /// 回を開いたとき: 前の回の状態を捨てて、ファイルから読み直す
    pub(super) fn guide_reset_and_load(&self) {
        {
            let _one = self.guide_lock.lock();
            let mut rt = self.guide_rt.write();
            let (gen, rev) = (rt.gen.wrapping_add(1), rt.rev);
            *rt = Default::default();
            rt.gen = gen;
            // 回を入れ替えた後（open_project は st を先に替える）の root を gen と同じ間に入れる
            rt.root = self.root();
            // 前の回の素材の音のメモリは持ち越さない
            *self.guide_audio_src.lock() = None;
            // 画面が前の回の番号で保存しても食い違いになるよう、番号は戻さない
            rt.rev = rev + 1;
        }
        self.guide_load();
    }

    /// guide.json・guide_analysis.json を読み直して、パートを組み立て直す（外で書き換わったときも）。
    /// 読めなければ前の中身のまま、理由を持つ（ファイルは上書きしない）
    pub fn guide_load(&self) {
        let (Some(gp), Some(ap)) = (self.studio_path(GUIDE_FILE), self.studio_path(ANALYSIS_FILE)) else { return };
        let g = read_guide_text(&gp);
        let a = read_analysis(&ap);
        let _one = self.guide_lock.lock();
        {
            let mut rt = self.guide_rt.write();
            let mut errs = vec![];
            rt.rev += 1;
            rt.guide_error = None;
            match g {
                Ok(v) => {
                    // 実際に読んだ中身の指紋（Claude の変更を戻せるかの判断に使う）
                    rt.fp = v.as_ref().map(|(_, t)| fingerprint(t));
                    rt.guide = v.map(|(g, _)| Arc::new(g));
                }
                Err(e) => {
                    rt.fp = None;
                    rt.guide_error = Some(e.clone());
                    errs.push(e);
                }
            }
            match a {
                Ok(v) => rt.analysis = v.map(Arc::new),
                Err(e) => errs.push(e),
            }
            rt.load_error = (!errs.is_empty()).then(|| errs.join("\n"));
        }
        if let Some(e) = self.guide_rt.read().load_error.clone() {
            self.engine.push_log(e);
        }
        self.guide_rebuild();
    }

    /// 今の guide.json・解析・設定からパートを組み立て、対応表（guide_map.json）を書いて、画面に知らせる
    /// 演出（fx）を読み直したら true
    fn guide_rebuild(&self) -> bool {
        let settings = self.settings.get().guide_fixed;
        // 人が直して、積んである Claude の変更と合わなくなっていたら、戻せる変更は捨てる
        self.guide_rt.write().drop_stale_claude();
        let (guide, analysis) = {
            let rt = self.guide_rt.read();
            (rt.guide.clone(), rt.analysis.clone())
        };
        let (plan, perr) = match build_plan(guide.as_deref(), analysis.as_deref(), &settings) {
            Ok(p) => (p.map(Arc::new), None),
            Err(e) => (None, Some(e)),
        };
        {
            let mut rt = self.guide_rt.write();
            rt.plan = plan.clone();
            rt.plan_error = perr;
        }
        let mut map_changed = false;
        let mut probe_errs: std::collections::BTreeMap<String, String> = Default::default();
        if let Some(mp) = self.studio_path(MAP_FILE) {
            match (&plan, guide.as_deref()) {
                (Some(p), Some(g)) => {
                    let root = self.root();
                    // 補足素材の絵の大きさ（小窓の箱を縦横比に合わせるため）。読めなければ sub_box のまま・警告に出す（描くときにもエラーが出る）
                    let errs = std::cell::RefCell::new(&mut probe_errs);
                    let dims = |rel: &str| -> Option<(u32, u32)> {
                        let path = root.as_ref()?.join(rel).to_string_lossy().to_string();
                        match self.media.video(&path) {
                            Ok(v) if v.info.has_video => Some((v.info.w, v.info.h)),
                            Ok(_) => {
                                errs.borrow_mut().insert(rel.to_string(), "映像がありません".into());
                                None
                            }
                            Err(e) => {
                                errs.borrow_mut().insert(rel.to_string(), format!("{e:#}"));
                                None
                            }
                        }
                    };
                    match serde_json::to_string(&map_json(p, g, analysis.as_deref(), &dims)) {
                        Err(e) => self.notice_err("studio/guide_map.json を作れませんでした", &e.into()),
                        // 中身が同じなら書かない・演出も読み直さない
                        Ok(text) => {
                            if std::fs::read_to_string(&mp).ok().as_deref() != Some(text.as_str()) {
                                self.mark_self_write(&mp);
                                match write_atomic(&mp, text) {
                                    Ok(()) => map_changed = true,
                                    Err(e) => self.notice_err("studio/guide_map.json を書けませんでした", &e.into()),
                                }
                            }
                        }
                    }
                }
                // 古い対応表を残すと、演出や Claude が今と違う時刻を読む。消したら演出も読み直す（「対応表がありません」を出すため）
                _ => {
                    self.mark_self_write(&mp);
                    match std::fs::remove_file(&mp) {
                        Ok(()) => map_changed = true,
                        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                        Err(e) => self.notice_err("古い studio/guide_map.json を消せませんでした", &e.into()),
                    }
                }
            }
        }
        self.guide_rt.write().sub_probe_errors =
            probe_errs.into_iter().map(|(rel, why)| format!("補足素材（{rel}）の大きさを読めませんでした: {why}。小窓の箱は設定のままにします")).collect();
        // 対応表が変わったら、演出（NOW/NEXT・操作の帯など）も今の対応表で読み直す
        let reload = map_changed && self.fx_timeline_loaded();
        if reload {
            self.reload_fx();
        }
        self.guide_rebuild_done();
        // 素材の音・効果音は裏で作り直す（組み立て・操作が変わっていなければ何もしない）
        self.guide_audio_kick();
        self.guide_se_kick();
        reload
    }

    fn guide_rebuild_done(&self) {
        self.emit("guide", self.guide_state());
    }

    /// 演出のタイムライン（fx/*_fx.py）が回に設定されていて、ファイルがあるか
    fn fx_timeline_loaded(&self) -> bool {
        self.st.read().as_ref().is_some_and(|ps| ps.project.fx_timeline().is_some_and(|p| p.exists()))
    }

    pub fn guide_state(&self) -> GuideState {
        let settings = self.settings.get().guide_fixed;
        let rt = self.guide_rt.read();
        let guide = rt.guide.as_deref().cloned();
        let fixed_effective = guide.as_ref().map(|g| effective_fixed(g, &settings)).unwrap_or_else(|| settings.clone());
        GuideState {
            guide,
            fixed_effective,
            fixed_settings: settings,
            bench: self.settings.get().guide_bench.clamped(),
            plan_parts: rt.plan.as_ref().map(|p| p.parts.clone()).unwrap_or_default(),
            anchor_out: rt.plan.as_ref().map(|p| p.anchor_out.iter().cloned().collect()).unwrap_or_default(),
            ops_out: match (&rt.guide, &rt.plan, &rt.analysis) {
                (Some(g), Some(p), Some(a)) => ops_out(g, p, &a.cost, &a.game),
                _ => vec![],
            },
            cuts_out: rt.plan.as_ref().map(|p| p.seams.clone()).unwrap_or_default(),
            analysis: rt.analysis.as_ref().map(|a| a.info()).unwrap_or_default(),
            error: rt.error(),
            scanning: rt.scanning,
            rev: rt.rev,
            claude_undo: rt.undoable().map(|c| ClaudeUndo { id: c.id, at: c.at.clone(), reason: c.reason.clone() }),
            audio_rev: rt.audio_rev,
            audio_err: rt.audio_err.clone(),
            warnings: state_warnings(rt.guide.as_deref(), rt.plan.as_deref(), rt.analysis.as_deref(), &[&rt.sub_probe_errors, rt.audio_warn.as_slice()]),
        }
    }

    /// guide.json を原子的に書いて、メモリの状態を入れ替える（呼ぶ側が guide_lock を持つ）。書いた文字列を返す
    fn guide_write(&self, g: &Guide) -> Result<String> {
        let text = serde_json::to_string_pretty(g)?;
        self.guide_write_text(g, text.clone())?;
        Ok(text)
    }

    /// 文字列 text（g の guide.json の中身）をそのまま原子的に書いて、メモリの状態を入れ替える（戻すときは控えの中身そのまま）
    fn guide_write_text(&self, g: &Guide, text: String) -> Result<()> {
        let p = self.studio_path(GUIDE_FILE).ok_or_else(no_project)?;
        // 自分の書き込みを、見張りが外の変更と取り違えないように
        self.mark_self_write(&p);
        if let Some(d) = p.parent() {
            std::fs::create_dir_all(d)?;
        }
        let fp = fingerprint(&text);
        write_atomic(&p, text).with_context(|| format!("{} を保存できませんでした", p.display()))?;
        let mut rt = self.guide_rt.write();
        rt.guide = Some(Arc::new(g.clone()));
        rt.fp = Some(fp);
        rt.rev += 1;
        Ok(())
    }

    /// guide.json が読めないなら Err（解析の始めと、アンカーの候補を書く前。解析のファイルが読めないだけなら通す）
    fn guide_check_guide_readable(&self) -> Result<()> {
        if let Some(e) = self.guide_rt.read().guide_error.clone() {
            bail!("{e}
直すまで保存しません（ファイルを直すと自動で読み直します）");
        }
        Ok(())
    }

    /// 読めないファイルを上書きしないための確認
    fn guide_check_writable(&self) -> Result<()> {
        if let Some(e) = self.guide_rt.read().load_error.clone() {
            bail!("{e}\n直すまで保存しません（ファイルを直すと自動で読み直します）");
        }
        Ok(())
    }

    /// 保存の前の整えと確かめ（`guide_set` と Claude の `guide_update` で同じ）。呼ぶ側が guide_lock を持つ。
    /// 操作・アンカー・カットの時刻をコマに合わせ、操作を時刻の順に並べる。組み立てられなくなる変更は断る
    fn guide_prepare(&self, root: &std::path::Path, mut g: Guide) -> Result<Guide> {
        g.src = rel_src(root, &g.src).map_err(|e| anyhow!(e))?;
        // 解析がまだなら並べるだけ
        let analysis = self.guide_rt.read().analysis.clone();
        normalize_guide(&mut g, analysis.as_deref().map(|a| a.pts.as_slice()).unwrap_or(&[])).map_err(|e| anyhow!(e))?;
        // 今は組み立てられているのに、この変更（入らないカット・順番の合わないアンカーなど）で組み立てられなくなるなら保存しない。
        // 1 回の操作でパート・操作の印が全部消えないように。素材の選び直しは解析し直す前提なので通す
        let (had_plan, old_src) = {
            let rt = self.guide_rt.read();
            (rt.plan.is_some(), rt.guide.as_ref().map(|x| x.src.clone()).unwrap_or_default())
        };
        if had_plan && g.src == old_src {
            if let Err(e) = build_plan(Some(&g), analysis.as_deref(), &self.settings.get().guide_fixed) {
                bail!("{e}（この変更は保存していません）");
            }
        }
        Ok(g)
    }

    /// 攻略動画の編集内容を保存して、パートを組み立て直す。
    /// base_rev は画面が持っている `GuideState::rev`。今と違えば（その間にほかで guide.json が変わっていた）保存せずに断る
    pub fn guide_set(&self, g: Guide, base_rev: Option<u64>) -> Result<GuideState> {
        let root = self.root().ok_or_else(no_project)?;
        let _one = self.guide_lock.lock();
        self.guide_check_writable()?;
        if self.guide_rt.read().guide.is_none() {
            bail!("攻略動画にしていません。先に素材を選んでください");
        }
        crate::guide::check_rev(base_rev, self.guide_rt.read().rev).map_err(bad_input)?;
        let g = self.guide_prepare(&root, g)?;
        self.guide_write(&g)?;
        self.guide_rebuild();
        Ok(self.guide_state())
    }

    /// 出力の時刻 t（手動パートの中）に操作を足して保存する。足した操作の id は Rust が振る
    pub fn guide_op_add(&self, t: f64) -> Result<GuideState> {
        let _one = self.guide_lock.lock();
        self.guide_check_writable()?;
        let (g, plan) = {
            let rt = self.guide_rt.read();
            (rt.guide.clone(), rt.plan.clone())
        };
        let Some(g) = g else { bail!("攻略動画にしていません。先に素材を選んでください") };
        let Some(plan) = plan else {
            bail!("パートを組み立てられていません。素材を解析して、アンカーを確かめてください");
        };
        let mut g = (*g).clone();
        add_op(&mut g, &plan, t).map_err(|e| anyhow!(e))?;
        self.guide_write(&g)?;
        self.guide_rebuild();
        Ok(self.guide_state())
    }

    /// 出力の時刻 t に見えている素材のコマ。組み立てが無い・範囲外は None
    pub fn guide_src_at(&self, t: f64) -> Option<SrcAt> {
        let plan = self.guide_plan()?;
        let (frame, t_src, part) = plan.src_at(t)?;
        Some(SrcAt { frame, t_src, part: part.to_string() })
    }

    /// 素材の時刻 t_src から delta コマ進めた（戻した）コマの時刻。解析が無ければ Err
    pub fn guide_step(&self, t_src: f64, delta: i32) -> Result<f64> {
        let Some(a) = self.guide_rt.read().analysis.clone() else {
            bail!("素材の解析がまだです。攻略タブの「解析」を押してください");
        };
        Ok(step_frame(&a.pts, t_src, delta))
    }

    /// 編成の枠 i の絵（編成パートの「暗転の 1 秒前」のコマから切り抜いた JPEG）。scale は素材の大きさに対する倍率
    pub fn guide_slot_jpeg(&self, i: usize, scale: f64) -> Result<Vec<u8>> {
        let (guide, analysis, plan) = {
            let rt = self.guide_rt.read();
            (rt.guide.clone(), rt.analysis.clone(), rt.plan.clone())
        };
        let (Some(guide), Some(analysis), Some(plan)) = (guide, analysis, plan) else {
            bail!("パートを組み立てられていません。素材を解析して、アンカーを確かめてください");
        };
        let slots = guide.formation.effective_slots();
        let Some(rect) = slots.get(i).copied() else { bail!("編成の枠 {i} はありません（{} 枠）", slots.len()) };
        let dark = plan.anchor_src_of("formation_dark").ok_or_else(|| anyhow!("編成の暗転のアンカーがありません"))?;
        let idx = crate::guide::plan::frame_at_time(&analysis.pts, dark - 1.0);
        let root = self.root().ok_or_else(no_project)?;
        let path = root.join(&guide.src).to_string_lossy().to_string();
        let v = self.media.video_pts(&path, &analysis.pts)?;
        let (w, h) = even_scaled(v.info.w, v.info.h, scale);
        let im = v.frame_opt(idx as i64, w, h, true)?;
        let r = slot_crop_px(rect, w, h);
        let c = crate::guide::layer::crop_rgb(&im, r);
        if c.w == 0 || c.h == 0 {
            bail!("編成の枠 {i} の範囲が空です");
        }
        crate::render::media::encode_jpeg(&c, 85)
    }

    /// guide.json を作る（素材は回のフォルダの中の動画）。あれば（読めなくても）作り直さない
    pub fn guide_create(&self, src: &str) -> Result<GuideState> {
        let root = self.root().ok_or_else(no_project)?;
        let _one = self.guide_lock.lock();
        let p = root.join("studio").join(GUIDE_FILE);
        if p.exists() || self.guide_rt.read().guide.is_some() {
            // 読めない guide.json は作り直さない（上書きすると中身が消える）
            if let Some(e) = self.guide_rt.read().load_error.clone() {
                bail!("{e}
studio/guide.json を直すか、別の場所へ移してから作り直してください");
            }
            bail!("すでに攻略動画になっています（studio/guide.json があります）");
        }
        let rel = rel_src(&root, src).map_err(|e| anyhow!(e))?;
        let full = root.join(&rel);
        if !full.is_file() {
            bail!("素材が見つかりません: {}", full.display());
        }
        if crate::render::media::MediaPool::is_image(&rel) || crate::render::media::MediaPool::is_audio(&rel) {
            bail!("動画を選んでください: {rel}");
        }
        let g = Guide { src: rel, ..Default::default() };
        self.guide_write(&g)?;
        // 演出がまだ無い回には、攻略動画の演出の入口も作る（作れなくても guide.json は作れているので、理由を出して続ける）
        let mut stub_made = false;
        if fx_stub_needed(&root) {
            // 見張りが自分の書き込みを外の変更と取り違えて、演出を 2 回読み直さないように
            self.mark_self_write(&fx_stub_path(&root));
            match create_fx_stub(&root) {
                Ok(_) => stub_made = true,
                Err(e) => self.notice_error(e),
            }
        }
        // 演出の読み直しは 1 回だけ（対応表が書けて読み直したなら、それで足りる。書けなければ、対応表が無い理由が演出のエラーに出る）
        let reloaded = self.guide_rebuild();
        if stub_made && !reloaded {
            self.reload_fx();
        }
        Ok(self.guide_state())
    }

    /// 演出の入口 `fx/guide_fx.py` を作って、演出を読み込む（既に攻略動画の回向け）。あれば Err
    pub fn guide_fx_create(&self) -> Result<()> {
        let root = self.root().ok_or_else(no_project)?;
        let _one = self.guide_lock.lock();
        if self.guide_rt.read().guide.is_none() {
            bail!("攻略動画にしていません。先に素材を選んでください");
        }
        self.mark_self_write(&fx_stub_path(&root));
        create_fx_stub(&root).map_err(|e| anyhow!(e))?;
        self.reload_fx();
        Ok(())
    }

    /// 設定の guide_fixed（全部の回で使う固定値）に保存する
    pub fn guide_save_fixed_default(&self, f: GuideFixed) -> Result<GuideState> {
        crate::guide::plan::check_fixed(&f).map_err(|e| anyhow!(e))?;
        let _one = self.guide_lock.lock();
        self.settings.try_update(|s| s.guide_fixed = f).context("固定値を設定に保存できませんでした")?;
        self.guide_rebuild();
        Ok(self.guide_state())
    }

    /// 設定の guide_bench（待機欄の大きさ・不透明度。全部の回で使う）に保存する。
    /// 範囲の外は断る。組み立て直して（`guide` イベントでモニターが描き直す。重ねる絵はキャッシュしていない）、効いている値を返す
    pub fn guide_save_bench(&self, bench: GuideBench) -> Result<GuideState> {
        bench.check().map_err(bad_input)?;
        let _one = self.guide_lock.lock();
        self.settings.try_update(|s| s.guide_bench = bench).context("待機欄の大きさ・不透明度を設定に保存できませんでした")?;
        self.guide_rebuild();
        Ok(self.guide_state())
    }

    // ------------------------------------------------------------------ Claude（MCP）
    /// 攻略動画でない回（guide.json が無い・読めない）のときの失敗
    fn guide_missing(&self) -> anyhow::Error {
        match self.guide_rt.read().load_error.clone() {
            Some(e) => anyhow!(e),
            None => coded(ErrorCode::NotFound, "この回は攻略動画ではありません（studio/guide.json がありません）"),
        }
    }

    /// `guide_read`: 攻略動画の今の状態（パート・アンカー・操作・編成・補足素材・座標の変換・警告）
    pub fn guide_read(&self) -> Result<Value> {
        let st = self.guide_state();
        let Some(g) = st.guide.clone() else { return Err(self.guide_missing()) };
        let (plan, analysis) = {
            let rt = self.guide_rt.read();
            (rt.plan.clone(), rt.analysis.clone())
        };
        let size = analysis.as_ref().map(|a| a.size);
        // クリア時間: 自動パートのタイマー（対応表の timer）の最後の値
        let clear = plan.as_deref().and_then(|p| clear_time_frames(&timer_values(p, analysis.as_deref())));
        Ok(read_json(&g, &st, plan.as_deref(), size, clear))
    }

    /// `guide_update`: 今の guide.json に patch を当てて保存し、組み立て直す。保存の前の guide.json は控えに残す。
    /// 戻り: (`guide_read` と同じ形, 変えたことの和文の一覧, この変更で新しく出た警告)。何も変わらなければ保存せず、一覧は空
    pub fn guide_update(&self, patch: GuidePatch) -> Result<(Value, Vec<String>, Vec<String>)> {
        let root = self.root().ok_or_else(no_project)?;
        let _one = self.guide_lock.lock();
        self.guide_check_writable()?;
        let (g, plan, analysis) = {
            let rt = self.guide_rt.read();
            (rt.guide.clone(), rt.plan.clone(), rt.analysis.clone())
        };
        let Some(g) = g else { return Err(self.guide_missing()) };
        let pts = analysis.map(|a| a.pts.clone()).unwrap_or_default();
        // 最新の guide.json に当てる（画面が古い中身から作って、先の変更を消すことが無い）
        let (new, changed) = apply_patch(&g, plan.as_deref(), &pts, &patch).map_err(bad_input)?;
        if changed.is_empty() {
            return Ok((self.guide_read()?, changed, vec![]));
        }
        let warnings_before = self.guide_state().warnings;
        let new = self.guide_prepare(&root, new).map_err(|e| bad_input(format!("{e:#}")))?;

        // 保存の前の guide.json を控えに写す（写せなければ保存しない）
        let gp = self.studio_path(GUIDE_FILE).ok_or_else(no_project)?;
        let old_text = std::fs::read_to_string(&gp)
            .with_context(|| format!("控えを作れませんでした（{} を読めません）", gp.display()))?;
        let dir = root.join("studio").join(BACKUP_DIR);
        let now = chrono::Local::now();
        let backup = write_backup(&dir, &now.format("%Y%m%d-%H%M%S").to_string(), &old_text).map_err(|e| anyhow!(e))?;

        let text = self.guide_write(&new)?;
        {
            let mut rt = self.guide_rt.write();
            rt.claude_seq += 1;
            let id = rt.claude_seq;
            rt.claude.push(ClaudeChange {
                id,
                backup: backup.clone(),
                reason: patch.reason.clone(),
                at: now.format("%Y-%m-%dT%H:%M:%S").to_string(),
                after: fingerprint(&text),
            });
            // 控えは BACKUP_KEEP 個までしか残さないので、積み重ねも同じだけ（古いほうを捨てる）
            let over = rt.claude.len().saturating_sub(BACKUP_KEEP);
            rt.claude.drain(..over);
        }
        // 古い控えを消す（消せなくても保存は済んでいる。知らせるだけ）
        match prune_dir(&dir, BACKUP_KEEP) {
            Ok(failed) => {
                for (name, why) in failed {
                    self.notice_error(format!("古い控え（{name}）を消せませんでした: {why}"));
                }
            }
            Err(e) => self.notice_error(format!("古い控えを整理できませんでした: {e}")),
        }
        self.guide_rebuild();
        // この変更で新しく出た警告（補足素材の小窓が、消した操作を指していた、など）。断らず、返事で知らせる
        let warnings_new: Vec<String> = self.guide_state().warnings.into_iter().filter(|w| !warnings_before.contains(w)).collect();
        self.emit("toast", json!({"text": format!("Claude が攻略動画を直しました: {}", patch.reason), "kind": "info"}));
        Ok((self.guide_read()?, changed, warnings_new))
    }

    /// 画面の「Claude の変更を戻す」: いちばん新しい Claude の変更を、控えの guide.json に戻す。
    /// 今の guide.json がその変更の保存の後と同じ（その後に人が直していない）ときだけ。戻したら積み重ねから外す
    /// id は画面が見せていた変更の番号（`ClaudeUndo::id`）。いちばん上がそれと違えば（押している間に変わった・2 回押した）断る
    pub fn guide_undo_claude(&self, id: u32) -> Result<GuideState> {
        let _one = self.guide_lock.lock();
        self.guide_check_writable()?;
        let gp = self.studio_path(GUIDE_FILE).ok_or_else(no_project)?;
        let current = std::fs::read_to_string(&gp).with_context(|| format!("{} を読めませんでした", gp.display()))?;
        let top = {
            let rt = self.guide_rt.read();
            undo_target(&rt.claude, &current).map_err(bad_input)?.clone()
        };
        if top.id != id {
            return Err(bad_input(format!(
                "戻せる Claude の変更が変わりました（今は {}「{}」）。画面を確かめてからもう一度押してください",
                top.at, top.reason
            )));
        }
        // 控えが読めない・guide.json として読めないなら、この変更は戻せない。これより前の変更も、この変更の後の中身から戻すことになるので戻せない
        let loaded = std::fs::read_to_string(&top.backup)
            .map_err(|e| format!("控え（{}）を読めませんでした: {e}", top.backup.display()))
            .and_then(|text| {
                crate::guide::parse_guide(&text)
                    .map(|g| (g, text))
                    .map_err(|e| format!("控え（{}）が guide.json として読めません: {e}", top.backup.display()))
            });
        let (g, text) = match loaded {
            Ok(x) => x,
            Err(e) => {
                self.guide_rt.write().claude.clear();
                self.guide_rebuild();
                return Err(anyhow!("{e}
この変更と、これより前の Claude の変更は戻せなくなりました"));
            }
        };
        self.guide_write_text(&g, text)?;
        self.guide_rt.write().claude.pop();
        self.guide_rebuild();
        self.emit("toast", json!({"text": format!("Claude の変更を戻しました: {}", top.reason), "kind": "info"}));
        Ok(self.guide_state())
    }

    /// `guide_still`: 素材そのままの 1 コマ（演出・レイヤーなし）を studio/stills/ に書く。crop・grid・scale は `guide::still`。
    /// 戻り: {path, frame, t_src, part, t_out, cost, game_text, size, crop, scale, grid}
    pub fn guide_still(&self, req: GuideStillReq) -> Result<Value> {
        let (guide, analysis, plan) = {
            let rt = self.guide_rt.read();
            (rt.guide.clone(), rt.analysis.clone(), rt.plan.clone())
        };
        let Some(guide) = guide else { return Err(self.guide_missing()) };
        let Some(analysis) = analysis else {
            bail!("素材の解析がまだです。攻略タブの「解析」を押してください");
        };
        let target = parse_target(&req).map_err(bad_input)?.unwrap_or_else(|| Target::Out(self.ui.read().t));
        let pick = pick_frame(&target, &analysis.pts, plan.as_deref(), &guide.ops).map_err(bad_input)?;
        let crop = resolve_crop(req.crop).map_err(bad_input)?;
        let scale = resolve_scale(req.scale, crop, req.crop.is_some()).map_err(bad_input)?;

        let root = self.root().ok_or_else(no_project)?;
        let path = root.join(&guide.src);
        let v = self.media.video_pts(&path.to_string_lossy(), &analysis.pts)?;
        if !v.info.has_video {
            bail!("映像がありません: {}", path.display());
        }
        check_size(analysis.size, v.info.w, v.info.h).map_err(|e| anyhow!(e))?;
        // 素材そのままの大きさで読んで、切り抜き・倍率はこちらで（キャッシュには入れない）
        let im = v.frame_opt(pick.frame as i64, v.info.w, v.info.h, false)?;
        let mut img = crop_and_scale(&im, crop, scale);
        if req.grid {
            draw_grid(&mut img, crop, scale);
        }
        let dir = root.join("studio").join("stills");
        std::fs::create_dir_all(&dir)?;
        let png = dir.join(format!("{}.png", still_stem(pick.frame, req.crop.map(|_| crop), req.grid)));
        let (w, h) = (img.w, img.h);
        let rgb = image::RgbImage::from_raw(w, h, img.data).ok_or_else(|| anyhow!("確認画像を作れませんでした"))?;
        rgb.save(&png).with_context(|| format!("確認画像（{}）を保存できませんでした", png.display()))?;
        // Claude に見せる JPEG（長い辺 2000px まで。MCP が png と同じ名前の .jpg を探す）
        let small = match fit_long(w, h, 2000) {
            Some((sw, sh)) => image::imageops::resize(&rgb, sw, sh, image::imageops::FilterType::Triangle),
            None => rgb,
        };
        let jpg = png.with_extension("jpg");
        let jbytes = crate::render::media::encode_jpeg(&Rgb { w: small.width(), h: small.height(), data: small.into_raw() }, 88)?;
        write_atomic(&jpg, jbytes).with_context(|| format!("確認画像（{}）を保存できませんでした", jpg.display()))?;
        Ok(json!({
            "path": png.to_string_lossy().replace('\\', "/"),
            "frame": pick.frame, "t_src": pick.t_src, "part": pick.part, "t_out": pick.t_out,
            "cost": analysis.cost.get(pick.frame as usize).copied().flatten(),
            "game_text": analysis.game.get(pick.frame as usize).copied().flatten().map(game_text),
            "size": [w, h], "crop": crop, "scale": scale, "grid": req.grid,
        }))
    }

    /// `guide_coords`: 素材の画素 ↔ 出力の画素（`geometry` と同じ式。`visible` は出力の範囲の中か）
    pub fn guide_coords(&self, req: GuideCoordsReq) -> Result<Value> {
        let (guide, size) = {
            let rt = self.guide_rt.read();
            (rt.guide.clone(), rt.analysis.as_ref().map(|a| a.size).filter(|s| s[0] > 0 && s[1] > 0))
        };
        let Some(guide) = guide else { return Err(self.guide_missing()) };
        // 解析の前は素材の大きさが分からない。2752x2064 とみなして計算し、そう伝える
        let assumed = size.is_none();
        let size = size.unwrap_or([ak_proto::guide::REF_W as u32, ak_proto::guide::REF_H as u32]);
        let (points, visible) = coords(&guide.layout, &req.part, (size[0], size[1]), &req.points, req.to.as_deref().unwrap_or("out"))
            .map_err(bad_input)?;
        let mut v = json!({"points": points, "visible": visible, "src_size": size});
        if assumed {
            v["size_assumed"] = json!(true);
        }
        Ok(v)
    }

    // ------------------------------------------------------------------ 解析
    /// 素材の解析を裏で始める。進み具合はイベント progress（task guide_scan）、終わりは guide イベントで届く
    pub fn guide_scan(self: &Arc<Self>) -> Result<()> {
        let root = self.root().ok_or_else(no_project)?;
        let out = root.join("studio").join(ANALYSIS_FILE);
        let (video, gen) = {
            let _one = self.guide_lock.lock();
            // 読めない guide.json があるうちは始めない（終わったときにアンカーの候補を書けず、上書きもできない）
            self.guide_check_guide_readable()?;
            let mut rt = self.guide_rt.write();
            let Some(g) = rt.guide.clone() else {
                bail!("攻略動画にしていません。先に素材を選んでください");
            };
            if rt.scanning {
                bail!("解析しています。終わるまでお待ちください");
            }
            let video = root.join(&g.src);
            if !video.is_file() {
                bail!("素材が見つかりません: {}", video.display());
            }
            rt.scanning = true;
            (video, rt.gen)
        };
        self.emit("guide", self.guide_state());
        self.emit("progress", json!({"task": "guide_scan", "done": 0, "total": 1000, "msg": "素材を解析しています"}));
        let me = self.clone();
        std::thread::spawn(move || {
            let r = me.run_scan(&video, &out);
            me.finish_scan(r, &out, gen);
        });
        Ok(())
    }

    /// 解析の Python を動かして、終わるまで待つ。失敗は和文で返す（動かし方は `crate::guide::run_scan_process`）
    fn run_scan(&self, video: &std::path::Path, out: &std::path::Path) -> Result<(), String> {
        let kit = crate::settings::fxkit_dir()?;
        let py = crate::settings::resolve_python(&self.settings.get().python);
        crate::guide::run_scan_process(
            &py,
            &kit,
            video,
            out,
            |x| {
                self.emit(
                    "progress",
                    json!({"task": "guide_scan", "done": (x * 1000.0).round(), "total": 1000, "msg": "素材を解析しています"}),
                )
            },
            |l| self.engine.push_log(format!("[guide_scan] {l}")),
        )
    }

    /// 解析の終わり: 結果を読んでアンカーの候補を書き、パートを組み立て直す。失敗は画面に出す。
    /// gen は始めたときの回の番号。その間に回を開き直していたら、今の回には何も書かない
    fn finish_scan(&self, r: Result<(), String>, out: &std::path::Path, gen: u64) {
        let _one = self.guide_lock.lock();
        if self.guide_rt.read().gen != gen {
            self.engine.push_log(format!("[guide_scan] 解析の間に回を開き直したので、結果を今の回には使いません（{}）", out.display()));
            self.emit("progress", json!({"task": "guide_scan", "done": 1000, "total": 1000, "finished": true,
                                         "msg": "前の回の解析が終わりました（今の回には使いません）"}));
            return;
        }
        let r = r.and_then(|()| {
            let a = read_analysis(out)?.ok_or_else(|| "解析の出力ファイルがありません".to_string())?;
            let cur = self.guide_rt.read().guide.clone();
            {
                // 新しい解析は読めたので、前の解析が読めなかった理由は消す（guide.json の理由は残す）
                let mut rt = self.guide_rt.write();
                rt.analysis = Some(Arc::new(a.clone()));
                rt.load_error = rt.guide_error.clone();
            }
            if let Some(g) = cur {
                let mut g = (*g).clone();
                if apply_auto_anchors(&mut g, &a.anchors) {
                    // 解析の間に guide.json が読めなくなっていたら、上書きしない（外での編集を消さない）
                    self.guide_check_guide_readable()
                        .map_err(|e| format!("アンカーの候補を guide.json に書いていません: {e:#}"))?;
                    self.guide_write(&g).map_err(|e| format!("アンカーの候補を guide.json に保存できませんでした: {e:#}"))?;
                }
            }
            Ok(())
        });
        self.guide_rt.write().scanning = false;
        self.guide_rebuild();
        let mut p = json!({"task": "guide_scan", "done": 1000, "total": 1000, "finished": true, "msg": "素材の解析が終わりました"});
        if let Err(e) = r {
            p["msg"] = json!("素材の解析に失敗しました");
            p["error"] = json!(e.clone());
            self.notice_error(e);
        }
        self.emit("progress", p);
    }

    // ------------------------------------------------------------------ 合成
    fn guide_plan(&self) -> Option<Arc<GuidePlan>> {
        self.guide_rt.read().plan.clone()
    }

    /// 出力のコマ数から決まる fps（組み立てがあれば 60）
    pub(crate) fn guide_fps(&self) -> Option<f64> {
        self.guide_plan().map(|p| p.fps)
    }

    /// タイムラインに加わる終わり（秒）
    pub(crate) fn guide_end(&self) -> f64 {
        self.guide_plan().map(|p| p.end()).unwrap_or(0.0)
    }

    /// シーンの帯に出すパートの名前
    pub(super) fn guide_scenes(&self) -> Vec<(String, f64, f64)> {
        self.guide_plan()
            .map(|p| p.parts.iter().map(|x| (part_label(&x.name).to_string(), x.t0, x.t1)).collect())
            .unwrap_or_default()
    }

    /// 戦闘画面のレイヤー（素材を 70% で中央に置き、HUD の切り抜きを重ねる。手動パートは縮めて左上へ）を描く。
    /// 攻略動画でなければ何もしない。読めない・組み立てられないときは Err（黙って黒にしない）
    pub(super) fn draw_guide(&self, fr: &mut Rgb, t: f64, scale: f64, video_cache: bool) -> Result<()> {
        let (guide, analysis, plan, err) = {
            let rt = self.guide_rt.read();
            let Some(g) = rt.guide.clone() else {
                // guide.json が読めない（前の中身も無い）ときは理由を出す
                return match rt.load_error.clone() {
                    Some(e) => Err(anyhow!(e)),
                    None => Ok(()),
                };
            };
            (g, rt.analysis.clone(), rt.plan.clone(), rt.error())
        };
        if let Some(e) = err {
            bail!(e);
        }
        let (Some(plan), Some(analysis)) = (plan, analysis) else { return Ok(()) };
        let Some(FrameInputs { pf, part, dim, hold }) = frame_inputs(&guide, &plan, &analysis, t) else { return Ok(()) };
        let root = self.root().ok_or_else(no_project)?;
        let path = root.join(&guide.src);
        let bench = self.settings.get().guide_bench.clamped();
        render_plan_frame(fr, &self.media, &path, &analysis, &guide.layout, &bench, part, pf, dim, hold, scale, video_cache)?;
        self.draw_guide_subs(fr, &guide, &plan, &root, t, scale, video_cache)
    }

    /// 補足素材の小窓（手動パートの間、操作のところに別の素材を縦横比を保って sub_box に収める）。
    /// 読めない・ファイルが無いときは Err（黙って描かない）。ほかの小窓は描いてから、最初のエラーを返す
    #[allow(clippy::too_many_arguments)]
    fn draw_guide_subs(&self, fr: &mut Rgb, guide: &Guide, plan: &GuidePlan, root: &std::path::Path, t: f64,
                       scale: f64, video_cache: bool) -> Result<()> {
        let mut first_err: Option<anyhow::Error> = None;
        for (s, st) in subs_at(guide, plan, t) {
            let r = (|| -> Result<()> {
                let path = root.join(&s.src).to_string_lossy().to_string();
                let v = self.media.video(&path)?;
                if !v.info.has_video {
                    bail!("映像がありません");
                }
                let [x, y, w, h] = sub_target(guide.layout.sub_box, v.info.w, v.info.h, scale);
                let idx = v.index_at(st);
                let im = v.frame_opt(idx, w as u32, h as u32, video_cache)?;
                crate::render::media::blit_in(fr, &im, x, y, 1.0, [0, 0, fr.w as i64, fr.h as i64]);
                Ok(())
            })();
            if let Err(e) = r {
                first_err.get_or_insert(anyhow!("補足素材 {}（{}）を描けませんでした: {e:#}", s.index + 1, s.src));
            }
        }
        first_err.map_or(Ok(()), Err)
    }
}
