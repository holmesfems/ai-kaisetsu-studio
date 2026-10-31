"""rec_info（desc の rec）が RecWindow の入り・抜け（fade）を返す。python engine/test_rec_info.py で確かめる。
fxfreeze で止めた古い kit の RecWindow は ENTER・ENTER_EASE・OUT_EASE を持たないので、当時の値が補われる。"""
import os
import sys
import types

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import akfx_worker as w  # noqa: E402


def group(*els):
    return types.SimpleNamespace(els=list(els))


def rec(cls_attrs, out):
    """RecWindow 風の要素（segs・box・ph を持つもの）"""
    e = types.SimpleNamespace(segs=[(10.0, "r1", "a.mp4", "説明")], box=(100, 100, 500, 400), ph=True, out=out,
                              _ak_t1=14.0)
    for k, v in cls_attrs.items():
        setattr(e, k, v)
    return e


# 今の公開の土台 fxcore.RecWindowBase（値は RecWindow が持つ。kit の RecWindow はこれを継ぐ）
sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "fxkit"))
import fxcore  # noqa: E402

RW = fxcore.RecWindowBase
assert RW.ENTER == 0.35 and RW.ENTER_EASE == "ease_out"
assert RW.OUT_EASE == "ease_in_out"
now = rec({"ENTER": RW.ENTER, "ENTER_EASE": RW.ENTER_EASE, "OUT_EASE": RW.OUT_EASE}, 0.25)
info = w.rec_info(group(now))
assert info[0]["fade"] == {"in": 0.35, "in_ease": "ease_out", "out": 0.25, "out_ease": "ease_in_out"}, info

# 値を変えた RecWindow はその値を返す（Rust はこれを読む）
custom = rec({"ENTER": 0.5, "ENTER_EASE": "linear", "OUT_EASE": "ease_out"}, 0.4)
assert w.rec_info(group(custom))[0]["fade"] == {"in": 0.5, "in_ease": "linear", "out": 0.4, "out_ease": "ease_out"}

# 古い kit（ENTER・ENTER_EASE・OUT_EASE が無い）は今の値を補う
old = rec({}, 0.25)
assert not hasattr(old, "ENTER") and not hasattr(old, "OUT_EASE")
o = w.rec_info(group(old))[0]
assert o["fade"] == {"in": 0.35, "in_ease": "ease_out", "out": 0.25, "out_ease": "ease_in_out"}, o
assert o["box"] == [100.0, 100.0, 500.0, 400.0] and o["t1"] == 14.0

# RecWindow でない要素は飛ばす
assert w.rec_info(group(types.SimpleNamespace(t0=0))) == []

print("ok")
