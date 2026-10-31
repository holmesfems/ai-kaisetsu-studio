//! 演出ワーカー（Python の akfx_worker.py）を何本か常駐させて、フレームの依頼を振り分ける。
//!
//! - 読み込み（load）と微調整の入れ替え（overrides）は全ワーカーに配る（制御チャンネル）
//! - フレームは「急ぎ」（再生・スクラブ）と「後回し」（先読み・書き出し）の2つの列から取る
//! - ワーカーは制御チャンネルを先に空にしてから仕事を取るので、微調整を送った後の依頼には必ず新しい値が効く

use crate::settings::hidden;
use anyhow::{anyhow, bail, Context, Result};
use crossbeam_channel::{bounded, select, unbounded, Receiver, Sender};
use parking_lot::{Mutex, RwLock};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use ak_proto::Cue;
use ts_rs::TS;

#[derive(Serialize, Deserialize, Clone, Debug, TS)]
#[ts(export)]
pub struct SeEv {
    pub t: f64,
    pub kind: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, TS)]
#[ts(export)]
pub struct Group {
    pub key: String,
    pub label: String,
    #[serde(default)]
    pub func: Option<String>,
    pub line: i64,
    pub src: String,
    pub t0: f64,
    pub t1: f64,
    pub n: usize,
    pub kinds: Vec<String>,
    pub z: f64,
    pub fp: String,
    pub gui: bool,
    pub se: Vec<SeEv>,
    /// クリップのおおよその範囲。左上と右下（x0, y0, x1, y1）。clips_list の bbox と同じ。
    /// 描いた絵の透明でない所の [x, y, 幅, 高さ] は OverlayFrame の rect で、別のもの
    #[ts(type = "[number, number, number, number] | null")]
    pub bbox: Option<[i64; 4]>,
    /// RecWindow（録画の窓）の窓の位置と区切り（segs: [時刻, 番号, ファイル名, 説明]）。録画を差し込む画面で使う
    #[serde(default)]
    pub rec: Vec<RecInfo>,
}

#[derive(Serialize, Deserialize, Clone, Debug, TS)]
#[ts(export)]
pub struct RecInfo {
    #[serde(rename = "box")]
    pub bx: [f64; 4],
    #[serde(default)]
    #[ts(type = "[number, string, string, string][]")]
    pub segs: Vec<Value>,
    #[serde(default)]
    pub t1: Option<f64>,
    /// 窓の入り・抜け（RecWindow が持つ。古いワーカーの返事には無い）
    #[serde(default)]
    pub fade: Option<RecFade>,
}

/// RecWindow の入り・抜けの長さ（秒）と緩急の名前（render/ease.rs の `by_name` で引く）
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, TS)]
#[ts(export)]
pub struct RecFade {
    #[serde(rename = "in")]
    pub in_: f64,
    pub in_ease: String,
    pub out: f64,
    pub out_ease: String,
}

/// ワーカーの警告（止めるほどではない失敗。効果音の付け方・画面から追加した演出・字幕のフォント）。
/// key で同じ警告かを見る（画面は増えた・変わったものだけを知らせる）
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq, TS)]
#[ts(export)]
pub struct Warning {
    pub key: String,
    pub message: String,
    /// 重さ。`"info"` は情報（公開版で kit が無いときなど、ふつうの状態）。赤くせず、通知もしない。無ければふつうの警告
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub level: Option<String>,
}

/// 描くのに使った演出の kit（ワーカーの desc の返事。kit の約束は docs/spec/issues-167.md の 3 節）
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, TS)]
#[ts(export)]
pub struct KitInfo {
    /// `private`・`basic` など（kit の NAME）
    pub name: String,
    /// kit が約束した版（KIT_API。素の kit・対応外のときは今のアプリの版）
    #[serde(default)]
    pub api: Option<i64>,
    /// kit が持つ機能（guide_scan・guide_template・thumb など）
    #[serde(default)]
    pub features: Vec<String>,
    #[serde(default)]
    pub path: Option<String>,
}

/// warnings の配列から読める要素だけを取り出す（形の違う要素は飛ばし、ほかは捨てない）。配列でなければ空
pub fn parse_warnings(v: &Value) -> Vec<Warning> {
    v.as_array()
        .map(|a| a.iter().filter_map(|w| serde_json::from_value(w.clone()).ok()).collect())
        .unwrap_or_default()
}

fn lenient_warnings<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<Warning>, D::Error> {
    Ok(parse_warnings(&Value::deserialize(d)?))
}

/// 返事の warnings を dst に足す（同じ key は後から来た文で置き換える）。増えた・変わったものを返す。
/// warnings が無い返事（古いワーカー）は何もしない
pub fn merge_warnings(dst: &mut Vec<Warning>, h: &Value) -> Vec<Warning> {
    let got = h.get("warnings").map(parse_warnings).unwrap_or_default();
    let mut fresh = vec![];
    for w in got {
        match dst.iter_mut().find(|d| d.key == w.key) {
            Some(d) if d.message == w.message => {}
            Some(d) => {
                d.message = w.message.clone();
                fresh.push(w);
            }
            None => {
                dst.push(w.clone());
                fresh.push(w);
            }
        }
    }
    fresh
}



#[derive(Serialize, Deserialize, Clone, Debug, TS)]
#[ts(export)]
pub struct Desc {
    pub path: String,
    pub end: f64,
    pub fps: f64,
    pub w: u32,
    pub h: u32,
    pub groups: Vec<Group>,
    pub cues: Vec<Cue>,
    #[ts(type = "{ cue: number; keyword: string; t: number; t_end: number; method: string }[]")]
    pub sync: Vec<Value>,
    #[ts(type = "any[]")]
    pub sync_used: Vec<Value>,
    /// 演出が読んだ同期点のファイル（古いワーカーの返事には無い）。手で動かした同期点は、この隣の sync_manual.json に書く
    #[serde(default)]
    pub sync_path: Option<String>,
    /// 描くのに使った fxkit の場所（fxkit.__file__。回のコピー・pip で入れた版・埋め込みの版のどれか。古いワーカーの返事には無い）
    #[serde(default)]
    pub fxkit: Option<String>,
    /// 描くのに使った kit（古いワーカーの返事には無い）
    #[serde(default)]
    pub kit: Option<KitInfo>,
    #[ts(type = "Record<string, any>")]
    pub meta: Value,
    pub build_sec: f64,
    pub stage: Vec<f64>,
    pub rec_box: Vec<f64>,
    /// 読み込みで起きた、止めるほどではない失敗（古いワーカーの返事には無い。形の違う要素は飛ばす）
    #[serde(default, deserialize_with = "lenient_warnings")]
    pub warnings: Vec<Warning>,
}

/// ワーカーが返す演出のコマ（透明でない所 rect だけの RGBA）。
#[derive(Clone)]
pub struct OverlayFrame {
    pub w: u32,
    pub h: u32,
    pub rect: Option<[u32; 4]>,
    pub rgba: Vec<u8>,
    pub ms: f64,
}

impl OverlayFrame {
    /// コマ全体の RGBA（rect の外は透明）
    pub fn to_full(&self) -> Vec<u8> {
        let mut full = vec![0u8; self.w as usize * self.h as usize * 4];
        if let Some(bb) = self.rect {
            let (x, y, bw, bh) = (bb[0] as usize, bb[1] as usize, bb[2] as usize, bb[3] as usize);
            for row in 0..bh {
                let di = ((y + row) * self.w as usize + x) * 4;
                let si = row * bw * 4;
                full[di..di + bw * 4].copy_from_slice(&self.rgba[si..si + bw * 4]);
            }
        }
        full
    }
}

pub struct FrameReq {
    pub t: f64,
    pub scale: f64,
    pub hide: Vec<String>,
    pub solo: Option<String>,
}

enum Job {
    Frame(FrameReq, Sender<Result<OverlayFrame>>),
    Raw(Value, Sender<Result<(Value, Vec<u8>)>>),
}

#[derive(Clone)]
enum Ctrl {
    Load { timeline: String, overrides: String, clips: String, placeholder: bool, gen: u64 },
    /// 調整と、渡したときの gen（読み込み・unload の後に返ってきた古い結果で警告を出さないため）
    Overrides(Value, u64),
    /// プロセスを止めて捨てる（Python を変えたとき。次の Load で今の Python から作り直す）
    Respawn,
    Quit,
}

#[derive(Serialize, Clone, Debug, Default, TS)]
#[ts(export)]
pub struct EngineStatus {
    /// idle / loading / ready / error / none
    pub state: String,
    pub error: Option<String>,
    pub timeline: Option<String>,
    pub workers: usize,
    pub ready_workers: usize,
    /// 今の読み込みに失敗したワーカーの数と、最初のエラー（ほかのワーカーが読めていても残す）
    pub failed_workers: usize,
    pub worker_error: Option<String>,
    pub gen: u64,
    pub build_sec: f64,
}

type OnChange = Box<dyn Fn(&str) + Send + Sync>;

/// ワーカーを n 本作ろうとして、作れなかったもの（番号とエラー）から、本数と状態のエラーを決める
fn spawn_summary(n: usize, failed: &[(usize, String)]) -> (usize, Option<String>) {
    let workers = n.saturating_sub(failed.len());
    let error = match failed.first() {
        Some((_, e)) if workers == 0 => Some(format!("演出のワーカーを起動できませんでした: {e}")),
        _ => None,
    };
    (workers, error)
}

pub struct Engine {
    /// 設定で変えられる（set_python）。次に作るプロセスから使う
    python: Mutex<String>,
    script: PathBuf,
    ctrl: Mutex<Vec<Sender<Ctrl>>>,
    hi_tx: Sender<Job>,
    lo_tx: Sender<Job>,
    hi_rx: Receiver<Job>,
    lo_rx: Receiver<Job>,
    pub desc: RwLock<Option<Arc<Desc>>>,
    pub status: RwLock<EngineStatus>,
    pub log: Arc<Mutex<VecDeque<String>>>,
    gen: AtomicU64,
    on_change: Mutex<Option<OnChange>>,
    n: usize,
    /// ログの札の頭（書き出し用の一時エンジンは「書き出し」）
    tag: String,
    /// シーン番号の札の絵（番号ごと。ワーカーに頼んだ分だけ覚える）
    labels: Mutex<std::collections::HashMap<String, Arc<crate::render::scenes::Rgba>>>,
    /// ワーカーごとの、調整を受け付けなかった理由（旧#141。次に受け付けた・読み込み直した・回を変えたら消す）。警告に出す
    ov_error: Mutex<Vec<Option<String>>>,
}

impl Engine {
    pub fn new(python: &str, script: PathBuf, n: usize) -> Arc<Engine> {
        Self::with_log(python, script, n, Arc::new(Mutex::new(VecDeque::new())), "")
    }

    /// ログを別のエンジンと共有する（書き出し用の一時エンジンの Python の出力も「演出エンジン」のログに出すため）
    pub fn with_log(python: &str, script: PathBuf, n: usize, log: Arc<Mutex<VecDeque<String>>>, tag: &str) -> Arc<Engine> {
        let (hi_tx, hi_rx) = unbounded();
        let (lo_tx, lo_rx) = unbounded();
        let e = Arc::new(Engine {
            python: Mutex::new(python.to_string()),
            script,
            ctrl: Mutex::new(vec![]),
            hi_tx,
            lo_tx,
            hi_rx,
            lo_rx,
            desc: RwLock::new(None),
            status: RwLock::new(EngineStatus { state: "none".into(), workers: n, ..Default::default() }),
            log,
            gen: AtomicU64::new(0),
            on_change: Mutex::new(None),
            n,
            tag: tag.to_string(),
            labels: Mutex::new(Default::default()),
            ov_error: Mutex::new(vec![None; n]),
        });
        let mut failed: Vec<(usize, String)> = vec![];
        for i in 0..n {
            let (tx, rx) = unbounded();
            let me = e.clone();
            match std::thread::Builder::new().name(format!("fx-worker-{i}")).spawn(move || me.worker_loop(i, rx)) {
                Ok(_) => e.ctrl.lock().push(tx),
                Err(err) => {
                    e.push_log(format!("[{}w{i}] 演出のワーカーを起動できませんでした: {err}", e.tag));
                    failed.push((i, err.to_string()));
                }
            }
        }
        if !failed.is_empty() {
            let (workers, error) = spawn_summary(n, &failed);
            let mut st = e.status.write();
            st.workers = workers;
            if let Some(msg) = error {
                st.state = "error".into();
                st.error = Some(msg);
            }
        }
        e
    }

    pub fn set_on_change(&self, f: impl Fn(&str) + Send + Sync + 'static) {
        *self.on_change.lock() = Some(Box::new(f));
    }

    fn notify(&self, what: &str) {
        if let Some(f) = self.on_change.lock().as_ref() {
            f(what);
        }
    }

    pub fn push_log(&self, s: String) {
        push_log(&self.log, s);
    }

    pub fn log_tail(&self, n: usize) -> Vec<String> {
        let l = self.log.lock();
        l.iter().rev().take(n).rev().cloned().collect()
    }

    /// タイムラインを（読み）込む。全ワーカーに配り、最初に終わったワーカーの結果で desc を差し替える。
    pub fn load(&self, timeline: &Path, overrides: &Path, clips: &Path, placeholder: bool) -> u64 {
        // gen は status を持って進める（finish_load が古い読み込みを見分けるのと同じ間に）
        let mut st = self.status.write();
        let gen = self.gen.fetch_add(1, Ordering::SeqCst) + 1;
        st.gen = gen;
        self.clear_ov_errors();
        // ワーカーが 1 本も無いと返事をする者がいないので、「読み込み中」にせずエラーのまま返す
        let no_workers = self.ctrl.lock().is_empty() && self.n > 0;
        if !no_workers {
            st.state = "loading".into();
            st.timeline = Some(timeline.to_string_lossy().to_string());
            st.ready_workers = 0;
            st.failed_workers = 0;
            st.worker_error = None;
        }
        drop(st);
        self.notify("status");
        if no_workers {
            return gen;
        }
        let c = Ctrl::Load {
            timeline: timeline.to_string_lossy().to_string(),
            overrides: overrides.to_string_lossy().to_string(),
            clips: clips.to_string_lossy().to_string(),
            placeholder,
            gen,
        };
        for tx in self.ctrl.lock().iter() {
            let _ = tx.send(c.clone());
        }
        gen
    }

    /// 読み込んだタイムラインを捨てる（開いた回に演出が無いとき）。前の回の desc を残すと、その演出がタイムラインに出続ける。
    /// gen を進めるので、読み込み中だった前の回の結果も desc に入らない。ワーカーは次の load まで仕事を断る
    pub fn unload(&self, error: &str) {
        {
            // gen を進めて desc を捨てるまで status を持つ（finish_load と同じ順。古い読み込みが後から desc を入れない）
            let mut st = self.status.write();
            let gen = self.gen.fetch_add(1, Ordering::SeqCst) + 1;
            *self.desc.write() = None;
            self.clear_ov_errors();
            for tx in self.ctrl.lock().iter() {
                let _ = tx.send(Ctrl::Respawn);
            }
            *st = EngineStatus {
                state: "none".into(),
                error: Some(error.into()),
                workers: self.workers(),
                gen,
                ..Default::default()
            };
        }
        self.notify("desc");
    }

    /// Python を変える。今のプロセスはすべて止めるので、呼んだ後に load で読み直すこと
    pub fn set_python(&self, python: &str) {
        *self.python.lock() = python.to_string();
        for tx in self.ctrl.lock().iter() {
            let _ = tx.send(Ctrl::Respawn);
        }
    }

    pub fn set_overrides(&self, ov: &Value) {
        let gen = self.gen.load(Ordering::SeqCst);
        for tx in self.ctrl.lock().iter() {
            let _ = tx.send(Ctrl::Overrides(ov.clone(), gen));
        }
    }

    /// ワーカーが調整を受け付けなかったときの警告（受け付けなかった調整は使わず、前の調整のまま描いている）
    pub fn overrides_warning(&self) -> Option<Warning> {
        self.ov_error.lock().iter().flatten().next().map(|e| Warning {
            key: "overrides".into(),
            message: format!("調整を演出に反映できませんでした（前の調整のまま描いています。値を直すと消えます）: {e}"),
            level: None,
        })
    }

    fn clear_ov_errors(&self) {
        self.ov_error.lock().iter_mut().for_each(|e| *e = None);
    }

    /// ワーカーに調整を渡す。ワーカーが断ったら（値が読めないなど）警告に出す。
    /// 通信の失敗はログだけ（プロセスが落ちているので、次の仕事で作り直して last_ov を渡し直す）。
    /// gen は調整を渡したときの読み込みの世代。返ってくるまでに別の回を開いた・読み直した（gen が進んだ）ら、結果は警告に入れない
    fn send_overrides(&self, idx: usize, p: &mut Proc, v: &Value, gen: u64) {
        let err = match p.call(&serde_json::json!({"op": "overrides", "overrides": v})) {
            Ok((h, _)) if h.get("ok").and_then(|x| x.as_bool()) == Some(false) => {
                Some(h.get("error").and_then(|x| x.as_str()).unwrap_or("理由が分かりません").to_string())
            }
            Ok(_) => None,
            Err(e) => {
                self.push_log(format!("[{}w{idx}] 調整を反映できませんでした: {e:#}", self.tag));
                return;
            }
        };
        if let Some(e) = &err {
            self.push_log(format!("[{}w{idx}] 調整を反映できませんでした: {e}", self.tag));
        }
        let changed = {
            let mut cur = self.ov_error.lock();
            // unload・load は gen を進めてから警告を消すので、ここで gen を見れば、消した後に古い結果を入れない
            if self.gen.load(Ordering::SeqCst) != gen {
                return;
            }
            match cur.get_mut(idx) {
                Some(slot) if *slot != err => {
                    *slot = err;
                    true
                }
                _ => false,
            }
        };
        if changed {
            self.notify("status");
        }
    }

    pub fn shutdown(&self) {
        for tx in self.ctrl.lock().iter() {
            let _ = tx.send(Ctrl::Quit);
        }
    }

    pub fn ready(&self) -> bool {
        self.status.read().ready_workers > 0
    }

    /// フレームを描かせて待つ。urgent なら急ぎの列に入れる。
    pub fn frame(&self, req: FrameReq, urgent: bool, timeout: Duration) -> Result<OverlayFrame> {
        let (tx, rx) = bounded(1);
        let q = if urgent { &self.hi_tx } else { &self.lo_tx };
        q.send(Job::Frame(req, tx)).map_err(|_| crate::app::error::coded(ak_proto::ErrorCode::EngineDown, "演出エンジンが止まっています"))?;
        rx.recv_timeout(timeout).map_err(|_| anyhow!("演出の描画が時間内に終わりませんでした"))?
    }

    /// ワーカーに任意の依頼（se / still_png など）を出す。
    pub fn raw(&self, req: Value, timeout: Duration) -> Result<(Value, Vec<u8>)> {
        let (tx, rx) = bounded(1);
        self.hi_tx.send(Job::Raw(req, tx)).map_err(|_| crate::app::error::coded(ak_proto::ErrorCode::EngineDown, "演出エンジンが止まっています"))?;
        rx.recv_timeout(timeout).map_err(|_| anyhow!("ワーカーから時間内に応答がありませんでした"))?
    }

    /// シーン番号の札の絵（全体の大きさの RGBA）。番号は呼ぶ側が決める。同じ番号は覚えておく
    pub fn scene_label(&self, id: &str) -> Result<Arc<crate::render::scenes::Rgba>> {
        if let Some(l) = self.labels.lock().get(id) {
            return Ok(l.clone());
        }
        // 番号は "scene" で渡す（"id" は依頼の番号として call が上書きする）
        let (h, data) = self.raw(serde_json::json!({"op": "scene_label", "scene": id}), Duration::from_secs(60))?;
        let f = parse_frame(&h, data).map_err(|e| anyhow!("シーン番号の札を描けませんでした（{id}）: {e:#}"))?;
        let label = Arc::new(crate::render::scenes::Rgba { w: f.w, h: f.h, data: f.to_full() });
        self.labels.lock().insert(id.to_string(), label.clone());
        Ok(label)
    }

    fn spawn(&self, idx: usize) -> Result<Proc> {
        let python = self.python.lock().clone();
        spawn_proc(&python, &self.script, self.log.clone(), &format!("{}w{idx}", self.tag))
    }

    fn worker_loop(self: Arc<Self>, idx: usize, ctrl: Receiver<Ctrl>) {
        let mut proc: Option<Proc> = None;
        let mut loaded: Option<Ctrl> = None; // 最後に読み込んだ内容（落ちたときに読み直す）
        let mut last_ov: Option<Value> = None;
        let mut crashes: VecDeque<std::time::Instant> = VecDeque::new();
        loop {
            // 1. 制御を先に全部こなす
            let mut msgs = vec![];
            while let Ok(m) = ctrl.try_recv() {
                msgs.push(m);
            }
            // 読み込みは最後の1回だけで十分
            let last_load = msgs.iter().rposition(|m| matches!(m, Ctrl::Load { .. }));
            for (i, m) in msgs.into_iter().enumerate() {
                match m {
                    Ctrl::Quit => {
                        if let Some(mut p) = proc.take() {
                            let _ = p.call(&serde_json::json!({"op": "quit"}));
                            p.kill();
                        }
                        return;
                    }
                    Ctrl::Load { .. } if Some(i) != last_load => {}
                    Ctrl::Load { .. } => {
                        last_ov = None;
                        crashes.clear();
                        let r = self.do_load(idx, &mut proc, &m);
                        // 読めなかったワーカーには仕事を渡さない（前のタイムラインのコマが新しい鍵でキャッシュに入るので）
                        let ok = r.is_ok();
                        self.finish_load(idx, &m, r);
                        loaded = ok.then_some(m);
                    }
                    Ctrl::Respawn => {
                        Self::drop_proc(&mut proc, &mut loaded);
                    }
                    Ctrl::Overrides(v, g) => {
                        last_ov = Some(v.clone());
                        // 読み込めていないワーカーは描かないので渡さない（前のタイムラインのまま受け付けて、警告を消してしまうため）
                        if let (Some(p), true) = (proc.as_mut(), loaded.is_some()) {
                            self.send_overrides(idx, p, &v, g);
                        }
                    }
                }
            }
            // 2. 仕事を取る（急ぎを優先）。制御が来たらそちらへ戻る
            let job = if let Ok(j) = self.hi_rx.try_recv() {
                Some(j)
            } else {
                select! {
                    recv(ctrl) -> m => {
                        if let Ok(m) = m {
                            // 取り出した制御は次の周回でこなせるよう戻せないので、ここで処理する
                            match m {
                                Ctrl::Quit => { if let Some(mut p) = proc.take() { p.kill(); } return; }
                                Ctrl::Respawn => Self::drop_proc(&mut proc, &mut loaded),
                                Ctrl::Load { .. } => {
                                    last_ov = None;
                                    crashes.clear();
                                    let r = self.do_load(idx, &mut proc, &m);
                                    let ok = r.is_ok();
                                    self.finish_load(idx, &m, r);
                                    loaded = ok.then_some(m);
                                }
                                Ctrl::Overrides(v, g) => {
                                    last_ov = Some(v.clone());
                                    if let (Some(p), true) = (proc.as_mut(), loaded.is_some()) {
                                        self.send_overrides(idx, p, &v, g);
                                    }
                                }
                            }
                        }
                        None
                    }
                    recv(self.hi_rx) -> j => j.ok(),
                    recv(self.lo_rx) -> j => j.ok(),
                }
            };
            let Some(job) = job else { continue };
            if proc.is_none() || loaded.is_none() {
                fail(job, anyhow!(crate::msg::NO_FX));
                continue;
            }
            let res = self.run_job(proc.as_mut().unwrap(), job);
            if let Err(e) = res {
                // プロセスが落ちた: 作り直して読み直す
                self.push_log(format!("[{}w{idx}] ワーカーが落ちたので再起動します: {e}", self.tag));
                if let Some(mut p) = proc.take() {
                    p.kill();
                }
                // 読めていたワーカーが 1 本減った。読み直しの結果は状態に入れる
                {
                    let mut st = self.status.write();
                    st.ready_workers = st.ready_workers.saturating_sub(1);
                }
                let now = std::time::Instant::now();
                crashes.push_back(now);
                while crashes.front().is_some_and(|t| now.duration_since(*t) > Duration::from_secs(60)) {
                    crashes.pop_front();
                }
                if let Some(m) = loaded.take() {
                    let r = if crashes.len() >= 3 {
                        // 必ず落ちるコマがあると作り直しを繰り返すので、1 分に 3 回落ちたら止める（読み直すと戻る）
                        Err(format!("[{}w{idx}] 1 分間に 3 回落ちたので止めました（再読み込みで再開します）:\n{e:#}", self.tag))
                    } else {
                        self.do_load(idx, &mut proc, &m)
                    };
                    let ok = r.is_ok();
                    // 読み込みの結果を先に入れてから調整を渡し直す（finish_load が警告を消すので、渡し直しで断られた分を残すため）
                    self.finish_load(idx, &m, r);
                    if ok {
                        if let (Some(p), Some(v)) = (proc.as_mut(), last_ov.as_ref()) {
                            self.send_overrides(idx, p, v, self.gen.load(Ordering::SeqCst));
                        }
                    }
                    loaded = ok.then_some(m);
                }
            }
        }
    }

    /// プロセスを止めて捨てる。読み込みも無くなるので、次の Load までは仕事を断る
    fn drop_proc(proc: &mut Option<Proc>, loaded: &mut Option<Ctrl>) {
        if let Some(mut p) = proc.take() {
            let _ = p.call(&serde_json::json!({"op": "quit"}));
            p.kill();
        }
        *loaded = None;
    }

    fn do_load(&self, idx: usize, proc: &mut Option<Proc>, m: &Ctrl) -> Result<Desc, String> {
        let Ctrl::Load { timeline, overrides, clips, placeholder, gen } = m else { unreachable!() };
        if proc.is_none() {
            match self.spawn(idx) {
                Ok(p) => *proc = Some(p),
                Err(e) => return Err(format!("{e:#}")),
            }
        }
        let p = proc.as_mut().unwrap();
        let req = serde_json::json!({"op": "load", "timeline": timeline, "overrides": overrides,
                                      "clips": clips, "placeholder": placeholder});
        match p.call(&req) {
            Ok((h, _)) => {
                if h.get("ok").and_then(|v| v.as_bool()) == Some(true) {
                    let mut d: Desc = serde_json::from_value(h).map_err(|e| format!("演出の内容を読み取れませんでした: {e}"))?;
                    d.path = timeline.clone();
                    let _ = gen;
                    Ok(d)
                } else {
                    Err(h.get("error").and_then(|v| v.as_str()).unwrap_or("演出を読み込めませんでした").to_string())
                }
            }
            Err(e) => {
                if let Some(mut pp) = proc.take() {
                    pp.kill();
                }
                Err(format!("{e:#}"))
            }
        }
    }

    fn finish_load(&self, idx: usize, m: &Ctrl, r: Result<Desc, String>) {
        let mut first_ok = false;
        {
            // gen を確かめてから desc を入れるまで status を持つ（load・unload は status を持って gen を進めるので、
            // 確かめた後に別の回を開かれて、前の回の desc を入れてしまうことがない）
            let mut st = self.status.write();
            // 古い読み込み（別の回を開いた・unload した後に終わったもの）の結果は状態にも desc にも入れない
            if matches!(m, Ctrl::Load { gen: g, .. } if *g != self.gen.load(Ordering::SeqCst)) {
                return;
            }
            match &r {
                Ok(d) => {
                    // 読み込みは調整のファイルも読む（読めない値なら読み込みが失敗する）ので、このワーカーが前に断った調整の警告は消す。
                    // 作り直しで last_ov を渡し直すときは、この後に send_overrides が結果を入れる
                    if let Some(slot) = self.ov_error.lock().get_mut(idx) {
                        *slot = None;
                    }
                    st.ready_workers += 1;
                    if st.state != "ready" {
                        st.state = "ready".into();
                        st.error = None;
                        st.build_sec = d.build_sec;
                        first_ok = true;
                    }
                }
                Err(e) => {
                    st.failed_workers += 1;
                    if st.worker_error.is_none() {
                        st.worker_error = Some(e.clone());
                    }
                    if st.ready_workers == 0 {
                        st.state = "error".into();
                        st.error = Some(e.clone());
                    }
                    self.push_log(format!("[{}w{idx}] 演出を読み込めませんでした:\n{e}", self.tag));
                }
            }
            if first_ok {
                if let Ok(d) = r {
                    if let Some(f) = &d.fxkit {
                        self.push_log(format!("[{}w{idx}] fxkit: {f}", self.tag));
                    }
                    *self.desc.write() = Some(Arc::new(d));
                }
            }
        }
        self.notify(if first_ok { "desc" } else { "status" });
    }

    fn run_job(&self, p: &mut Proc, job: Job) -> Result<()> {
        match job {
            Job::Frame(r, tx) => {
                let req = serde_json::json!({"op": "frame", "t": r.t, "scale": r.scale, "hide": r.hide,
                                              "solo": r.solo});
                match p.call(&req) {
                    Ok((h, data)) => {
                        let _ = tx.send(parse_frame(&h, data));
                        Ok(())
                    }
                    Err(e) => {
                        let _ = tx.send(Err(crate::app::error::coded(ak_proto::ErrorCode::EngineDown, format!("{e:#}"))));
                        Err(e)
                    }
                }
            }
            Job::Raw(v, tx) => match p.call(&v) {
                Ok((h, data)) => {
                    if h.get("ok").and_then(|x| x.as_bool()) == Some(false) {
                        let msg = h.get("error").and_then(|x| x.as_str()).unwrap_or("ワーカーでエラーが起きました").to_string();
                        let _ = tx.send(Err(anyhow!(msg)));
                    } else {
                        let _ = tx.send(Ok((h, data)));
                    }
                    Ok(())
                }
                Err(e) => {
                    let _ = tx.send(Err(crate::app::error::coded(ak_proto::ErrorCode::EngineDown, format!("{e:#}"))));
                    Err(e)
                }
            },
        }
    }

    /// 動いているワーカーの本数（作れなかったスレッドは数えない。書き出しはこの本数が読み終わるのを待つ）
    pub fn workers(&self) -> usize {
        self.ctrl.lock().len()
    }
}

pub(crate) fn push_log(log: &Mutex<VecDeque<String>>, s: String) {
    let mut l = log.lock();
    l.push_back(s);
    while l.len() > 800 {
        l.pop_front();
    }
}

/// 依頼に失敗を返す。種類は付けない（ワーカーの停止・通信の失敗は `run_job` などが `EngineDown` を付ける。
/// 演出がまだ読み込まれていないだけ、は止まっているわけではないので `Failed` のまま）
fn fail(job: Job, e: anyhow::Error) {
    match job {
        Job::Frame(_, tx) => {
            let _ = tx.send(Err(e));
        }
        Job::Raw(_, tx) => {
            let _ = tx.send(Err(e));
        }
    }
}

/// ワーカーの返事のヘッダの上限（これを超えたら通信が壊れているとみなす）
const MAX_HEADER: u64 = 16 * 1024 * 1024;
/// ワーカーの返事のデータ（コマの RGBA など）の上限
const MAX_DATA: u64 = 512 * 1024 * 1024;

/// n バイトを読む（途中で切れていたらエラー）。n は呼び出し側で上限を確かめてあるので、先に確保してよい
fn read_exact_vec(r: &mut impl Read, n: u64, what: &str) -> Result<Vec<u8>> {
    let mut v = Vec::with_capacity(n as usize);
    r.take(n).read_to_end(&mut v)?;
    if v.len() as u64 != n {
        bail!("ワーカーの返事が途中で切れています（{what} {n} バイトのうち {} バイト）", v.len());
    }
    Ok(v)
}

/// ワーカーの返事を 1 通読む（4 バイトのヘッダ長 + JSON のヘッダ + nbytes 分のデータ）。壊れていたらエラー
fn read_msg(r: &mut impl Read) -> Result<(Value, Vec<u8>)> {
    let mut lenb = [0u8; 4];
    r.read_exact(&mut lenb).context("ワーカーが終了しました")?;
    let n = u32::from_le_bytes(lenb) as u64;
    if n > MAX_HEADER {
        bail!("ワーカーの返事が壊れています（ヘッダ長 {n} バイト）");
    }
    let hb = read_exact_vec(r, n, "ヘッダ")?;
    let h: Value = serde_json::from_slice(&hb).context("ワーカーの返事が壊れています（ヘッダが JSON ではありません）")?;
    let nb = match h.get("nbytes") {
        None => 0,
        Some(v) => v.as_u64().ok_or_else(|| anyhow!("ワーカーの返事が壊れています（nbytes が数ではありません）"))?,
    };
    if nb > MAX_DATA {
        bail!("ワーカーの返事が壊れています（データ長 {nb} バイト）");
    }
    let data = read_exact_vec(r, nb, "データ")?;
    Ok((h, data))
}

fn parse_frame(h: &Value, data: Vec<u8>) -> Result<OverlayFrame> {
    if h.get("ok").and_then(|v| v.as_bool()) != Some(true) {
        bail!("{}", h.get("error").and_then(|v| v.as_str()).unwrap_or("演出のコマを描けませんでした"));
    }
    let w = h["w"].as_u64().unwrap_or(0);
    let hh = h["h"].as_u64().unwrap_or(0);
    let rect = match h.get("rect") {
        None | Some(Value::Null) => {
            if !data.is_empty() {
                bail!("演出のコマの返事が壊れています（rect が無いのにデータが {} バイトあります）", data.len());
            }
            None
        }
        Some(b) => {
            let a: Vec<u64> = b
                .as_array()
                .filter(|a| a.len() == 4)
                .and_then(|a| a.iter().map(|v| v.as_u64()).collect())
                .ok_or_else(|| anyhow!("演出のコマの返事が壊れています（rect）"))?;
            let (x, y, bw, bh) = (a[0], a[1], a[2], a[3]);
            if x.saturating_add(bw) > w || y.saturating_add(bh) > hh {
                bail!("演出のコマの返事が壊れています（rect [{x}, {y}, {bw}, {bh}] がコマ {w}x{hh} からはみ出しています）");
            }
            let want = bw.saturating_mul(bh).saturating_mul(4);
            if data.len() as u64 != want {
                bail!("演出のコマの返事が壊れています（データが {} バイトで、rect から求めた {want} バイトと合いません）", data.len());
            }
            Some([x as u32, y as u32, bw as u32, bh as u32])
        }
    };
    let (w, hh) = (w as u32, hh as u32);
    Ok(OverlayFrame { w, h: hh, rect, rgba: data, ms: h["ms"].as_f64().unwrap_or(0.0) })
}

/// 埋め込みの fxkit を書き出せなかった理由（次に書き出せたら消える）
static FXKIT_WRITE_ERROR: Mutex<Option<String>> = Mutex::new(None);

/// 埋め込みの fxkit を書き出せなかったときの警告（画面の警告に足す。回の fx/ か pip に fxkit があれば演出は動くが、無いと読めない）
pub fn fxkit_write_warning() -> Option<Warning> {
    FXKIT_WRITE_ERROR.lock().clone().map(|e| Warning {
        key: "fxkit-write".into(),
        message: format!("{e}（回の fx/ や pip に fxkit が無いと、演出を読み込めません）"),
        level: None,
    })
}

/// ワーカーのプロセスを 1 本起動する（字幕だけのワーカー subs.rs でも使う）。
pub(crate) fn spawn_proc(python: &str, script: &Path, log: Arc<Mutex<VecDeque<String>>>, tag: &str) -> Result<Proc> {
    let mut cmd = hidden(Command::new(python));
    cmd.arg("-u")
        .arg(script)
        .env("PYTHONUTF8", "1")
        .env("PYTHONIOENCODING", "utf-8")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // 回の fx/・pip で入れた版のどちらにも fxkit が無いときのために、アプリに埋め込んだ版の場所を渡す（sys.path の末尾に足される）
    match crate::settings::fxkit_dir() {
        Ok(d) => {
            *FXKIT_WRITE_ERROR.lock() = None;
            cmd.env("AK_FXKIT_FALLBACK", d);
        }
        Err(e) => {
            // ログだけでは見えないので、画面が出す警告（engine_info の warnings）にも載せる
            push_log(&log, format!("[{tag}] {e}"));
            *FXKIT_WRITE_ERROR.lock() = Some(e);
        }
    }
    let mut child = cmd.spawn().with_context(|| format!("python を起動できません: {python}"))?;
    let stdin = child.stdin.take().unwrap();
    let stdout = BufReader::with_capacity(1 << 20, child.stdout.take().unwrap());
    let stderr = child.stderr.take().unwrap();
    let tag = tag.to_string();
    let log2 = log.clone();
    let tag2 = tag.clone();
    std::thread::spawn(move || {
        let r = BufReader::new(stderr);
        for line in r.lines().map_while(Result::ok) {
            push_log(&log2, format!("[{tag2}] {line}"));
        }
    });
    let child = Arc::new(Mutex::new(child));
    let watch = Arc::new(Watch::default());
    // 見張り: 依頼が決めた時間を過ぎても返ってこない（fx の無限ループ・返ってこない子プロセス）なら、
    // プロセスを止めて read_exact を抜けさせる（抜けたら呼び出し側が作り直す）
    {
        let (child, watch) = (Arc::downgrade(&child), Arc::downgrade(&watch));
        std::thread::spawn(move || loop {
            std::thread::sleep(Duration::from_millis(250));
            let (Some(c), Some(w)) = (child.upgrade(), watch.upgrade()) else { return };
            let dl = *w.deadline.lock();
            if let Some((d, op)) = dl {
                if std::time::Instant::now() > d && !w.killed.swap(true, Ordering::SeqCst) {
                    push_log(&log, format!("[{tag}] {op} が時間内に終わらないので、ワーカーを止めます"));
                    let _ = c.lock().kill();
                }
            }
        });
    }
    Ok(Proc { child, stdin, stdout, next_id: 1, watch })
}

#[derive(Default)]
struct Watch {
    /// 今の依頼の締め切りと op（None なら待っていない）
    deadline: Mutex<Option<(std::time::Instant, &'static str)>>,
    killed: std::sync::atomic::AtomicBool,
}

/// 依頼の種類ごとの、返ってくるまでの上限。呼び出し側の待ち時間（モニター 20 秒・書き出し 300 秒）より長くする。効果音（export_se）は読み込みを含めて 600 秒で待ち、過ぎたら自分で止める
fn op_timeout(op: &str) -> (Duration, &'static str) {
    match op {
        "load" => (Duration::from_secs(300), "load"),
        "frame" => (Duration::from_secs(330), "frame"),
        "se" => (Duration::from_secs(900), "se"),
        "subtitle" => (Duration::from_secs(60), "subtitle"),
        "scene_label" => (Duration::from_secs(60), "scene_label"),
        "subtitle_measure" => (Duration::from_secs(120), "subtitle_measure"),
        "quit" => (Duration::from_secs(5), "quit"),
        #[cfg(test)]
        "hang_test" => (Duration::from_secs(1), "hang_test"),
        _ => (Duration::from_secs(300), "op"),
    }
}

/// ほかのスレッドから止める手（依頼の途中でも read が切れて call が抜ける。書き出しの中止用）
pub(crate) struct ProcKiller(std::sync::Weak<Mutex<Child>>);

impl ProcKiller {
    pub(crate) fn kill(&self) {
        if let Some(c) = self.0.upgrade() {
            let _ = c.lock().kill();
        }
    }
}

pub(crate) struct Proc {
    child: Arc<Mutex<Child>>,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next_id: u64,
    watch: Arc<Watch>,
}

impl Proc {
    pub(crate) fn killer(&self) -> ProcKiller {
        ProcKiller(Arc::downgrade(&self.child))
    }

    /// プロセスの番号（止めた後に、そのプロセスが残した一時ファイルを片付けるため）
    pub(crate) fn pid(&self) -> u32 {
        self.child.lock().id()
    }

    pub(crate) fn kill(&mut self) {
        let mut c = self.child.lock();
        let _ = c.kill();
        let _ = c.wait();
    }

    pub(crate) fn call(&mut self, req: &Value) -> Result<(Value, Vec<u8>)> {
        let (limit, op) = op_timeout(req.get("op").and_then(|v| v.as_str()).unwrap_or(""));
        *self.watch.deadline.lock() = Some((std::time::Instant::now() + limit, op));
        let r = self.call_inner(req);
        *self.watch.deadline.lock() = None;
        if r.is_err() && self.watch.killed.load(Ordering::SeqCst) {
            bail!("ワーカーが {} 秒以内に応答しないので止めました（{op}）", limit.as_secs());
        }
        r
    }

    fn call_inner(&mut self, req: &Value) -> Result<(Value, Vec<u8>)> {
        let mut req = req.clone();
        let id = self.next_id;
        self.next_id += 1;
        req["id"] = Value::from(id);
        let mut line = serde_json::to_vec(&req)?;
        line.push(b'\n');
        self.stdin.write_all(&line)?;
        self.stdin.flush()?;
        loop {
            let (h, data) = read_msg(&mut self.stdout)?;
            if h.get("id").and_then(|v| v.as_u64()) == Some(id) || h.get("id").is_none() {
                return Ok((h, data));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 返事を 1 通ぶんのバイト列にする
    fn msg_bytes(h: &Value, data: &[u8]) -> Vec<u8> {
        let hb = serde_json::to_vec(h).unwrap();
        let mut v = (hb.len() as u32).to_le_bytes().to_vec();
        v.extend(hb);
        v.extend(data);
        v
    }

    #[test]
    fn spawn_summary_counts_workers_and_reports_only_when_none() {
        assert_eq!(spawn_summary(4, &[]), (4, None));
        assert_eq!(spawn_summary(4, &[(1, "x".into())]), (3, None));
        let (w, e) = spawn_summary(2, &[(0, "a".into()), (1, "b".into())]);
        assert_eq!(w, 0);
        assert!(e.unwrap().contains('a'));
    }

    #[test]
    fn read_msg_reads_valid_messages() {
        let b = msg_bytes(&serde_json::json!({"ok": true, "nbytes": 3}), &[1, 2, 3]);
        let (h, d) = read_msg(&mut std::io::Cursor::new(b)).unwrap();
        assert_eq!(h["ok"], true);
        assert_eq!(d, vec![1, 2, 3]);
        let b = msg_bytes(&serde_json::json!({"ok": true}), &[]);
        let (_, d) = read_msg(&mut std::io::Cursor::new(b)).unwrap();
        assert!(d.is_empty());
    }

    #[test]
    fn read_msg_rejects_broken_messages() {
        // ヘッダ長が上限超え（本体は無い）
        let b = u32::MAX.to_le_bytes().to_vec();
        let e = read_msg(&mut std::io::Cursor::new(b)).unwrap_err();
        assert!(format!("{e:#}").contains("ヘッダ長"), "{e:#}");
        // nbytes が上限超え・数でない・負
        for nb in [serde_json::json!(MAX_DATA + 1), serde_json::json!("x"), serde_json::json!(-1)] {
            let b = msg_bytes(&serde_json::json!({"nbytes": nb}), &[]);
            assert!(read_msg(&mut std::io::Cursor::new(b)).is_err(), "{nb}");
        }
        // データが途中で切れている・ヘッダが途中で切れている・JSON でない
        let b = msg_bytes(&serde_json::json!({"nbytes": 10}), &[1, 2]);
        assert!(read_msg(&mut std::io::Cursor::new(b)).is_err());
        let mut b = msg_bytes(&serde_json::json!({"ok": true}), &[]);
        b.truncate(6);
        assert!(read_msg(&mut std::io::Cursor::new(b)).is_err());
        let mut b = 3u32.to_le_bytes().to_vec();
        b.extend(b"abc");
        assert!(read_msg(&mut std::io::Cursor::new(b)).is_err());
    }

    #[test]
    fn parse_frame_checks_rect_and_data() {
        let h = |rect: Value| serde_json::json!({"ok": true, "w": 100, "h": 50, "rect": rect});
        // 正しい
        let f = parse_frame(&h(serde_json::json!([10, 20, 2, 3])), vec![0; 24]).unwrap();
        assert_eq!(f.rect, Some([10, 20, 2, 3]));
        // 3 個（panic しない）・負・数でない
        assert!(parse_frame(&h(serde_json::json!([1, 2, 3])), vec![]).is_err());
        assert!(parse_frame(&h(serde_json::json!([1, 2, 3, -1])), vec![]).is_err());
        assert!(parse_frame(&h(serde_json::json!("x")), vec![]).is_err());
        // データの長さ違い
        assert!(parse_frame(&h(serde_json::json!([10, 20, 2, 3])), vec![0; 23]).is_err());
        // はみ出し
        assert!(parse_frame(&h(serde_json::json!([99, 0, 2, 1])), vec![0; 8]).is_err());
        assert!(parse_frame(&h(serde_json::json!([0, 49, 1, 2])), vec![0; 8]).is_err());
        // rect が null でデータ空 → Ok、データありなら Err
        let f = parse_frame(&h(Value::Null), vec![]).unwrap();
        assert_eq!(f.rect, None);
        assert!(parse_frame(&h(Value::Null), vec![0; 4]).is_err());
        // ok でない返事はそのまま理由を返す
        assert!(parse_frame(&serde_json::json!({"ok": false, "error": "x"}), vec![]).is_err());
    }

    fn desc_json(extra: Value) -> Value {
        let mut v = serde_json::json!({"path": "x.py", "end": 10.0, "fps": 30.0, "w": 1920, "h": 1080, "groups": [],
            "cues": [], "sync": [], "sync_used": [], "meta": {}, "build_sec": 0.1, "stage": [], "rec_box": []});
        if let (Some(m), Some(e)) = (v.as_object_mut(), extra.as_object()) {
            m.extend(e.clone());
        }
        v
    }

    /// warnings が無い古いワーカーの返事も読める。有れば受け取る
    #[test]
    fn desc_warnings_default() {
        let d: Desc = serde_json::from_value(desc_json(serde_json::json!({}))).unwrap();
        assert!(d.warnings.is_empty());
        let d: Desc = serde_json::from_value(desc_json(serde_json::json!(
            {"warnings": [{"key": "gui:a", "message": "作成できませんでした"}]}))).unwrap();
        assert_eq!(d.warnings, vec![Warning { key: "gui:a".into(), message: "作成できませんでした".into(), level: None }]);
    }

    /// 返事の警告を足す: 増えた・文が変わったものだけを返す。warnings の無い返事は何もしない
    #[test]
    fn merge_warnings_reports_only_changes() {
        let w = |k: &str, m: &str| serde_json::json!({"key": k, "message": m});
        let mut dst = vec![];
        let f = merge_warnings(&mut dst, &serde_json::json!({"warnings": [w("font:a", "1"), w("font:b", "2")]}));
        assert_eq!(f.len(), 2);
        assert!(merge_warnings(&mut dst, &serde_json::json!({"warnings": [w("font:a", "1")]})).is_empty());
        let f = merge_warnings(&mut dst, &serde_json::json!({"warnings": [w("font:a", "3")]}));
        assert_eq!(f, vec![Warning { key: "font:a".into(), message: "3".into(), level: None }]);
        assert_eq!(dst.len(), 2);
        assert_eq!(dst[0].message, "3");
        assert!(merge_warnings(&mut dst, &serde_json::json!({"ok": true})).is_empty());
        assert!(merge_warnings(&mut dst, &serde_json::json!({"warnings": "壊れた形"})).is_empty());
        assert_eq!(dst.len(), 2);
        // 形の違う要素だけ飛ばし、ほかは受け取る（Desc も同じ）
        let f = merge_warnings(&mut dst, &serde_json::json!({"warnings": [{"key": 1}, w("font:c", "4")]}));
        assert_eq!(f, vec![Warning { key: "font:c".into(), message: "4".into(), level: None }]);
        let d: Desc = serde_json::from_value(desc_json(serde_json::json!({"warnings": [7, w("se", "5")]}))).unwrap();
        assert_eq!(d.warnings.len(), 1);
    }

    /// 状態が cond を満たすまで待つ（最大 15 秒）
    fn wait_status(e: &Engine, cond: impl Fn(&EngineStatus) -> bool) -> EngineStatus {
        let t0 = std::time::Instant::now();
        loop {
            let st = e.status.read().clone();
            if cond(&st) || t0.elapsed() > Duration::from_secs(15) {
                return st;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// Python を変えたら、動いているワーカーを止めて新しい Python で作り直す（旧#31）
    #[test]
    fn set_python_respawns_workers() {
        let py = crate::settings::resolve_python("python");
        let dir = std::env::temp_dir().join(format!("aks-respawn-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // load に desc を返すだけの偽のワーカー
        let script = dir.join("fake.py");
        let desc = desc_json(serde_json::json!({}));
        std::fs::write(&script, format!(
            "import sys, json, struct\nDESC = json.loads({:?})\nfor line in sys.stdin:\n    req = json.loads(line)\n    \
             if req['op'] == 'quit':\n        break\n    h = dict(DESC, ok=True, id=req['id'])\n    b = json.dumps(h).encode()\n    \
             sys.stdout.buffer.write(struct.pack('<I', len(b)) + b)\n    sys.stdout.buffer.flush()\n",
            desc.to_string()
        )).unwrap();
        let tl = dir.join("x_fx.py");
        let e = Engine::new(&py, script, 2);
        e.load(&tl, &dir.join("o.json"), &dir.join("c.json"), false);
        let st = wait_status(&e, |s| s.ready_workers == 2);
        if st.state != "ready" {
            return; // python が無い環境
        }
        // 起動できない Python に変えて読み直すと、全部のワーカーがその Python で作り直そうとして失敗する
        e.set_python("ak-studio-no-such-python");
        e.load(&tl, &dir.join("o.json"), &dir.join("c.json"), false);
        let st = wait_status(&e, |s| s.failed_workers == 2);
        assert_eq!(st.ready_workers, 0, "{st:?}");
        assert_eq!(st.state, "error");
        assert!(st.error.as_deref().unwrap_or("").contains("ak-studio-no-such-python"), "{st:?}");
        // 戻せばまた読める
        e.set_python(&py);
        e.load(&tl, &dir.join("o.json"), &dir.join("c.json"), false);
        let st = wait_status(&e, |s| s.ready_workers == 2);
        assert_eq!(st.state, "ready", "{st:?}");
        e.shutdown();
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// ワーカーが調整を断ったら警告に出し、次に受け付けたら消す（旧#141）
    #[test]
    fn rejected_overrides_become_warning() {
        let py = crate::settings::resolve_python("python");
        let dir = std::env::temp_dir().join(format!("aks-ovwarn-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // load には desc を、overrides には "bad" があれば断る偽のワーカー
        let script = dir.join("fake.py");
        let desc = desc_json(serde_json::json!({}));
        std::fs::write(&script, format!(
            "import sys, json, struct\nDESC = json.loads({:?})\nfor line in sys.stdin:\n    req = json.loads(line)\n    \
             if req['op'] == 'quit':\n        break\n    if req['op'] == 'frame':\n        sys.exit(1)\n    \
             h = dict(DESC, ok=True, id=req['id'])\n    \
             if req['op'] == 'overrides' and 'bad' in req['overrides']:\n        h = {{'ok': False, 'error': 'bad の end が数ではありません', 'id': req['id']}}\n    \
             b = json.dumps(h).encode()\n    \
             sys.stdout.buffer.write(struct.pack('<I', len(b)) + b)\n    sys.stdout.buffer.flush()\n",
            desc.to_string()
        )).unwrap();
        let tl = dir.join("x_fx.py");
        let e = Engine::new(&py, script, 2);
        e.load(&tl, &dir.join("o.json"), &dir.join("c.json"), false);
        let st = wait_status(&e, |s| s.ready_workers == 2);
        if st.state != "ready" {
            return; // python が無い環境
        }
        let wait_warn = |want: bool| {
            let t0 = std::time::Instant::now();
            while e.overrides_warning().is_some() != want && t0.elapsed() < Duration::from_secs(15) {
                std::thread::sleep(Duration::from_millis(50));
            }
            e.overrides_warning()
        };
        e.set_overrides(&serde_json::json!({"bad": {"end": "x"}}));
        let w = wait_warn(true).expect("断った調整は警告に出る");
        assert_eq!(w.key, "overrides");
        assert!(w.message.contains("bad の end が数ではありません"), "{w:?}");
        // ワーカーが落ちて作り直すと、断られた調整を渡し直して、警告は残る（読み込みの結果で消さない）
        let req = FrameReq { t: 0.0, scale: 1.0, hide: vec![], solo: None };
        assert!(e.frame(req, true, Duration::from_secs(15)).is_err());
        std::thread::sleep(Duration::from_millis(1500));
        assert_eq!(wait_status(&e, |s| s.ready_workers == 2).ready_workers, 2);
        assert!(e.ov_error.lock().iter().all(|x| x.is_some()), "作り直したワーカーの分も警告は残る: {:?}", e.ov_error.lock());
        e.set_overrides(&serde_json::json!({"ok": {"end": 1.0}}));
        assert!(wait_warn(false).is_none(), "受け付けたら消える");
        // 読み込み直しでも消える
        e.set_overrides(&serde_json::json!({"bad": {}}));
        assert!(wait_warn(true).is_some());
        e.load(&tl, &dir.join("o.json"), &dir.join("c.json"), false);
        assert!(wait_warn(false).is_none(), "読み込めたら消える");
        // 演出の無い回に変えても消える
        e.set_overrides(&serde_json::json!({"bad": {}}));
        assert!(wait_warn(true).is_some());
        e.unload("なし");
        assert!(e.overrides_warning().is_none(), "回を変えたら消える");
        e.shutdown();
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 演出の無い回を開いたら（unload）、前の回の desc を捨てる。読み込み中だった前の回の結果も後から入らない
    #[test]
    fn unload_drops_desc_and_ignores_stale_load() {
        let py = crate::settings::resolve_python("python");
        let dir = std::env::temp_dir().join(format!("aks-unload-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let script = dir.join("fake.py");
        let desc = desc_json(serde_json::json!({}));
        std::fs::write(&script, format!(
            "import sys, json, struct\nDESC = json.loads({:?})\nfor line in sys.stdin:\n    req = json.loads(line)\n    \
             if req['op'] == 'quit':\n        break\n    h = dict(DESC, ok=True, id=req['id'])\n    b = json.dumps(h).encode()\n    \
             sys.stdout.buffer.write(struct.pack('<I', len(b)) + b)\n    sys.stdout.buffer.flush()\n",
            desc.to_string()
        )).unwrap();
        let tl = dir.join("x_fx.py");
        let e = Engine::new(&py, script, 2);
        e.load(&tl, &dir.join("o.json"), &dir.join("c.json"), false);
        let st = wait_status(&e, |s| s.ready_workers == 2);
        if st.state != "ready" {
            return; // python が無い環境
        }
        assert!(e.desc.read().is_some());
        e.unload("なし");
        assert!(e.desc.read().is_none());
        assert_eq!(e.status.read().state, "none");
        // 読み込みを出した直後に捨てる: 後から終わった読み込みは desc にも状態にも入らない
        e.load(&tl, &dir.join("o.json"), &dir.join("c.json"), false);
        e.unload("なし");
        std::thread::sleep(Duration::from_millis(1500));
        assert!(e.desc.read().is_none());
        let st = e.status.read().clone();
        assert_eq!((st.state.as_str(), st.ready_workers), ("none", 0), "{st:?}");
        // 次の回を開けばまた読める
        e.load(&tl, &dir.join("o.json"), &dir.join("c.json"), false);
        let st = wait_status(&e, |s| s.ready_workers == 2);
        assert_eq!(st.state, "ready", "{st:?}");
        assert!(e.desc.read().is_some());
        e.shutdown();
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 返ってこないワーカーは見張りが止め、call はエラーで抜ける
    #[test]
    fn hung_worker_is_killed() {
        let py = crate::settings::resolve_python("python");
        let dir = std::env::temp_dir().join(format!("aks-hang-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let script = dir.join("hang.py");
        std::fs::write(&script, "import sys, time\nsys.stdin.readline()\ntime.sleep(60)\n").unwrap();
        let Ok(mut p) = spawn_proc(&py, &script, Arc::new(Mutex::new(VecDeque::new())), "t") else { return };
        let t0 = std::time::Instant::now();
        let r = p.call(&serde_json::json!({"op": "hang_test"}));
        assert!(r.is_err());
        assert!(t0.elapsed() < Duration::from_secs(10));
        assert!(format!("{:#}", r.unwrap_err()).contains("応答しない"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 別のスレッドから ProcKiller で止めると、呼び出し中の call がすぐ抜ける
    #[test]
    fn killer_stops_a_running_call() {
        let py = crate::settings::resolve_python("python");
        let dir = std::env::temp_dir().join(format!("aks-killer-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let script = dir.join("hang.py");
        std::fs::write(&script, "import sys, time
sys.stdin.readline()
time.sleep(60)
").unwrap();
        let Ok(mut p) = spawn_proc(&py, &script, Arc::new(Mutex::new(VecDeque::new())), "t") else { return };
        let k = p.killer();
        let t0 = std::time::Instant::now();
        let h = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(500));
            k.kill();
        });
        // se の見張り（900 秒）よりずっと前に、killer が止める
        let r = p.call(&serde_json::json!({"op": "se"}));
        h.join().unwrap();
        assert!(r.is_err());
        assert!(t0.elapsed() < Duration::from_secs(10));
        p.kill();
        let _ = std::fs::remove_dir_all(&dir);
    }
}
