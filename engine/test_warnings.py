"""ワーカーの警告（describe / subtitle の warnings）。python engine/test_warnings.py で確かめる。"""
import os
import sys
import types

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import akfx_worker as w  # noqa: E402

# add_warning: 同じ key は 1 回だけ
ws = []
w.add_warning(ws, "a", "一つ目")
w.add_warning(ws, "a", "二つ目")
w.add_warning(ws, "b", "三つ目")
assert ws == [{"key": "a", "message": "一つ目"}, {"key": "b", "message": "三つ目"}], ws

# se_of: 効果音の付け方を調べられないときは [] にして、理由を警告に足す（効果音の当て方は kit の se_collect）
def _boom(_els):
    raise RuntimeError("bank.json が壊れています")


w._kit = types.SimpleNamespace(se_collect=_boom)
ws = []
assert w.se_of([], ws) == []
assert w.se_of([], ws) == []  # 2 回目でも警告は 1 件
assert [x["key"] for x in ws] == ["se"], ws
assert "bank.json が壊れています" in ws[0]["message"], ws
assert w.se_of([]) == []  # warnings を渡さなくても落ちない
w._kit_reset()

# gui_clips: 作成できない演出は除いて、id ごとに警告を足す（芯と kit は最小の偽物）
fxkit = types.ModuleType("fxkit")
fxkit.STAGE_CX = 960


class _Sprite:
    def __init__(self, *a, **k):
        self.t0, self.t1 = a[3], a[4]


fxkit.Sprite = _Sprite
sys.modules["fxkit"] = fxkit
w._kit = types.SimpleNamespace(
    studio_clip=lambda kind, t0, t1, params: [_Sprite(None, 0, 0, t0, t1)], COLORS={})
tl = types.SimpleNamespace(els=[], cues={})
clips = [
    {"id": "ok1", "type": "note", "start": 1.0, "end": 2.0, "params": {"text": "※"}},
    {"id": "img1", "type": "image", "start": 1.0, "end": 2.0, "params": {"path": "Z:/無い/画像.png"}},
    {"id": "off", "type": "image", "disabled": True, "params": {}},
    {"id": "new1", "type": "未来の種類", "start": 3.0, "params": {}},
]
ws = []
gs = w.gui_clips(tl, clips, ws)
assert [g.key for g in gs] == ["gui:ok1"], [g.key for g in gs]
assert [x["key"] for x in ws] == ["gui:img1", "gui:new1"], ws
assert "img1（image・1.0 秒）" in ws[0]["message"], ws
assert "未来の種類" in ws[1]["message"], ws
del sys.modules["fxkit"]
w._kit_reset()

# 字幕のフォント: 見つからないフォントは Rust が送る代わりのフォントで描いて、その依頼の返事に警告を付ける（代わりのフォントがあるときだけ）
SUB_STYLE = {
    "font": "C:/Windows/Fonts/NotoSansJP-VF.ttf", "weight": 900, "size": 54, "color": "#ffffff",
    "stroke": "#1e50c8", "stroke_w": 6, "shadow": 0, "shadow_color": "#000000", "shadow_x": 4, "shadow_y": 4,
    "shadow_blur": 6, "y": 1060, "line_h": 70, "margin_l": 210, "margin_r": 210, "align": "center", "wrap": True,
}
FB = SUB_STYLE["font"]
if os.path.exists(FB):
    missing = "Z:/無い/フォント.ttf"
    w._req_fonts.clear()
    w.sub_layout("こんにちは", dict(SUB_STYLE, font=missing), False, FB)
    fw = w.font_warnings()
    assert [x["key"] for x in fw] == [f"font:{missing}"], fw
    # 2 回目（フォントは覚えている）でも、使えば警告を返す
    w._req_fonts.clear()
    w.sub_layout("こんにちは", dict(SUB_STYLE, font=missing), False, FB)
    assert [x["key"] for x in w.font_warnings()] == [f"font:{missing}"]
    # 使っていない依頼には付けない
    w._req_fonts.clear()
    w.sub_layout("こんにちは", SUB_STYLE, False, FB)
    assert w.font_warnings() == [], w.font_warnings()
print("ok")
