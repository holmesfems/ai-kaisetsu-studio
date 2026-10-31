"""調整（overrides）の入れ替え。読めない値なら何も変えずに ValueError（旧#141）。python engine/test_overrides.py で確かめる。"""
import os
import sys
import types

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import akfx_worker as w  # noqa: E402


def el(t0, t1, z):
    return types.SimpleNamespace(t0=t0, t1=t1, _ak_t1=t1, z=z)


def group(key, *els):
    g = w.Group(0, 1, "build", "", key)
    g.key = key
    g.els = list(els)
    return g


a, b = el(1.0, 3.0, 0), el(2.0, 4.0, 5)
w.L.groups = [group("telop@c1#1", a), group("telop@c2#1", b)]

# 正しい値: end は t1 を、z は z を変える
w.set_overrides({"telop@c1#1": {"end": 0.5, "z": 9}})
assert (a.t1, a.z, b.t1, b.z) == (3.5, 9.0, 4.0, 5), (a.t1, a.z, b.t1, b.z)
good = w.L.overrides

# 読めない値: 2 つ目のクリップで失敗しても、1 つ目も含めて何も変えない
for bad, words in [
    ({"telop@c1#1": {"end": 1.0}, "telop@c2#1": {"z": "前"}}, ["telop@c2#1", "z"]),
    ({"telop@c1#1": {"end": "abc"}}, ["telop@c1#1", "end"]),
    ({"telop@c1#1": {"end": float("nan")}}, ["end"]),
    ({"telop@c1#1": [1, 2]}, ["telop@c1#1"]),
    ([1], ["形"]),
]:
    try:
        w.set_overrides(bad)
    except ValueError as e:
        assert all(x in str(e) for x in words), (bad, str(e))
    else:
        raise AssertionError(f"ValueError になるはず: {bad}")
    assert w.L.overrides is good, bad
    assert (a.t1, a.z, b.t1, b.z) == (3.5, 9.0, 4.0, 5), (bad, a.t1, a.z, b.t1, b.z)

# frame・効果音が読む項目も確かめる（読めないと全部のコマが描けなくなるので、受け付けない）
for f in ["shift", "dx", "dy", "scale", "opacity"]:
    try:
        w.set_overrides({"telop@c1#1": {f: "abc"}})
    except ValueError as e:
        assert f in str(e), str(e)
    else:
        raise AssertionError(f"{f} の読めない値は ValueError になるはず")
    assert w.L.overrides is good, f

# 前と同じく受け付ける値: 空の end・shift・scale（既定とみなす）、z の 0、数の文字列、無いクリップのキー
w.set_overrides({"telop@c1#1": {"end": "", "shift": False, "scale": 0, "z": 0, "opacity": None},
                 "telop@c2#1": {"end": "1.5"}, "telop@c9#9": {"end": "何でも"}})
assert (a.t1, a.z, b.t1, b.z) == (3.0, 0.0, 5.5, 5), (a.t1, a.z, b.t1, b.z)

# 空にすると元に戻る
w.set_overrides({})
assert (a.t1, a.z, b.t1, b.z) == (3.0, 0, 4.0, 5), (a.t1, a.z, b.t1, b.z)
print("OK")
