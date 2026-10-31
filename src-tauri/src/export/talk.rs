//! 台本 → 演出の入力（files/script.srt・talk/）の書き出しと、cue 番号がずれたときのキーの付け替え。

use crate::core::Core;
use super::{ms, sorted_layout, srt_time};
use crate::voice::layout::VoiceItem;
use ak_proto::{Cue, CueTable};
use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::Arc;

// ======================================================================= 台本 → 演出の入力
pub fn export_talk(core: &Arc<Core>) -> Result<Value> {
    let root = core.root().ok_or_else(crate::app::error::no_project)?;
    let items = sorted_layout(core);
    if items.is_empty() {
        bail!("書き出すセリフがありません");
    }
    let missing: Vec<usize> = items.iter().filter(|i| i.audio.is_none()).map(|i| i.n).collect();
    if !missing.is_empty() {
        bail!("まだ音声を作っていない行があります（行 {:?}）。先にすべての行の音声を作ってください", &missing[..missing.len().min(12)]);
    }
    let cache = root.join("voice_cache");
    // 1. voice_out/（台本エディタの書き出しと同じ。フルWAV・話者別・SRT）
    let body = json!({"name": "talk", "items": items.iter().map(|it| json!({
        "id": it.audio, "speaker": it.sp, "subtitle": it.text, "start": (it.start * 1000.0).round() / 1000.0
    })).collect::<Vec<_>>()});
    let assembled = core.voice.post("/api/assemble", &body).ok();

    // 2. files/script.srt（[話者] 付き）と files/talk/
    let files = root.join("files");
    std::fs::create_dir_all(&files)?;
    let srt_p = files.join("script.srt");
    let (srt, cues) = talk_files(&items);
    let mut warn = vec![];
    // 前の script.srt と今のセリフ（cue 番号がずれたら、script.srt を書いたあとで演出クリップのキーを付け替える。旧#38）
    let mut rekey: Option<Rekey> = None;
    match std::fs::read(&srt_p) {
        Ok(bytes) => {
            let old_cow = String::from_utf8_lossy(&bytes);
            let utf8 = matches!(old_cow, std::borrow::Cow::Borrowed(_));
            let old = old_cow.into_owned();
            if old != srt {
                let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S-%3f");
                if let Err(e) = std::fs::write(files.join(format!("script_{stamp}.srt")), &bytes) {
                    warn.push(format!("前の script.srt の控えを保存できませんでした: {}", crate::app::error::reason(e)));
                }
                // cue 番号のずれ（タイムラインの c(n) が別のセリフを指すようになる）
                const SKIP: &str = "ので、演出の調整と差し込んだ録画のキーを付け替えていません。調整と録画が外れていないか確かめてください";
                match (utf8, crate::script::rekey::srt_lines(&old), crate::script::rekey::srt_lines(&srt)) {
                    (false, _, _) => warn.push(format!("前の script.srt が UTF-8 ではない{SKIP}")),
                    (_, None, _) => warn.push(format!("前の script.srt の cue を読めなかった（番号が 1 から順に並んでいない・3 行に満たない cue がある）{SKIP}")),
                    (_, _, None) => warn.push(format!("書き出す script.srt の cue を読めなかった（セリフに空行が入っているなど）{SKIP}")),
                    (_, Some(old_txt), Some(new_txt)) => {
                        if let Some(k) = (0..old_txt.len().min(new_txt.len())).find(|&k| old_txt[k] != new_txt[k]) {
                            warn.push(format!("cue {} から文が変わりました（前: {} / 今: {}）", k + 1, old_txt[k], new_txt[k]));
                        }
                        if old_txt.len() != new_txt.len() {
                            warn.push(format!("cue の数が {} → {} に変わりました。タイムラインの c(n)・s(n, …) の番号を見直してください",
                                              old_txt.len(), new_txt.len()));
                        }
                        if old_txt.is_empty() && !new_txt.is_empty() {
                            warn.push(format!("前の script.srt に cue が無い{SKIP}"));
                        } else if old_txt != new_txt {
                            rekey = Some(Rekey { root: root.clone(), map: crate::script::rekey::cue_map(&old_txt, &new_txt), old_txt, new_txt });
                        }
                    }
                }
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => warn.push(format!("前の script.srt を読めなかったので、演出の調整と差し込んだ録画のキーを付け替えていません: {}", crate::app::error::reason(e))),
    }
    let talk = files.join("talk");
    let width = if items.len() >= 100 { 3 } else { 2 };
    // 一時フォルダに作ってから入れ替える（途中で失敗しても、前の talk/ は半端にならない。NEW-3）。
    // 番号がずれると別のセリフが残るので、前の talk/ の中身は使わずに作り直す
    let tmp = files.join(".talk.tmp");
    let _ = std::fs::remove_dir_all(&tmp);
    let built = (|| -> Result<()> {
        for (i, it) in items.iter().enumerate() {
            let d = tmp.join(&it.sp);
            std::fs::create_dir_all(&d)?;
            let stem = format!("{:0width$}", i + 1, width = width);
            let Some(audio) = it.audio.as_ref() else {
                bail!("行 {} の音声がありません", i + 1);
            };
            let src = cache.join(format!("{audio}.wav"));
            std::fs::copy(&src, d.join(format!("{stem}.wav"))).with_context(|| format!("{}", src.display()))?;
            std::fs::write(d.join(format!("{stem}.txt")), &it.text)?;
        }
        Ok(())
    })();
    if let Err(e) = built {
        let _ = std::fs::remove_dir_all(&tmp);
        return Err(e);
    }
    let old = files.join(".talk.old");
    let _ = std::fs::remove_dir_all(&old);
    if talk.exists() {
        std::fs::rename(&talk, &old).with_context(|| format!("{} を入れ替えられません（開いているアプリを閉じてください）", talk.display()))?;
    }
    if let Err(e) = std::fs::rename(&tmp, &talk) {
        let _ = std::fs::rename(&old, &talk);
        return Err(anyhow::Error::new(e).context(format!("{} を入れ替えられません", talk.display())));
    }
    let _ = std::fs::remove_dir_all(&old);
    // script_cues.json（演出の fxkit が読む cue の表）と script.srt は talk/ ができてから書く（片方だけ新しくならないように）。
    // srt は前の版との突き合わせ（rekey）に使うので、表を先に書く
    let cues_p = files.join("script_cues.json");
    crate::settings::write_atomic(&cues_p, serde_json::to_string_pretty(&cues)?)?;
    crate::settings::write_atomic(&srt_p, &srt)?;
    // 行ごとのメモ（Claude 向け）。cue は script.srt の番号。メモが無ければ [] にして、古いメモを残さない
    let script = core.script();
    let memos: Vec<Value> = items
        .iter()
        .enumerate()
        .filter_map(|(i, it)| {
            let memo = script.as_ref().and_then(|s| crate::script::ops::memo_of(s, &it.line_id))?;
            (!memo.is_empty()).then(|| json!({"cue": i + 1, "sp": it.sp, "text": it.text, "memo": memo}))
        })
        .collect();
    let memo_p = files.join("script_memo.json");
    crate::settings::write_atomic(&memo_p, serde_json::to_string_pretty(&memos)?)?;
    // script.srt はもう新しいので、付け替えに失敗しても書き出しは失敗にせず、errors で知らせる（画面はエラーの知らせで出す）
    let mut errors: Vec<String> = vec![];
    if let Some(r) = rekey {
        r.apply(core, &mut warn, &mut errors);
    }
    if !errors.is_empty() {
        // Claude（MCP）から書き出したときも画面に出るように、Rust から知らせる
        core.emit("toast", json!({"text": errors.join("\n"), "kind": "error", "ms": 30000}));
    }
    Ok(json!({"srt": srt_p, "cues_json": cues_p, "memo": memo_p, "talk": talk, "cues": items.len(), "warnings": warn, "errors": errors, "voice_out": assembled}))
}

/// セリフの並びから script.srt の中身と cue の表を作る（時刻は同じミリ秒の丸め）
fn talk_files(items: &[VoiceItem]) -> (String, CueTable) {
    let mut srt = String::new();
    let mut cues = Vec::with_capacity(items.len());
    for (i, it) in items.iter().enumerate() {
        srt += &format!("{}\n{} --> {}\n[{}] {}\n\n", i + 1, srt_time(it.start), srt_time(it.start + it.dur), it.sp, it.text);
        cues.push(Cue {
            n: i as i64 + 1,
            start: ms(it.start) as f64 / 1000.0,
            end: ms(it.start + it.dur) as f64 / 1000.0,
            text: it.text.clone(),
            speaker: it.sp.clone(),
        });
    }
    (srt, CueTable { cues })
}

/// cue 番号がずれたときの、fx/studio_overrides.json と project.json の nests のキー（部品名@c<cue>#<順番>）の付け替え（旧#38）
struct Rekey {
    root: PathBuf,
    map: crate::script::rekey::CueMap,
    old_txt: Vec<String>,
    new_txt: Vec<String>,
}

/// 付け替える前の内容を、日時つきの名前で同じフォルダに保存する（保存したファイル名を返す）
fn save_backup(p: &Path, stamp: &str, v: &impl serde::Serialize) -> Result<String> {
    let name = format!("{}_{stamp}.json", p.file_stem().and_then(|s| s.to_str()).unwrap_or("backup"));
    let out = p.with_file_name(&name);
    if let Some(d) = out.parent() {
        std::fs::create_dir_all(d)?;
    }
    crate::settings::write_atomic(&out, serde_json::to_string_pretty(v)?)
        .with_context(|| format!("付け替えの前に {} の控えを保存できませんでした", out.display()))?;
    Ok(name)
}

/// キーの一覧（多いときは 12 個まで）
fn key_list<S: AsRef<str>>(keys: &[S]) -> String {
    let mut s = keys.iter().take(12).map(|k| k.as_ref()).collect::<Vec<_>>().join(", ");
    if keys.len() > 12 {
        s += &format!(" ほか {} 件", keys.len() - 12);
    }
    s
}

impl Rekey {
    /// script.srt を書いたあとで付け替える。付け替えた数などは warn、失敗は errors に入れる
    fn apply(self, core: &Arc<Core>, warn: &mut Vec<String>, errors: &mut Vec<String>) {
        use crate::script::rekey;
        let n_old = self.old_txt.len();
        let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S-%3f").to_string();
        // 控えは、書き換えるのと同じ鍵の中で、今の内容から保存する（保存できなければ何も変えない）
        let res = core.update_doc_and_overrides(&self.root, |project, doc, ov| -> Result<_> {
            let (ov_path, project_path) = (project.overrides_path(), project.studio_dir().join("project.json"));
            let ov_keys: Vec<String> = ov.keys().cloned().collect();
            let ov_plan = rekey::plan(&ov_keys, &self.map, n_old);
            let nest_plan = rekey::plan(doc.nests.keys(), &self.map, n_old);
            let mut saved = vec![];
            if !nest_plan.is_empty() {
                saved.push(save_backup(&project_path, &stamp, &*doc)?);
            }
            if !ov_plan.is_empty() {
                saved.push(save_backup(&ov_path, &stamp, &*ov)?);
            }
            // 録画を差し込んだ窓の no_placeholder（自動で書く印）だけのものは、調整の数に入れない
            let auto_only = |k: &str| ov.get(k).and_then(|v| v.as_object()).map(|o| o.keys().all(|x| x == "no_placeholder")).unwrap_or(false);
            let n_ov = ov_plan.renamed.iter().filter(|(o, _)| !auto_only(o)).count();
            rekey::apply(&mut doc.nests, &nest_plan);
            rekey::apply_json(ov, &ov_plan);
            Ok((ov_plan, nest_plan, n_ov, saved))
        });
        let (ov_plan, nest_plan, n_ov, saved) = match res {
            Ok(x) => x,
            Err(e) => {
                let (text, detail) = crate::app::error::notice_text("演出の調整と差し込んだ録画のキーを付け替えられませんでした", &e);
                core.engine.push_log(detail);
                errors.push(text);
                return;
            }
        };
        let saved = if saved.is_empty() { String::new() } else { format!("前の内容は {} に保存しました。", saved.join("・")) };
        let n_nest = nest_plan.renamed.len();
        if n_ov + n_nest > 0 {
            warn.push(format!("cue 番号が変わったので、演出の調整 {n_ov} 件・差し込んだ録画 {n_nest} 件のキーを新しい番号に付け替えました。{saved}\
                               演出のコードの c(n) を新しい番号に直すと、同じクリップに付きます（直すまでは別のクリップに付いて見えます）"));
        }
        // 文が変わった行に付け替えたもの（同じ場面か、目で確かめてもらう）
        let mut changed: Vec<String> = ov_plan
            .renamed
            .iter()
            .chain(&nest_plan.renamed)
            .filter(|(o, n)| {
                let cue = |k: &str| rekey::parse_key(k).map(|(_, c, _)| c);
                match (cue(o), cue(n)) {
                    (Some(a), Some(b)) => self.old_txt.get(a - 1) != self.new_txt.get(b - 1),
                    _ => false,
                }
            })
            .map(|(o, n)| format!("{o} → {n}"))
            .collect();
        changed.sort();
        changed.dedup();
        if !changed.is_empty() {
            warn.push(format!("文が変わった行に付け替えたキーがあります。同じ場面か確かめてください: {}", key_list(&changed)));
        }
        let mut kept: Vec<&String> = ov_plan.kept.iter().chain(&nest_plan.kept).collect();
        kept.sort();
        kept.dedup();
        if !kept.is_empty() {
            warn.push(format!("削除した行に付いていた調整・録画は付け替えられませんでした（前のキーのまま残しています）: {}", key_list(&kept)));
        }
        let mut dropped: Vec<&String> = ov_plan.dropped.iter().chain(&nest_plan.dropped).collect();
        dropped.sort();
        dropped.dedup();
        if !dropped.is_empty() {
            warn.push(format!("付け替えたキーと重なったので、次の調整・録画を削除しました: {}。{saved}", key_list(&dropped)));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(n: usize, sp: &str, text: &str, start: f64, dur: f64) -> VoiceItem {
        VoiceItem { line_id: format!("l{n}"), n, sp: sp.into(), text: text.into(), start, dur, audio: None, block: String::new(), key: String::new() }
    }

    fn sample() -> Vec<VoiceItem> {
        vec![
            item(1, "トーク1", "こんにちは 世界", 0.0, 1.5),
            item(2, "トーク2", "二行目です。", 1.5, 0.1235),
            item(3, "トーク1", "さいごの  行", 1.6235, 3.0005),
        ]
    }

    fn fixture(name: &str) -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(name)
    }

    /// 見本（script_cues.json・script_cues.srt）と一字一句同じ。fxkit/test_timeline.py も同じ見本の JSON を読む。
    /// 書き出しの形を変えたときは AK_UPDATE_FIXTURES=1 で作り直し、fxkit 側のテストも通ることを確かめる
    #[test]
    fn talk_files_match_fixtures() {
        let (srt, cues) = talk_files(&sample());
        let json = serde_json::to_string_pretty(&cues).unwrap();
        if std::env::var("AK_UPDATE_FIXTURES").is_ok() {
            std::fs::write(fixture("script_cues.json"), &json).unwrap();
            std::fs::write(fixture("script_cues.srt"), &srt).unwrap();
        }
        let read = |n: &str| std::fs::read_to_string(fixture(n)).unwrap().replace("\r\n", "\n");
        assert_eq!(json, read("script_cues.json"));
        assert_eq!(srt, read("script_cues.srt"));
    }

    #[test]
    fn cue_times_round_like_srt() {
        let (srt, cues) = talk_files(&sample());
        assert_eq!(cues.cues[1].end, 1.624);
        assert!(srt.contains("00:00:01,500 --> 00:00:01,624"));
        assert_eq!(cues.cues[2].end, 4.624);
        assert_eq!(cues.cues[0].n, 1);
        assert_eq!(cues.cues[2].speaker, "トーク1");
    }

    /// srt を rekey の読み方で読むと、表の `[speaker] text` と同じ並びになる
    #[test]
    fn srt_and_table_agree() {
        let (srt, cues) = talk_files(&sample());
        let lines = crate::script::rekey::srt_lines(&srt).unwrap();
        let want: Vec<String> = cues.cues.iter().map(|c| format!("[{}] {}", c.speaker, c.text)).collect();
        assert_eq!(lines, want);
    }
}
