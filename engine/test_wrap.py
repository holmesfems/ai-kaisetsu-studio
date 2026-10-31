"""字幕の自動の折り返しの禁則（akfx_worker.wrap_text）。python engine/test_wrap.py で確かめる。"""
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import akfx_worker as w  # noqa: E402

tw = len  # 1 文字 = 幅 1


def check(text, width, want):
    got = w.wrap_text(text, tw, width)
    assert got == want, f"{text!r} width={width}: {got} != {want}"


# 句読点は行頭に置かず、前の行にぶら下げる（an389 の 45 秒の字幕と同じ形）
check("高台から削る。基本の型", 6, ["高台から削る。", "基本の型"])
# 閉じ括弧が続いても全部ぶら下げる
check("あいう。」えお", 3, ["あいう。」", "えお"])
# 小さい仮名・長音も行頭に置かない
check("アークライト", 3, ["アーク", "ライト"])
check("ちょっと", 1, ["ちょっ", "と"])
# 開き括弧は行末に置かず、次の行へ送る
check("あいう「えお」", 4, ["あいう", "「えお」"])
# 禁則が無ければ今までどおり
check("あいうえおかきく", 3, ["あいう", "えおか", "きく"])
check("", 5, [""])
# テスト用の字幕の見た目（本物は Rust が全項目を入れて送る。足りない項目はエラー）
SUB_STYLE = {
    "font": "C:/Windows/Fonts/NotoSansJP-VF.ttf", "weight": 900, "size": 54, "color": "#ffffff",
    "stroke": "#1e50c8", "stroke_w": 6, "shadow": 0, "shadow_color": "#000000", "shadow_x": 4, "shadow_y": 4,
    "shadow_blur": 6, "y": 1060, "line_h": 70, "margin_l": 210, "margin_r": 210, "align": "center", "wrap": True,
}
# 中央揃えでも、ぶら下げた句読点ははみ出しに数えない（sub_layout の判定。PIL があるときだけ）
try:
    st = dict(SUB_STYLE, margin_l=210, margin_r=210)
    _, f, out, (left, right) = w.sub_layout("先鋒で稼いで、重装で止めて、回復で支えて、高台から削る。基本の型に沿った人選ですね", st, False, SUB_STYLE["font"])
    assert out[0]["text"].endswith("。"), out[0]["text"]
    assert not any(o["over"] for o in out), [(o["text"], o["x"], o["w"]) for o in out]
    # 足りない項目は既定で補わずエラーにする
    try:
        w.sub_layout("あ", {"size": 54}, False, SUB_STYLE["font"])
        raise AssertionError("足りない項目がエラーにならない")
    except w.SeError as e:
        assert "font" in str(e), e
except ImportError:
    pass
print("ok")
