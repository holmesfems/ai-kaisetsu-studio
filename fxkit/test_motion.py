"""入り・抜けの動きの式が Rust と同じか。python fxkit/test_motion.py で確かめる。
Rust 側（src-tauri/src/render/ease.rs・trans.rs）のテストが作る見本 src-tauri/tests/fixtures/ の
ease.json・trans_curves.json・glitch.json を読んで、緩急・入りの曲線・グリッチの画素を比べる。
式を変えたときは Rust 側で AK_UPDATE_FIXTURES=1 cargo test -p ak-studio して見本を作り直す。"""
import base64
import json
import os
import sys

import numpy as np

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import fxcore  # noqa: E402

FIX = os.path.join(HERE, "..", "src-tauri", "tests", "fixtures")
EPS = 1e-12


def read_json(name):
    with open(os.path.join(FIX, name), encoding="utf-8") as f:
        return json.load(f)


# --- 緩急
ease = read_json("ease.json")
assert set(fxcore.EASES) == set(ease["names"]), (sorted(fxcore.EASES), ease["names"])
for name in ease["names"]:
    for x, want in zip(ease["x"], ease[name]):
        got = fxcore.EASES[name](x)
        assert abs(got - want) <= EPS, (name, x, got, want)
for s in ("2.2", "1.7"):
    for x, want in zip(ease["x"], ease["ease_back_" + s]):
        got = fxcore.ease_back(x, float(s))
        assert abs(got - want) <= EPS, ("ease_back", s, x, got, want)

# --- 入りの曲線（Sprite の名前 left などは slide の行と比べる）
tc = read_json("trans_curves.json")
names = {"fade": ["fade"], "slide": ["up", "down", "left", "right"], "pop": ["pop"], "slam": ["slam"],
         "wipe": ["wipe"], "glitch": ["glitch"]}
assert set(names) == set(tc["curves"]), sorted(tc["curves"])
for kind, rows in tc["curves"].items():
    assert len(rows) == len(tc["ks"])
    for enter in names[kind]:
        for k, want in zip(tc["ks"], rows):
            got = fxcore.enter_curve(enter, k)
            assert set(got) == set(want), (enter, sorted(got))
            for f in ("alpha", "scale", "slide", "glitch"):
                assert abs(got[f] - want[f]) <= EPS, (enter, k, f, got[f], want[f])
            if want["wipe"] is None:
                assert got["wipe"] is None, (enter, k)
            else:
                assert abs(got["wipe"] - want["wipe"]) <= EPS, (enter, k)

# --- グリッチ: 種と画素
g = read_json("glitch.json")
for s in g["seeds"]:
    assert fxcore.glitch_seed(s["id"], s["frame"]) == s["seed"], s
w, h, ch = g["w"], g["h"], g["ch"]
base = (np.arange(w * h * ch) * 7 % 251).astype(np.uint8).reshape(h, w, ch)
for c in g["cases"]:
    want = np.frombuffer(base64.b64decode(c["out"]), dtype=np.uint8).reshape(h, w, ch)
    got = fxcore.glitch_pixels(base.copy(), c["amt"], c["seed"], c["unit"])
    assert got.shape == want.shape and got.dtype == np.uint8
    bad = int((got != want).sum())
    assert bad == 0, (c["amt"], c["unit"], bad, "画素が違います")
assert np.array_equal(base, (np.arange(w * h * ch) * 7 % 251).astype(np.uint8).reshape(h, w, ch))  # 入力は書き換えない

# 丸めは 0 から遠い方（Python の round は偶数へ丸める）
assert [fxcore._round_half_away(x) for x in (0.5, 1.5, 2.5, -0.5, -2.5, 0.49, -0.49)] == [1, 2, 3, -1, -3, 0, 0]

# glitch(im, k, key, fi) は形のまま: 弱ければそのまま、強ければ画素が変わる
from PIL import Image  # noqa: E402

im = Image.fromarray(base)
assert fxcore.glitch(im, 0.999, ("x",), 3) is im
assert fxcore.glitch(im, 0.0, ("x",), 3).size == im.size

print("ok")
