"""fxkit を探す順（回の fx/ のコピー → pip で入れた版 → AK_FXKIT_FALLBACK）。python engine/test_fxkit_path.py で確かめる。
子プロセスで -I を使わずに sys.path を組む（pip で入れた版は PYTHONPATH で真似る。-S で実際に入っている fxkit は見えなくする）"""
import os
import subprocess
import sys
import tempfile

HERE = os.path.dirname(os.path.abspath(__file__))
tmp = tempfile.mkdtemp()


def mk(name, mark):
    d = os.path.join(tmp, name)
    os.makedirs(d)
    open(os.path.join(d, "fxkit.py"), "w", encoding="utf-8").write(f"MARK = {mark!r}\n")
    return d


ep, pip, fb = mk("ep_fx", "episode"), mk("pip", "pip"), mk("fallback", "fallback")
empty = os.path.join(tmp, "empty_fx")
os.makedirs(empty)

# ワーカーを読み込んだ後、load() と同じく回の fx/ を sys.path の先頭に入れてから import fxkit する
CHILD = """
import sys
sys.path.insert(0, %r)
import akfx_worker
sys.path.insert(0, sys.argv[1])
import fxkit
print('MARK=' + fxkit.MARK, file=sys.stderr)  # akfx_worker が stdout を stderr に回すので
"""


def which(fx_dir, with_pip, with_fb):
    env = {k: v for k, v in os.environ.items() if k not in ("PYTHONPATH", "AK_FXKIT_FALLBACK")}
    if with_pip:
        env["PYTHONPATH"] = pip
    if with_fb:
        env["AK_FXKIT_FALLBACK"] = fb
    r = subprocess.run([sys.executable, "-S", "-c", CHILD % HERE, fx_dir], env=env, capture_output=True, text=True)
    assert r.returncode == 0, r.stderr
    return [ln for ln in r.stderr.splitlines() if ln.startswith('MARK=')][-1][5:]


# 回の fx/ にコピーがあればそれ（pip の版・埋め込みの版があっても）
assert which(ep, True, True) == "episode"
# 回に無ければ、pip で入れた版が埋め込みの版より先
assert which(empty, True, True) == "pip"
# どちらも無ければ AK_FXKIT_FALLBACK
assert which(empty, False, True) == "fallback"

# 環境変数が無ければ sys.path は変わらない。あれば末尾に 1 回だけ足す
sys.path.insert(0, HERE)
import akfx_worker as w  # noqa: E402

before = sys.path[:]
assert w.add_fxkit_fallback({}) is None and sys.path == before
assert w.add_fxkit_fallback({"AK_FXKIT_FALLBACK": fb}) == fb
assert sys.path[-1] == fb
w.add_fxkit_fallback({"AK_FXKIT_FALLBACK": fb})
assert sys.path.count(fb) == 1
sys.path.remove(fb)

print("test_fxkit_path: ok")
