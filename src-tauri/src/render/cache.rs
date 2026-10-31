//! 演出のコマのキャッシュ。鍵は「そのコマに出ているクリップの指紋＋微調整」なので、
//! 1 つのクリップを直しても、そのクリップが出ていないコマのキャッシュはそのまま使える。

use crate::render::engine::{Desc, OverlayFrame};
use parking_lot::Mutex;
use serde_json::Value;
use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::sync::Arc;

pub struct CachedOverlay {
    pub w: u32,
    pub h: u32,
    pub rect: Option<[u32; 4]>,
    lz: Vec<u8>,
    raw_len: usize,
}

impl CachedOverlay {
    pub fn from_frame(f: &OverlayFrame) -> Self {
        CachedOverlay { w: f.w, h: f.h, rect: f.rect, lz: lz4_flex::compress(&f.rgba), raw_len: f.rgba.len() }
    }

    pub fn rgba(&self) -> Vec<u8> {
        lz4_flex::decompress(&self.lz, self.raw_len).unwrap_or_default()
    }

    pub fn bytes(&self) -> usize {
        self.lz.len() + 64
    }
}

pub struct OverlayCache {
    map: Mutex<HashMap<u64, (Arc<CachedOverlay>, u64)>>,
    tick: Mutex<u64>,
    bytes: Mutex<usize>,
    pub budget: Mutex<usize>,
    /// 描くのに失敗したコマの鍵（待ち続けないために覚える。鍵は中身の指紋なので、直せば別の鍵になる）
    failed: Mutex<HashSet<u64>>,
}

/// 覚えておく失敗の数の上限（超えたら空にする）
const FAILED_MAX: usize = 10000;

impl OverlayCache {
    pub fn new(budget_mb: usize) -> Self {
        OverlayCache {
            map: Mutex::new(HashMap::new()),
            tick: Mutex::new(0),
            bytes: Mutex::new(0),
            budget: Mutex::new(budget_mb << 20),
            failed: Mutex::new(HashSet::new()),
        }
    }

    /// このコマは描くのに失敗した、と覚える。
    pub fn mark_failed(&self, k: u64) {
        let mut f = self.failed.lock();
        if f.len() >= FAILED_MAX {
            f.clear();
        }
        f.insert(k);
    }

    pub fn is_failed(&self, k: u64) -> bool {
        self.failed.lock().contains(&k)
    }

    pub fn get(&self, k: u64) -> Option<Arc<CachedOverlay>> {
        let mut t = self.tick.lock();
        *t += 1;
        let mut m = self.map.lock();
        if let Some(e) = m.get_mut(&k) {
            e.1 = *t;
            return Some(e.0.clone());
        }
        None
    }

    pub fn contains(&self, k: u64) -> bool {
        self.map.lock().contains_key(&k)
    }

    pub fn put(&self, k: u64, v: Arc<CachedOverlay>) {
        let t = {
            let mut t = self.tick.lock();
            *t += 1;
            *t
        };
        let mut m = self.map.lock();
        let mut b = self.bytes.lock();
        *b += v.bytes();
        if let Some(old) = m.insert(k, (v, t)) {
            *b -= old.0.bytes();
        }
        let budget = *self.budget.lock();
        if *b > budget {
            // 古い方から 2 割捨てる
            let mut ticks: Vec<(u64, u64)> = m.iter().map(|(k, (_, t))| (*t, *k)).collect();
            ticks.sort_unstable();
            let target = budget * 8 / 10;
            for (_, k) in ticks {
                if *b <= target {
                    break;
                }
                if let Some(old) = m.remove(&k) {
                    *b -= old.0.bytes();
                }
            }
        }
    }

    pub fn clear(&self) {
        self.map.lock().clear();
        self.failed.lock().clear();
        *self.bytes.lock() = 0;
    }

    pub fn stats(&self) -> (usize, usize) {
        (self.map.lock().len(), *self.bytes.lock())
    }
}

pub fn ov_num(o: Option<&Value>, k: &str) -> f64 {
    o.and_then(|o| o.get(k)).and_then(|v| v.as_f64()).unwrap_or(0.0)
}

/// クリップの、微調整を入れた後の見えている区間。画面の groupSpan と見本 ui_rules.json で縛っている（app/rules_fixture.rs）。
pub fn group_span(g: &crate::render::engine::Group, ov: &Value) -> (f64, f64, bool) {
    let o = ov.get(&g.key);
    let hidden = o.and_then(|o| o.get("hidden")).and_then(|v| v.as_bool()).unwrap_or(false);
    let sh = ov_num(o, "shift");
    let en = ov_num(o, "end");
    (g.t0 + sh, (g.t1 + sh + en).max(g.t0 + sh + 0.05), hidden)
}

/// 時刻 t に、隠していないクリップが 1 つでもかかっているか（区間は始まりを含み、終わりを含まない）。
pub fn any_group_at(groups: &[crate::render::engine::Group], ov: &Value, t: f64) -> bool {
    groups.iter().any(|g| {
        let (a, b, hid) = group_span(g, ov);
        !hid && t >= a && t < b
    })
}

/// コマの鍵。そのコマに出ているクリップの指紋と微調整だけから作る。
pub fn frame_key(desc: &Desc, ov: &Value, f: i64, scale: f64, placeholder: bool) -> u64 {
    let t = f as f64 / desc.fps;
    let mut h = DefaultHasher::new();
    f.hash(&mut h);
    ((scale * 1000.0).round() as i64).hash(&mut h);
    placeholder.hash(&mut h);
    desc.path.hash(&mut h);
    for g in &desc.groups {
        let (a, b, hidden) = group_span(g, ov);
        if hidden || t < a || t >= b {
            continue;
        }
        g.fp.hash(&mut h);
        if let Some(o) = ov.get(&g.key) {
            o.to_string().hash(&mut h);
        }
    }
    h.finish()
}

/// キャッシュに入っている区間（[t0, t1] の列）。タイムラインの上の「レンダリング済み」の帯に使う。
pub fn cached_ranges(cache: &OverlayCache, desc: &Desc, ov: &Value, scale: f64, placeholder: bool) -> Vec<[f64; 3]> {
    let n = (desc.end * desc.fps).ceil() as i64;
    let mut out: Vec<[f64; 3]> = vec![];
    // 区間ごとに「何も出ていない」コマは描かなくてよいので、空として扱う（緑）
    let mut cur: Option<(i64, i64, f64)> = None; // (開始, 終わり, 状態 1=済 0=空)
    for f in 0..n {
        let t = f as f64 / desc.fps;
        let any = any_group_at(&desc.groups, ov, t);
        let state = if !any {
            0.0
        } else if cache.contains(frame_key(desc, ov, f, scale, placeholder)) {
            1.0
        } else {
            -1.0
        };
        match cur {
            Some((s, _, st)) if st == state => cur = Some((s, f, st)),
            _ => {
                if let Some((s, e, st)) = cur {
                    out.push([s as f64 / desc.fps, (e + 1) as f64 / desc.fps, st]);
                }
                cur = Some((f, f, state));
            }
        }
    }
    if let Some((s, e, st)) = cur {
        out.push([s as f64 / desc.fps, (e + 1) as f64 / desc.fps, st]);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn group(key: &str, t0: f64, t1: f64) -> crate::render::engine::Group {
        serde_json::from_value(json!({
            "key": key, "label": "", "line": 0, "src": "", "t0": t0, "t1": t1, "n": 1, "kinds": [],
            "z": 0.0, "fp": "", "gui": false, "se": [], "bbox": null
        }))
        .unwrap()
    }

    #[test]
    fn any_group_at_edges_hidden_and_shift() {
        let gs = vec![group("a", 1.0, 2.0)];
        let none = json!({});
        assert!(!any_group_at(&gs, &none, 0.99));
        assert!(any_group_at(&gs, &none, 1.0));
        assert!(any_group_at(&gs, &none, 1.99));
        assert!(!any_group_at(&gs, &none, 2.0));
        assert!(!any_group_at(&[], &none, 1.5));
        // 隠した群は出ない
        assert!(!any_group_at(&gs, &json!({"a": {"hidden": true}}), 1.5));
        // ずらした群は、ずらした後の区間で
        let shifted = json!({"a": {"shift": 5.0}});
        assert!(!any_group_at(&gs, &shifted, 1.5));
        assert!(any_group_at(&gs, &shifted, 6.0));
        assert!(!any_group_at(&gs, &shifted, 7.0));
        // 別の群が出ていればよい
        let two = vec![group("a", 1.0, 2.0), group("b", 3.0, 4.0)];
        assert!(any_group_at(&two, &json!({"a": {"hidden": true}}), 3.5));
    }
}
