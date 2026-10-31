"""Timeline(CUES, SYNC) の同期点ファイルの場所を拾う（akfx_worker.sync_arg_path）。python engine/test_syncpath.py で確かめる。"""
import os
import sys

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "fxkit"))
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import akfx_worker as w  # noqa: E402

want = os.path.abspath("files/sync_points.json")
# 2 番目の位置引数（srt, sync）
assert w.sync_arg_path(("a.srt", "files/sync_points.json"), {}) == want
# キーワード
assert w.sync_arg_path(("a.srt",), {"sync": "files/sync_points.json"}) == want
# 無い・空・文字列でないときは None
assert w.sync_arg_path(("a.srt",), {}) is None
assert w.sync_arg_path(("a.srt", None), {}) is None
assert w.sync_arg_path(("a.srt", ""), {}) is None
assert w.sync_arg_path(("a.srt", 3), {}) is None

# load() の中の Timeline.__init__ のラップが、実際の fxkit.Timeline から拾って、終わったら元に戻す（fxkit があるときだけ）
try:
    import fxkit  # noqa: F401
except ImportError:
    fxkit = None
if fxkit is not None:
    import tempfile
    d = tempfile.mkdtemp()
    open(os.path.join(d, "script_cues.json"), "w", encoding="utf-8").write(
        '{"cues": [{"n": 1, "start": 0.0, "end": 1.0, "text": "あ", "speaker": "トーク1"}]}')
    # 第 1 引数は script_cues.json でも、互換の .srt（隣の script_cues.json を読む）でも拾える
    for name in ("script_cues.json", "a.srt"):
        open(os.path.join(d, "t_fx.py"), "w", encoding="utf-8").write(
            "from fxkit import *\nCUES = '%s'\n\ndef build():\n    return Timeline(CUES, 'sync_points.json', end=1.0)\n" % name)
        before = fxkit.Timeline.__init__
        try:
            r = w.load(os.path.join(d, "t_fx.py"), placeholder=False)
            assert r["sync_path"] == os.path.join(os.path.realpath(d), "sync_points.json") or \
                os.path.normcase(r["sync_path"]) == os.path.normcase(os.path.abspath(os.path.join(d, "sync_points.json"))), r["sync_path"]
        finally:
            assert fxkit.Timeline.__init__ is before
print("ok")
