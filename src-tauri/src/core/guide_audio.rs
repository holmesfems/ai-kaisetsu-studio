//! 攻略動画の素材の音と、効果音の自動の作り直し。純粋な部分は `crate::guide::audio`。
//!
//! - 素材の音: 通しの素材の音を、組み立ての区間どおりに並べた `studio/guide_audio.wav`。モニターの再生と書き出しが読む。
//!   組み立て（区間）か素材が変わったときだけ作り直す。`guide_rebuild` の後に裏のスレッドで作り、できたら `guide` イベントで知らせる
//! - 効果音: 操作（時刻・種類）が変わったら、裏で 0.8 秒待ってから `export_se` と同じ作り直しを走らせ、`se_bank` イベントで知らせる

use super::Core;
use crate::app::error::{coded, no_project};
use crate::export::audio::{decode_audio, write_wav};
use crate::guide::audio::{
    arrange, audio_key, needs_rebuild, parse_stream_starts, se_signature, se_signature_for, se_wav_path, timeline_in_root, AUDIO_FILE,
    AUDIO_KEY_FILE, SE_KEY_FILE,
};
use crate::guide::plan::ops_out;
use crate::settings::{hidden, write_atomic};
use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::Duration;

/// 効果音を作り直すまでの待ち（続けて操作が変わったら、最後の 1 回にまとめる）
const SE_DEBOUNCE: Duration = Duration::from_millis(800);
/// 素材の音を作り直すまでの待ち（ドラッグなどで続けて組み立てが変わる間は待つ）
const AUDIO_DEBOUNCE: Duration = Duration::from_millis(300);

/// `guide_audio_ensure` の結果
#[derive(Debug, Clone, PartialEq)]
pub enum GuideAudio {
    /// 攻略動画の回ではない
    NotGuide,
    /// 素材に音が無い
    NoAudio,
    /// 今の組み立ての素材の音の wav
    Ready(PathBuf),
}

/// 調べた素材の音の情報（音そのもの（PCM）は、作るたびに読んで、作り終えたら捨てる。数百 MB になるので持ち続けない）
pub(super) struct AudioSrc {
    /// 素材のパス・大きさ・更新時刻（ミリ秒）
    key: (String, u64, u64),
    /// 素材の音のずれ（音のストリームの start − 映像のストリームの start）
    origin: f64,
}

fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

fn mtime_ms(md: &std::fs::Metadata) -> u64 {
    md.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_millis() as u64).unwrap_or(0)
}

fn is_canceled(e: &anyhow::Error) -> bool {
    crate::app::error::api_error(e).code == ak_proto::ErrorCode::Canceled
}

/// 素材の音のずれ（音のストリームの start − 映像のストリームの start）。ffprobe が ffmpeg の隣にあるときだけ読み、
/// 無い・読めないときは 0（映像と音を同じ時刻から始まるものとみなす）
fn stream_origin(ffmpeg: &str, path: &str) -> f64 {
    let probe = Path::new(ffmpeg).with_file_name(if cfg!(windows) { "ffprobe.exe" } else { "ffprobe" });
    if !probe.exists() {
        return 0.0;
    }
    let out = hidden(Command::new(&probe))
        .args(["-v", "error", "-show_entries", "stream=codec_type,start_time", "-of", "csv=p=0", path])
        .output();
    match out {
        Ok(o) if o.status.success() => match parse_stream_starts(&String::from_utf8_lossy(&o.stdout)) {
            (Some(v), Some(a)) => a - v,
            _ => 0.0,
        },
        _ => 0.0,
    }
}

impl Core {
    /// `studio/guide_audio.wav`
    pub fn guide_audio_path(&self) -> Option<PathBuf> {
        self.root().map(|r| r.join("studio").join(AUDIO_FILE))
    }

    /// 素材の音の状態を入れる（回を開き直していれば入れない）。変わったら知らせる
    fn guide_audio_set(&self, gen: u64, rev: Option<u64>, warn: Option<String>, err: Option<String>) {
        {
            let mut rt = self.guide_rt.write();
            if rt.gen != gen {
                return;
            }
            if rt.audio_rev == rev && rt.audio_warn == warn && rt.audio_err == err {
                return;
            }
            rt.audio_rev = rev;
            rt.audio_warn = warn;
            rt.audio_err = err;
        }
        self.emit("guide", self.guide_state());
    }

    /// 素材の音の wav を今の組み立てのものにして返す（要れば作る。呼んだスレッドで読み・書きするので、画面のスレッドでは呼ばない）。
    /// 作れなければ Err（書き出しは止める。黙って音なしにしない）
    pub fn guide_audio_ensure(&self) -> Result<GuideAudio> {
        let _one = self.guide_audio_lock.lock();
        // guide・plan・gen・root は同じ瞬間のものを使う（回を開き直すと、gen と root が一緒に変わる）
        let (guide, plan, gen, root, why) = {
            let rt = self.guide_rt.read();
            (rt.guide.clone(), rt.plan.clone(), rt.gen, rt.root.clone(), rt.error())
        };
        let Some(guide) = guide else { return Ok(GuideAudio::NotGuide) };
        let Some(plan) = plan else {
            bail!("{}", why.unwrap_or_else(|| "パートを組み立てられていません。素材を解析して、アンカーを確かめてください".into()));
        };
        let root = root.ok_or_else(no_project)?;
        let src = root.join(&guide.src);
        let src_s = src.to_string_lossy().to_string();
        let ff = self.ffmpeg();
        if ff.is_empty() {
            return Err(coded(ak_proto::ErrorCode::NeedFfmpeg, "ffmpeg が見つかりません"));
        }
        let md = std::fs::metadata(&src).with_context(|| format!("素材（{}）を開けませんでした", src.display()))?;
        if !self.media.video(&src_s)?.info.has_audio {
            self.guide_audio_set(gen, None, Some("素材に音がありません".into()), None);
            return Ok(GuideAudio::NoAudio);
        }
        let ckey = (src_s.clone(), md.len(), mtime_ms(&md));
        let origin = {
            let mut c = self.guide_audio_src.lock();
            if c.as_ref().map(|x| &x.key) != Some(&ckey) {
                *c = Some(AudioSrc { key: ckey.clone(), origin: stream_origin(&ff, &src_s) });
            }
            c.as_ref().map(|x| x.origin).unwrap_or(0.0)
        };
        let segs = plan.audio_segments();
        let key = audio_key(&segs, &guide.src, ckey.1, ckey.2, origin);
        let studio = root.join("studio");
        let (wav, kp) = (studio.join(AUDIO_FILE), studio.join(AUDIO_KEY_FILE));
        let prev: Option<Value> = std::fs::read_to_string(&kp).ok().and_then(|t| serde_json::from_str(&t).ok());
        if needs_rebuild(prev.as_ref(), &key, wav.exists()) {
            let out = {
                // 読んだ音は並べ終えたらすぐ捨てる
                let pcm = decode_audio(&ff, &src_s)?;
                arrange(&pcm, origin, &segs, plan.end())
            };
            std::fs::create_dir_all(&studio)?;
            // 書き込みの途中を読まれないよう、一時ファイルに書いてから名前を変える
            let tmp = studio.join("guide_audio.tmp.wav");
            write_wav(&tmp, &out).with_context(|| format!("{} を書けませんでした", tmp.display()))?;
            drop(out);
            // 作っている間に回を開き直していたら、置き換えない（別の回の組み立てで今の回の wav・鍵を書かない）
            if self.guide_rt.read().gen != gen {
                let _ = std::fs::remove_file(&tmp);
                return Err(coded(ak_proto::ErrorCode::Canceled, "回が切り替わったので、素材の音の作成をやめました"));
            }
            std::fs::rename(&tmp, &wav).with_context(|| format!("{} を置き換えられませんでした", wav.display()))?;
            write_atomic(&kp, serde_json::to_string(&key)?).with_context(|| format!("{} を書けませんでした", kp.display()))?;
        }
        let rev = std::fs::metadata(&wav).map(|m| mtime_ms(&m)).unwrap_or_else(|_| now_ms());
        self.guide_audio_set(gen, Some(rev), None, None);
        Ok(GuideAudio::Ready(wav))
    }

    /// 素材の音を裏で作り直す（組み立て・素材が変わっていなければ何もしない）。できたら `guide` イベントで知らせ、
    /// 作れなければ通知を出す（同じ理由は続けて出さない）
    pub(super) fn guide_audio_kick(&self) {
        let (has_plan, gen) = {
            let rt = self.guide_rt.read();
            (rt.plan.is_some(), rt.gen)
        };
        if !has_plan {
            // 組み立てられなくなったら、前の wav を鳴らし続けない
            self.guide_audio_set(gen, None, None, None);
            return;
        }
        let Some(core) = self.me.get().and_then(|w| w.upgrade()) else { return };
        let seq = self.guide_audio_seq.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
        std::thread::spawn(move || {
            std::thread::sleep(AUDIO_DEBOUNCE);
            if core.guide_audio_seq.load(std::sync::atomic::Ordering::SeqCst) != seq {
                return; // もっと新しい依頼がある
            }
            let gen = core.guide_rt.read().gen;
            match core.guide_audio_ensure() {
                Ok(_) => {}
                Err(e) if is_canceled(&e) => {}
                Err(e) => {
                    // 理由だけを持つ（行・詳細が「素材の音を作れませんでした: 理由」と出す）
                    let (text, _) = crate::app::error::notice_text("", &e);
                    let (same_gen, first) = {
                        let rt = core.guide_rt.read();
                        (rt.gen == gen, rt.audio_err.as_deref() != Some(text.as_str()))
                    };
                    if !same_gen {
                        return; // 回を開き直した（今の回のことではない）
                    }
                    // 古い wav を鳴らし続けないよう、作れなかったら印を外す
                    core.guide_audio_set(gen, None, None, Some(text));
                    if first {
                        core.notice_err("素材の音を作れませんでした", &e);
                    }
                }
            }
        });
    }

    // ------------------------------------------------------------------ 効果音
    /// 今の操作の効果音の指紋（組み立てが無ければ None）
    /// 読み込まれた演出が今の回のものでなければ（回を切り替える間・読み込み中）None。指紋には演出のパスも入る
    pub fn guide_se_sig(&self) -> Option<u64> {
        if self.engine.status.read().state == "loading" {
            return None;
        }
        let desc = self.desc()?;
        let rt = self.guide_rt.read();
        if !timeline_in_root(&desc.path, rt.root.as_ref()?) {
            return None;
        }
        let (g, p) = (rt.guide.as_ref()?, rt.plan.as_ref()?);
        let (cost, game) = rt.analysis.as_ref().map(|a| (a.cost.as_slice(), a.game.as_slice())).unwrap_or((&[], &[]));
        Some(se_signature_for(se_signature(g, &ops_out(g, p, cost, game)), &desc.path))
    }

    /// 攻略動画の回で、書き出しの前に効果音の wav が在ることを確かめる（無ければ Err。黙って効果音なしにしない）。
    /// 演出が無い回（書き出し側が別に断る）・攻略動画でない回は何もしない
    pub fn guide_se_require(&self) -> Result<()> {
        if self.guide_rt.read().guide.is_none() {
            return Ok(());
        }
        let Some(desc) = self.desc() else { return Ok(()) };
        if self.engine.status.read().state == "loading" {
            bail!("演出を読み込んでいる途中です。終わってからもう一度書き出してください");
        }
        let root_ok = self.guide_rt.read().root.as_ref().is_some_and(|r| timeline_in_root(&desc.path, r));
        if !root_ok {
            bail!("演出がこの回のものではありません。回を開き直すか、演出を読み直してください");
        }
        match se_wav_path(&desc.path) {
            Some(p) if p.exists() => Ok(()),
            Some(p) => bail!("効果音の wav（{}）がありません。「効果音を書き出す」で作ってから書き出してください", p.display()),
            None => bail!("効果音の wav の場所を決められませんでした（演出: {}）", desc.path),
        }
    }

    /// 効果音を最後に作ったときの指紋（回を開いて最初に呼ぶときにファイルから読む）
    fn guide_se_built(&self) -> Option<u64> {
        {
            let rt = self.guide_rt.read();
            if rt.se_built_loaded {
                return rt.se_built;
            }
        }
        let built = self
            .root()
            .and_then(|r| std::fs::read_to_string(r.join("studio").join(SE_KEY_FILE)).ok())
            .and_then(|t| serde_json::from_str::<Value>(&t).ok())
            .and_then(|v| v["sig"].as_u64());
        let mut rt = self.guide_rt.write();
        if !rt.se_built_loaded {
            rt.se_built = built;
            rt.se_built_loaded = true;
        }
        rt.se_built
    }

    /// 効果音を指紋 sig で作ったと覚える（ファイルにも）
    pub fn guide_se_record(&self, sig: Option<u64>) {
        let Some(sig) = sig else { return };
        {
            let mut rt = self.guide_rt.write();
            rt.se_built = Some(sig);
            rt.se_built_loaded = true;
        }
        if let Some(r) = self.root() {
            let p = r.join("studio").join(SE_KEY_FILE);
            if let Err(e) = write_atomic(&p, json!({"sig": sig}).to_string()) {
                self.notice_err("効果音を作ったときの操作の記録を書けませんでした", &e.into());
            }
        }
    }

    /// 効果音が今の操作のものでなければ作り直して、画面に知らせる（攻略動画の回だけ。演出が読み込まれていなければ何もしない）。
    /// 作り直したら true。続けて呼ばれても 1 つずつ（先の作り直しが済んでいれば、後のは作らない）
    pub fn guide_se_ensure(self: &Arc<Self>) -> Result<bool> {
        let _one = self.guide_se_lock.lock();
        let Some(sig) = self.guide_se_sig() else { return Ok(false) };
        if self.guide_se_built() == Some(sig) {
            return Ok(false);
        }
        let r = crate::export::audio::export_se(self)?;
        // 作っている間に回・演出が替わっていたら、今のものとして記録しない
        if self.guide_se_sig() != Some(sig) {
            return Ok(false);
        }
        self.guide_se_record(Some(sig));
        // 画面に作り直したことを知らせる（se_bank の通知。画面は rev で効果音の wav を読み直す）
        let bank = crate::sebank::get(self).context("効果音の設定を読めませんでした")?;
        self.emit(
            "se_bank",
            ak_proto::SeBankUpdate { bank, rendered: true, rev: now_ms(), wav: r["wav"].as_str().map(String::from), count: r["count"].as_u64().map(|n| n as u32), warnings: crate::sebank::warnings_of(&r) },
        );
        Ok(true)
    }

    /// 操作が変わっていたら、0.8 秒待ってから効果音を裏で作り直す（続けて変わったら最後の 1 回）。攻略動画の回だけ
    pub(super) fn guide_se_kick(&self) {
        let Some(sig) = self.guide_se_sig() else { return };
        if self.guide_se_built() == Some(sig) {
            return;
        }
        let Some(core) = self.me.get().and_then(|w| w.upgrade()) else { return };
        let seq = self.guide_se_seq.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
        std::thread::spawn(move || {
            std::thread::sleep(SE_DEBOUNCE);
            if core.guide_se_seq.load(std::sync::atomic::Ordering::SeqCst) != seq {
                return;
            }
            match core.guide_se_ensure() {
                Ok(_) => {}
                Err(e) if is_canceled(&e) => {}
                Err(e) => core.notice_err("効果音を作り直せませんでした", &e),
            }
        });
    }
}
