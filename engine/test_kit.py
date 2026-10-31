"""演出の kit の約束（版 1）とワーカーのつなぎ方（旧#168）。python engine/test_kit.py で確かめる。
素の kit（ak_kit_basic）で、シーン札・画面から足す 4 種類の演出・効果音の割り当てが動くこと。
kit が無い・版が合わない・KIT_API が無い・入口が欠けている・読み込めないときの警告と、素の kit での描画。
素の kit と芯は fxparts・fxkit（見た目）を読まないこと。"""
import os
import shutil
import subprocess
import sys
import tempfile
import types

HERE = os.path.dirname(os.path.abspath(__file__))
FXKIT = os.path.join(os.path.dirname(HERE), "fxkit")
sys.path.insert(0, HERE)
import akfx_worker as w  # noqa: E402

KIT_MODS = ("ak_kit", "ak_kit_basic", "fxcore", "fxse", "fxkit", "fxparts", "fxsound")


class scenario:
    """公開の芯と素の kit だけが見える sys.path にする（extra があれば ak_kit.py として置く）。入るとき・出るとき kit を読み直す"""

    def __init__(self, ak_kit_src=None):
        self.src = ak_kit_src

    def __enter__(self):
        self.dir = tempfile.mkdtemp(prefix="aks-kit-")
        for f in ("fxcore.py", "fxse.py", "ak_kit_basic.py"):
            shutil.copy(os.path.join(FXKIT, f), self.dir)
        if self.src is not None:
            with open(os.path.join(self.dir, "ak_kit.py"), "w", encoding="utf-8") as f:
                f.write(self.src)
        self.saved_path = sys.path[:]
        self.saved_mods = {k: sys.modules.pop(k) for k in KIT_MODS if k in sys.modules}
        sys.path[:] = [self.dir] + [p for p in self.saved_path
                                    if not any(os.path.isfile(os.path.join(p or ".", f))
                                               for f in ("ak_kit.py", "fxkit.py", "fxcore.py"))]
        w._kit_reset()
        return self

    def __exit__(self, *exc):
        for k in KIT_MODS:
            sys.modules.pop(k, None)
        sys.modules.update(self.saved_mods)
        sys.path[:] = self.saved_path
        w._kit_reset()
        shutil.rmtree(self.dir, ignore_errors=True)


def keys(kit):
    return [k for k, _, _ in kit.warnings]


def render(els, t):
    fx = w._fxmod()
    fr = fx.new(fx.W, fx.H)
    for e in sorted((e for e in els if e.alive(t)), key=lambda e: e.z):
        e.draw(fr, t)
    return fr


# ---- 素の kit は芯だけを読む（fxparts・fxkit を読まない）
r = subprocess.run([sys.executable, "-c",
                    "import sys; sys.path.insert(0, %r); import ak_kit_basic; "
                    "bad = [m for m in ('fxparts', 'fxkit', 'fxsound', 'ak_kit') if m in sys.modules]; "
                    "print('BAD=' + ','.join(bad))" % FXKIT],
                   capture_output=True, text=True, encoding="utf-8")
assert r.returncode == 0, r.stderr
assert r.stdout.strip().endswith("BAD="), r.stdout

# ---- kit が無い（公開版のふつうの状態）: 素の kit。警告 kit-missing は情報（level: info）
with scenario(None):
    kit = w.get_kit()
    assert keys(kit) == ["kit-missing"], kit.warnings
    assert kit.warnings[0][2] == "info" and "演出の kit がありません" in kit.warnings[0][1], kit.warnings
    assert (kit.name, kit.api, kit.features) == ("basic", 1, set()), (kit.name, kit.api, kit.features)
    assert kit.SE_DIR is None and kit.se_collect([]) == [] and kit.se_labels() == {}
    # w._fxmod() は fxkit が無ければ fxcore。describe の kit・warnings に出る
    assert w._fxmod().__name__ == "fxcore"
    w.L.tl = types.SimpleNamespace(cues={}, sync={}, sync_used=[], end=1.0)
    w.L.groups, w.L.warnings = [], []
    d = w.describe()
    assert d["kit"]["name"] == "basic" and d["kit"]["api"] == 1 and d["kit"]["features"] == [], d["kit"]
    assert os.path.basename(d["kit"]["path"]) == "ak_kit_basic.py", d["kit"]
    assert d["warnings"] == [{"key": "kit-missing", "message": "演出の kit がありません。素の見た目で描いています",
                              "level": "info"}], d["warnings"]
    assert (d["w"], d["h"]) == (1920, 1080) and len(d["stage"]) == 4 and len(d["rec_box"]) == 4

    # シーン札（ワーカーの依頼）
    hdr, data = w.scene_label_render("57-2")
    assert hdr["rect"] is not None and len(data) == hdr["rect"][2] * hdr["rect"][3] * 4
    assert w.scene_label_render("57-3")[1] != data

    # 画面から足す 4 種類と image が素の kit で動く（描ける）
    clips = [
        {"id": "a", "type": "telop", "start": 1.0, "end": 3.0, "params": {"text": "テロップ", "sub": "小さい", "color": "#ff0000"}},
        {"id": "b", "type": "note", "start": 1.0, "end": 3.0, "params": {"text": "※注釈"}},
        {"id": "c", "type": "stamp", "start": 1.0, "end": 3.0, "params": {"text": "NG", "color": "red"}},
        {"id": "d", "type": "chips", "start": 1.0, "end": 3.0, "params": {"words": ["攻撃", "防御"], "tag": "KEY"}},
        {"id": "e", "type": "chips", "start": 1.0, "end": 3.0, "params": {"words": []}},
    ]
    tl = types.SimpleNamespace(els=[], cues={})
    ws = []
    gs = w.gui_clips(tl, clips, ws)
    assert [g.key for g in gs] == ["gui:a", "gui:b", "gui:c", "gui:d", "gui:e"] and ws == [], (gs, ws)
    for g in gs[:4]:
        assert g.els, g.key
        for t in (1.05, 1.5, 3.1):
            assert render(g.els, t).getbbox() is not None, (g.key, t)
    # 色の指定: 名前・#rrggbb・[r, g, b]。素の kit の表は白（ice）・赤（red）
    assert w._color("RED", None) == (255, 0, 0) and w._color("#0a0b0c", None) == (10, 11, 12)
    assert w._color([1, 2, 3, 4], None) == (1, 2, 3) and w._color("無い名前", "既定") == "既定"
    # 画面から足す演出の引数の既定は公開側（種類と引数）で決める
    p = w._studio_params("stamp", {})
    assert p == {"text": "OK", "cx": 1500, "cy": 300, "color": None, "size": 64, "angle": -7}, p

    # 録画の窓は土台のまま。ワーカーは segs・box・ph で見分ける
    import ak_kit_basic
    import fxcore
    assert issubclass(ak_kit_basic.RecWindow, fxcore.RecWindowBase)
    rw = ak_kit_basic.RecWindow([(1.0, "01", "a.mp4", "説明"), (3.0, "02", "b.mp4", "別")], 5.0, placeholder=True)
    assert w._is_rec(rw)
    for t in (1.1, 2.0, 3.1, 4.0, 5.1):
        assert render([rw], t).getbbox() is not None, t
    rw._ak_t1 = 5.0
    info = w.rec_info(types.SimpleNamespace(els=[rw]))
    assert info[0]["fade"] == {"in": 0.35, "in_ease": "ease_out", "out": 0.25, "out_ease": "ease_in_out"}, info

# ---- 版が合わない kit: 素の kit で描き、kit-api を出す
with scenario("KIT_API = 2\nNAME = 'future'\n"):
    kit = w.get_kit()
    assert keys(kit) == ["kit-api"], kit.warnings
    assert kit.warnings[0][1] == ("演出の kit の版が合いません（kit: 2、アプリ: 1）。kit かアプリを更新してください。"
                                  "素の見た目で描いています"), kit.warnings
    assert kit.warnings[0][2] is None
    assert (kit.name, kit.api) == ("basic", 1)
    assert w.scene_label_render("1")[0]["rect"] is not None

# ---- KIT_API が無い kit
with scenario("NAME = 'old'\n"):
    kit = w.get_kit()
    assert keys(kit) == ["kit-api"] and "kit: なし" in kit.warnings[0][1], kit.warnings
    assert kit.name == "basic"

# ---- 入口が欠けた kit: その入口だけ素の kit。kit-entry:<名前> を欠けた分だけ出す
PARTIAL = """
KIT_API = 1
NAME = 'partial'
FEATURES = {'guide_scan'}
SE_DIR = 'X:/se'


def scene_label(sid):
    from fxcore import new
    return new(11, 7)


def se_labels():
    return {'slam': 'ハンコ'}
"""
with scenario(PARTIAL):
    kit = w.get_kit()
    assert keys(kit) == ["kit-entry:studio_clip", "kit-entry:RecWindow", "kit-entry:se_collect"], kit.warnings
    assert kit.warnings[0][1] == "演出の kit に studio_clip がありません。素の見た目で描いています"
    assert (kit.name, kit.api, kit.features, kit.SE_DIR) == ("partial", 1, {"guide_scan"}, "X:/se")
    # kit にある入口は kit のもの、無い入口は素の kit のもの
    assert kit.scene_label("1").size == (11, 7)
    assert kit.se_labels() == {"slam": "ハンコ"}
    label = w._se_ctx(os.path.dirname(kit.path))[0].label     # 効果音の設定の画面の名前（知らない種類は種類の名前）
    assert label("slam") == "ハンコ" and label("zz") == "zz"
    assert kit.se_collect([]) == []
    els = kit.studio_clip("note", 1.0, 2.0, {"text": "x"})
    assert els and els[0].t0 == 1.0
    # describe の warnings に、欠けた入口の分が出る（level なし）
    w.L.tl = types.SimpleNamespace(cues={}, sync={}, sync_used=[], end=1.0)
    w.L.groups, w.L.warnings = [], []
    d = w.describe()
    assert [x["key"] for x in d["warnings"]] == keys(kit), d["warnings"]
    assert all("level" not in x for x in d["warnings"])
    assert d["kit"]["name"] == "partial" and d["kit"]["features"] == ["guide_scan"]

# ---- 読み込めない kit（中で import が失敗）: 素の kit で描き、kit-load を出す（黙らない）
with scenario("import no_such_module_xyz\nKIT_API = 1\n"):
    kit = w.get_kit()
    assert keys(kit) == ["kit-load"] and "no_such_module_xyz" in kit.warnings[0][1], kit.warnings
    assert kit.name == "basic"
with scenario("raise RuntimeError('壊れた kit')\n"):
    kit = w.get_kit()
    assert keys(kit) == ["kit-load"] and "壊れた kit" in kit.warnings[0][1], kit.warnings

# ---- 全部そろった kit は警告なし
FULL = PARTIAL.replace("def se_labels", "def studio_clip(kind, t0, t1, params):\n    return []\n\n\n"
                       "RecWindow = type('RecWindow', (), {})\n\n\n"
                       "def se_collect(els):\n    return [1]\n\n\ndef se_labels")
with scenario(FULL):
    kit = w.get_kit()
    assert kit.warnings == [], kit.warnings
    assert kit.se_collect([]) == [1] and w.se_of([]) == [1]
    # 色の名前の表（COLORS）は任意。無ければ素の kit の表で、警告は出ない
    assert w._color("cyan", None) == (0, 255, 255)

# ---- 素の kit が読めなくても（回に固定した古い fxcore と合わないなど）、そろった kit だけで描く。kit-basic を出す
with scenario(FULL) as sc:
    with open(os.path.join(sc.dir, "ak_kit_basic.py"), "w", encoding="utf-8") as f:
        f.write("from fxcore import no_such_name_xyz\n")
    kit = w.get_kit()
    assert keys(kit) == ["kit-basic"] and "no_such_name_xyz" in kit.warnings[0][1], kit.warnings
    assert kit.name == "partial" and kit.se_collect([]) == [1]
# どちらも読めなければ、黙らずに失敗する
with scenario() as sc:
    with open(os.path.join(sc.dir, "ak_kit_basic.py"), "w", encoding="utf-8") as f:
        f.write("raise RuntimeError('壊れた素の kit')\n")
    try:
        w.get_kit()
        raise AssertionError("失敗するはず")
    except RuntimeError as e:
        assert "素の kit" in str(e) and "壊れた素の kit" in str(e), e

# ---- 初めての load より前に kit を決めていたら（シーン札が先に来た）、初めての load で決め直す
saved_seen = dict(w._seen_found)
w._seen_found.clear()
with scenario():
    w.get_kit()
    assert w._kit is not None
    fx_dir = tempfile.mkdtemp(prefix="aks-fx-")
    try:
        assert w._purge_if_changed(fx_dir) is True and w._kit is None
    finally:
        shutil.rmtree(fx_dir, ignore_errors=True)
w._seen_found.clear()
w._seen_found.update(saved_seen)

# ---- add_warning の level
ws = []
w.add_warning(ws, "a", "x", "info")
w.add_warning(ws, "b", "y")
assert ws == [{"key": "a", "message": "x", "level": "info"}, {"key": "b", "message": "y"}], ws

print("ok")
