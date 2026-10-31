//! Claude とのやり取り（チャット）と、台本・演出の推敲案。
//!
//! 画面で送ったメッセージは inbox に積まれ、MCP サーバー（ak-studio-mcp）が長いポーリングで受け取って、
//! Claude Code のセッションに channel の通知として流す。Claude の返事は MCP の reply ツールから戻ってくる。

use anyhow::Context;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use crate::app::dto::UiState;
use serde_json::{json, Value};
use std::collections::VecDeque;
use std::io::Write;
use ts_rs::TS;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, TS)]
#[ts(export)]
pub struct ChatMsg {
    pub id: u64,
    pub ts: String,
    /// user / claude / system
    #[ts(type = "\"user\" | \"claude\" | \"system\"")]
    pub role: String,
    pub text: String,
    /// 送ったときの状況（再生位置・シーン・選んでいたクリップや行など）
    #[serde(default)]
    #[ts(type = "any")]
    pub ctx: Value,
    /// 画像（確認画像のパス）や、押すとその場所へ飛ぶリンク（{"t": 12.3} / {"clip": "telop@c15#1"} / {"line": "l12"}）
    #[serde(default)]
    #[ts(type = "any[]")]
    pub attachments: Vec<Value>,
    /// ユーザーのメッセージが Claude に届いたか: queued（まだ inbox）/ delivered（Claude が受け取った）/
    /// cancelled（届く前に取り消した）/ cancel_late（届いた後に取り消し、Claude に「無視して」と伝えた）/
    /// cancel_aborted（届いた後に取り消し、Mod が Claude の作業を止めた）。Claude の返事などは空
    #[serde(default, skip_serializing_if = "String::is_empty")]
    #[ts(type = "\"\" | \"queued\" | \"delivered\" | \"cancelled\" | \"cancel_late\" | \"cancel_aborted\" | \"undelivered\"")]
    pub state: String,
    /// Claude の返事へのリアクション（"like" = 好き！。空なら無し）
    #[serde(default, skip_serializing_if = "String::is_empty")]
    #[ts(type = "\"\" | \"like\"")]
    pub reaction: String,
}

/// 取り消しの結果
#[derive(Debug, Clone, PartialEq)]
pub enum Cancel {
    /// まだ inbox にあった。Claude には届いていない
    Retracted(ChatMsg),
    /// もう届いていた。Claude への知らせ（2 つ目）を inbox に積んだ
    Late(ChatMsg, ChatMsg),
}

/// 届いた後に取り消したときに Claude へ送る知らせ
pub fn cancel_notice_text(m: &ChatMsg) -> String {
    let mut head: String = m.text.chars().take(80).collect();
    if m.text.chars().count() > 80 {
        head.push('…');
    }
    format!(
        "【取り消し】ユーザーが直前のメッセージを取り消しました（送り間違い）。\n\
         取り消したメッセージ: 「{head}」\n\
         この指示は実行しないでください。もう手を付けていたらそこで止めて、何をしたかを reply で短く伝え、\
         元に戻すかどうかはユーザーに確かめてください。"
    )
}

pub fn is_cancel_notice(m: &ChatMsg) -> bool {
    m.ctx.get("kind").and_then(|v| v.as_str()) == Some("cancel_notice")
}

/// アプリが代わりに送ったメッセージ（推敲案の結果・取り消しの知らせ）。画面の chatcancel.ts の AUTO_KINDS と同じ
pub fn is_auto(m: &ChatMsg) -> bool {
    matches!(m.ctx.get("kind").and_then(|v| v.as_str()), Some("cancel_notice") | Some("proposal_result"))
}

/// 分:秒.2 桁。先に 1/100 秒に丸めてから分と秒に分ける（59.996 秒が 0:60.00 にならないように）
pub fn fmt_t(t: f64) -> String {
    let c = (t.max(0.0) * 100.0).round() as i64;
    format!("{}:{:05.2}", c / 6000, (c % 6000) as f64 / 100.0)
}

/// 送ったときの状況（ctx）を、「再生位置 …」「シーン …」などの言い回しにする。無いものは出さない
pub fn situation(ctx: &Value) -> Vec<String> {
    let mut out = vec![];
    if let Some(t) = ctx["t"].as_f64() {
        out.push(format!("再生位置 {}（{t:.2} 秒）", fmt_t(t)));
    }
    match &ctx["scene"] {
        Value::String(s) if !s.is_empty() => out.push(format!("シーン {s}")),
        Value::Number(n) => out.push(format!("シーン {n}")),
        _ => {}
    }
    if let Some(c) = ctx["clip"].as_str().filter(|c| !c.is_empty()) {
        let src = ctx["clip_src"].as_str().filter(|s| !s.is_empty()).map(|s| format!("（コード: {s}）")).unwrap_or_default();
        out.push(format!("選んでいる演出クリップ {c}{src}"));
    }
    if let Some(ls) = ctx["lines"].as_array().filter(|l| !l.is_empty()) {
        let ns: Vec<String> = ls.iter().map(|x| x.as_str().map(String::from).unwrap_or_else(|| x.to_string())).collect();
        out.push(format!("選んでいる台本の行 {}", ns.join(", ")));
    }
    out
}

/// 色の名前（印の文と画面で同じ意味）
fn mark_color_name(c: &str) -> &str {
    ak_proto::MARK_COLORS.iter().find(|(k, _)| *k == c).map(|(_, n)| *n).unwrap_or(c)
}

/// 点の列の外接矩形 (x, y, 幅, 高さ)
fn bounds(ps: &[[i32; 2]]) -> (i32, i32, i32, i32) {
    let xs = ps.iter().map(|p| p[0]);
    let ys = ps.iter().map(|p| p[1]);
    let (x0, x1) = (xs.clone().min().unwrap_or(0), xs.max().unwrap_or(0));
    let (y0, y1) = (ys.clone().min().unwrap_or(0), ys.max().unwrap_or(0));
    (x0, y0, x1 - x0, y1 - y0)
}

/// 印 1 つを Claude に渡す 1 行にする。`media_name` は素材のクリップ id から名前を引く
pub fn mark_line(m: &ak_proto::Mark, media_name: &dyn Fn(&str) -> Option<String>) -> String {
    use ak_proto::MarkShape::*;
    let mut s = format!("[印{}・{}]", ak_proto::circled(m.id), mark_color_name(&m.color));
    if !m.note.trim().is_empty() {
        s.push_str(&format!(" 「{}」", m.note.trim()));
    }
    let shape = match &m.shape {
        Pin { at } => format!("点 ({}, {})", at[0], at[1]),
        Rect { bbox } => format!("矩形 x={} y={} 幅{} 高さ{}", bbox[0], bbox[1], bbox[2], bbox[3]),
        Arrow { from, to } => {
            let (dx, dy) = (to[0] - from[0], to[1] - from[1]);
            format!("矢印 ({}, {}) → ({}, {})（{}へ {}・{}へ {}）", from[0], from[1], to[0], to[1],
                    if dx >= 0 { "右" } else { "左" }, dx.abs(), if dy >= 0 { "下" } else { "上" }, dy.abs())
        }
        Pen { points } => {
            let (x, y, w, h) = bounds(points);
            format!("線 外接 x={x} y={y} 幅{w} 高さ{h}（{} 点）", points.len())
        }
    };
    s.push_str(&format!(" {shape} / 区間 {}〜{}", fmt_t(m.t0), fmt_t(m.t1)));
    if !m.clips.is_empty() {
        s.push_str(&format!(" / 下の演出 {}", m.clips.join(", ")));
    }
    if !m.media.is_empty() {
        let v: Vec<String> = m.media.iter().map(|id| match media_name(id) {
            Some(n) if !n.is_empty() => format!("{id}（{n}）"),
            _ => id.clone(),
        }).collect();
        s.push_str(&format!(" / 下の素材 {}", v.join(", ")));
    }
    s
}

/// ctx.mark_found を要素ごとに読む。壊れた要素は飛ばし、その数を返す
pub fn read_mark_found(ctx: &Value) -> (Vec<ak_proto::MarkFound>, usize) {
    let Some(a) = ctx["mark_found"].as_array() else { return (vec![], 0) };
    let mut bad = 0;
    let found = a.iter().filter_map(|x| match serde_json::from_value::<ak_proto::MarkFound>(x.clone()) {
        Ok(f) => Some(f),
        Err(_) => {
            bad += 1;
            None
        }
    }).collect();
    (found, bad)
}

/// ctx.marks（印の番号の配列）を今の印に引いた行。無い番号は飛ばす。1 つも無ければ空
fn marks_lines(ctx: &Value, marks: &[ak_proto::Mark], media_name: &dyn Fn(&str) -> Option<String>) -> Vec<String> {
    let Some(ids) = ctx["marks"].as_array() else { return vec![] };
    // 送ったときに画面が調べた下のクリップ（ctx.mark_found）。印の記録より先に届いても同じ文になるように、こちらを優先する
    let (found, _) = read_mark_found(ctx);
    let mut out: Vec<String> = ids.iter().filter_map(|x| x.as_u64())
        .filter_map(|id| marks.iter().find(|m| m.id as u64 == id))
        .map(|m| match found.iter().find(|f| f.id == m.id) {
            Some(f) => mark_line(&ak_proto::Mark { clips: f.clips.clone(), media: f.media.clone(), ..m.clone() }, media_name),
            None => mark_line(m, media_name),
        })
        .collect();
    if !out.is_empty() {
        out.push("（印の座標は出力の画素 1920x1080。確認画像の丸数字が印の番号です）".into());
    }
    out
}

/// 画面のメッセージを Claude に渡す本文にする: 本文 → 画像 → 印 → 好き！ → [画面の状況]。
/// `marks` は今の印の全部（ctx.marks の番号を引く）、`media_name` は素材のクリップ id から名前を引く
pub fn prompt_text(m: &ChatMsg, marks: &[ak_proto::Mark], media_name: &dyn Fn(&str) -> Option<String>) -> String {
    let mut out = m.text.clone();
    for a in &m.attachments {
        if let Some(p) = a["image"].as_str() {
            out.push_str(&format!("\n（画像: {p}。Read で見てください）"));
        }
    }
    for l in marks_lines(&m.ctx, marks, media_name) {
        out.push('\n');
        out.push_str(&l);
    }
    // 前の返事に押した「好き！」（押したときは知らせず、次のメッセージにまとめて添える）
    if let Some(ls) = m.ctx["liked"].as_array() {
        for x in ls {
            if let Some(h) = x["head"].as_str() {
                out.push_str(&format!("\n[好き！] ユーザーが前の返事『{h}』に好き！を押しました"));
            }
        }
    }
    let sit = situation(&m.ctx);
    if !sit.is_empty() {
        out.push_str(&format!("\n\n[画面の状況] {}", sit.join(" / ")));
    }
    out
}

/// 採否を待っている推敲案の項目の数（画面の Claude タブのバッジと同じ数え方）
pub fn pending_items(props: &[Proposal]) -> usize {
    props.iter().flat_map(|p| &p.items).filter(|i| i.status == "pending").count()
}

/// ターミナルで打ったときに添える 1 行（`ui` は画面が送った状態。画面は scene を送らないので、再生位置から求めて渡す）
pub fn status_line(project: Option<&str>, ui: &UiState, scene: &Value) -> String {
    let mut ctx = serde_json::Map::new();
    ctx.insert("t".into(), json!(ui.t));
    ctx.insert("scene".into(), scene.clone());
    if let Some(sel) = ui.selection.as_ref().and_then(|s| s.as_object()).filter(|s| s.get("kind").and_then(|k| k.as_str()) == Some("fx")) {
        if let Some(k) = sel.get("key") {
            ctx.insert("clip".into(), k.clone());
        }
    }
    ctx.insert("lines".into(), json!(ui.lines));
    let mut parts = vec![format!("[AK Studio] 回 {}", project.unwrap_or("-"))];
    parts.extend(situation(&Value::Object(ctx)));
    parts.join(" / ")
}

/// 推敲案で足す・消すブロックの行の写し（block_add・block_delete）
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, TS)]
#[ts(export)]
pub struct ProposalLine {
    pub sp: String,
    pub text: String,
    #[serde(default)]
    pub memo: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, TS)]
#[ts(export)]
pub struct ProposalItem {
    /// edit / insert / delete / speaker / memo / notes / move / block_add / block_delete / block_rename
    #[ts(type = "\"edit\" | \"insert\" | \"delete\" | \"speaker\" | \"memo\" | \"notes\" | \"move\" | \"block_add\" | \"block_delete\" | \"block_rename\"")]
    pub kind: String,
    pub line_id: Option<String>,
    /// insert のとき、この行の後ろに入れる（None ならブロックの頭）
    pub after_id: Option<String>,
    pub block_id: Option<String>,
    #[serde(default)]
    pub old_text: String,
    #[serde(default)]
    pub new_text: String,
    #[serde(default)]
    pub sp: String,
    #[serde(default)]
    pub reason: String,
    /// pending / accepted / rejected / stale
    #[ts(type = "\"pending\" | \"accepted\" | \"rejected\" | \"stale\"")]
    pub status: String,
    /// insert を採用したときに置いた行の id（同じ置き場所の案を items の順に並べるのに使う）
    #[serde(default)]
    pub new_id: Option<String>,
    /// block_add の置き場所（このブロックの後ろ。None なら最後）
    #[serde(default)]
    pub after_block: Option<String>,
    /// block_add で足すブロックのメモ（notes）。block_delete では消すブロックの notes の写し
    #[serde(default)]
    pub notes: String,
    /// block_id のブロックのファイル名（ブロックの id は回を開き直すと変わるので、そのときはこれで引く）
    #[serde(default)]
    pub block_file: Option<String>,
    /// after_block のブロックのファイル名（同上）
    #[serde(default)]
    pub after_block_file: Option<String>,
    /// block_add で足す行・block_delete で消す行の写し
    #[serde(default)]
    pub lines: Vec<ProposalLine>,
}

impl ProposalItem {
    /// 結果の報告・一覧に出す 1 行分の中身（edit などは `old → new`）
    pub fn summary(&self) -> String {
        let n = self.lines.len();
        match self.kind.as_str() {
            "memo" => format!("行のメモ: {} → {}", self.old_text, self.new_text),
            "notes" => format!("ブロックのメモ: {} → {}", self.old_text, self.new_text),
            "move" => format!("行の移動: {}", self.old_text),
            "block_add" => format!("ブロックの追加: {}（{n} 行）", self.new_text),
            "block_delete" => format!("ブロックの削除: {}（{n} 行）", self.old_text),
            "block_rename" => format!("ブロックの改名: {} → {}", self.old_text, self.new_text),
            _ => format!("{} → {}", self.old_text, self.new_text),
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, TS)]
#[ts(export)]
pub struct Proposal {
    pub id: u64,
    pub ts: String,
    pub title: String,
    #[serde(default)]
    pub note: String,
    pub items: Vec<ProposalItem>,
}

impl Proposal {
    /// 決め終えた推敲案の結果として Claude に送る文。不採用の案は送らない（不採用なら、ユーザーが別に条件を伝える。旧#48）。
    /// 送るものが無い（すべて不採用）ときは None
    pub fn report_text(&self) -> Option<String> {
        let lines: Vec<String> = self
            .items
            .iter()
            .filter_map(|i| {
                let mark = match i.status.as_str() {
                    "accepted" => "採用",
                    "rejected" => return None,
                    "stale" => "適用できず（行が変わっていた）",
                    _ => "保留",
                };
                Some(format!("- {mark}: {}", i.summary()))
            })
            .collect();
        if lines.is_empty() {
            return None;
        }
        Some(format!("推敲案「{}」の結果:\n{}", self.title, lines.join("\n")))
    }
}

pub struct Hub {
    pub msgs: Mutex<Vec<ChatMsg>>,
    /// Claude にまだ渡していないメッセージと、渡してよくなる時刻（取り消しの猶予）
    inbox: Mutex<VecDeque<(ChatMsg, Instant)>>,
    pub notify: tokio::sync::Notify,
    next: AtomicU64,
    path: Mutex<Option<PathBuf>>,
    pub last_poll: Mutex<Option<Instant>>,
    pub proposals: Mutex<Vec<Proposal>>,
    prop_path: Mutex<Option<PathBuf>>,
    /// Claude が今やっていること（「台本を読んでいます」など。reply の status で更新）
    pub claude_status: Mutex<String>,
    /// Claude が作業中か、何をしているか（Mod が /api/agent/activity で知らせる）
    activity: Mutex<Activity>,
    /// フック方式の待ち受け（ak-studio-mcp --wait）。いちばん新しいものだけに渡す
    pub watcher: Mutex<Option<(u64, Instant)>>, // (起動時刻 ms, 最後に来た時刻)
    /// 保存の失敗を画面に知らせる口（Core が notice_error につなぐ）
    on_error: Mutex<Option<Box<dyn Fn(&anyhow::Error) + Send + Sync>>>,
    /// 最後に知らせた保存の失敗の文（同じ失敗をメッセージのたびに知らせない）。チャットと推敲案で別に持つ
    /// （1 つだと、両方が失敗し続けると文が交互に変わって毎回知らせ、片方の成功で覚えが消える）
    last_err_chat: Mutex<Option<String>>,
    last_err_prop: Mutex<Option<String>>,
}

/// Claude の作業状況（Mod の知らせ）。更新が途絶えたまま古くなったら、作業中とみなさない
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Activity {
    pub working: bool,
    pub text: String,
    /// 作業を始めた時刻（Unix ms）
    pub since_ms: u64,
    /// 最後に知らせが来た時刻（Unix ms）
    pub updated_ms: u64,
}

/// 更新がこれより途絶えたら、作業中の表示をやめる（Mod が落ちたとき用）
pub const ACTIVITY_STALE_MS: u64 = 10 * 60 * 1000;

pub fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

pub fn now() -> String {
    chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string()
}

/// チャットの履歴に 1 行足す
fn append_line(p: &std::path::Path, m: &ChatMsg) -> anyhow::Result<()> {
    let r = std::fs::OpenOptions::new().create(true).append(true).open(p).and_then(|mut f| {
        writeln!(f, "{}", serde_json::to_string(m).unwrap_or_default())
    });
    r.with_context(|| format!("チャットを保存できませんでした: {}", p.display()))
}

/// 読めなかったファイルを `<名前>.broken-<日時>` に写す（元のファイルは残す）。写した名前を返す
pub(crate) fn copy_broken(p: &std::path::Path) -> std::io::Result<String> {
    let file = p.file_name().unwrap_or_default().to_string_lossy().to_string();
    let name = format!("{file}.broken-{}", chrono::Local::now().format("%Y%m%d-%H%M%S"));
    std::fs::copy(p, p.with_file_name(&name))?;
    Ok(name)
}

impl Hub {
    pub fn new() -> Self {
        Hub {
            msgs: Mutex::new(vec![]),
            inbox: Mutex::new(VecDeque::new()),
            notify: tokio::sync::Notify::new(),
            next: AtomicU64::new(1),
            path: Mutex::new(None),
            last_poll: Mutex::new(None),
            proposals: Mutex::new(vec![]),
            prop_path: Mutex::new(None),
            claude_status: Mutex::new(String::new()),
            activity: Mutex::new(Activity::default()),
            watcher: Mutex::new(None),
            on_error: Mutex::new(None),
            last_err_chat: Mutex::new(None),
            last_err_prop: Mutex::new(None),
        }
    }

    pub fn set_on_error(&self, f: impl Fn(&anyhow::Error) + Send + Sync + 'static) {
        *self.on_error.lock() = Some(Box::new(f));
    }

    /// 保存の結果を受ける。失敗は、直前に知らせたのと同じ文でなければ知らせる。成功したら覚えを消す（次に失敗したらまた知らせる）
    fn report(&self, last_err: &Mutex<Option<String>>, r: anyhow::Result<()>) {
        match r {
            Ok(()) => *last_err.lock() = None,
            Err(e) => {
                let msg = format!("{e:#}");
                {
                    let mut last = last_err.lock();
                    if last.as_deref() == Some(msg.as_str()) {
                        return;
                    }
                    *last = Some(msg);
                }
                if let Some(f) = self.on_error.lock().as_ref() {
                    f(&e);
                }
            }
        }
    }

    /// 作業状況を更新する。変わったか（再描画が要るか）を返す。working=false で `claude_status` も消す
    pub fn set_activity(&self, working: bool, text: &str, now_ms: u64) -> bool {
        let mut a = self.activity.lock();
        let before = a.clone();
        if working {
            if !a.working {
                a.since_ms = now_ms;
            }
            a.working = true;
            a.text = text.to_string();
            // 新しい知らせが来たら、前の status（「メッセージを読んでいます」や前の reply の status）より優先する
            if !text.is_empty() {
                self.claude_status.lock().clear();
            }
        } else {
            *a = Activity::default();
            self.claude_status.lock().clear();
        }
        a.updated_ms = now_ms;
        // 時刻だけの更新（同じ文の知らせ）は変更とみなさない
        a.working != before.working || a.text != before.text || a.since_ms != before.since_ms
    }

    /// 今の作業状況（古すぎれば作業中ではない）
    pub fn activity_at(&self, now_ms: u64) -> Activity {
        let a = self.activity.lock().clone();
        if a.working && now_ms.saturating_sub(a.updated_ms) > ACTIVITY_STALE_MS {
            return Activity::default();
        }
        a
    }

    /// 回を開いたとき: その回のチャットと推敲案を読み込む。
    /// 読めなかったもの・壊れていたものがあれば、元のファイルを残したうえで、知らせる文を返す
    /// （そのまま次の保存で上書きして、黙って消さないため）。
    /// 開くときに知らせること（文と、あれば元の失敗の文）を返す
    pub fn open(&self, studio_dir: &std::path::Path) -> Vec<(String, Option<String>)> {
        let mut notices = vec![];
        *self.last_err_chat.lock() = None;
        *self.last_err_prop.lock() = None;
        let p = studio_dir.join("chat.jsonl");
        let mut chat_path = Some(p.clone());
        let mut msgs = vec![];
        // 壊れた行を写せたら、読めた行だけで書き直す（残すと、開くたびに写しと知らせが増える）
        let mut rewrite = false;
        match std::fs::read_to_string(&p) {
            Ok(t) => {
                let mut bad = 0;
                for l in t.lines().filter(|l| !l.trim().is_empty()) {
                    match serde_json::from_str::<ChatMsg>(l) {
                        Ok(m) => msgs.push(m),
                        Err(_) => bad += 1,
                    }
                }
                if bad > 0 {
                    match copy_broken(&p) {
                        Ok(name) => {
                            rewrite = true;
                            notices.push((format!("チャットの履歴の {bad} 行を読めませんでした。元のファイルは {name} に残しました"), None));
                        }
                        Err(e) => {
                            chat_path = None;
                            notices.push((format!(
                                "チャットの履歴の {bad} 行を読めず、元のファイルも残せませんでした（{}）。このあいだはチャットを保存しません",
                                crate::app::error::reason(e)), None));
                        }
                    }
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                chat_path = None;
                notices.push((format!("チャットの履歴を読めませんでした: {}: {}。このあいだはチャットを保存しません", p.display(), crate::app::error::reason(e)), None));
            }
        }
        for m in msgs.iter_mut() {
            if m.state == "queued" {
                // 前に開いたときに Claude に渡す前に閉じた（inbox は開き直すと空）。黙って消えないよう印を残す
                m.state = "undelivered".into();
            }
        }
        let max = msgs.iter().map(|m| m.id).max().unwrap_or(0);
        let pp = studio_dir.join("proposals.json");
        let mut prop_path = Some(pp.clone());
        let (v, warn) = crate::project::load_json_or_keep(&pp, "推敲案", serde_json::json!([]));
        // 退避できずにファイルが残っているときは、空のまま上書きしないよう保存しない
        let mut keep_off = warn.is_some() && pp.exists();
        let mut warn = warn.map(|w| if keep_off { format!("{w}このあいだは推敲案を保存しません") } else { w });
        let mut warn_detail = None;
        let props: Vec<Proposal> = match serde_json::from_value(v) {
            Ok(p) => p,
            Err(e) => {
                // 配列だが中身の形が違う。空で始めて上書きする前に、元のファイルを残す
                warn_detail = Some(format!("proposals.json: {e}"));
                let e = crate::app::error::reason(e);
                warn = Some(match copy_broken(&pp) {
                    Ok(name) => format!("推敲案（proposals.json）の形が違うため読み込めませんでした: {e}。元のファイルは {name} に残しました"),
                    Err(e2) => {
                        keep_off = true;
                        let e2 = crate::app::error::reason(e2);
                        format!("推敲案（proposals.json）の形が違うため読み込めず、元のファイルも残せませんでした（{e}／{e2}）。このあいだは推敲案を保存しません")
                    }
                });
                vec![]
            }
        };
        if keep_off {
            prop_path = None;
        }
        notices.extend(warn.map(|w| (w, warn_detail)));
        let pmax = props.iter().map(|p| p.id).max().unwrap_or(0);
        self.next.store(max.max(pmax) + 1, Ordering::SeqCst);
        *self.msgs.lock() = msgs;
        *self.path.lock() = chat_path;
        *self.proposals.lock() = props;
        *self.prop_path.lock() = prop_path;
        self.inbox.lock().clear();
        if rewrite {
            self.save_msgs();
        }
        notices
    }

    pub fn next_id(&self) -> u64 {
        self.next.fetch_add(1, Ordering::SeqCst)
    }

    pub fn push(&self, role: &str, text: &str, ctx: Value, attachments: Vec<Value>) -> ChatMsg {
        self.push_held(role, text, ctx, attachments, Duration::ZERO)
    }

    /// ユーザーのメッセージは hold のあいだ inbox で待たせる（その間なら取り消すと Claude に届かない）。
    pub fn push_held(&self, role: &str, text: &str, ctx: Value, attachments: Vec<Value>, hold: Duration) -> ChatMsg {
        let state = if role == "user" { "queued" } else { "" };
        let m = ChatMsg {
            id: self.next_id(),
            ts: now(),
            role: role.into(),
            text: text.into(),
            ctx,
            attachments,
            state: state.into(),
            reaction: String::new(),
        };
        let saved = {
            // path を持ったまま足して書く（save_msgs の書き直しと重なって、同じ行が 2 回書かれないように）
            let path = self.path.lock();
            self.msgs.lock().push(m.clone());
            match path.as_ref() {
                Some(p) => append_line(p, &m),
                None => Ok(()),
            }
        };
        self.report(&self.last_err_chat, saved);
        if role == "user" {
            self.inbox.lock().push_back((m.clone(), Instant::now() + hold));
            self.notify.notify_waiters();
        }
        m
    }

    /// チャットの履歴をまるごと書き直す（取り消し・届いた印など、前の行が変わったとき）
    fn save_msgs(&self) {
        let saved = match self.path.lock().as_ref() {
            Some(p) => {
                let body: String =
                    self.msgs.lock().iter().map(|m| serde_json::to_string(m).unwrap_or_default() + "\n").collect();
                crate::settings::write_atomic(p, body)
                    .with_context(|| format!("チャットを保存できませんでした: {}", p.display()))
            }
            None => Ok(()),
        };
        self.report(&self.last_err_chat, saved);
    }

    /// Claude の返事に「好き！」を付ける・外す（reaction が空なら外す）。書き換えたメッセージを返す。
    pub fn react(&self, id: u64, reaction: &str) -> anyhow::Result<ChatMsg> {
        if !matches!(reaction, "" | "like") {
            anyhow::bail!("リアクションは like だけです");
        }
        let m = {
            let mut msgs = self.msgs.lock();
            let Some(m) = msgs.iter_mut().find(|m| m.id == id) else { anyhow::bail!("メッセージ {id} が見つかりません") };
            if m.role != "claude" {
                anyhow::bail!("リアクションは Claude の返事にだけ付けられます");
            }
            m.reaction = reaction.into();
            m.clone()
        };
        self.save_msgs();
        Ok(m)
    }

    /// Claude に届いていないメッセージを取り出す（猶予が過ぎたものだけ）。届いた印を付ける。
    pub fn drain(&self) -> Vec<ChatMsg> {
        self.drain_where(|_| true)
    }

    /// drain のうち、条件に合うものだけ（Mod が Claude の作業中に、取り消しの知らせだけを受け取る）。
    pub fn drain_where(&self, pred: impl Fn(&ChatMsg) -> bool) -> Vec<ChatMsg> {
        *self.last_poll.lock() = Some(Instant::now());
        let now = Instant::now();
        let out: Vec<ChatMsg> = {
            // msgs → inbox の順に持ったまま、inbox から抜くのと「届いた」の印を一度にやる
            // （cancel も同じ順に持つので、取り消しと渡すのが同時でも、どちらか一方だけになる）
            let mut msgs = self.msgs.lock();
            let mut q = self.inbox.lock();
            let mut out = vec![];
            q.retain(|(m, at)| {
                if *at <= now && pred(m) {
                    out.push(m.clone());
                    false
                } else {
                    true
                }
            });
            for o in out.iter_mut() {
                o.state = "delivered".into();
                if let Some(x) = msgs.iter_mut().find(|x| x.id == o.id) {
                    x.state = "delivered".into();
                }
            }
            out
        };
        if !out.is_empty() {
            self.save_msgs();
        }
        out
    }

    /// 猶予中のメッセージのうち、いちばん早く渡せるようになる時刻
    pub fn next_ready(&self) -> Option<Instant> {
        self.inbox.lock().iter().map(|(_, at)| *at).min()
    }

    /// 送ったメッセージを取り消す。まだ inbox にあれば抜いて Claude に届かないようにする。
    /// もう届いていれば、Claude に「無視して」の知らせを積む（猶予なしですぐ渡す）。
    pub fn cancel(&self, id: u64) -> anyhow::Result<Cancel> {
        // 調べてから印を付けるまで msgs を持ったまま（同時に 2 回取り消しても、知らせは 1 通だけ）。
        // inbox は drain と同じく msgs の後に持つ
        let (m, removed) = {
            let mut msgs = self.msgs.lock();
            let Some(m) = msgs.iter_mut().find(|m| m.id == id) else { anyhow::bail!("メッセージ {id} が見つかりません") };
            if m.role != "user" || is_auto(m) {
                anyhow::bail!("取り消せるのは自分が送ったメッセージだけです");
            }
            if m.state.starts_with("cancel") {
                anyhow::bail!("このメッセージはもう取り消しています");
            }
            if m.state != "queued" && m.state != "delivered" {
                anyhow::bail!("前回開いたときのメッセージは取り消せません");
            }
            let removed = {
                let mut q = self.inbox.lock();
                let n = q.len();
                q.retain(|(x, _)| x.id != id);
                q.len() != n
            };
            m.state = if removed { "cancelled" } else { "cancel_late" }.into();
            (m.clone(), removed)
        };
        if removed {
            self.save_msgs();
            return Ok(Cancel::Retracted(m));
        }
        let notice = self.push("user", &cancel_notice_text(&m), serde_json::json!({"kind": "cancel_notice", "cancel_of": id}), vec![]);
        self.save_msgs();
        Ok(Cancel::Late(m, notice))
    }

    /// Mod が Claude の作業（取り消したメッセージで始まったターン）を止めたことの知らせ。
    /// 届いた後に取り消したもの（cancel_late）だけ
    pub fn ack_abort(&self, id: u64) -> Option<ChatMsg> {
        let m = {
            let mut msgs = self.msgs.lock();
            let m = msgs.iter_mut().find(|m| m.id == id && m.state == "cancel_late")?;
            m.state = "cancel_aborted".into();
            m.clone()
        };
        self.save_msgs();
        Some(m)
    }

    pub fn pending(&self) -> usize {
        self.inbox.lock().len()
    }

    pub fn connected(&self) -> bool {
        self.last_poll.lock().map(|t| t.elapsed().as_secs() < 70).unwrap_or(false)
    }

    /// フックの待ち受けが今いるか
    pub fn watching(&self) -> bool {
        self.watcher.lock().map(|(_, t)| t.elapsed().as_secs() < 70).unwrap_or(false)
    }

    pub fn save_proposals(&self) {
        let saved = match self.prop_path.lock().as_ref() {
            Some(p) => crate::settings::write_atomic(p, serde_json::to_string_pretty(&*self.proposals.lock()).unwrap_or_default())
                .with_context(|| format!("推敲案を保存できませんでした: {}", p.display())),
            None => Ok(()),
        };
        self.report(&self.last_err_prop, saved);
    }

    pub fn add_proposal(&self, title: &str, note: &str, items: Vec<ProposalItem>) -> Proposal {
        let p = Proposal { id: self.next_id(), ts: now(), title: title.into(), note: note.into(), items };
        self.proposals.lock().push(p.clone());
        self.save_proposals();
        p
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn user_msg(text: &str, ctx: Value, attachments: Vec<Value>) -> ChatMsg {
        ChatMsg { id: 1, ts: String::new(), role: "user".into(), text: text.into(), ctx, attachments, state: String::new(), reaction: String::new() }
    }

    #[test]
    fn fmt_t_rounds_before_splitting() {
        assert_eq!(fmt_t(12.349), "0:12.35");
        assert_eq!(fmt_t(59.996), "1:00.00");
        assert_eq!(fmt_t(-1.0), "0:00.00");
    }

    #[test]
    fn prompt_text_tells_claude_to_read_images() {
        let m = user_msg("これ見て", json!({}), vec![json!({"image": "C:/v/studio/chat/img-1.png"}), json!({"t": 1.0})]);
        let c = prompt_text(&m, &[], &|_| None);
        assert!(c.contains("（画像: C:/v/studio/chat/img-1.png。Read で見てください）"), "{c}");
        assert!(c.starts_with("これ見て"));
    }

    #[test]
    fn prompt_text_orders_text_images_liked_situation() {
        let ctx = json!({"t": 12.349, "scene": "3", "clip": "telop@c15#1", "clip_src": "x_fx.py:12",
            "lines": [4, 5], "liked": [{"id": 3, "head": "了解です"}, {"id": 5, "head": "直しました"}]});
        let m = user_msg("次へ", ctx, vec![json!({"image": "a.png"})]);
        let c = prompt_text(&m, &[], &|_| None);
        let pos = |s: &str| c.find(s).unwrap_or_else(|| panic!("{s} が無い: {c}"));
        let (img, la, lb, st) = (pos("（画像: a.png"), pos("『了解です』"), pos("『直しました』"), pos("[画面の状況]"));
        assert!(0 < img && img < la && la < lb && lb < st, "{c}");
        assert!(c.contains("[画面の状況] 再生位置 0:12.35（12.35 秒） / シーン 3 / 選んでいる演出クリップ telop@c15#1（コード: x_fx.py:12） / 選んでいる台本の行 4, 5"), "{c}");
    }

    #[test]
    fn prompt_text_without_ctx_has_no_situation_or_liked() {
        assert_eq!(prompt_text(&user_msg("x", json!({}), vec![]), &[], &|_| None), "x");
    }

    fn mk(id: u32, color: &str, note: &str, shape: ak_proto::MarkShape) -> ak_proto::Mark {
        ak_proto::Mark { id, color: color.into(), shape, note: note.into(), t: 13.0, t0: 12.0, t1: 15.5, state: "draft".into(), by: "user".into(),
            clips: vec![], media: vec![], msg: None, done_note: String::new(), done_image: None }
    }

    #[test]
    fn mark_found_skips_only_broken_items() {
        let ctx = json!({"mark_found": [{"id": 1, "clips": ["a"]}, {"id": "x"}, {"id": 2}]});
        let (f, bad) = read_mark_found(&ctx);
        assert_eq!((f.iter().map(|f| f.id).collect::<Vec<_>>(), bad), (vec![1, 2], 1));
        assert_eq!(read_mark_found(&json!({})).1, 0);
    }

    #[test]
    fn prompt_text_prefers_mark_found_in_ctx() {
        // 印の記録（marks_sent）より先に Claude に渡っても、送ったときの下のクリップが文に出る
        let a = mk(1, "red", "", ak_proto::MarkShape::Pin { at: [10, 20] });
        let m = user_msg("x", json!({"marks": [1], "mark_found": [{"id": 1, "clips": ["telop@c15#1"], "media": []}]}), vec![]);
        let t = prompt_text(&m, &[a], &|_| None);
        assert!(t.contains("下の演出 telop@c15#1"), "{t}");
    }

    #[test]
    fn prompt_text_lists_marks_in_four_shapes() {
        use ak_proto::MarkShape::*;
        let mut a = mk(1, "red", "もう少し小さく", Rect { bbox: [820, 410, 300, 120] });
        a.clips = vec!["telop@c15#1".into()];
        let b = mk(2, "yellow", "ここ0.5秒遅く", Pin { at: [1500, 200] });
        let mut c = mk(3, "green", "", Arrow { from: [900, 500], to: [1200, 450] });
        c.media = vec!["img-3".into(), "img-4".into()];
        let d = mk(4, "blue", "参考", Pen { points: vec![[10, 20], [110, 80], [60, 200]] });
        let marks = [a, b, c, d];
        let name = |id: &str| (id == "img-3").then(|| "立ち絵A".to_string());
        let m = user_msg("直して", json!({"marks": [1, 2, 3, 4, 9], "t": 13.0}), vec![json!({"image": "a.png"})]);
        let t = prompt_text(&m, &marks, &name);
        assert!(t.contains("
[印①・赤＝直して] 「もう少し小さく」 矩形 x=820 y=410 幅300 高さ120 / 区間 0:12.00〜0:15.50 / 下の演出 telop@c15#1"), "{t}");
        assert!(t.contains("
[印②・黄＝注目] 「ここ0.5秒遅く」 点 (1500, 200) / 区間 0:12.00〜0:15.50"), "{t}");
        assert!(t.contains("
[印③・緑＝良い（残す）] 矢印 (900, 500) → (1200, 450)（右へ 300・上へ 50） / 区間 0:12.00〜0:15.50 / 下の素材 img-3（立ち絵A）, img-4"), "{t}");
        assert!(t.contains("
[印④・水色＝参考] 「参考」 線 外接 x=10 y=20 幅100 高さ180（3 点） / 区間"), "{t}");
        assert!(!t.contains("印(9)") && !t.contains("[印⑨"), "存在しない番号は飛ばす: {t}");
        assert!(t.contains("
（印の座標は出力の画素 1920x1080。確認画像の丸数字が印の番号です）"), "{t}");
        // 画像の後ろ・[画面の状況] の前
        let (img, mk1, last, st) = (t.find("（画像:").unwrap(), t.find("[印①").unwrap(), t.find("（印の座標").unwrap(), t.find("[画面の状況]").unwrap());
        assert!(img < mk1 && mk1 < last && last < st, "{t}");
    }

    #[test]
    fn prompt_text_skips_marks_when_none_resolve() {
        let m = user_msg("x", json!({"marks": [7]}), vec![]);
        assert_eq!(prompt_text(&m, &[], &|_| None), "x");
        let m = user_msg("x", json!({"marks": []}), vec![]);
        assert_eq!(prompt_text(&m, &[], &|_| None), "x");
    }

    #[test]
    fn situation_lines_same_for_numbers_and_strings() {
        assert_eq!(situation(&json!({"lines": [4, 5]})), situation(&json!({"lines": ["4", "5"]})));
        assert_eq!(situation(&json!({"lines": ["l1", "l2"]})), ["選んでいる台本の行 l1, l2"]);
    }

    #[test]
    fn status_line_shape() {
        let ui: UiState = serde_json::from_value(json!({"t": 12.349, "selection": {"kind": "fx", "key": "telop@c15#1"}, "lines": [1, 2]})).unwrap();
        assert_eq!(
            status_line(Some("E:/v/an1"), &ui, &json!("3")),
            "[AK Studio] 回 E:/v/an1 / 再生位置 0:12.35（12.35 秒） / シーン 3 / 選んでいる演出クリップ telop@c15#1 / 選んでいる台本の行 1, 2"
        );
        let ui: UiState = serde_json::from_value(json!({"t": 0.0, "selection": {"kind": "media", "key": "m1"}, "lines": []})).unwrap();
        assert_eq!(status_line(None, &ui, &Value::Null), "[AK Studio] 回 - / 再生位置 0:00.00（0.00 秒）");
        // 何も送られていない（既定）でも落ちない
        assert_eq!(status_line(None, &UiState::default(), &Value::Null), "[AK Studio] 回 - / 再生位置 0:00.00（0.00 秒）");
    }

    fn tmp(tag: &str) -> PathBuf {
        let n = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
        let d = std::env::temp_dir().join(format!("ak-studio-test-chat-{tag}-{}-{n}", std::process::id()));
        let _ = std::fs::create_dir_all(&d);
        d
    }

    fn hub(tag: &str) -> Hub {
        let h = Hub::new();
        h.open(&tmp(tag));
        h
    }

    #[test]
    fn activity_flow() {
        let h = hub("activity");
        assert!(!h.activity_at(1000).working);
        assert!(h.set_activity(true, "", 1000), "始まりは変更");
        assert!(h.set_activity(true, "台本を読んでいます", 1500), "文が変われば変更");
        assert!(!h.set_activity(true, "台本を読んでいます", 2000), "同じ文は変更ではない");
        let a = h.activity_at(2500);
        assert!(a.working && a.since_ms == 1000 && a.updated_ms == 2000 && a.text == "台本を読んでいます");
        *h.claude_status.lock() = "確認中".into();
        assert!(h.set_activity(false, "", 3000));
        assert!(h.claude_status.lock().is_empty(), "終わったら reply の status も消す");
        assert!(!h.activity_at(3000).working);
        assert!(!h.set_activity(false, "", 3500), "終わったままなら変更なし");
    }

    #[test]
    fn activity_stale() {
        let h = hub("activity-stale");
        h.set_activity(true, "x", 1000);
        assert!(h.activity_at(1000 + ACTIVITY_STALE_MS).working);
        assert!(!h.activity_at(1000 + ACTIVITY_STALE_MS + 1).working, "更新が途絶えたら作業中にしない");
        h.set_activity(true, "x", 5000);
        assert!(h.activity_at(5000 + ACTIVITY_STALE_MS).working, "同じ文でも更新時刻は進む");
    }

    fn prop(statuses: &[&str]) -> Proposal {
        Proposal {
            id: 1,
            ts: String::new(),
            title: "テンポ".into(),
            note: String::new(),
            items: statuses
                .iter()
                .enumerate()
                .map(|(n, st)| ProposalItem {
                    kind: "edit".into(),
                    line_id: Some(format!("l{n}")),
                    after_id: None,
                    block_id: None,
                    old_text: format!("前{n}"),
                    new_text: format!("後{n}"),
                    sp: String::new(),
                    reason: String::new(),
                    status: st.to_string(),
                    ..Default::default()
                })
                .collect(),
        }
    }

    /// 件数は案ではなく項目の数（画面のバッジと同じ）
    #[test]
    fn pending_items_counts_items_across_proposals() {
        assert_eq!(pending_items(&[prop(&["pending", "pending", "accepted"]), prop(&["pending"])]), 3);
        assert_eq!(pending_items(&[prop(&["accepted", "rejected"])]), 0);
        assert_eq!(pending_items(&[]), 0);
    }

    #[test]
    fn 推敲案の結果に不採用の案は入れない() {
        let t = prop(&["accepted", "rejected", "stale"]).report_text().unwrap();
        assert_eq!(t, "推敲案「テンポ」の結果:\n- 採用: 前0 → 後0\n- 適用できず（行が変わっていた）: 前2 → 後2");
        assert!(!t.contains("不採用"));
    }

    #[test]
    fn 新しい種類の結果の文() {
        let mk = |kind: &str, old: &str, new: &str, n: usize| ProposalItem {
            kind: kind.into(),
            old_text: old.into(),
            new_text: new.into(),
            status: "accepted".into(),
            lines: vec![ProposalLine::default(); n],
            ..Default::default()
        };
        let p = Proposal {
            id: 1,
            ts: String::new(),
            title: "構成".into(),
            note: String::new(),
            items: vec![
                mk("memo", "旧", "新", 0),
                mk("notes", "旧", "新", 0),
                mk("move", "文", "", 0),
                mk("block_add", "", "導入", 3),
                mk("block_delete", "見出し", "", 2),
                mk("block_rename", "旧題", "新題", 0),
            ],
        };
        assert_eq!(
            p.report_text().unwrap(),
            "推敲案「構成」の結果:
- 採用: 行のメモ: 旧 → 新
- 採用: ブロックのメモ: 旧 → 新
- 採用: 行の移動: 文
- 採用: ブロックの追加: 導入（3 行）
- 採用: ブロックの削除: 見出し（2 行）
- 採用: ブロックの改名: 旧題 → 新題"
        );
    }

    #[test]
    fn すべて不採用なら推敲案の結果は送らない() {
        assert!(prop(&["rejected", "rejected"]).report_text().is_none());
        assert!(prop(&[]).report_text().is_none());
    }

    #[test]
    fn 猶予中のメッセージは渡さず_取り消すと届かない() {
        let h = hub("hold");
        let m = h.push_held("user", "まちがい", json!({}), vec![], Duration::from_secs(30));
        assert_eq!(m.state, "queued");
        assert!(h.drain().is_empty(), "猶予中は渡さない");
        assert!(h.next_ready().is_some());
        match h.cancel(m.id).unwrap() {
            Cancel::Retracted(x) => assert_eq!(x.state, "cancelled"),
            c => panic!("{c:?}"),
        }
        assert_eq!(h.pending(), 0);
        assert!(h.next_ready().is_none());
        assert!(h.cancel(m.id).is_err(), "二度は取り消せない");
    }

    #[test]
    fn 猶予が過ぎたら渡し_届いた後の取り消しは知らせを積む() {
        let h = hub("late");
        let m = h.push_held("user", "これを直して", json!({}), vec![], Duration::ZERO);
        let got = h.drain();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].state, "delivered");
        assert_eq!(h.msgs.lock()[0].state, "delivered");
        match h.cancel(m.id).unwrap() {
            Cancel::Late(x, notice) => {
                assert_eq!(x.state, "cancel_late");
                assert!(is_cancel_notice(&notice));
                assert_eq!(notice.ctx["cancel_of"], json!(m.id));
                assert!(notice.text.contains("これを直して"));
            }
            c => panic!("{c:?}"),
        }
        // 知らせは猶予なしで、知らせだけを取り出せる
        let n = h.drain_where(is_cancel_notice);
        assert_eq!(n.len(), 1);
        assert!(h.cancel(n[0].id).is_err(), "知らせそのものは取り消せない");
        assert_eq!(h.ack_abort(m.id).unwrap().state, "cancel_aborted");
    }

    #[test]
    fn 取り消しと渡すのが同時でも_どちらか一方になる() {
        let h = std::sync::Arc::new(Hub::new());
        for i in 0..300 {
            let m = h.push_held("user", &format!("m{i}"), json!({}), vec![], Duration::ZERO);
            let (h1, h2) = (h.clone(), h.clone());
            let a = std::thread::spawn(move || h1.drain());
            let b = std::thread::spawn(move || (h2.cancel(m.id).ok(), h2.cancel(m.id).ok()));
            let got = a.join().unwrap();
            let (c1, c2) = b.join().unwrap();
            assert!(c2.is_none(), "二度目の取り消しは断る");
            let state = h.msgs.lock().iter().find(|x| x.id == m.id).unwrap().state.clone();
            match c1.unwrap() {
                Cancel::Retracted(_) => {
                    assert!(got.iter().all(|x| x.id != m.id), "取り消したものは渡さない");
                    assert_eq!(state, "cancelled");
                }
                Cancel::Late(..) => assert_eq!(state, "cancel_late", "渡した印で取り消しの印を上書きしない"),
            }
            h.drain(); // 知らせを片付ける
        }
    }

    #[test]
    fn 作業を止めた知らせは届いた後の取り消しにだけ付く() {
        let h = hub("ack");
        let m = h.push_held("user", "a", json!({}), vec![], Duration::from_secs(30));
        assert!(h.ack_abort(m.id).is_none(), "送信待ちのものには付けない");
        h.cancel(m.id).unwrap();
        assert!(h.ack_abort(m.id).is_none(), "届く前に取り消したものには付けない");
    }

    #[test]
    fn 知らせだけの取り出しは普通のメッセージを残す() {
        let h = hub("notice");
        h.push("user", "ふつう", json!({}), vec![]);
        assert!(h.drain_where(is_cancel_notice).is_empty());
        assert_eq!(h.pending(), 1);
    }

    #[test]
    fn 取り消しはファイルにも残り_開き直すと送信待ちは届いていない印になる() {
        let d = tmp("reopen");
        let h = Hub::new();
        h.open(&d);
        let a = h.push_held("user", "a", json!({}), vec![], Duration::from_secs(30));
        let b = h.push_held("user", "b", json!({}), vec![], Duration::from_secs(30));
        h.cancel(a.id).unwrap();
        let h2 = Hub::new();
        h2.open(&d);
        let ms = h2.msgs.lock().clone();
        assert_eq!(ms.iter().find(|m| m.id == a.id).unwrap().state, "cancelled");
        assert_eq!(ms.iter().find(|m| m.id == b.id).unwrap().state, "undelivered", "開き直すと inbox は空なので、届いていない印にする");
        assert!(h2.cancel(b.id).is_err(), "前回開いたときのメッセージは取り消せません");
        assert_eq!(std::fs::read_to_string(d.join("chat.jsonl")).unwrap().lines().count(), 2, "同じ行を二度書かない");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn 知らせの文は長いメッセージを縮める() {
        let m = ChatMsg {
            id: 1,
            ts: String::new(),
            role: "user".into(),
            text: "あ".repeat(200),
            ctx: Value::Null,
            attachments: vec![],
            state: String::new(),
            reaction: String::new(),
        };
        let t = cancel_notice_text(&m);
        assert!(t.contains(&format!("{}…", "あ".repeat(80))));
        assert!(!t.contains(&"あ".repeat(81)));
        assert!(t.contains("。\n取り消したメッセージ: 「"), "行の頭に空白を入れない: {t:?}");
        assert!(!t.contains("  "), "途中に余分な空白を入れない: {t:?}");
    }

    #[test]
    fn リアクションは履歴に残り開き直しても読める() {
        let dir = tmp("react");
        let h = Hub::new();
        h.open(&dir);
        let u = h.push("user", "こんにちは", Value::Null, vec![]);
        let c = h.push("claude", "はい", Value::Null, vec![]);
        assert!(h.react(u.id, "like").is_err(), "ユーザーのメッセージには付けられない");
        assert!(h.react(c.id, "love").is_err());
        assert!(h.react(999, "like").is_err());
        assert_eq!(h.react(c.id, "like").unwrap().reaction, "like");
        let h2 = Hub::new();
        h2.open(&dir);
        let msgs = h2.msgs.lock().clone();
        assert_eq!(msgs.iter().find(|m| m.id == c.id).unwrap().reaction, "like");
        assert_eq!(msgs.iter().filter(|m| m.id == c.id).count(), 1, "書き直しで行が増えない");
        // 外すと保存からも消える（空のときは書かない）
        h2.react(c.id, "").unwrap();
        let text = std::fs::read_to_string(dir.join("chat.jsonl")).unwrap();
        assert!(!text.contains("reaction"), "{text}");
        // 古い履歴（reaction なし）も読める
        let old: ChatMsg = serde_json::from_str(r#"{"id":1,"ts":"t","role":"claude","text":"x"}"#).unwrap();
        assert_eq!(old.reaction, "");
    }

    fn broken_files(d: &std::path::Path, prefix: &str) -> Vec<PathBuf> {
        std::fs::read_dir(d).unwrap().flatten().map(|e| e.path())
            .filter(|p| p.file_name().unwrap().to_string_lossy().starts_with(prefix)).collect()
    }

    #[test]
    fn 壊れた推敲案は退避して空で始め_知らせる() {
        let d = tmp("broken-prop");
        std::fs::write(d.join("proposals.json"), "{").unwrap();
        let h = Hub::new();
        let notices = h.open(&d);
        assert_eq!(notices.len(), 1, "{notices:?}");
        assert!(notices[0].0.contains("推敲案"), "{notices:?}");
        assert!(h.proposals.lock().is_empty());
        let kept = broken_files(&d, "proposals.json.broken-");
        assert_eq!(kept.len(), 1);
        assert_eq!(std::fs::read_to_string(&kept[0]).unwrap(), "{");
        // そのあと足しても、退避したファイルは残る
        h.add_proposal("題", "", vec![]);
        assert_eq!(std::fs::read_to_string(&kept[0]).unwrap(), "{");
        assert!(std::fs::read_to_string(d.join("proposals.json")).unwrap().contains("題"));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn 読めない行のあるチャットは元の全行を残して知らせる() {
        let d = tmp("broken-chat");
        let ok = serde_json::to_string(&user_msg("こんにちは", json!({}), vec![])).unwrap();
        let text = format!("{ok}
これは壊れた行
");
        std::fs::write(d.join("chat.jsonl"), &text).unwrap();
        let h = Hub::new();
        let notices = h.open(&d);
        assert_eq!(notices.len(), 1, "{notices:?}");
        assert!(notices[0].0.contains("1 行"), "{notices:?}");
        assert_eq!(h.msgs.lock().len(), 1);
        let kept = broken_files(&d, "chat.jsonl.broken-");
        assert_eq!(kept.len(), 1);
        assert_eq!(std::fs::read_to_string(&kept[0]).unwrap(), text);
        // 読めた行だけで書き直したので、開き直しても知らせ・写しは増えない
        assert!(Hub::new().open(&d).is_empty());
        assert_eq!(broken_files(&d, "chat.jsonl.broken-").len(), 1);
        // 空行だけなら壊れた行とは数えない
        let d2 = tmp("blank-chat");
        std::fs::write(d2.join("chat.jsonl"), format!("{ok}

")).unwrap();
        assert!(Hub::new().open(&d2).is_empty());
        let _ = std::fs::remove_dir_all(&d);
        let _ = std::fs::remove_dir_all(&d2);
    }

    #[test]
    fn 保存の失敗は同じ文を続けて知らせない() {
        let d = tmp("save-fail");
        let blocker = d.join("file");
        std::fs::write(&blocker, "x").unwrap();
        let h = Hub::new();
        let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let c2 = calls.clone();
        h.set_on_error(move |_| {
            c2.fetch_add(1, Ordering::SeqCst);
        });
        // 親がファイルなので書けない
        *h.path.lock() = Some(blocker.join("chat.jsonl"));
        h.save_msgs();
        h.save_msgs();
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        // 書ける状態に戻して保存し、また失敗させると、もう一度知らせる
        *h.path.lock() = Some(d.join("chat.jsonl"));
        h.save_msgs();
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        *h.path.lock() = Some(blocker.join("chat.jsonl"));
        h.save_msgs();
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        // 推敲案の保存が通っても、チャットの失敗の覚えは消えない（種類ごとに持つ）
        *h.prop_path.lock() = Some(d.join("proposals.json"));
        h.save_proposals();
        h.save_msgs();
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        // 両方が失敗し続けても、交互に知らせ直さない
        *h.prop_path.lock() = Some(blocker.join("proposals.json"));
        h.save_proposals();
        h.save_msgs();
        h.save_proposals();
        h.save_msgs();
        assert_eq!(calls.load(Ordering::SeqCst), 3);
        let _ = std::fs::remove_dir_all(&d);
    }
}
