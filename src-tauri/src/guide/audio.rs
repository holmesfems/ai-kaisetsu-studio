//! 攻略動画の素材の音（純粋な部分）。
//!
//! 通しの素材（戦闘画面の録画）の音を、映像と同じ「出力 → 素材」の対応（`GuidePlan::audio_segments`）で並べ直した wav
//! （`studio/guide_audio.wav`）にする。読み込み・保存・裏のスレッドは `core/guide_audio.rs`。

use super::plan::AudioSeg;
use ak_proto::guide::OpOut;
use ak_proto::Guide;
use serde_json::{json, Value};

/// 素材の音の wav（回のフォルダの `studio/` から）
pub const AUDIO_FILE: &str = "guide_audio.wav";
/// 作ったときの区間・素材の指紋（比べて、同じなら作り直さない）
pub const AUDIO_KEY_FILE: &str = "guide_audio.json";
/// 効果音を作ったときの操作の指紋
pub const SE_KEY_FILE: &str = "guide_se.json";
/// 標準のサンプルレート（`export::audio::SR` と同じ）
pub const SR: u32 = 48000;
/// `AUDIO_KEY_FILE` の形の版。区間の並べ方を変えたら上げる
const KEY_VERSION: u32 = 2; // 2: 秒をマイクロ秒の整数に

/// 素材の音（48 kHz ステレオ f32、インターリーブ）を区間どおりに並べる。
/// origin は src の先頭のサンプルが素材の何秒にあたるか（音のストリームの start − 映像のストリームの start。普通は 0）。
/// 素材の時刻が音の範囲の外（編成の頭の 0 秒より前など）は無音。total は出力の長さ（秒）
pub fn arrange(src: &[f32], origin: f64, segs: &[AudioSeg], total: f64) -> Vec<f32> {
    let n_out = (total * SR as f64).round().max(0.0) as usize;
    let mut out = vec![0f32; n_out * 2];
    let n_src = (src.len() / 2) as i64;
    for s in segs {
        let p0 = (s.out0 * SR as f64).round() as i64;
        let p1 = ((s.out1 * SR as f64).round() as i64).min(n_out as i64);
        // 出力のサンプル p が読む素材のサンプル = p + shift
        let shift = ((s.src0 - origin - s.out0) * SR as f64).round() as i64;
        let fi = (s.fade_in * SR as f64).round() as i64;
        let fo = (s.fade_out * SR as f64).round() as i64;
        for p in p0.max(0)..p1 {
            let i = p + shift;
            if i < 0 || i >= n_src {
                continue;
            }
            let mut g = 1.0f32;
            if fi > 0 && p - p0 < fi {
                g *= (p - p0) as f32 / fi as f32;
            }
            // 区間の終わりの手前 fo サンプルで直線に下げる（次の区間の fade_in と足して 1）
            let end = (s.out1 * SR as f64).round() as i64;
            if fo > 0 && end - p < fo {
                g *= (end - p) as f32 / fo as f32;
            }
            let (o, i) = (p as usize * 2, i as usize * 2);
            out[o] += src[i] * g;
            out[o + 1] += src[i + 1] * g;
        }
    }
    out
}

/// 出力の区間 [t0, t1) の分だけ切り出す（秒 → サンプルに丸めて、範囲外は切り詰める）
pub fn slice_range(st: &[f32], t0: f64, t1: f64) -> &[f32] {
    let a = ((t0.max(0.0) * SR as f64).round() as usize * 2).min(st.len());
    let b = ((t1.max(0.0) * SR as f64).round() as usize * 2).min(st.len());
    &st[a.min(b)..b]
}

/// 作り直しの判断の鍵: 区間の一覧・素材（パス・大きさ・更新時刻）・音のずれ。同じなら同じ wav になる
pub fn audio_key(segs: &[AudioSeg], src: &str, size: u64, mtime_ms: u64, origin: f64) -> Value {
    // 秒はマイクロ秒の整数で持つ（浮動小数のままだと、JSON に書いて読み直すと最後の桁が変わることがあり、毎回作り直していた）
    let us = |x: f64| (x * 1e6).round() as i64;
    json!({
        "version": KEY_VERSION,
        "src": src,
        "size": size,
        "mtime": mtime_ms,
        "origin": us(origin),
        "segs": segs.iter().map(|s| json!([us(s.out0), us(s.out1), us(s.src0), us(s.fade_in), us(s.fade_out)])).collect::<Vec<_>>(),
    })
}

/// 作り直すか。前の鍵が無い・違う・wav が無いとき true
pub fn needs_rebuild(prev: Option<&Value>, key: &Value, wav_exists: bool) -> bool {
    !(wav_exists && prev == Some(key))
}

/// 効果音に効く操作のデータの指紋（操作ごとの id・種類・精密か・出力の時刻と NOW の開始）。
/// これが変わったときだけ効果音を作り直す（文言・位置の変更では作り直さない）。同じなら必ず同じ値
pub fn se_signature(guide: &Guide, outs: &[OpOut]) -> u64 {
    let ms = |v: Option<f64>| v.map(|x| (x * 1000.0).round() as i64);
    let ops: Vec<Value> = guide
        .ops
        .iter()
        .zip(outs)
        .map(|(o, u)| json!([o.id, o.kind, o.precise, ms(u.t_out), ms(u.t_now), u.in_cut]))
        .collect();
    super::undo::fingerprint(&Value::Array(ops).to_string())
}

/// 区切り・大文字小文字を揃えた比べ用のパス（Windows は大文字小文字を区別しない）
fn norm_path(p: &str) -> String {
    let s = p.replace('\\', "/");
    let s = s.trim_end_matches('/');
    if cfg!(windows) { s.to_lowercase() } else { s.to_string() }
}

/// 演出のタイムライン（`Desc::path`）が回のフォルダ root の下にあるか。
/// 回を切り替える間、前の回の演出が残っていないかの見分けに使う
pub fn timeline_in_root(timeline: &str, root: &std::path::Path) -> bool {
    let (t, r) = (norm_path(timeline), norm_path(&root.to_string_lossy()));
    !r.is_empty() && t.starts_with(&format!("{r}/"))
}

/// 効果音の指紋に、効果音を作った演出のタイムラインのパスを混ぜる（別の回・別の演出の記録と取り違えない）
pub fn se_signature_for(sig: u64, timeline: &str) -> u64 {
    super::undo::fingerprint(&format!("{sig}|{}", norm_path(timeline)))
}

/// 演出のタイムラインから、効果音の全部入りの wav（`out/<名前>_se.wav`。`export_se` が書く）
pub fn se_wav_path(timeline: &str) -> Option<std::path::PathBuf> {
    let tl = std::path::Path::new(timeline);
    let stem = tl.file_stem()?.to_string_lossy().to_string();
    Some(tl.parent()?.join("out").join(format!("{stem}_se.wav")))
}

/// ffprobe の `-show_entries stream=codec_type,start_time -of csv=p=0` の出力から、映像・音の最初のストリームの start（秒）。
/// 読めない行（N/A など）は None
pub fn parse_stream_starts(text: &str) -> (Option<f64>, Option<f64>) {
    let (mut v, mut a) = (None, None);
    let mut seen_v = false;
    let mut seen_a = false;
    for line in text.lines() {
        let mut it = line.trim().split(',');
        let (Some(kind), start) = (it.next(), it.next()) else { continue };
        let start = start.and_then(|s| s.trim().parse::<f64>().ok()).filter(|x| x.is_finite());
        match kind.trim() {
            "video" if !seen_v => {
                seen_v = true;
                v = start;
            }
            "audio" if !seen_a => {
                seen_a = true;
                a = start;
            }
            _ => {}
        }
    }
    (v, a)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seg(out0: f64, out1: f64, src0: f64, fade_in: f64, fade_out: f64) -> AudioSeg {
        AudioSeg { out0, out1, src0, fade_in, fade_out }
    }

    /// 素材: 1 秒ごとに値が 1,2,3… の（左右同じ）合成の音。値でどの素材の時刻が出力に来たか分かる
    fn ramp_src(secs: usize) -> Vec<f32> {
        let mut v = Vec::with_capacity(secs * SR as usize * 2);
        for s in 0..secs {
            for _ in 0..SR as usize {
                v.push((s + 1) as f32 * 0.1);
                v.push((s + 1) as f32 * 0.1);
            }
        }
        v
    }

    fn at(st: &[f32], t: f64) -> f32 {
        st[(t * SR as f64).round() as usize * 2]
    }

    #[test]
    fn arrange_places_segments_and_silences_before_zero() {
        let src = ramp_src(10);
        // 出力 0〜2 秒に素材の −1〜1 秒、2〜4 秒に素材の 5〜7 秒
        let out = arrange(&src, 0.0, &[seg(0.0, 2.0, -1.0, 0.0, 0.0), seg(2.0, 4.0, 5.0, 0.0, 0.0)], 4.0);
        assert_eq!(out.len(), 4 * SR as usize * 2);
        assert_eq!(at(&out, 0.5), 0.0, "素材の 0 秒より前は無音");
        assert!((at(&out, 1.5) - 0.1).abs() < 1e-6, "素材の 0.5 秒");
        assert!((at(&out, 2.5) - 0.6).abs() < 1e-6, "素材の 5.5 秒");
        assert!((at(&out, 3.5) - 0.7).abs() < 1e-6, "素材の 6.5 秒");
    }

    /// カットのクロスフェード: 重なりの所で A は直線に下がり B は直線に上がる（足すと元の値）
    #[test]
    fn arrange_crossfades_linearly_in_overlap() {
        // A は素材の 1〜3 秒（値 0.2 → 0.3）を出力の 0〜2 秒に（終わりの 1 秒がフェードアウト）、
        // B は素材の 6〜8 秒を出力の 1〜3 秒に（頭の 1 秒がフェードイン）
        let src: Vec<f32> = vec![1.0; 10 * SR as usize * 2];
        let out = arrange(&src, 0.0, &[seg(0.0, 2.0, 1.0, 0.0, 1.0), seg(1.0, 3.0, 6.0, 1.0, 0.0)], 3.0);
        // 重なりの外は 1.0。重なりの中は A + B = 1.0（直線）
        for &t in &[0.2, 0.9, 2.2, 2.8] {
            assert!((at(&out, t) - 1.0).abs() < 1e-3, "t={t} {}", at(&out, t));
        }
        for &t in &[1.0, 1.25, 1.5, 1.75, 1.99] {
            assert!((at(&out, t) - 1.0).abs() < 1e-3, "重なりの中 t={t} {}", at(&out, t));
        }
        // A と B で別の値にすると、重なりの真ん中は半分ずつ
        let mut src = vec![1.0f32; 5 * SR as usize * 2];
        for v in src[2 * SR as usize * 2..].iter_mut() {
            *v = 3.0;
        }
        // A: 素材 0〜2 秒（値 1.0）→ 出力 0〜2 秒、B: 素材 2〜4 秒（値 3.0）→ 出力 1〜3 秒
        let out = arrange(&src, 0.0, &[seg(0.0, 2.0, 0.0, 0.0, 1.0), seg(1.0, 3.0, 2.0, 1.0, 0.0)], 3.0);
        assert!((at(&out, 0.5) - 1.0).abs() < 1e-3);
        assert!((at(&out, 1.0) - 1.0).abs() < 1e-3, "重なりの頭は A だけ");
        assert!((at(&out, 1.5) - 2.0).abs() < 1e-3, "真ん中は A 0.5 + B 1.5");
        assert!((at(&out, 2.5) - 3.0).abs() < 1e-3);
    }

    #[test]
    fn arrange_applies_stream_offset() {
        let src = ramp_src(10);
        // 音が映像より 2 秒遅れて始まる（origin 2.0）: 素材の 3.5 秒の音は src の 1.5 秒の所
        let out = arrange(&src, 2.0, &[seg(0.0, 1.0, 3.0, 0.0, 0.0)], 1.0);
        assert!((at(&out, 0.5) - 0.2).abs() < 1e-6, "{}", at(&out, 0.5));
        // 素材の 1 秒の音は、音が始まる前なので無音
        let out = arrange(&src, 2.0, &[seg(0.0, 1.0, 1.0, 0.0, 0.0)], 1.0);
        assert_eq!(at(&out, 0.5), 0.0);
    }

    #[test]
    fn arrange_clips_to_total_and_source_end() {
        let src = ramp_src(2);
        let out = arrange(&src, 0.0, &[seg(0.0, 10.0, 0.0, 0.0, 0.0)], 3.0);
        assert_eq!(out.len(), 3 * SR as usize * 2);
        assert!(at(&out, 1.5) > 0.0);
        assert_eq!(out[2 * SR as usize * 2 + 10], 0.0, "素材の終わりより先は無音");
    }

    #[test]
    fn slice_range_clamps() {
        let st = vec![1.0f32; 2 * SR as usize * 2];
        assert_eq!(slice_range(&st, 0.5, 1.0).len(), SR as usize);
        assert_eq!(slice_range(&st, 1.5, 9.0).len(), SR as usize);
        assert!(slice_range(&st, 5.0, 9.0).is_empty());
        assert!(slice_range(&st, 1.0, 0.5).is_empty());
    }

    #[test]
    fn rebuild_only_when_key_or_wav_changes() {
        let a = vec![seg(0.0, 8.0, -4.0, 0.0, 0.0), seg(8.0, 20.0, 7.2, 0.0, 0.0)];
        let key = audio_key(&a, "QS.MP4", 100, 5, 0.0);
        // 同じ区間・素材で wav もある → 作らない
        assert!(!needs_rebuild(Some(&key), &audio_key(&a, "QS.MP4", 100, 5, 0.0), true));
        // wav が無い・前の鍵が無い → 作る
        assert!(needs_rebuild(Some(&key), &key, false));
        assert!(needs_rebuild(None, &key, true));
        // 区間・素材（更新時刻・大きさ・パス）・音のずれが変われば作る
        let mut b = a.clone();
        b[1].src0 = 7.3;
        assert!(needs_rebuild(Some(&key), &audio_key(&b, "QS.MP4", 100, 5, 0.0), true));
        assert!(needs_rebuild(Some(&key), &audio_key(&a, "QS.MP4", 100, 6, 0.0), true));
        assert!(needs_rebuild(Some(&key), &audio_key(&a, "QS.MP4", 101, 5, 0.0), true));
        assert!(needs_rebuild(Some(&key), &audio_key(&a, "B.MP4", 100, 5, 0.0), true));
        assert!(needs_rebuild(Some(&key), &audio_key(&a, "QS.MP4", 100, 5, 0.02), true));
        // 保存して読み直した鍵とも同じ（浮動小数が丸まらない）
        let back: Value = serde_json::from_str(&key.to_string()).unwrap();
        assert!(!needs_rebuild(Some(&back), &key, true));
        // 実際の回の値（最後の桁まである秒）でも、読み直した鍵と同じ
        let r = vec![seg(0.0, 3.0, 0.9750000000000001, 0.0, 0.0), seg(34.75, 168.08333333333334, 49.2083, 0.25, 0.0),
                     seg(3.0, 35.0, 7.2082999999999995, 0.0, 0.25)];
        let k = audio_key(&r, "QSMF5499.MP4", 585501951, 1792562920000, 0.0);
        let back: Value = serde_json::from_str(&serde_json::to_string(&k).unwrap()).unwrap();
        assert!(!needs_rebuild(Some(&back), &k, true));
    }

    #[test]
    fn timeline_in_root_checks_prefix_and_separators() {
        let root = std::path::Path::new("/tmp/an389");
        assert!(timeline_in_root("\\tmp\\an389\\fx\\an389_fx.py", root));
        assert!(timeline_in_root("/tmp/an389/fx/a_fx.py", root));
        // 別の回（名前が前方一致するだけ）・前の回は違う
        assert!(!timeline_in_root("/tmp/an3890/fx/a_fx.py", root));
        assert!(!timeline_in_root("/tmp/an388/fx/a_fx.py", root));
        assert!(!timeline_in_root("/tmp/an389/fx/a_fx.py", std::path::Path::new("")));
    }

    #[test]
    fn se_signature_for_follows_timeline() {
        let a = se_signature_for(7, "E:/x/fx/a_fx.py");
        assert_eq!(a, se_signature_for(7, "E:\\x\\fx\\a_fx.py"));
        assert_ne!(a, se_signature_for(7, "E:/y/fx/a_fx.py"));
        assert_ne!(a, se_signature_for(8, "E:/x/fx/a_fx.py"));
    }

    #[test]
    fn se_wav_path_is_out_stem_se() {
        let p = se_wav_path("E:/x/fx/guide_fx.py").unwrap();
        assert_eq!(p, std::path::Path::new("E:/x/fx").join("out").join("guide_fx_se.wav"));
        assert!(se_wav_path("").is_none());
    }

    #[test]
    fn se_signature_ignores_text_but_follows_time_kind_precise() {
        use ak_proto::GuideOp;
        let mut g = Guide::default();
        g.ops = vec![GuideOp { id: "op1".into(), t: 20.0, kind: "deploy".into(), op: "A".into(), ..Default::default() }];
        let out = |t: f64| vec![OpOut { id: "op1".into(), t_out: Some(t), t_now: Some(t - 1.0), ..Default::default() }];
        let base = se_signature(&g, &out(30.0));
        assert_eq!(base, se_signature(&g, &out(30.0)));
        // 文言（キャラ名・置き場所・補足）では変わらない
        let mut g2 = g.clone();
        g2.ops[0].op = "B".into();
        g2.ops[0].r#where = "R1C1".into();
        g2.ops[0].info = "補足".into();
        assert_eq!(base, se_signature(&g2, &out(30.0)));
        // 時刻・種類・精密は変わる（1 ms 未満の揺れは変えない）
        assert_ne!(base, se_signature(&g, &out(30.5)));
        assert_eq!(base, se_signature(&g, &out(30.0001)));
        let mut g3 = g.clone();
        g3.ops[0].kind = "skill".into();
        assert_ne!(base, se_signature(&g3, &out(30.0)));
        let mut g4 = g.clone();
        g4.ops[0].precise = true;
        assert_ne!(base, se_signature(&g4, &out(30.0)));
        // 操作を足す・消す
        let mut g5 = g.clone();
        g5.ops.push(GuideOp { id: "op2".into(), ..Default::default() });
        let mut o5 = out(30.0);
        o5.push(OpOut { id: "op2".into(), t_out: None, ..Default::default() });
        assert_ne!(base, se_signature(&g5, &o5));
    }

    #[test]
    fn stream_starts_from_ffprobe_csv() {
        assert_eq!(parse_stream_starts("video,0.000000\naudio,0.000000\n"), (Some(0.0), Some(0.0)));
        assert_eq!(parse_stream_starts("audio,0.023220\nvideo,0.033333\n"), (Some(0.033333), Some(0.02322)));
        // 読めない行・無いストリーム
        assert_eq!(parse_stream_starts("video,N/A\n"), (None, None));
        assert_eq!(parse_stream_starts(""), (None, None));
        // 最初のストリームだけを見る
        assert_eq!(parse_stream_starts("audio,0.5\naudio,0.9\n").1, Some(0.5));
    }
}
