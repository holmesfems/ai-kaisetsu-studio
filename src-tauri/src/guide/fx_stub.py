"""攻略動画の演出（入口）。決まった演出は fxkit の guide_template.py が描く。
回ごとの演出（次の操作のマスの印など）は build() の中に足す。座標は素材の画素で g.at(x, y, "manual") など。"""
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
try:
    from guide_template import *   # noqa: E402,F401
except ImportError:
    # 演出の kit に guide_template が無い（公開版）。決まった演出は描かず、空のタイムラインで動く（アプリが警告を出す）
    from fxcore import *   # noqa: E402,F401

    class GuideFx:
        def __init__(self, root):
            pass

        def timeline(self):
            return Timeline.blank()

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))


def build():
    g = GuideFx(ROOT)          # studio/guide_map.json を読む
    tl = g.timeline()          # 編成・手動・自動・リザルトの決まった演出
    # 例: tl.add(Sprite(chip_img("ここに置く"), *g.at(1500, 900, "manual"), g.op_t("op3") - 2, g.op_t("op3")))
    return tl
