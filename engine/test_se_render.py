"""効果音の作り直し（se_render）が一時フォルダに作ってから置き換えること（旧#33）。python engine/test_se_render.py で確かめる。"""
import os
import shutil
import sys
import tempfile
import time
import types

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "fxkit"))
import akfx_worker as w  # noqa: E402
import fxse  # noqa: E402

FAIL = {"on": False}


def _render_all(ev, total, out_dir, name, profile, bank):
    """偽の fxse.render_all: 指定のフォルダに全部入りと種類別を書く。FAIL のときは途中で例外"""
    with open(os.path.join(out_dir, f"{name}_se.wav"), "wb") as f:
        f.write(b"new-full")
    sd = os.path.join(out_dir, "se_stems")
    os.makedirs(sd, exist_ok=True)
    with open(os.path.join(sd, f"{name}_se_01.wav"), "wb") as f:
        f.write(b"new-stem")
    if FAIL["on"]:
        raise RuntimeError("途中で失敗")
    return [{"t": 1.0, "kind": "slam"}]


fxse.render_all = _render_all
fxse.build_bank = lambda ctx, **kw: ({}, {})
w._se_events = lambda: []
w._se_fx_dir = lambda fx_dir=None: os.path.dirname(os.path.abspath(__file__))
w.L.tl = types.SimpleNamespace(end=10.0)


def read(p):
    with open(p, "rb") as f:
        return f.read()


d = tempfile.mkdtemp(prefix="aks-serender-")
try:
    out = os.path.join(d, "out")
    # 古い一時フォルダは消え、新しいもの（ほかのプロセスが作業中）は残る
    old = os.path.join(out, ".se_tmp_old")
    fresh = os.path.join(out, ".se_tmp_fresh")
    os.makedirs(old)
    os.makedirs(fresh)
    t = time.time() - 7200
    os.utime(old, (t, t))

    r = w.se_render(out, "ep")
    assert r["count"] == 1 and r["wav"] == os.path.join(out, "ep_se.wav"), r
    assert read(r["wav"]) == b"new-full"
    assert read(os.path.join(out, "se_stems", "ep_se_01.wav")) == b"new-stem"
    left = sorted(n for n in os.listdir(out) if n.startswith(".se_tmp_"))
    assert left == [".se_tmp_fresh"], left

    # 途中で例外が出ても、前の wav はそのまま・自分の一時フォルダは残さない
    with open(os.path.join(out, "ep_se.wav"), "wb") as f:
        f.write(b"previous")
    with open(os.path.join(out, "se_stems", "ep_se_01.wav"), "wb") as f:
        f.write(b"previous-stem")
    FAIL["on"] = True
    try:
        w.se_render(out, "ep")
        raise AssertionError("例外になるはず")
    except RuntimeError as e:
        assert "途中で失敗" in str(e)
    assert read(os.path.join(out, "ep_se.wav")) == b"previous"
    assert read(os.path.join(out, "se_stems", "ep_se_01.wav")) == b"previous-stem"
    assert not os.path.exists(os.path.join(out, f".se_tmp_{os.getpid()}"))

    # 全部入りをほかで開いていて置き換えられないときは、理由の分かる SeError（前の wav のまま）
    FAIL["on"] = False
    real_replace = os.replace

    def deny(a, b):
        raise PermissionError(5, "アクセスが拒否されました")
    os.replace = deny
    try:
        w.se_render(out, "ep")
        raise AssertionError("SeError になるはず")
    except w.SeError as e:
        assert "置き換えられませんでした" in str(e) and "ep_se.wav" in str(e), e
    finally:
        os.replace = real_replace
    assert read(os.path.join(out, "ep_se.wav")) == b"previous"
    assert not os.path.exists(os.path.join(out, f".se_tmp_{os.getpid()}"))
finally:
    shutil.rmtree(d, ignore_errors=True)

print("ok")
