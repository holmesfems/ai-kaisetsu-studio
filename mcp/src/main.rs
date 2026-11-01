//! ak-studio-mcp — Claude Code と AK Studio をつなぐ MCP サーバー（stdio）。
//!
//! - ツール: 台本を読む・推敲案を出す・演出クリップの一覧と微調整・確認画像・演出の読み直し・返事・書き出し
//! - channel（claude/channel）: AK Studio の画面のチャットで送ったメッセージを、実行中のセッションに直接届ける
//!
//! 登録:  claude mcp add -s user ak-studio -- <この exe のパス>
//! 起動:  claude --dangerously-load-development-channels server:ak-studio
//! AK Studio の場所（ポートと合言葉）は %USERPROFILE%\.ak-studio\session.json から読む（ak_proto::session）。

// tools() の json! が長くなったので、マクロの展開の深さを広げる
#![recursion_limit = "256"]

use ak_proto::http::{ApplyReq, ClipIdReq, ClipsQ, ExportReq, GuideCoordsReq, GuidePatch, GuideStillReq, MarkAddReq, MarkResolveReq, MarksQ, MediaPatch, MemoReq, ProposeReq, ReplyReq, SeSet, SheetReq, StillReq, SubsSetReq, UiFocus};
use ak_proto::schema::input_schema;
use serde_json::{json, Value};
use std::io::{BufRead, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

const PROTOCOL: &str = "2025-06-18";

const INSTRUCTIONS: &str = r#"AK Studio（解説動画の編集アプリ）とつながっています。
ユーザーが AK Studio の画面のチャットで送ったメッセージは <channel source="ak-studio" ...> で届きます。
属性に、送ったときの再生位置（t 秒）・シーン番号（scene）・選んでいた演出クリップ（clip）・台本の行（lines）が入ります。

返事のしかた:
- 画面から来たメッセージには、必ず ak-studio の reply ツールで返事をしてください（ユーザーはターミナルではなく AK Studio の画面を見ています）。
  長い作業の前には reply の status で「台本を読んでいます」のように今やっていることを知らせると親切です。
- メッセージに（画像: …）があれば、Read ツールでその画像を見てから答えてください。
- 返事には attachments で、確認画像（{"image": "<render_still のパス>"}）や、押すとその場所へ飛ぶリンク
  （{"t": 秒, "label": "…"} / {"clip": "<キー>"} / {"line": <通し番号>} / {"mark": <印の番号>}）を付けられます。

作業の道具:
- 台本: script_read で通し番号つきの台本を読み、直しは script_propose で「推敲案」として出す（ユーザーが画面で1件ずつ採用する）。
  ユーザーがはっきり「直接直して」と言ったときだけ script_apply。セリフの数字・事実は、データのスキルが入っていればそれで確かめること。
  行には memo（ユーザーが書いた演出のためのメモ。読み上げない・字幕に出さない）がついていることがある。演出を作る・直すときに必ず参考にし、
  たとえば「ここで実際のPUメンバーを出す」なら、その演出を出すこと。メモは、ユーザーに頼まれたときだけ script_propose の kind: memo で直す（勝手に直さない）。書き出し（export talk）は files/script_memo.json（cue・sp・text・memo の配列）にも書く。
  台本のファイルは script/<名前>.yaml（ブロックごと。title・notes・lines の id/sp/start/text/memo。書式は docs/script-format.md）。
  行は id で指す。直しても id は変わらない。声のアクセント・抑揚の手直し（_voice.json・_voice_params.json）は行の id が鍵で、
  文や話者を直すと使われなくなる（script_read の adjusted は、今の文で手直しが効いている行）。ファイルを直接編集するより script_propose を使うこと。
- 素材（立ち絵・録画・BGM・画像）: media_list / media_add / media_update / media_remove で、AK Studio の素材トラックに置く。
  **立ち絵や録画を演出のコード（fx/*.py）に描き込まないこと**（演出のコードは、テロップ・図・表などのデータ演出だけ）。
  立ち絵は image のトラック（「立ち絵」）、録画は RecWindow の窓に入れるなら media_add の nest にその RecWindow のクリップのキー。
  掛け合いの立ち絵は speaker（トーク1 / トーク2）を付けると、話していない人が少し暗くなる。
  立ち絵などの入り・抜けの動き（ワイプ・グリッチ・スライドなど）は media_update の trans_in / trans_out（演出のコードで描かない）。
- 演出: 1 回の add(...) が 1 つのクリップ（キーは 部品名@c<cue>#<順番>）。clips_list で一覧、render_still で目で見てから直す。
  render_sheet は cue ごとのコマを 16 コマ 1 枚に並べて、流れと抜けを一度に見る（番号は返りの JSON）。sync_report は使った発話同期点の一覧で、
  method が ratio（estimated=true）のものは文字位置からの推定なので、音と合っているか確かめる。
  位置・大きさ・タイミングの小さな直しは clip_adjust（コードを書き換えない微調整。fx/studio_overrides.json に残る）。
  新しい演出や中身の変更は fx/<名前>_fx.py を直接編集して fx_reload（エラーはそのまま返ってくる）。演出の作り方は、入っていれば演出の kit のスキル（ak-studio-fx など）に従う。
- 録画: RecWindow の窓に入れる録画は、ユーザーが画面で RecWindow をダブルクリックして差し込む（ネスト。studio/project.json の nests）。
  clips_list の nest.missing で素材が足りない区間が分かる。録画トラックだけの mp4 は export の rec_track。
  **studio/project.json は直接書き換えないこと**（AK Studio は見張っていないので、次に画面で保存したときに消える）。
  素材は media_* で、ネストの中身は media_add の nest で変える。
- 字幕: subs_read でセリフごとの字幕の行の分け方・はみ出し・大きさを読み、subs_set で改行（text に \n）・大きさ（size）・フォントを直す（行の id か通し番号で指す。null で外す）。
  改行は文節の切れ目に。改行を入れたセリフは自動で折り返さないので、返りの overflow が無く row_count が狙いどおりかを見て size を決める。
  字幕の文そのものは変えない。render_still の subs=true で見た目を確かめる。**project.json の subtitle_lines は直接書き換えない**。
- 効果音（演出の動きから自動で付く SE）: se_list で種類ごとの候補・鳴らす音・音量、se_set で変える（fx/se/bank.json）。
  音の好みはユーザーが画面の「効果音の設定」（タイムラインの SE の ⚙）で聞き比べて決めるので、変えるのは頼まれたときだけ。
- 攻略動画の回（studio/guide.json がある回。studio_status や guide_read で分かる）: 台本・声は無く、通しの素材をパート（編成・手動・自動・リザルト）に組み立てる。
  時刻は素材の秒（`t`）と出力の秒（`t_out`）の 2 つがあり、操作は素材の秒で持つ。
  流れ: guide_read（パート・操作・編成・警告）→ guide_still（`op` で操作のコマ、`grid: true` で素材の画素の目盛り、`crop` で拡大）で素材そのままの画面を見る
  → guide_update で操作（足す・直す・消す。時刻は t・t_out・frame のどれか 1 つ）・編成（使う枠・補足）・補足素材の小窓（subs）を直す。
  guide_update はすぐ保存してモニターに出し、画面に「Claude の変更を戻す」が出る（理由 reason は必須）。
  決まった演出（NOW/NEXT・操作の帯・コストの拡大・自動のタイマー）は演出の kit が描く。
  回ごとの演出（次の操作のマスの印など）は fx/guide_fx.py の build() に素材の画素で書く（`g.at(x, y, "manual")`・`g.op_t("op3")`）。
  座標の確かめは guide_coords（素材の画素 ↔ 出力の画素）。素材の画素は 2752x2064 での値。
  サムネなどに使うクリア時間（自動パートのタイマーが止まった値）は guide_read の `clear_time`（`frames` と `text` = 時:分:秒:フレーム。読めなければ null）。
  **アンカー・カット・固定値・重ね方（layout）は画面でユーザーが直す**。頼まれても guide_update では直せないので、画面（攻略タブ）での直し方を伝えること。
  studio/guide.json は直接書き換えない（guide_update を使う）。
- 印（モニターに描かれた、場所つきの指示）: メッセージに `[印①・赤＝直して] 「もう少し小さく」 矩形 x=… / 区間 … / 下の演出 telop@c15#1` の行と、
  丸数字を描いた確認画像（画像: …）が付いて届く（属性 marks に印の番号）。座標は出力の画素（1920x1080）で、矩形は x・y・幅・高さ。
  色は 赤＝直して・黄＝注目・緑＝良い（残す）・水色＝参考。「下の演出」「下の素材」は印の下にあるクリップなので、直す先の手がかりにする。
  印はユーザーが済にするか、あなたが済にするまで残る。marks_list で今の印を読み（既定は送られたもので済でないもの。all=true で全部）、
  直したら render_still で確認画像を撮って mark_resolve（id・note=直した内容の一言・image=その画像のパス）で済にする。
  迷ったところ・確認してほしい所は mark_add（color・shape・note・t）で自分の印として示せる（破線で出る）。印は消せない・戻せない（ユーザーが画面で）。
- アクセント・読みの直しは voisona-accent スキル。
- 書き出し（export）は時間がかかるので、ユーザーに頼まれたときだけ。"#;

/// 非公開の `presets/mcp_context.md`（題材に固有の補足。`mcp/build.rs` が埋め込む。無ければ空。旧#184）
const CONTEXT: &str = include_str!(concat!(env!("OUT_DIR"), "/mcp_context.txt"));

/// Claude に渡す説明。補足があれば末尾に付ける
fn instructions() -> String {
    with_context(INSTRUCTIONS, CONTEXT)
}

fn with_context(base: &str, context: &str) -> String {
    let c = context.trim();
    if c.is_empty() {
        base.to_string()
    } else {
        format!("{base}

{c}")
    }
}

struct Session {
    port: u16,
    token: String,
}

fn session() -> Option<Session> {
    let p = ak_proto::session::session_file()?;
    let v: Value = serde_json::from_str(&std::fs::read_to_string(p).ok()?).ok()?;
    Some(Session { port: v["port"].as_u64()? as u16, token: v["token"].as_str()?.to_string() })
}

fn api(method: &str, path: &str, body: Option<Value>, timeout: u64) -> Result<Value, String> {
    let s = session().ok_or("AK Studio が起動していません（AK Studio を開いてから、もう一度試してください）")?;
    let url = format!("http://127.0.0.1:{}{}", s.port, path);
    let req = ureq::request(method, &url)
        .set("Authorization", &format!("Bearer {}", s.token))
        .timeout(Duration::from_secs(timeout));
    let r = match body {
        Some(b) => req.send_json(b),
        None => req.call(),
    };
    match r {
        Ok(r) => r.into_json::<Value>().map_err(|e| e.to_string()),
        Err(ureq::Error::Status(code, r)) => {
            let v: Value = r.into_json().unwrap_or(Value::Null);
            Err(error_text(&v, code))
        }
        Err(e) => Err(connect_error(&e.to_string())),
    }
}

/// 本体の失敗の応答（error・code・detail）を Claude に渡す文にする。
/// code を添え、detail が error と違うときは元の連鎖（英語の原因を含む）も次の行に付ける
fn error_text(v: &Value, status: u16) -> String {
    let error = v["error"].as_str().unwrap_or("error");
    let mut s = match v["code"].as_str() {
        Some(c) => format!("{error}（code: {c}、HTTP {status}）"),
        None => format!("{error} ({status})"),
    };
    if let Some(d) = v["detail"].as_str() {
        if d != error {
            s.push_str(&format!("
詳細: {d}"));
        }
    }
    s
}

/// つなげなかったときの文。session.json があるのにつながらないのは、AK Studio が落ちて接続先が残っているとき
/// （本体は終わるときに自分の session.json を消す）。起動していないときは session() の None で別の文を出す
fn connect_error(e: &str) -> String {
    format!("AK Studio につながりません。起動していないか、前に落ちたときの接続先（session.json）が残っています。AK Studio を開き直してください（{e}）")
}

struct Out(Mutex<std::io::Stdout>);

impl Out {
    fn send(&self, v: &Value) {
        let mut o = self.0.lock().unwrap();
        let _ = writeln!(o, "{}", v);
        let _ = o.flush();
    }
}

/// 引数が構造体のツールは、HTTP の要求と同じ型（ak-proto）から inputSchema を作る。
/// 構造体でないもの（clip_adjust・media_add の自由な形など）は手書きのまま
fn tools() -> Value {
    json!([
        {"name": "studio_status", "description": "AK Studio の今の状況（開いている回、再生位置・シーン・今のセリフ、画面に出ている演出クリップ、演出エンジンの状態、保留中の推敲案。pending_items は採否を待っている項目の数（画面のバッジと同じ）、pending_proposals は保留の項目がある案の数）",
         "inputSchema": {"type": "object", "properties": {}}},
        {"name": "script_read", "description": "台本を通し番号つきで読む（ブロック・行の id・話者・文・開始秒・長さ・音声の有無・アクセントや抑揚の手直しが効いているか（adjusted）、構成メモ。行にユーザーのメモ memo があれば付く。memo は演出のための参考情報で読み上げない。演出を作る・直すときに必ず参考にする）",
         "inputSchema": {"type": "object", "properties": {}}},
        {"name": "script_propose", "description": "台本の推敲案を出す。ユーザーが AK Studio の画面で差分を見て1件ずつ採用・不採用を決める（結果はチャットで届く。届くのは採用・適用できずの案だけで、不採用は届かない。不採用なら、ユーザーが別に条件を伝える）。行の文のほか、行のメモ（memo）・ブロックのメモ（notes）・ブロックの追加／削除／改名（block_add / block_delete / block_rename）・行の移動（move）も出せる。台本の YAML や _order.json は直接書かない。同じ行の後ろに複数足す・動かすときは items の順に並ぶ",
         "inputSchema": input_schema::<ProposeReq>()},
        {"name": "script_apply", "description": "台本を直接書き換える（ユーザーが「直接直して」と頼んだときだけ。ふだんは script_propose）",
         "inputSchema": input_schema::<ApplyReq>()},
        {"name": "memo_write", "description": "台本の構成メモ（script/_memo.md）を書き換える",
         "inputSchema": input_schema::<MemoReq>()},
        {"name": "clips_list", "description": "演出クリップ（1 回の add(...) = 1 クリップ）の一覧。キー・部品名・コードの行・時刻（微調整後）・シーン・今の微調整。RecWindow（録画の窓）には nest（差し込んだ録画のクリップ数・素材が無い区間 missing・PLAYBACK の区切り segs）が付く",
         "inputSchema": input_schema::<ClipsQ>()},
        {"name": "clip_adjust", "description": "演出クリップの微調整（コードは書き換えない）。渡した項目だけを今の微調整に重ねる（ほかの項目はそのまま）。項目に null を渡すとその項目だけ戻す。reset=true で全部戻す",
         "inputSchema": {"type": "object", "properties": {
             "key": {"type": "string"},
             "shift": {"type": ["number", "null"], "description": "出る時刻を何秒ずらすか（全体を平行移動）"},
             "end": {"type": ["number", "null"], "description": "消える時刻を何秒ずらすか（長さの調整）"},
             "dx": {"type": ["number", "null"], "description": "右へ何 px（1920x1080 の座標）"}, "dy": {"type": ["number", "null"], "description": "下へ何 px"},
             "scale": {"type": ["number", "null"], "description": "倍率（1=そのまま）"}, "opacity": {"type": ["number", "null"], "description": "0〜1"},
             "pivot": {"type": ["array", "null"], "items": {"type": "number"}, "description": "拡大の中心 [x, y]（省略時はクリップの中心）"},
             "hidden": {"type": ["boolean", "null"]}, "z": {"type": ["number", "null"], "description": "重なり順"},
             "note": {"type": ["string", "null"], "description": "メモ"}, "reset": {"type": "boolean"}}, "required": ["key"]}},
        {"name": "media_list", "description": "素材トラック（背景・録画・立ち絵・BGM）と、RecWindow のネストの中の素材の一覧（クリップの id・ファイル・時刻・位置・倍率・話者）",
         "inputSchema": {"type": "object", "properties": {}}},
        {"name": "media_add", "description": "素材（立ち絵の画像・録画の動画・BGM の音声）をタイムラインに置く。種類に合うトラックに入り、置き方の既定は画面の「素材を追加」と同じ（立ち絵は左の領域に高さ 1000px、1920x1080 の録画は録画の窓に 66.667%）。nest に RecWindow のキーを渡すと、その窓のネストの中に置く（キャンバスいっぱい）",
         "inputSchema": {"type": "object", "properties": {
             "path": {"type": "string", "description": "素材のファイル（絶対パス）"},
             "start": {"type": "number", "description": "置く時刻（秒）。省略時は画面の再生位置"},
             "dur": {"type": "number", "description": "長さ（秒）。省略時は動画・音声はその長さ、画像はタイムラインの終わりまで（最低 5 秒）。ネストの中は窓が見えている区間に収める"},
             "track": {"type": "string", "description": "トラックの id か名前（「立ち絵」「録画」「BGM」など）。省略時は種類で選ぶ"},
             "nest": {"type": "string", "description": "RecWindow のクリップのキー（例 RecWindow@c70#1）。その窓に録画を差し込む"},
             "x": {"type": "number"}, "y": {"type": "number"}, "scale": {"type": "number"},
             "src_in": {"type": "number", "description": "素材のどこから使うか（秒）"},
             "speaker": {"type": "string", "description": "立ち絵の話者（トーク1 / トーク2）"},
             "flip": {"type": "boolean"}, "loop": {"type": "boolean"}, "opacity": {"type": "number"},
             "fade_in": {"type": "number"}, "fade_out": {"type": "number"}, "volume_db": {"type": "number"},
             "trans_in": {"description": "入りのトランジション（media_update の説明を見て）"},
             "trans_out": {"description": "抜けのトランジション"},
             "name": {"type": "string"}}, "required": ["path"]}},
        {"name": "media_update", "description": "置いた素材を変える（時刻・長さ・位置・倍率・話者・フェード・音量・再生速度 speed（ネストの素材だけ。1 が等速、0.1〜10。長さは変わらないので、素材の同じ範囲を使うなら dur も合わせる）・クロップ crop_l/crop_t/crop_r/crop_b（%。crop: [左, 上, 右, 下] でもよい）・境目のぼかし feather（px）など。patch に変える値だけ。知らない項目はエラーになる）。トランジション（入り trans_in・抜け trans_out）は {kind, dur, dir} か種類だけの文字列、null で外す。kind: fade（フェード）/ slide（dir の側から入る・dir の側へ去る、160px）/ pop（小さく弾んで出る）/ slam（大きく叩きつける）/ wipe（dir の辺から走査線で開く・閉じる。TitleCard と同じ）/ glitch（横ずれ＋色ずれ）。dur は秒（既定 0.35）、dir は left / right / up / down（既定 left）。立ち絵の切り替えは、前のクリップの trans_out と次の trans_in を同じ長さにして、その長さだけ重ねる（wipe / slide は前の dir を次の dir の逆にする。同じ向きだとワイプの途中で背景が見える）",

         "inputSchema": input_schema::<MediaPatch>()},
        {"name": "media_remove", "description": "置いた素材をタイムラインから外す（ファイルは消さない）",
         "inputSchema": input_schema::<ClipIdReq>()},
        {"name": "subs_read", "description": "セリフごとの字幕を読む。track・speakers は共通・話者ごとの見た目（font・size・line_h・margin_l・margin_r・wrap）、width は字幕を収める幅（px）。lines[] は n（script_read の通し番号）・id・sp・text（台本の字幕の文）・sub_text（今の字幕の文。改行は \\n）・manual（改行を入れてあり、自動で折り返さない）・custom（字幕だけ文が違う）・size・font・own（このセリフだけの設定）・rows（今の行の分け方）・row_count・overflow（行ごとの、幅からはみ出しているか）。Python で測るので、測れないときは rows・overflow を省いて measure_error に理由が入る。声の無い行は出ない",
         "inputSchema": {"type": "object", "properties": {
             "lines": {"type": "array", "items": {"type": ["string", "integer"]}, "description": "読むセリフの指し方（行の id・\"#12\"・通し番号）。省くと全部のセリフ"}}}},
        {"name": "subs_set", "description": "セリフごとの字幕の改行（text に \\n）・大きさ（size）・フォント（font）を直して保存し、モニターに出す（画面で元に戻せる）。渡した項目だけを重ね、null でその項目を外す（話者・共通の見た目に戻る）。字幕の文そのものは変えられない（文字は台本の字幕の文のまま、改行だけ）。改行を入れたセリフは自動で折り返さない。返りは直したセリフの subs_read と同じ形（rows・row_count・overflow で、狙いの行数に収まったかを確かめる）。1 件でも断れば何も書かない",
         "inputSchema": input_schema::<SubsSetReq>()},
        {"name": "se_list", "description": "演出の動きから自動で付く効果音（SE）の設定。種類（kind: chapter・slam・wipe・blip など。label は画面の表示名）ごとに、候補の音（synth=合成の音、ほかは fx/se/ の下の .wav）・今鳴らす音（use。複数なら出てくる順に順番に使う）・候補ごとの音量（db。標準の大きさからの dB）・この回で鳴る時刻（times）。中身は fx/se/bank.json。攻略動画の操作の効果音は 4 つの種類（guide_pre=操作の予備・guide_done=操作の完了・guide_precise_pre=精密操作の予備・guide_precise_done=精密操作の完了。shelf が true）で、音はアプリの効果音の棚（全部の回で使う。選んだ音と音量は設定に保存）から 1 つ選ぶ",
         "inputSchema": {"type": "object", "properties": {}}},
        {"name": "se_set", "description": "効果音の設定を変えて保存し、効果音を作り直す（画面の「効果音の設定」と同じ。再生と書き出しに反映）。kinds に変える種類だけ。use=鳴らす候補の並び（[\"synth\"] で合成の音に戻す）、db={候補: dB}（null で 0 dB に戻す。-30〜+24）、add/remove=候補の一覧に足す・外す（se/ の下の .wav の相対パス）。棚の種類（shelf が true）は use に棚の音（\"guide_done/名前.wav\"）を 1 つだけ・db は選んだ音の分だけ（add は棚の音が自動で出るので不要）。音の好みはユーザーが耳で決めるので、勝手に変えずに提案してから",
         "inputSchema": input_schema::<SeSet>()},
        {"name": "scenes_list", "description": "シーン番号（SCENE 57-2 など）と時刻の一覧",
         "inputSchema": {"type": "object", "properties": {}}},
        {"name": "render_still", "description": "確認画像（背景・録画・立ち絵・演出・字幕を重ねた 1920x1080）を描いて見る。t（秒）か scene か cue で指定。省略時は画面の再生位置",
         "inputSchema": input_schema::<StillReq>()},
        {"name": "render_sheet", "description": "cue ごとのコマを並べた確認画像。cues=[最初, 最後] なら各 cue の見どころ（cue の頭から、長さの 60% か 2.2 秒の早い方）、ts=[秒, …] なら指定の時刻のコマを、1 枚 1920x1080 に 4x4 の 16 コマ（1 コマ 480x270）で並べる。16 を超えたら 2 枚目以降。コマの中に文字は描かないので、番号は返りの JSON（pages[].cells[] の i=左上から右へ・t・scene・cue）で見る。全体の流れや抜けを見るときに使い、細部は render_still で見る。字幕は subs=true で入れる",
         "inputSchema": input_schema::<SheetReq>()},
        {"name": "guide_read", "description": "攻略動画（studio/guide.json）の今の状態: 素材・パート（編成・手動・自動・リザルトの出力の時刻）・アンカー（素材の秒 t と出力の秒 out）・固定値・カット・操作（id・素材の秒 t・出力の秒 t_out・NOW が始まる出力の秒 t_now・先出しの個別値 now_lead（null は固定値の汎用値）・コマ・コスト・ゲーム内の時間 game_text・種類・キャラ・置き場所・精密・コストの文・注意）・編成（枠の矩形・使う枠・補足）・補足素材の小窓（subs）・座標の変換（geo）・クリア時間（clear_time: 自動パートのタイマーが止まった値。frames と text。サムネなどに使う。読めなければ null）・解析の要約・警告・戻せる Claude の変更（claude_undo）。攻略動画でない回はエラー",
         "inputSchema": {"type": "object", "properties": {}}},
        {"name": "guide_update", "description": "攻略動画の操作・編成・補足素材の小窓を直して、すぐ保存してモニターに出す（保存の前の guide.json は控えに残り、画面の攻略タブに「Claude の変更を戻す」が出る）。ops は add（足す）・update（渡した項目だけ変える）・remove（id で消す）。操作の時刻は t（素材の秒）・t_out（出力の秒。手動パートの中）・frame（素材のコマ番号）のどれか 1 つ。操作の項目は kind・op・where（置き場所・向き）・info（補足。自由文で、動画の NOW/NEXT と下の帯に小さく出る）・note（注意書き）・cue_text・precise・now_lead（NOW を操作の瞬間より何秒前に出すか。0〜10。省くと固定値の汎用値で、update で null にすると汎用値に戻る。信頼枠は常に 0）。formation は used（使う枠。null で全部）と notes（枠の番号 → 補足。null で消す）。subs は渡したら丸ごと置き換え。全部か無しか（1 つでもエラーなら何も保存しない）。アンカー・カット・固定値・重ね方は直せない（画面で）。返りは guide_read と同じ形に changed（変えたことの一覧）と warnings_new（この変更で新しく出た警告。補足素材の小窓が消した操作を指していた、など。断らず保存するので、返ったら直す）",
         "inputSchema": input_schema::<GuidePatch>()},
        {"name": "guide_still", "description": "攻略動画の素材そのままの 1 コマの確認画像（演出・レイヤーは入れない）。op（操作の id）・t（素材の秒）・frame（コマ番号）・t_out（出力の秒）のどれか 1 つ。省略は画面の再生位置。crop=[x0,y0,x1,y1]（素材の画素。2752x2064 での値）で拡大して読める。grid=true で 100px ごとの線と 200px ごとの素材の画素の数字を重ね、座標を読み取れる。返りは {path, frame, t_src, part, t_out, cost, game_text, size, crop, scale}",
         "inputSchema": input_schema::<GuideStillReq>()},
        {"name": "guide_coords", "description": "素材の画素と出力（1920x1080）の画素の変換。part ごとの置き方（編成は 70% で中央、手動は 75% で左上など）を本体が決めたとおりに変換する。to=out（既定）は素材 → 出力、to=src は出力 → 素材。visible は変換した点が出力の範囲（素材が見えている所）の中か。切り抜きで重ねた HUD の上は考えない。返りの src_size は計算に使った素材の大きさ。素材を解析する前は大きさが分からず 2752x2064 とみなし、size_assumed: true が付く（解析後に変わりうる）",
         "inputSchema": input_schema::<GuideCoordsReq>()},
        {"name": "marks_list", "description": "モニターの印（ユーザーが描いた場所つきの指示と、自分が足した印）の一覧。id（丸数字の番号）・color（red=直して / yellow=注目 / green=良い・残す / blue=参考）・shape（座標は出力の画素 1920x1080）・note・t（確認画像のコマ）・t0〜t1（出る区間）・state（draft=まだ送っていない / sent=送った・作業待ち / done=済）・by・clips（印の下にある演出クリップ）・media（素材のクリップ id）・done_note・done_image。既定は送られていて済でないもの、all=true で全部",
         "inputSchema": input_schema::<MarksQ>()},
        {"name": "mark_resolve", "description": "印を済にする（直したとき）。note に直した内容の一言、image に直したあとの確認画像（render_still のパス）を付けると、ユーザーの画面の印に出る。済にしたら画面に知らせが出る。戻すのはユーザーが画面で",
         "inputSchema": input_schema::<MarkResolveReq>()},
        {"name": "mark_add", "description": "自分の印をモニターに足す（迷ったところ・確認してほしい所を示す。破線で出る）。座標は出力の画素（1920x1080）。区間 t0〜t1 を省くと t を含むシーン。返りで番号 id が分かるので、返事の attachments に {\"mark\": id} で付けられる",
         "inputSchema": input_schema::<MarkAddReq>()},
        {"name": "sync_report", "description": "演出が使った発話同期点の一覧（cue・keyword・end=言い終わりか・t・method）。method=whisper は音声から、manual は手で動かしたもの、ratio は文字位置からの推定（estimated=true）で、音と合っていないかもしれないので耳で確かめる",
         "inputSchema": {"type": "object", "properties": {}}},
        {"name": "fx_reload", "description": "演出のコード（fx/*.py）を直した後に読み直す。終わるまで待ち、エラーがあれば Python のトレースバックを返す（ファイルを保存すると自動でも読み直す）",
         "inputSchema": {"type": "object", "properties": {}}},
        {"name": "fx_log", "description": "演出エンジン（Python ワーカー）のログ",
         "inputSchema": {"type": "object", "properties": {}}},
        {"name": "reply", "description": "AK Studio の画面のチャットに返事を出す。status だけなら「今やっていること」の表示を更新する",
         "inputSchema": input_schema::<ReplyReq>()},
        {"name": "ui_focus", "description": "AK Studio の画面を動かす（再生位置・クリップや行の選択）",
         "inputSchema": input_schema::<UiFocus>()},
        {"name": "export", "description": "書き出し。talk=台本から files/script.srt と files/talk/、mov=演出の透過 .mov（仮表示つき）、mov_clean=本番用、se=効果音、mp4=完成版、preview_mp4=確認用の小さい mp4、rec_track=RecWindow に差し込んだ録画だけを黒の 1920x1080 に置いた録画トラック（studio/export/material_track.mp4。0 秒始まり・演出と同じ長さ。Premiere で演出の下に敷く）、subs_mov=字幕だけの透過 .mov（＋ .srt）、subs_srt=字幕の .srt（手で入れた改行・字幕だけの書き換えも反映）、mix=声＋効果音＋BGM（攻略動画の回は素材の音も）の wav、guide_audio=攻略動画の素材の音だけの wav（studio/export/<回>_guide_audio.wav。範囲 from・to で切り出す。音量は掛けない。攻略動画の回だけ）。攻略動画の回の mp4・mix は、攻略動画の終わりまで・60 fps で素材の音と効果音を混ぜる",
         "inputSchema": input_schema::<ExportReq>()}
    ])
}

/// render_sheet の返事に画像で付けるページの数
const SHEET_IMAGES: usize = 4;

/// 確認画像を Claude に見せる。軽い JPEG（1280x720）があればそちら、無ければ PNG。原寸の PNG は path にある。大きすぎれば付けない
fn image_content(path: &str) -> Option<Value> {
    let jpg = std::path::Path::new(path).with_extension("jpg");
    let (file, mime) = if jpg.exists() { (jpg, "image/jpeg") } else { (path.into(), "image/png") };
    let b = std::fs::read(&file).ok()?;
    if b.len() >= 4_500_000 {
        return None;
    }
    use base64::Engine;
    Some(json!({"type": "image", "mimeType": mime, "data": base64::engine::general_purpose::STANDARD.encode(&b)}))
}

fn text_result(v: &Value) -> Value {
    json!({"content": [{"type": "text", "text": serde_json::to_string_pretty(v).unwrap_or_default()}]})
}

fn err_result(e: String) -> Value {
    json!({"content": [{"type": "text", "text": e}], "isError": true})
}

/// guide_update の「新しい警告:」の行（変えたことの後ろに足す。無ければ空）
fn warnings_new_text(w: &Value) -> String {
    let v: Vec<&str> = w.as_array().map(|a| a.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
    v.iter().map(|s| format!("\n新しい警告: {s}")).collect()
}

fn call_tool(name: &str, a: &Value) -> Value {
    let r = match name {
        "studio_status" => api("GET", "/api/status", None, 10),
        "script_read" => api("GET", "/api/script", None, 10),
        "script_propose" => api("POST", "/api/script/propose", Some(a.clone()), 30),
        "script_apply" => api("POST", "/api/script/apply", Some(a.clone()), 30),
        "memo_write" => api("POST", "/api/memo", Some(a.clone()), 30),
        "clips_list" => {
            let mut q = vec![];
            for k in ["from", "to"] {
                if let Some(v) = a[k].as_f64() {
                    q.push(format!("{k}={v}"));
                }
            }
            if let Some(k) = a["key"].as_str() {
                q.push(format!("key={}", urlenc(k)));
            }
            api("GET", &format!("/api/clips?{}", q.join("&")), None, 10)
        }
        "clip_adjust" => {
            let key = a["key"].as_str().unwrap_or("").to_string();
            let patch = adjust_patch(&a);
            api("POST", "/api/clips/override", Some(json!({"key": key, "patch": patch})), 30)
        }
        "scenes_list" => api("GET", "/api/scenes", None, 10),
        "media_list" => api("GET", "/api/media", None, 10),
        "media_add" => api("POST", "/api/media/add", Some(a.clone()), 60),
        "media_update" => api("POST", "/api/media/update", Some(a.clone()), 30),
        "media_remove" => api("POST", "/api/media/remove", Some(a.clone()), 30),
        "subs_read" => {
            // 配列で受けて "," で繋ぐ（通し番号の数も文字にする）
            let refs: Vec<String> = a["lines"]
                .as_array()
                .map(|v| v.iter().map(|x| x.as_str().map(str::to_string).unwrap_or_else(|| x.to_string())).collect())
                .unwrap_or_default();
            let q = if refs.is_empty() { String::new() } else { format!("?lines={}", urlenc(&refs.join(","))) };
            api("GET", &format!("/api/subs{q}"), None, 60)
        }
        "subs_set" => api("POST", "/api/subs/set", Some(a.clone()), 60),
        "se_list" => api("GET", "/api/se", None, 60),
        "se_set" => api("POST", "/api/se/set", Some(a.clone()), 600),
        "render_still" => match api("POST", "/api/still", Some(a.clone()), 120) {
            Ok(v) => {
                let path = v["path"].as_str().unwrap_or("").to_string();
                let mut content = vec![json!({"type": "text", "text": serde_json::to_string_pretty(&v).unwrap_or_default()})];
                content.extend(image_content(&path));
                return json!({"content": content});
            }
            Err(e) => Err(e),
        },
        "render_sheet" => match api("POST", "/api/still/sheet", Some(a.clone()), 600) {
            Ok(v) => {
                let mut content = vec![json!({"type": "text", "text": serde_json::to_string_pretty(&v).unwrap_or_default()})];
                // 1 回の返事が重くならないよう、画像で見せるのは先頭の数ページ。残りは path から読める
                for pg in v["pages"].as_array().into_iter().flatten().take(SHEET_IMAGES) {
                    content.extend(image_content(pg["path"].as_str().unwrap_or("")));
                }
                return json!({"content": content});
            }
            Err(e) => Err(e),
        },
        "guide_read" => api("GET", "/api/guide", None, 10),
        "guide_update" => match api("POST", "/api/guide/update", Some(a.clone()), 60) {
            // 変えたことの一覧を先に（長い guide_read の形の終わりに埋もれないように）
            Ok(v) => {
                let changed: Vec<String> =
                    v["changed"].as_array().map(|a| a.iter().filter_map(|s| s.as_str().map(str::to_string)).collect()).unwrap_or_default();
                let mut head = if changed.is_empty() { "変わったことはありません（保存していません）".to_string() } else { format!("変えたこと:\n- {}", changed.join("\n- ")) };
                head.push_str(&warnings_new_text(&v["warnings_new"]));
                return json!({"content": [
                    {"type": "text", "text": head},
                    {"type": "text", "text": serde_json::to_string_pretty(&v).unwrap_or_default()},
                ]});
            }
            Err(e) => Err(e),
        },
        "guide_coords" => api("POST", "/api/guide/coords", Some(a.clone()), 10),
        "guide_still" => match api("POST", "/api/guide/still", Some(if a.is_null() { json!({}) } else { a.clone() }), 120) {
            Ok(v) => {
                let path = v["path"].as_str().unwrap_or("").to_string();
                let mut content = vec![json!({"type": "text", "text": serde_json::to_string_pretty(&v).unwrap_or_default()})];
                content.extend(image_content(&path));
                return json!({"content": content});
            }
            Err(e) => Err(e),
        },
        "marks_list" => {
            let all = a["all"].as_bool().unwrap_or(false);
            api("GET", &format!("/api/marks?all={all}"), None, 10)
        }
        "mark_add" => api("POST", "/api/marks/add", Some(a.clone()), 30),
        "mark_resolve" => api("POST", "/api/marks/resolve", Some(a.clone()), 30),
        "sync_report" => api("GET", "/api/sync", None, 30),
        "fx_reload" => api("POST", "/api/fx/reload", Some(json!({})), 150),
        "fx_log" => api("GET", "/api/fx/log", None, 10),
        "reply" => api("POST", "/api/reply", Some(a.clone()), 10),
        "ui_focus" => api("POST", "/api/ui", Some(a.clone()), 10),
        "export" => api("POST", "/api/export", Some(a.clone()), 3600),
        _ => Err(format!("unknown tool {name}")),
    };
    match r {
        Ok(v) => text_result(&v),
        Err(e) => err_result(e),
    }
}

fn urlenc(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// 画面から来たメッセージを channel の通知にする。
fn to_channel(m: &Value) -> Value {
    let (content, meta) = format_msg(m);
    json!({"jsonrpc": "2.0", "method": "notifications/claude/channel", "params": {"content": content, "meta": meta}})
}

/// 画面から来たメッセージを、本文と属性（再生位置・シーン・クリップ・行）にする。
fn format_msg(m: &Value) -> (String, serde_json::Map<String, Value>) {
    let ctx = &m["ctx"];
    let mut meta = serde_json::Map::new();
    meta.insert("msg_id".into(), json!(m["id"].to_string()));
    if let Some(t) = ctx["t"].as_f64() {
        meta.insert("t".into(), json!(format!("{t:.2}")));
    }
    if let Some(s) = ctx["scene"].as_str() {
        meta.insert("scene".into(), json!(s));
    }
    if let Some(c) = ctx["clip"].as_str() {
        meta.insert("clip".into(), json!(c));
    }
    if let Some(ls) = ctx["lines"].as_array() {
        if !ls.is_empty() {
            let ns: Vec<String> = ls.iter().map(|x| x.to_string().trim_matches('"').to_string()).collect();
            meta.insert("lines".into(), json!(ns.join(",")));
        }
    }
    if let Some(ms) = ctx["marks"].as_array() {
        let ns: Vec<String> = ms.iter().filter_map(|x| x.as_u64()).map(|n| n.to_string()).collect();
        if !ns.is_empty() {
            meta.insert("marks".into(), json!(ns.join(",")));
        }
    }
    if let Some(k) = ctx["kind"].as_str() {
        meta.insert("kind".into(), json!(k));
    }
    // 本文は本体が作った文（画像・好き！・[画面の状況] まで入っている）。古い本体なら text に、落ちたことを添える
    let content = match m["prompt"].as_str() {
        Some(p) => p.to_string(),
        None => format!("{}\n{OLD_APP}", m["text"].as_str().unwrap_or("")),
    };
    (content, meta)
}

/// 本体が古くて、Claude に渡す文（prompt・situation）を返さないとき（黙って画像・画面の状況を落とさない）
const OLD_APP: &str = "（AK Studio の本体が古いため、添えた画像・画面の状況を付けられませんでした。本体を作り直してください）";

fn poll_loop(out: Arc<Out>, stop: Arc<AtomicBool>) {
    while !stop.load(Ordering::SeqCst) {
        match api("GET", "/api/inbox?wait=25&consumer=channel", None, 40) {
            Ok(v) => {
                if let Some(ms) = v["messages"].as_array() {
                    for m in ms {
                        out.send(&to_channel(m));
                    }
                }
            }
            Err(_) => std::thread::sleep(Duration::from_secs(3)),
        }
    }
}

/// フック用: 画面からのメッセージが来るまで待ち、来たら stderr に書いて終了コード 2（Claude を起こす）。
/// 来ないまま timeout 秒たったら 0 で終わる（何もしない）。
fn wait_mode(timeout: u64) -> i32 {
    let start = std::time::Instant::now();
    let started = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0);
    loop {
        let left = timeout.saturating_sub(start.elapsed().as_secs());
        if left == 0 {
            return 0;
        }
        let w = left.min(50);
        match api("GET", &format!("/api/inbox?wait={w}&consumer=hook&started={started}"), None, w + 15) {
            Ok(v) => {
                if v["superseded"].as_bool() == Some(true) {
                    return 0; // 新しい待ち受けに任せる
                }
                let ms = v["messages"].as_array().cloned().unwrap_or_default();
                if !ms.is_empty() {
                    eprintln!("{}", hook_text(&ms));
                    return 2;
                }
            }
            Err(_) => std::thread::sleep(Duration::from_secs(5)),
        }
    }
}

fn hook_text(ms: &[Value]) -> String {
    let mut out = String::from("[AK Studio の画面からのメッセージ] 返事は ak-studio の reply ツールで（ユーザーは AK Studio の画面を見ています）
");
    for m in ms {
        let (content, _) = format_msg(m);
        out.push_str(&format!("
---
{content}
"));
    }
    out
}

/// UserPromptSubmit フック用: ターミナルで打ったときにも、画面の状況（再生位置・選択）と未読のメッセージを添える。
fn context_mode() -> i32 {
    let mut parts = vec![];
    if let Ok(v) = api("GET", "/api/inbox?wait=0&consumer=hook", None, 5) {
        let ms = v["messages"].as_array().cloned().unwrap_or_default();
        if !ms.is_empty() {
            parts.push(hook_text(&ms));
        }
    }
    if let Ok(st) = api("GET", "/api/status", None, 5) {
        parts.push(st["situation"].as_str().unwrap_or(OLD_APP).to_string());
    }
    if !parts.is_empty() {
        let ctx = parts.join("
");
        println!("{}", json!({"hookSpecificOutput": {"hookEventName": "UserPromptSubmit", "additionalContext": ctx}}));
    }
    0
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(|s| s.as_str()) == Some("--wait") {
        let t = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(3600);
        // フックの入力（stdin の JSON）は読み捨てる
        std::process::exit(wait_mode(t));
    }
    if args.get(1).map(|s| s.as_str()) == Some("--context") {
        std::process::exit(context_mode());
    }
    let out = Arc::new(Out(Mutex::new(std::io::stdout())));
    let stop = Arc::new(AtomicBool::new(false));
    let mut polling = false;
    let stdin = std::io::stdin();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let Ok(msg) = serde_json::from_str::<Value>(&line) else { continue };
        let id = msg.get("id").cloned();
        let method = msg["method"].as_str().unwrap_or("");
        let result = match method {
            "initialize" => Some(json!({
                "protocolVersion": PROTOCOL,
                "capabilities": {"tools": {}, "experimental": {"claude/channel": {}}},
                "serverInfo": {"name": "ak-studio", "version": env!("CARGO_PKG_VERSION")},
                "instructions": instructions(),
            })),
            "notifications/initialized" | "initialized" => {
                if !polling {
                    polling = true;
                    let (o, s) = (out.clone(), stop.clone());
                    std::thread::spawn(move || poll_loop(o, s));
                }
                None
            }
            "ping" => Some(json!({})),
            "tools/list" => Some(json!({"tools": tools()})),
            "tools/call" => {
                let name = msg["params"]["name"].as_str().unwrap_or("").to_string();
                let args = msg["params"]["arguments"].clone();
                // 時間のかかるツール（書き出しなど）でも他の依頼を止めないよう、別スレッドで答える
                if let Some(id) = id.clone() {
                    let o = out.clone();
                    std::thread::spawn(move || {
                        let r = call_tool(&name, if args.is_null() { &Value::Null } else { &args });
                        o.send(&json!({"jsonrpc": "2.0", "id": id, "result": r}));
                    });
                }
                None
            }
            _ => {
                if id.is_some() && !method.starts_with("notifications/") {
                    out.send(&json!({"jsonrpc": "2.0", "id": id,
                                     "error": {"code": -32601, "message": format!("method not found: {method}")}}));
                }
                None
            }
        };
        if let (Some(id), Some(r)) = (id, result) {
            out.send(&json!({"jsonrpc": "2.0", "id": id, "result": r}));
        }
    }
    stop.store(true, Ordering::SeqCst);
}

/// clip_adjust の引数から本体に送る patch を作る。reset=true なら null（全部戻す）。
/// それ以外は、キーがある項目をそのまま入れる（null は「その項目だけ戻す」）。key・reset は入れない。
fn adjust_patch(a: &Value) -> Value {
    if a["reset"].as_bool() == Some(true) {
        return Value::Null;
    }
    let mut m = serde_json::Map::new();
    for k in ["shift", "end", "dx", "dy", "scale", "opacity", "pivot", "hidden", "z", "note"] {
        if let Some(v) = a.get(k) {
            m.insert(k.into(), v.clone());
        }
    }
    Value::Object(m)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn connect_error_says_to_reopen_and_keeps_cause() {
        let m = connect_error("Connection Failed: Connect error: refused");
        assert!(m.contains("開き直して"), "{m}");
        assert!(m.contains("session.json"), "{m}");
        assert!(m.contains("Connection Failed: Connect error: refused"), "{m}");
    }

    #[test]
    fn error_text_has_code_and_detail() {
        let v = json!({"ok": false, "error": "ffmpeg が見つかりません", "code": "need_ffmpeg", "detail": "動画: ffmpeg が見つかりません: No such file"});
        let t = error_text(&v, 500);
        assert!(t.starts_with("ffmpeg が見つかりません（code: need_ffmpeg"), "{t}");
        assert!(t.contains("
詳細: 動画: ffmpeg が見つかりません: No such file"), "{t}");
        // detail が error と同じなら足さない。code が無い古い応答もそのまま読める
        let same = json!({"error": "だめ", "code": "failed", "detail": "だめ"});
        assert!(!error_text(&same, 400).contains("詳細"));
        assert_eq!(error_text(&json!({"error": "だめ"}), 404), "だめ (404)");
    }

    fn tool<'a>(tools: &'a Value, name: &str) -> &'a Value {
        tools.as_array().unwrap().iter().find(|t| t["name"] == name).unwrap_or_else(|| panic!("ツール {name} が無い"))
    }

    fn names(v: &Value, key: &str) -> Vec<String> {
        let mut out: Vec<String> = match &v[key] {
            Value::Object(m) => m.keys().cloned().collect(),
            Value::Array(a) => a.iter().map(|x| x.as_str().unwrap().to_string()).collect(),
            _ => vec![],
        };
        out.sort();
        out
    }

    /// 全ツールの inputSchema は object で、$ref を含まない
    #[test]
    fn every_input_schema_is_a_plain_object() {
        let t = tools();
        assert!(t.as_array().unwrap().len() >= 20);
        for x in t.as_array().unwrap() {
            let s = &x["inputSchema"];
            let name = x["name"].as_str().unwrap();
            assert_eq!(s["type"], "object", "{name}");
            let text = s.to_string();
            for bad in ["$ref", "$defs", "$schema"] {
                assert!(!text.contains(bad), "{name} に {bad} がある: {text}");
            }
        }
    }

    /// schemars で作るようにしたツールの properties の名前と required が、手書きだったときと同じ
    #[test]
    fn generated_schemas_keep_property_names_and_required() {
        let t = tools();
        // (ツール, properties, required)。手書きだった頃の値
        let table: [(&str, &[&str], &[&str]); 19] = [
            ("subs_set", &["items", "reason"], &["items"]),
            ("script_propose", &["items", "note", "title"], &["items"]),
            ("script_apply", &["items", "title"], &["items"]),
            ("memo_write", &["memo"], &["memo"]),
            ("clips_list", &["from", "key", "to"], &[]),
            ("media_update", &["clip_id", "patch"], &["clip_id", "patch"]),
            ("media_remove", &["clip_id"], &["clip_id"]),
            ("se_set", &["kinds", "render"], &["kinds"]),
            ("render_still", &["cue", "scene", "solo", "subs", "t"], &[]),
            ("render_sheet", &["cues", "subs", "ts"], &[]),
            ("reply", &["attachments", "status", "text"], &[]),
            ("ui_focus", &["clip", "line", "scene", "t"], &[]),
            ("export", &["from", "kind", "to"], &["kind"]),
            ("guide_update", &["formation", "ops", "reason", "subs"], &["reason"]),
            ("guide_still", &["crop", "frame", "grid", "op", "scale", "t", "t_out"], &[]),
            ("guide_coords", &["part", "points", "to"], &["part", "points"]),
            ("marks_list", &["all"], &[]),
            ("mark_resolve", &["id", "image", "note"], &["id"]),
            ("mark_add", &["color", "note", "shape", "t", "t0", "t1"], &["color", "shape", "t"]),
        ];
        for (name, props, req) in table {
            let s = &tool(&t, name)["inputSchema"];
            assert_eq!(names(s, "properties"), props, "{name} の properties");
            assert_eq!(names(s, "required"), req, "{name} の required");
        }
        // 推敲案の 1 件
        for name in ["script_propose", "script_apply"] {
            let item = &tool(&t, name)["inputSchema"]["properties"]["items"]["items"];
            assert_eq!(item["type"], "object", "{name}");
            assert_eq!(names(item, "properties"), ["after", "after_block", "block", "expect", "kind", "line", "lines", "reason", "sp", "text", "title"], "{name}");
            assert_eq!(names(item, "required"), ["kind"], "{name}");
            assert_eq!(item["properties"]["kind"]["enum"], json!(["edit", "insert", "delete", "speaker", "memo", "notes", "move", "block_add", "block_delete", "block_rename"]), "{name}");
        }
        // 候補（enum）と説明が出る
        let kinds = &tool(&t, "export")["inputSchema"]["properties"]["kind"]["enum"];
        assert_eq!(kinds.as_array().unwrap().len(), 11);
        assert_eq!(tool(&t, "render_still")["inputSchema"]["properties"]["solo"]["description"], "このクリップだけを描く");
    }

    /// 字幕のツール: subs_read の引数は配列、subs_set の 1 件は line が必須で、説明（INSTRUCTIONS）に出ている
    #[test]
    fn subs_tools_exist_and_are_described() {
        let t = tools();
        assert_eq!(tool(&t, "subs_read")["inputSchema"]["properties"]["lines"]["type"], "array");
        let item = &tool(&t, "subs_set")["inputSchema"]["properties"]["items"]["items"];
        assert_eq!(names(item, "properties"), ["font", "line", "size", "text"]);
        assert_eq!(names(item, "required"), ["line"]);
        for name in ["subs_read", "subs_set"] {
            assert!(INSTRUCTIONS.contains(name), "{name} が INSTRUCTIONS に無い");
        }
    }

    /// 攻略動画のツール: 説明の文・操作の中身の項目・説明（INSTRUCTIONS）に出ている
    #[test]
    fn guide_tools_exist_and_are_described() {
        let t = tools();
        assert_eq!(tool(&t, "guide_read")["inputSchema"], json!({"type": "object", "properties": {}}));
        let u = &tool(&t, "guide_update")["inputSchema"];
        // 知らない項目は断る（アンカー・カット・固定値は直せない）
        assert_eq!(u["additionalProperties"], false);
        let add = &u["properties"]["ops"]["properties"]["add"]["items"];
        assert_eq!(names(add, "properties"), ["cue_text", "frame", "info", "kind", "note", "now_lead", "op", "precise", "t", "t_out", "where"]);
        assert_eq!(names(add, "required"), ["kind"]);
        let upd = &u["properties"]["ops"]["properties"]["update"]["items"];
        assert_eq!(names(upd, "required"), ["id"]);
        assert!(upd["properties"]["now_lead"]["description"].as_str().unwrap().contains("null"));
        assert_eq!(u["properties"]["ops"]["properties"]["remove"]["items"]["type"], "string");
        assert_eq!(names(&u["properties"]["formation"], "properties"), ["notes", "used"]);
        assert_eq!(u["properties"]["subs"]["type"], "array");
        // 引数の説明は日本語の 1 行
        assert!(add["properties"]["t_out"]["description"].as_str().unwrap().contains("出力の秒"));
        assert_eq!(tool(&t, "guide_coords")["inputSchema"]["properties"]["to"]["enum"], json!(["out", "src"]));
        assert_eq!(tool(&t, "guide_still")["inputSchema"]["properties"]["crop"]["items"]["type"], "number");
        for name in ["guide_read", "guide_update", "guide_still", "guide_coords", "g.op_t"] {
            assert!(INSTRUCTIONS.contains(name), "{name} が INSTRUCTIONS に無い");
        }
        assert!(INSTRUCTIONS.contains("アンカー・カット・固定値"));
        // クリア時間は説明にも INSTRUCTIONS にも出ている
        assert!(INSTRUCTIONS.contains("clear_time"));
        assert!(tool(&t, "guide_read")["description"].as_str().unwrap().contains("clear_time"));
    }

    /// guide_update の新しい警告は、変えたことの後ろに「新しい警告:」の行で並ぶ（無ければ何も足さない）
    #[test]
    fn warnings_new_lines() {
        assert_eq!(warnings_new_text(&json!([])), "");
        assert_eq!(warnings_new_text(&Value::Null), "");
        assert_eq!(warnings_new_text(&json!(["補足素材 1（B.MP4）: 操作「op3」がありません", "b"])),
                   "\n新しい警告: 補足素材 1（B.MP4）: 操作「op3」がありません\n新しい警告: b");
    }

    #[test]
    fn sheet_and_sync_tools_exist() {
        let t = tools();
        let s = &tool(&t, "render_sheet")["inputSchema"];
        assert_eq!(s["properties"]["cues"]["type"], "array");
        assert_eq!(s["properties"]["cues"]["items"]["type"], "integer");
        assert_eq!(s["properties"]["ts"]["items"]["type"], "number");
        assert_eq!(s["properties"]["subs"]["type"], "boolean");
        assert_eq!(tool(&t, "sync_report")["inputSchema"], json!({"type": "object", "properties": {}}));
        for name in ["render_sheet", "sync_report"] {
            assert!(INSTRUCTIONS.contains(name), "{name} が INSTRUCTIONS に無い");
        }
    }

    #[test]
    fn adjust_patch_keeps_present_fields_including_null() {
        assert_eq!(adjust_patch(&json!({"key": "a", "dx": 10})), json!({"dx": 10}));
        assert_eq!(adjust_patch(&json!({"key": "a", "dx": null})), json!({"dx": null}));
        assert_eq!(adjust_patch(&json!({"key": "a", "reset": true, "dx": 1})), Value::Null);
        assert_eq!(adjust_patch(&json!({"key": "a", "reset": false})), json!({}));
    }

    #[test]
    fn format_msg_uses_prompt_then_text() {
        let m = json!({"id": 7, "text": "元", "prompt": "本体の文", "ctx": {"t": 1.5, "scene": "2", "clip": "a@c1#1", "lines": [3, 4], "kind": "x"}});
        let (content, meta) = format_msg(&m);
        assert_eq!(content, "本体の文");
        assert_eq!(meta["msg_id"], "7");
        assert_eq!(meta["t"], "1.50");
        assert_eq!(meta["scene"], "2");
        assert_eq!(meta["clip"], "a@c1#1");
        assert_eq!(meta["lines"], "3,4");
        assert_eq!(meta["kind"], "x");
        // 印の番号は meta の marks に（無ければ付けない）
        let (_, meta) = format_msg(&json!({"id": 9, "prompt": "p", "ctx": {"marks": [1, 2, 5]}}));
        assert_eq!(meta["marks"], "1,2,5");
        assert!(!format_msg(&json!({"id": 9, "prompt": "p", "ctx": {"marks": []}})).1.contains_key("marks"));
        assert_eq!(format_msg(&json!({"id": 8, "text": "だけ", "ctx": {}})).0, format!("だけ\n{OLD_APP}"));
    }

    /// 印のツール: 3 つあり、説明・INSTRUCTIONS に使い方が出ている。形は色の候補と 4 つの形
    #[test]
    fn mark_tools_exist_and_are_described() {
        let t = tools();
        for name in ["marks_list", "mark_resolve", "mark_add"] {
            assert!(INSTRUCTIONS.contains(name), "{name} が INSTRUCTIONS に無い");
            assert!(!tool(&t, name)["description"].as_str().unwrap().is_empty());
        }
        assert!(INSTRUCTIONS.contains("{\"mark\": <印の番号>}"));
        let add = &tool(&t, "mark_add")["inputSchema"];
        assert_eq!(add["properties"]["color"]["enum"], json!(["red", "yellow", "green", "blue"]));
        let kinds: Vec<&str> = add["properties"]["shape"]["oneOf"].as_array().unwrap().iter()
            .map(|o| o["properties"]["kind"]["const"].as_str().unwrap()).collect();
        assert_eq!(kinds, ["pin", "rect", "arrow", "pen"]);
        assert!(add["properties"]["note"]["description"].is_null() || add["properties"]["note"]["type"] == "string");
        assert_eq!(tool(&t, "mark_resolve")["inputSchema"]["properties"]["image"]["type"], "string");
    }

    #[test]
    fn instructions_mention_attached_images() {
        assert!(INSTRUCTIONS.contains("（画像: …）があれば、Read ツール"));
    }

    /// 補足が無ければ説明のまま、あれば末尾に付く
    #[test]
    fn context_is_appended_only_when_present() {
        assert_eq!(with_context("本文", ""), "本文");
        assert_eq!(with_context("本文", " 
"), "本文");
        assert_eq!(with_context("本文", "補足
"), "本文

補足");
    }
}
