//! 動きの緩急の表（Rust が正）。fxkit.py の `ease_out`・`ease_in_out`・`ease_back` と `EASES` は、
//! 見本 tests/fixtures/ease.json（下のテスト `ease_matches_fixture` が作る）で縛っている（fxkit/test_motion.py が同じ見本を読む）。

pub fn clamp01(x: f64) -> f64 {
    x.clamp(0.0, 1.0)
}

pub fn ease_out(x: f64) -> f64 {
    let x = clamp01(x);
    1.0 - (1.0 - x).powi(3)
}

pub fn ease_in_out(x: f64) -> f64 {
    let x = clamp01(x);
    x * x * (3.0 - 2.0 * x)
}

pub fn ease_back(x: f64, s: f64) -> f64 {
    let x = clamp01(x) - 1.0;
    x * x * ((s + 1.0) * x + s) + 1.0
}

pub fn lerp(a: f64, b: f64, k: f64) -> f64 {
    a + (b - a) * k
}

/// 名前で引ける緩急（fxkit.EASES と同じ名前。RecWindow の入り・抜けが名前で渡される）
pub const NAMES: [&str; 3] = ["linear", "ease_out", "ease_in_out"];

pub fn by_name(name: &str) -> Option<fn(f64) -> f64> {
    match name {
        "linear" => Some(clamp01),
        "ease_out" => Some(ease_out),
        "ease_in_out" => Some(ease_in_out),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::path::Path;

    fn fixture(name: &str) -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(name)
    }

    #[test]
    fn ease_endpoints() {
        for f in [ease_out, ease_in_out] {
            assert_eq!(f(0.0), 0.0);
            assert_eq!(f(1.0), 1.0);
            assert_eq!(f(-1.0), 0.0);
            assert_eq!(f(2.0), 1.0);
        }
        assert_eq!(ease_back(1.0, 2.2), 1.0);
        assert!(by_name("nope").is_none());
        for n in NAMES {
            assert!(by_name(n).is_some());
        }
    }

    /// 見本（ease.json）と一字一句同じ。fxkit/test_motion.py も同じ見本を読む。
    /// 式を変えたときは AK_UPDATE_FIXTURES=1 で作り直し、fxkit 側のテストも通ることを確かめる
    #[test]
    fn ease_matches_fixture() {
        let xs: Vec<f64> = (-2..=22).map(|i| i as f64 * 0.05).collect();
        let col = |f: &dyn Fn(f64) -> f64| xs.iter().map(|&x| f(x)).collect::<Vec<_>>();
        let v = json!({
            "names": NAMES,
            "x": xs,
            "linear": col(&|x| by_name("linear").unwrap()(x)),
            "ease_out": col(&ease_out),
            "ease_in_out": col(&ease_in_out),
            "ease_back_2.2": col(&|x| ease_back(x, 2.2)),
            "ease_back_1.7": col(&|x| ease_back(x, 1.7)),
        });
        let text = serde_json::to_string_pretty(&v).unwrap();
        if std::env::var("AK_UPDATE_FIXTURES").is_ok() {
            std::fs::write(fixture("ease.json"), &text).unwrap();
        }
        let read = std::fs::read_to_string(fixture("ease.json")).unwrap().replace("\r\n", "\n");
        assert_eq!(read, text);
    }
}
