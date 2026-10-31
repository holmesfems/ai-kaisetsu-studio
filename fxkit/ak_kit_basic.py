"""
素の kit（公開）。AK Studio が演出の kit（ak_kit.py）を見つけられないときに使う、地味な見た目の kit。

kit の約束（版 1）を満たす最小の実装で、fxcore（芯）だけを使う。見た目は白い文字・黒い帯・素の枠。
効果音は付けない（se_collect は []）。ワーカーは `import ak_kit`、無ければ `ak_kit_basic` を使う。
自分の見た目の kit を作るときは、この名前（KIT_API・NAME・scene_label・studio_clip・RecWindow・se_collect・
se_labels・SE_DIR・FEATURES）を持つ ak_kit.py を書く（足りない入口だけ、ワーカーがこちらで補う）。
"""

import functools

from PIL import ImageDraw

from fxcore import NOTE_XY, STAGE_CX, RecWindowBase, Sprite, new, rgba, text_img

KIT_API = 1
NAME = "basic"
FEATURES = set()
SE_DIR = None

# 画面から足す演出の色の名前（params の color・accent）。名前は演出の kit によらず使えるよう、素の色で答える
COLORS = {
    "cyan": (0, 255, 255), "blue": (0, 0, 255), "ice": (255, 255, 255), "dim": (160, 160, 160),
    "amber": (255, 191, 0), "gold": (255, 215, 0), "red": (255, 0, 0), "green": (0, 200, 0),
}

WHITE = (255, 255, 255)
RED = (255, 0, 0)


@functools.lru_cache(maxsize=1024)
def _scene_label(sid):
    t = text_img(f"SCENE {sid}", 30, WHITE, 600, kind="mono", shadow=0)
    p = t.info["pad"]
    w, h = t.width - 2 * p + 28, t.height - 2 * p + 14
    im = new(w, h)
    ImageDraw.Draw(im).rectangle((0, 0, w - 1, h - 1), fill=rgba((0, 0, 0), 0.72))
    im.alpha_composite(t, (14 - p, 7 - p))
    return im


def scene_label(sid):
    """シーン番号の札（「SCENE 57-2」）の絵。RGBA。白い文字に黒の帯。"""
    return _scene_label(str(sid))


def _place(im, x, y, anchor="lt"):
    """text_img の絵を anchor（l/c/r と t/m/b）の基準で置いたときの左上（pad 込み）"""
    p = im.info.get("pad", 0)
    w, h = im.width - 2 * p, im.height - 2 * p
    ax = {"l": 0, "c": w / 2, "r": w}[anchor[0]]
    ay = {"t": 0, "m": h / 2, "b": h}[anchor[1]]
    return x - ax - p, y - ay - p


def _text_sprite(im, x, y, t0, t1, anchor="lt", **kw):
    px, py = _place(im, x, y, anchor)
    return Sprite(im, px, py, t0, t1, **kw)


def _band(w, h, color=(0, 0, 0), a=0.7, outline=None, width=3):
    im = new(w, h)
    ImageDraw.Draw(im).rectangle((0, 0, w - 1, h - 1), fill=rgba(color, a),
                                 outline=rgba(outline, 1.0) if outline else None, width=width)
    return im


def studio_clip(kind, t0, t1, params):
    """画面から足す簡単な演出（telop・note・stamp・chips）の絵。El のリストを返す。
    params はワーカーが既定を入れて渡す（color・accent は色の指定が無ければ None）。"""
    p = params
    if kind == "telop":
        col = p.get("color") or WHITE
        main = text_img(p.get("text", ""), p.get("size", 56), col, p.get("weight", 900), shadow=0)
        sub = text_img(p["sub"], 32, WHITE, 700, shadow=0) if p.get("sub") else None
        pad = main.info["pad"]
        mw, mh = main.width - 2 * pad, main.height - 2 * pad
        sw, sh = (sub.width - 2 * pad, sub.height - 2 * pad) if sub else (0, 0)
        w, h = int(max(mw, sw) + 96), int(mh + 48 + (sh + 12 if sub else 0))
        cx, cy = p.get("cx", STAGE_CX), p.get("cy", 450)
        x, y = cx - w / 2, cy - h / 2
        els = [Sprite(_band(w, h, a=0.75), x, y, t0, t1, enter="fade", dur=0.3, z=20),
               _text_sprite(main, cx, y + 24, t0, t1, anchor="ct", enter="fade", dur=0.3, z=21)]
        if sub:
            els.append(_text_sprite(sub, cx, y + 24 + mh + 12, t0, t1, anchor="ct", enter="fade", dur=0.3, z=21))
        return els
    if kind == "note":
        im = text_img(p.get("text", "※"), 24, WHITE, 600, a=0.95)
        return [_text_sprite(im, NOTE_XY[0], NOTE_XY[1], t0, t1, anchor="rb", enter="fade", dur=0.3, z=30)]
    if kind == "stamp":
        col = p.get("color") or RED
        t = text_img(p.get("text", "OK"), p.get("size", 64), col, 900, shadow=0)
        pad = t.info["pad"]
        w, h = t.width - 2 * pad + 56, t.height - 2 * pad + 30
        im = _band(w, h, a=0.6, outline=col, width=5)
        im.alpha_composite(t, (28 - pad, 15 - pad))
        return [Sprite(im, p.get("cx", 1500) - w / 2, p.get("cy", 300) - h / 2, t0, t1, enter="fade", dur=0.28, z=60)]
    if kind == "chips":
        words = p.get("words", [])
        size = p.get("size", 56)
        ims = []
        for wd in words:
            t = text_img(wd, size, WHITE, 900, shadow=0)
            pad = t.info["pad"]
            w, h = t.width - 2 * pad + 68, t.height - 2 * pad + 36
            im = _band(w, h, a=0.75, outline=WHITE, width=2)
            im.alpha_composite(t, (34 - pad, 18 - pad))
            ims.append(im)
        if not ims:
            return []
        gap = 44
        x = p.get("cx", STAGE_CX) - (sum(im.width for im in ims) + gap * (len(ims) - 1)) / 2
        cy = p.get("cy", 440)
        els = []
        for i, im in enumerate(ims):
            els.append(Sprite(im, x, cy - im.height / 2, t0, t1, enter="fade", dur=0.32, delay=i * 0.12, z=20))
            x += im.width + gap
        return els
    raise ValueError(f"対応していない種類です: {kind}")


class RecWindow(RecWindowBase):
    """録画の窓。土台のまま（細い四角の枠・黒い帯のラベル・斜線の仮表示）。"""


def se_collect(els):
    """効果音の割り当て。素の kit は自動の効果音を付けない。"""
    return []


def se_labels():
    """効果音の種類 → 設定の画面に出す名前。"""
    return {}
