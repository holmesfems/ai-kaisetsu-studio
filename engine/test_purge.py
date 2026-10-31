"""ワーカーの読み直し（_purge_if_changed）。python engine/test_purge.py で確かめる。
ワーカーは回をまたいで使い回すので、前の回の fxkit を使い続けないこと・fxfreeze で置いた回のコピーに切り替わること。"""
import os
import sys
import tempfile
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import akfx_worker as w  # noqa: E402

# 本物の fxkit（pip で入れた版）が見つからないように、偽物だけを探させる
for k in [k for k in sys.modules if k == "fxkit"]:
    del sys.modules[k]
sys.path[:] = [p for p in sys.path if not os.path.isfile(os.path.join(p, "fxkit.py"))]


def fake(d, tag):
    os.makedirs(d, exist_ok=True)
    with open(os.path.join(d, "fxkit.py"), "w", encoding="utf-8") as f:
        f.write(f"TAG = {tag!r}\n")


def load_like_worker(fx_dir):
    """load() と同じ順: 回の fx/ を先頭に入れてから読み直しを判断し、fxkit を import する"""
    if fx_dir not in sys.path:
        sys.path.insert(0, fx_dir)
    w._purge_if_changed(fx_dir)
    import fxkit
    return fxkit.TAG


root = tempfile.mkdtemp()
shared = os.path.join(root, "shared")  # pip で入れた版の代わり（sys.path の後ろ）
fake(shared, "shared")
sys.path.append(shared)
ep_a = os.path.abspath(os.path.join(root, "a", "fx"))  # コピーを持つ回
ep_b = os.path.abspath(os.path.join(root, "b", "fx"))  # コピーを持たない回
fake(ep_a, "a")
os.makedirs(ep_b)

# 回 A は自分のコピー
assert load_like_worker(ep_a) == "a"
# 回 B に移ると、A のコピーではなく共通の版（A の fx/ は探す先から外れる）
assert load_like_worker(ep_b) == "shared"
assert ep_a not in sys.path
# 共通の版を直すと読み直す
time.sleep(0.05)
with open(os.path.join(shared, "fxkit.py"), "w", encoding="utf-8") as f:
    f.write("TAG = 'shared2'\n")
os.utime(os.path.join(shared, "fxkit.py"), (time.time() + 5, time.time() + 5))
assert load_like_worker(ep_b) == "shared2"
# 回 B に fxkit を置く（fxfreeze）と、そちらに切り替わる
fake(ep_b, "b")
assert load_like_worker(ep_b) == "b"
# 回 A に戻ると A のコピー
assert load_like_worker(ep_a) == "a"
# 何も変わっていなければ読み直さない
assert w._purge_if_changed(ep_a) is False


# 演出の kit の入口（ak_kit）も同じ: 増えたとき・直したとき・回に置いたときに読み直す（芯の fxcore も同じ判断）
def fake_kit(d, tag):
    with open(os.path.join(d, "ak_kit.py"), "w", encoding="utf-8") as f:
        f.write(f"TAG = {tag!r}\n")


def kit_tag(fx_dir):
    w._purge_if_changed(fx_dir)
    import ak_kit
    return ak_kit.TAG


fake_kit(shared, "k1")
assert w._purge_if_changed(ep_a) is True            # 共通の場所に kit が増えた
assert kit_tag(ep_a) == "k1"
assert w._purge_if_changed(ep_a) is False
fake_kit(shared, "k2")
os.utime(os.path.join(shared, "ak_kit.py"), (time.time() + 10, time.time() + 10))
assert kit_tag(ep_a) == "k2"                        # 直した
fake_kit(ep_a, "k-ep")
assert kit_tag(ep_a) == "k-ep"                      # 回に固定した
assert w._purge_if_changed(ep_a) is False
os.remove(os.path.join(ep_a, "ak_kit.py"))
assert kit_tag(ep_a) == "k2"                        # 消すと共通の版に戻る

print("ok")
