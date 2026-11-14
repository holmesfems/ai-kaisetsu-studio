"""演出のコマを RecWindow のところで層に分ける（#26）と、add(rw) のキー。python engine/test_frame_layers.py で確かめる。"""
import os
import sys
import tempfile
import types

FXKIT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "fxkit")
sys.path.insert(0, FXKIT)  # pip で入れた別の版ではなく、この作業フォルダの fxkit を使う
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import akfx_worker as w  # noqa: E402

fx = w._fxmod()


class Box:
    """矩形を塗るだけの要素。rec=True なら RecWindow 風（segs・box・ph を持つ）。"""

    def __init__(self, key, z, rect, color, rec=False):
        self.z, self.rect, self.color = z, rect, color
        self.t0, self.t1, self.out = 0.0, 10.0, 0.0
        self._akg = types.SimpleNamespace(key=key)
        if rec:
            self.segs, self.box, self.ph = [(0.0, "r1", "a.mp4", "")], (0, 0, 1, 1), False

    def alive(self, t):
        return True

    def draw(self, fr, t):
        x, y, ww, hh = self.rect
        fr.alpha_composite(fx.new(ww, hh), (x, y))
        for yy in range(y, y + hh):
            for xx in range(x, x + ww):
                fr.putpixel((xx, yy), self.color)


def setup(els, ov=None):
    w.L.tl = types.SimpleNamespace(els=els)
    w.L.overrides = ov or {}


RED, GREEN, BLUE = (255, 0, 0, 255), (0, 255, 0, 255), (0, 0, 255, 255)
panel = Box("panel@c1#1", 20, (10, 10, 4, 4), RED)
rec = Box("RecWindow@c1#1", 30, (100, 100, 4, 4), GREEN, rec=True)
telop = Box("telop@c1#1", 40, (200, 200, 4, 4), BLUE)


def has(im, xy):
    return im.getpixel(xy)[3] > 0


# パネル(20)・RecWindow(30)・テロップ(40): 下の層はパネルだけ、層 1 つに RecWindow とテロップ
setup([telop, rec, panel])
base, layers = w.frame_layers(1.0)
assert has(base, (10, 10)) and not has(base, (100, 100)) and not has(base, (200, 200))
assert [k for k, _ in layers] == ["RecWindow@c1#1"]
assert has(layers[0][1], (100, 100)) and has(layers[0][1], (200, 200)) and not has(layers[0][1], (10, 10))
# 1 枚に重ねた絵は、層を分けないときと同じ
flat = w.frame(1.0)
assert flat.getpixel((10, 10)) == RED and flat.getpixel((100, 100)) == GREEN and flat.getpixel((200, 200)) == BLUE

# RecWindow がパネルより下（z=10）: いちばん下の層は空で、層にパネルも入る
rec.z = 10
base, layers = w.frame_layers(1.0)
assert base.getbbox() is None
assert has(layers[0][1], (10, 10)) and has(layers[0][1], (100, 100)) and has(layers[0][1], (200, 200))
rec.z = 30

# 微調整で動かすクリップでも、まとめて描く直前で層を分ける（そのクリップはまとめて 1 回）
setup([telop, rec, panel], {"RecWindow@c1#1": {"dx": 5}})
base, layers = w.frame_layers(1.0)
assert has(base, (10, 10)) and not has(base, (105, 100))
assert [k for k, _ in layers] == ["RecWindow@c1#1"] and has(layers[0][1], (105, 100)) and has(layers[0][1], (200, 200))

# 同じクリップに RecWindow が 2 つあっても、分けるのは 1 回
rec2 = Box("RecWindow@c1#1", 35, (300, 300, 4, 4), GREEN, rec=True)
setup([telop, rec, rec2, panel])
base, layers = w.frame_layers(1.0)
assert len(layers) == 1 and has(layers[0][1], (300, 300))
# 別のクリップの RecWindow は別の層
rec3 = Box("RecWindow@c2#1", 35, (300, 300, 4, 4), GREEN, rec=True)
setup([telop, rec, rec3, panel])
assert [k for k, _ in w.frame_layers(1.0)[1]] == ["RecWindow@c1#1", "RecWindow@c2#1"]

# RecWindow が無いコマには層が無い。絵は今と同じ
setup([telop, panel])
base, layers = w.frame_layers(1.0)
assert layers == [] and has(base, (10, 10)) and has(base, (200, 200))
hdr, data = w.frame_layers_bytes(base, layers)
assert "layers" not in hdr and hdr == w.frame_bytes(base)[0] and data == w.frame_bytes(base)[1]

# solo のときは分けない
setup([telop, rec, panel])
base, layers = w.frame_layers(1.0, solo="RecWindow@c1#1")
assert layers == [] and has(base, (100, 100))

# 返事: ヘッダの layers とデータの並び（いちばん下の層 → 各層）
setup([telop, rec, panel])
base, layers = w.frame_layers(1.0)
hdr, data = w.frame_layers_bytes(base, layers)
assert hdr["layers"] == [{"key": "RecWindow@c1#1", "rect": [100, 100, 104, 104]}], hdr["layers"]
assert hdr["rect"] == [10, 10, 4, 4]
assert len(data) == 4 * 4 * 4 + 104 * 104 * 4, len(data)
# RecWindow の層が空なら rect は None でデータも無い
setup([panel, Box("RecWindow@c1#1", 30, (0, 0, 1, 1), (0, 0, 0, 0), rec=True)])
hdr, data = w.frame_layers_bytes(*w.frame_layers(1.0))
assert hdr["layers"] == [{"key": "RecWindow@c1#1", "rect": None}] and len(data) == 4 * 4 * 4, hdr

# 倍率: 層ごとに同じ縮め方
base, layers = w.frame_layers(1.0, scale=0.5)
assert base.size == (fx.W // 2, fx.H // 2)

# add(rw) のキー: RecWindow@c<n>#1。add(Sprite(...)) などは変わらない
d = tempfile.mkdtemp(prefix="aks-lay-")
open(os.path.join(d, "script_cues.json"), "w", encoding="utf-8").write(
    '{"cues": [{"n": 6, "start": 1.0, "end": 3.0, "text": "こんにちは", "speaker": "トーク1"}]}')
open(os.path.join(d, "t_fx.py"), "w", encoding="utf-8").write(
    "import os\nimport fxcore\n\n\ndef sc_intro(tl):\n"
    "    rw = fxcore.RecWindowBase([(1.0, 'r1', 'a.mp4', 'x')], 3.0, placeholder=False)\n"
    "    tl.add(rw)\n\n\n"
    "def build():\n"
    "    tl = fxcore.Timeline(os.path.join(os.path.dirname(os.path.abspath(__file__)), 'script_cues.json'))\n"
    "    sc_intro(tl)\n"
    "    tl.add(fxcore.RecWindowBase([(1.5, 'r2', 'b.mp4', 'y')], 3.0, placeholder=False))\n"
    "    tl.add(fxcore.El(1.0, 2.0))\n"
    "    return tl\n")
info = w.load(os.path.join(d, "t_fx.py"), None, None, True)
keys = [g["key"] for g in info["groups"]]
assert keys[0] == "RecWindow@c6#1", keys
assert keys[1] == "RecWindowBase@c6#1", keys   # add(部品名(...)) のキーは変わらない
assert keys[2].startswith("El@c6#"), keys
print("ok")
