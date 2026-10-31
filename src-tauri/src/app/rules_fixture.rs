//! 画面（ui/src/logic/*.ts）に残してある Rust の決まりの写しを、見本の表 tests/fixtures/ui_rules.json で縛る。
//! Rust の関数で表を作り（`AK_UPDATE_FIXTURES=1`）、画面側は ui/test/rules.test.ts が同じ表を読んで比べる。
//! 式を変えたときは AK_UPDATE_FIXTURES=1 で作り直し、npm test も通す。

use crate::core::{clean_overrides, merge_override, window_box};
use crate::mediaops::{fit_scale, free, insert_track, new_nest_track, new_track};
use crate::project::{MediaClip, Nest, Track, Trans};
use crate::render::cache::group_span;
use crate::render::engine::Group;
use crate::render::media::{AUDIO_EXTS, IMAGE_EXTS, VIDEO_EXTS};
use crate::render::trans::{trans_dur, DEFAULT_DUR};
use serde_json::{json, Value};

fn fixture(name: &str) -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(name)
}

fn group(t0: f64, t1: f64) -> Group {
    Group {
        key: "k".into(),
        label: String::new(),
        func: None,
        line: 0,
        src: String::new(),
        t0,
        t1,
        n: 1,
        kinds: vec![],
        z: 0.0,
        fp: String::new(),
        gui: false,
        se: vec![],
        bbox: None,
        rec: vec![],
    }
}

fn group_span_cases() -> Value {
    // (t0, t1, 微調整)
    let cases: Vec<(f64, f64, Value)> = vec![
        (10.0, 14.0, json!({})),
        (10.0, 14.0, json!({"shift": 2.5})),
        (10.0, 14.0, json!({"shift": -3.0})),
        (10.0, 14.0, json!({"end": 1.5})),
        (10.0, 14.0, json!({"end": -1.5})),
        (10.0, 14.0, json!({"end": -10.0})),
        (10.0, 14.0, json!({"shift": 2.0, "end": 1.0})),
        (10.0, 14.0, json!({"shift": -2.0, "end": -4.0})),
        (10.0, 14.0, json!({"hidden": true})),
        (10.0, 14.0, json!({"hidden": false, "shift": 1.0})),
        (10.0, 14.0, json!({"shift": "x"})),
        (10.0, 14.0, json!({"end": "x", "shift": 1.0})),
        (10.0, 14.0, json!({"shift": null, "end": null})),
        (10.0, 10.0, json!({})),
    ];
    Value::Array(
        cases
            .into_iter()
            .map(|(t0, t1, ov)| {
                let (a, b, hidden) = group_span(&group(t0, t1), &json!({ "k": ov.clone() }));
                json!({"t0": t0, "t1": t1, "ov": ov, "a": a, "b": b, "hidden": hidden})
            })
            .collect(),
    )
}

fn nest_map_cases() -> Value {
    // (w, h, fit, 窓)
    let wide = [600.0, 100.0, 1880.0, 820.0]; // 1280x720
    let tall = [100.0, 100.0, 500.0, 900.0]; // 400x800
    let mut v = vec![];
    for fit in ["contain", "stretch", "cover"] {
        // キャンバスが窓より横長・縦長・同じ比
        for (w, h) in [(1920.0, 1080.0), (2560.0, 800.0), (800.0, 2400.0), (1280.0, 720.0)] {
            for bx in [wide, tall] {
                v.push((w, h, fit, bx));
            }
        }
    }
    v.push((0.0, 1080.0, "contain", wide));
    v.push((1920.0, 0.0, "contain", wide));
    v.push((0.0, 0.0, "stretch", wide));
    v.push((0.0, 0.0, "cover", tall));
    v.push((1920.0, 1080.0, "zoom", wide));
    v.push((800.0, 2400.0, "", tall));
    Value::Array(
        v.into_iter()
            .map(|(w, h, fit, bx)| {
                let n = Nest { w, h, fit: fit.into(), ..Default::default() };
                let (sx, sy, ox, oy) = n.map(bx);
                json!({"w": w, "h": h, "fit": fit, "box": bx, "sx": sx, "sy": sy, "ox": ox, "oy": oy})
            })
            .collect(),
    )
}

fn window_box_cases() -> Value {
    let base = [600.0, 100.0, 1880.0, 820.0];
    let other = [100.0, 50.0, 500.0, 450.0];
    let cases: Vec<([f64; 4], Value)> = vec![
        (base, json!({})),
        (base, json!({"dx": 40.0, "dy": -25.5})),
        (base, json!({"scale": 0.5})),
        (base, json!({"scale": 1.5, "dx": 10.0})),
        (base, json!({"scale": 2.0, "pivot": [600.0, 100.0]})),
        (base, json!({"scale": 0.5, "pivot": [1880.0, 820.0], "dx": -30.0, "dy": 12.0})),
        (base, json!({"scale": 2.0, "pivot": [700.0]})),
        (base, json!({"scale": 2.0, "pivot": ["a", 100.0]})),
        (base, json!({"scale": 2.0, "pivot": [100.0, null]})),
        (base, json!({"scale": 2.0, "pivot": "x"})),
        (base, json!({"scale": "big", "dx": "1", "dy": null})),
        (other, json!({"scale": 1.2, "pivot": [0.0, 0.0], "dx": 5.0})),
    ];
    Value::Array(
        cases
            .into_iter()
            .map(|(b, ov)| json!({"base": b, "ov": ov.clone(), "box": window_box(b, Some(&ov))}))
            .collect(),
    )
}

fn trans_dur_cases() -> Value {
    let t = |kind: &str, dur: f64| Trans { kind: kind.into(), dur, dir: String::new() };
    let cases: Vec<(Option<Trans>, f64)> = vec![
        (None, 5.0),
        (Some(t("", 0.5)), 5.0),
        (Some(t("none", 0.5)), 5.0),
        (Some(t("fade", 0.0)), 5.0),
        (Some(t("fade", -1.0)), 5.0),
        (Some(t("fade", 0.6)), 0.4),
        (Some(t("fade", 0.0)), 0.2),
        (Some(t("slide", 0.5)), -3.0),
        (Some(t("wipe", 0.5)), 0.0),
        (Some(t("glitch", 0.5)), 5.0),
        (Some(t("pop", 0.35)), 0.35),
    ];
    let cases: Vec<Value> = cases
        .into_iter()
        .map(|(tr, clip_dur)| {
            // 無し（null）は Trans::default() と同じ扱い（kind が空）
            let dur = trans_dur(&tr.clone().unwrap_or_default(), clip_dur);
            json!({"tr": tr, "clip_dur": clip_dur, "dur": dur})
        })
        .collect();
    json!({"default_dur": DEFAULT_DUR, "cases": cases})
}

fn clean_overrides_cases() -> Value {
    let cases: Vec<Value> = vec![
        json!({"k": {"shift": 0.0, "end": 0.0, "dx": 0.0, "dy": 0.0, "scale": 1.0, "opacity": 1.0, "hidden": false}}),
        json!({"k": {"pivot": [10.0, 20.0]}}),
        json!({"k": {"note": "メモ"}}),
        json!({"k": {"pivot": [10.0, 20.0], "note": "メモ"}}),
        json!({"k": {"note": null, "z": null}}),
        json!({"k": {"shift": 1e-7, "dx": -1e-7}}),
        json!({"k": {"shift": 2e-6}}),
        json!({"k": {"scale": 1.0000001, "opacity": 0.9999999}}),
        json!({"k": {"scale": 1.01}}),
        json!({"k": {"hidden": false}}),
        json!({"k": {"hidden": true}}),
        json!({"k": {"shift": "x"}}),
        json!({"k": {"shift": null}}),
        json!({"k": {"scale": "1"}}),
        json!({"k": 3.0, "j": "abc", "l": null, "m": [1.0], "n": true}),
        json!({"k": {"z": 2.0}}),
        json!({"k": {"unknown": 5}}),
        json!({"a": {"shift": 1.5}, "b": {"shift": 0.0}, "c": {"end": -0.5, "scale": 1.0}}),
        json!({}),
    ];
    Value::Array(cases.into_iter().map(|i| json!({"in": i.clone(), "out": clean_overrides(i)})).collect())
}

fn merge_override_cases() -> Value {
    // (今の微調整, 鍵, patch)
    let cases: Vec<(Value, &str, Value)> = vec![
        (json!({}), "k", json!({"shift": 1.5})),
        (json!({"k": {"shift": 1.5}}), "k", json!({"dx": 10.0})),
        (json!({"k": {"shift": 1.5, "dx": 2.0}}), "k", json!({"shift": 3.0})),
        (json!({"k": {"shift": 1.5, "dx": 2.0}}), "k", json!({"shift": null})),
        (json!({"k": {"shift": 1.5}, "j": {"end": 1.0}}), "k", json!(null)),
        (json!({"k": {"shift": 1.5}, "j": {"end": 1.0}}), "k", json!({"dy": 4.0})),
        (json!({"k": {"shift": 1.5}}), "k", json!(5)),
        (json!({"k": {"shift": 1.5}}), "k", json!("x")),
        (json!({"k": {"scale": 1.2}}), "k", json!({"scale": 1.0})),
        (json!({"k": 3}), "k", json!({"note": "メモ"})),
        (json!({"k": {"note": "メモ"}}), "k", json!({"note": null})),
    ];
    Value::Array(
        cases
            .into_iter()
            .map(|(ov, key, patch)| {
                let out = clean_overrides(merge_override(&ov, key, &patch));
                json!({"ov": ov, "key": key, "patch": patch, "out": out})
            })
            .collect(),
    )
}

fn tracks_cases() -> Value {
    let main: Vec<Value> = ["video", "image", "audio"]
        .iter()
        .flat_map(|&kind| (1..=3).map(move |n| (kind, n)))
        .map(|(kind, n)| {
            let t = new_track(kind, "x".into(), n);
            json!({"kind": kind, "n": n, "name": t.name, "volume_db": t.volume_db, "track_kind": t.kind})
        })
        .collect();
    let nest: Vec<Value> = (1..=2)
        .map(|n| {
            let t = new_nest_track("x".into(), n);
            json!({"n": n, "name": t.name, "kind": t.kind})
        })
        .collect();

    // 並びの kind から作ったトラックに、新しいトラックを入れた位置
    let mk = |k: &str, i: usize| Track { id: format!("t{i}"), kind: k.into(), ..Default::default() };
    let insert: Vec<Value> = [
        (vec!["video", "fx", "audio"], "image", false),
        (vec!["video", "fx", "audio"], "audio", true),
        (vec!["video", "audio"], "image", false),
        (vec![], "video", false),
        (vec!["fx"], "image", false),
        (vec!["video", "fx", "fx", "audio"], "video", false),
        (vec!["video", "fx", "audio"], "video", true),
        (vec!["video", "fx"], "audio", true),
    ]
    .into_iter()
    .map(|(kinds, new, top)| {
        let mut tracks: Vec<Track> = kinds.iter().enumerate().map(|(i, k)| mk(k, i)).collect();
        let at = insert_track(&mut tracks, mk(new, 99), top);
        json!({"kinds": kinds, "new": new, "top": top, "at": at})
    })
    .collect();

    let clip = |s: f64, d: f64| MediaClip { id: format!("c{s}"), start: s, dur: d, ..Default::default() };
    let free_cases: Vec<Value> = [
        (vec![[0.0, 2.0], [5.0, 1.0]], 2.0, 5.0),
        (vec![[0.0, 2.0]], 2.0, 4.0),
        (vec![[0.0, 2.0]], 2.0 - 1e-7, 4.0),
        (vec![[0.0, 2.0]], 1.0, 3.0),
        (vec![[1.0, 2.0]], 0.0, 1.0),
        (vec![[1.0, 2.0]], 0.0, 1.0 + 1e-7),
        (vec![[1.0, 2.0]], 0.0, 1.5),
        (vec![[1.0, 2.0]], 3.0, 4.0),
        (vec![[1.0, 2.0]], 0.0, 10.0),
        (vec![], 0.0, 1.0),
    ]
    .into_iter()
    .map(|(clips, a, b)| {
        let tr = Track { clips: clips.iter().map(|c| clip(c[0], c[1])).collect(), ..Default::default() };
        json!({"clips": clips, "a": a, "b": b, "free": free(&tr, a, b)})
    })
    .collect();

    let fit: Vec<Value> = [
        (1920.0, 1080.0, 1920.0, 1080.0),
        (1920.0, 1080.0, 3840.0, 1080.0),
        (1920.0, 1080.0, 1080.0, 1920.0),
        (1920.0, 1080.0, 640.0, 480.0),
        (800.0, 600.0, 1000.0, 1000.0),
        (1920.0, 1080.0, 1234.0, 777.0),
        (1920.0, 1080.0, 0.0, 100.0),
        (1920.0, 1080.0, 100.0, 0.0),
    ]
    .into_iter()
    .map(|(cw, ch, w, h)| json!({"cw": cw, "ch": ch, "w": w, "h": h, "scale": fit_scale(cw, ch, w, h)}))
    .collect();

    json!({"main": main, "nest": nest, "insert": insert, "free": free_cases, "fit_scale": fit})
}

fn media_exts() -> Value {
    json!({"image": IMAGE_EXTS, "video": VIDEO_EXTS, "audio": AUDIO_EXTS})
}

/// 見本と一字一句同じか（AK_UPDATE_FIXTURES=1 で作り直す）。ui/test/rules.test.ts も同じ見本を読む
#[test]
fn rules_fixture() {
    let v = json!({
        "group_span": group_span_cases(),
        "nest_map": nest_map_cases(),
        "nest_default": serde_json::to_value(Nest::default()).unwrap(),
        "window_box": window_box_cases(),
        "trans_dur": trans_dur_cases(),
        "clean_overrides": clean_overrides_cases(),
        "tracks": tracks_cases(),
        "media_exts": media_exts(),
        "merge_override": merge_override_cases(),
    });
    let text = serde_json::to_string_pretty(&v).unwrap();
    let path = fixture("ui_rules.json");
    if std::env::var("AK_UPDATE_FIXTURES").is_ok() {
        std::fs::write(&path, &text).unwrap();
    }
    let read = std::fs::read_to_string(&path).unwrap_or_default().replace("\r\n", "\n");
    assert_eq!(read, text, "ui_rules.json が今の式と違います（式を変えたなら AK_UPDATE_FIXTURES=1 で作り直し、npm test も通す）");
}
