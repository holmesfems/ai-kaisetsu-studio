#!/usr/bin/env python3
"""
解説動画用の透過オーバーレイ演出の芯（公開）。見た目（色・パネル・部品）は持たない。

PIL で要素ごとに描いて、フレームごとに合成し、QuickTime Animation (qtrle, アルファ付き) で書き出す。
時刻はすべて「動画全体の秒」。cue の表（script_cues.json）の開始と、発話同期点 (sync_points.json) から引く。

使い方の流れ（台本ごとのタイムライン <名前>_fx.py の中で）:
    tl = Timeline(cues_path, sync_path)   # cues_path = <files>/script_cues.json（AK Studio が書き出す）
    tl.add(Sprite(...)) / tl.add(部品(...))      # 部品は演出の kit（fxparts など）が持つ
    build() が tl を返す。コマの確認・書き出しは AK Studio（render_still・render_sheet・export）が行う

部品はすべて draw(fr, t) を持ち、t の純関数として描く（並列レンダリングのため）。
乱数は hrand(...) を使う（プロセスが違っても同じ値になる）。

見た目を持つ kit の fxkit.py は `from fxcore import *` ＋色・パネルなどの見た目。
回の fx/ は今までどおり `from fxkit import *` と書ける（公開版だけなら `from fxcore import *`）。
"""

import functools
import hashlib
import json
import math
import os

import imageio_ffmpeg
import numpy as np
from PIL import Image, ImageDraw, ImageFilter, ImageFont

W, H, FPS = 1920, 1080, 30
FFMPEG = imageio_ffmpeg.get_ffmpeg_exe()

# 色の既定（見た目の色は kit が決める。芯は中立の白）。Sprite のワイプの走査線は Rust の render/trans.rs と同じ色
DEFAULT_COLOR = (255, 255, 255)
EDGE_COLOR = (214, 238, 255)

# ---- 画面レイアウト: 左に立ち絵、下に字幕。演出はステージ内に置く ----
STAGE = (600, 6, 1880, 880)           # x0, y0, x1, y1（y0 は HUD の上端。背景のタイトル文字 y=88 より上に置く）
STAGE_CX = (STAGE[0] + STAGE[2]) // 2
CONTENT_Y0 = 84                        # HUD の下
NOTE_XY = (STAGE[2], 872)              # 注釈は右下に右寄せ（字幕より上）
REC_BOX = (600, 100, 1880, 820)        # 録画の窓 1280x720 = 1080p 素材の 66.667%

FONT_FILES = {
    "jp": "C:/Windows/Fonts/NotoSansJP-VF.ttf",
    "sc": "C:/Windows/Fonts/NotoSansSC-VF.ttf",
    "ui": "C:/Windows/Fonts/bahnschrift.ttf",
    "mono": "C:/Windows/Fonts/CascadiaMono.ttf",
}


@functools.lru_cache(maxsize=None)
def font(kind, size, weight=800):
    f = ImageFont.truetype(FONT_FILES[kind], int(size))
    if kind in ("jp", "sc"):
        f.set_variation_by_axes([weight])
    elif kind == "ui":
        # weight は 300..700、幅は 75(Condensed)..100
        w = max(300, min(700, weight))
        f.set_variation_by_axes([w, 100])
    return f


def rgba(c, a=1.0):
    return (c[0], c[1], c[2], int(round(255 * max(0.0, min(1.0, a)))))


# ---------------------------------------------------------------- easing
def clamp01(x):
    return 0.0 if x < 0 else 1.0 if x > 1 else x


def ease_out(x):
    x = clamp01(x)
    return 1 - (1 - x) ** 3


def ease_in_out(x):
    x = clamp01(x)
    return x * x * (3 - 2 * x)


def ease_back(x, s=1.7):
    x = clamp01(x)
    x -= 1
    return x * x * ((s + 1) * x + s) + 1


def lerp(a, b, k):
    return a + (b - a) * k


# 名前で引く緩急（Rust の render/ease.rs の by_name と同じ名前。RecWindow の入り・抜けが名前で渡される）。
# 式は Rust が正で、見本 src-tauri/tests/fixtures/ease.json と test_motion.py で縛っている
EASES = {"linear": clamp01, "ease_out": ease_out, "ease_in_out": ease_in_out}


def kf_value(kfs, t, ease=ease_in_out, dur=0.3):
    """[(t, v), ...] のキーフレーム。各キーで dur 秒かけて前の値から移る。"""
    v = kfs[0][1]
    for tk, vk in kfs[1:]:
        if t < tk:
            break
        v = lerp(v, vk, ease((t - tk) / dur)) if dur > 0 else vk
    return v


def hrand(*key):
    """決定的な乱数 [0,1)。並列レンダリングでも同じ結果になる。"""
    h = hashlib.blake2b(repr(key).encode(), digest_size=8).digest()
    return int.from_bytes(h, "little") / 2 ** 64


# ---------------------------------------------------------------- 画像ヘルパ
def new(w, h):
    return Image.new("RGBA", (max(1, int(w)), max(1, int(h))), (0, 0, 0, 0))


def with_alpha(im, a):
    if a >= 0.999:
        return im
    if a <= 0.001:
        return None
    r, g, b, al = im.split()
    al = al.point(lambda v: int(v * a + 0.5))
    return Image.merge("RGBA", (r, g, b, al))


def put(fr, im, x, y, a=1.0):
    """フレームからはみ出しても良い alpha_composite。"""
    if im is None:
        return
    im = with_alpha(im, a)
    if im is None:
        return
    x, y = int(round(x)), int(round(y))
    sx0, sy0 = max(0, -x), max(0, -y)
    dx0, dy0 = max(0, x), max(0, y)
    w = min(im.width - sx0, fr.width - dx0)
    h = min(im.height - sy0, fr.height - dy0)
    if w <= 0 or h <= 0:
        return
    fr.alpha_composite(im, (dx0, dy0), (sx0, sy0, sx0 + w, sy0 + h))


def glow(im, radius=8, strength=1.0):
    """im の下にぼかしを敷いて発光させる（静的スプライト用）。"""
    pad = radius * 3
    big = new(im.width + pad * 2, im.height + pad * 2)
    big.alpha_composite(im, (pad, pad))
    g = big.filter(ImageFilter.GaussianBlur(radius))
    if strength != 1.0:
        g = with_alpha(g, min(1.0, strength))
    out = new(big.width, big.height)
    out.alpha_composite(g)
    if strength > 1.0:
        out.alpha_composite(with_alpha(g, strength - 1.0))
    out.alpha_composite(big)
    return out, pad


def supersample(w, h, draw_fn, s=3):
    """図形を s 倍で描いて縮小（アンチエイリアス）。draw_fn(d, s) で s 倍座標を使う。"""
    im = new(w * s, h * s)
    draw_fn(ImageDraw.Draw(im), s)
    return im.resize((w, h), Image.LANCZOS)


# ---------------------------------------------------------------- テキスト
def text_w(s, f):
    return ImageDraw.Draw(new(1, 1)).textlength(s, font=f)


def auto_font(s, kind):
    # 日本語フォントに無い字（簡体字の「迟钝」など）が入っていたら SC フォントに切り替える
    if kind == "jp" and any(ord(ch) not in cmap("jp") and ord(ch) in cmap("sc") for ch in s):
        return "sc"
    return kind


@functools.lru_cache(maxsize=None)
def cmap(kind):
    from fontTools.ttLib import TTFont
    return frozenset(TTFont(FONT_FILES[kind], lazy=True).getBestCmap().keys())


def split_fallback(runs):
    """英字フォント（ui/mono）に無い字（−→＋ など）を日本語フォントに逃がす。"""
    out = []
    for s, st in runs:
        kind = st.get("kind", "jp")
        if kind not in ("ui", "mono"):
            out.append((s, st))
            continue
        cm = cmap(kind)
        buf, cur = "", None
        for ch in s:
            ok = ord(ch) in cm
            if cur is not None and ok != cur:
                out.append((buf, st if cur else dict(st, kind="jp", weight=min(900, st.get("weight", 700) + 150))))
                buf = ""
            buf += ch
            cur = ok
        if buf:
            out.append((buf, st if cur else dict(st, kind="jp", weight=min(900, st.get("weight", 700) + 150))))
    return out


def text_img(s, size, color=None, weight=800, kind="jp", a=1.0, tracking=0, shadow=0.55):
    """1行テキスト。高さはフォントの ascent+descent で固定（行揃えが崩れない）。color が None なら DEFAULT_COLOR。"""
    return runs_img([(s, dict(size=size, color=DEFAULT_COLOR if color is None else color, weight=weight, kind=kind, a=a, tracking=tracking))],
                    shadow=shadow)


def runs_img(runs, shadow=0.55, pad=6):
    """[(text, style), ...] を同じベースラインで横に並べる。style: size,color,weight,kind,a,tracking,dy"""
    parts = []
    asc_max = desc_max = 0
    x = 0
    for s, st in split_fallback(runs):
        kind = auto_font(s, st.get("kind", "jp"))
        f = font(kind, st.get("size", 40), st.get("weight", 800))
        asc, desc = f.getmetrics()
        tr = st.get("tracking", 0)
        if tr:
            w = sum(text_w(ch, f) + tr for ch in s) - tr
        else:
            w = text_w(s, f)
        parts.append((s, f, st, x, tr))
        asc_max = max(asc_max, asc + st.get("dy", 0))
        desc_max = max(desc_max, desc)
        x += w + st.get("gap", 0)
    im = new(x + pad * 2, asc_max + desc_max + pad * 2)
    d = ImageDraw.Draw(im)
    base = pad + asc_max
    for s, f, st, x0, tr in parts:
        col = rgba(st.get("color", DEFAULT_COLOR), st.get("a", 1.0))
        y = base - st.get("dy", 0)
        if tr:
            xx = pad + x0
            for ch in s:
                d.text((xx, y), ch, font=f, fill=col, anchor="ls")
                xx += text_w(ch, f) + tr
        else:
            d.text((pad + x0, y), s, font=f, fill=col, anchor="ls")
    if shadow:
        sh = new(im.width, im.height)
        sh.putalpha(im.getchannel("A").filter(ImageFilter.GaussianBlur(3)).point(lambda v: int(v * shadow)))
        out = new(im.width, im.height)
        out.alpha_composite(sh, (0, 2))
        out.alpha_composite(im)
        im = out
    im.info["pad"] = pad
    im.info["base"] = base
    return im



# ---------------------------------------------------------------- 要素
class El:
    """t0 で入り、t1 で抜ける要素。draw(fr, t) を実装する。"""
    z = 0
    OUT_EASE = "ease_in_out"        # 抜けの緩急の名前（EASES）。ワーカーが RecWindow の分を Rust に渡す

    def __init__(self, t0, t1=None, z=0, out=0.22):
        self.t0, self.t1, self.z, self.out = t0, t1, z, out

    def alive(self, t):
        return t >= self.t0 and (self.t1 is None or t < self.t1 + self.out)

    def k_out(self, t):
        if self.t1 is None or t < self.t1:
            return 1.0
        return 1 - EASES[self.OUT_EASE]((t - self.t1) / self.out)

    def draw(self, fr, t):
        raise NotImplementedError


class Sprite(El):
    """静止画1枚を入り/抜けアニメ付きで置く。

    enter: 'up' 'down' 'left' 'right' 'fade' 'wipe'(左から走査線で開く) 'pop' 'glitch' 'slam'
    alpha_kf: [(t, a), ...] で途中の不透明度（強調・減光）を動かす
    """

    def __init__(self, im, x, y, t0, t1=None, enter="up", dur=0.32, z=0, alpha_kf=None, pad=0,
                 delay=0.0, dist=26, out=0.22):
        super().__init__(t0 + delay, t1, z, out)
        self.im, self.x, self.y = im, x - pad, y - pad
        self.enter, self.dur, self.alpha_kf, self.dist = enter, dur, alpha_kf, dist
        self.key = (x, y, t0, im.size)

    def draw(self, fr, t):
        k = (t - self.t0) / self.dur
        c = enter_curve(self.enter, k)
        a = self.k_out(t)
        if self.alpha_kf:
            a *= kf_value(self.alpha_kf, t)
        a *= c["alpha"]
        im, x, y = self.im, self.x, self.y
        en = self.enter
        if c["slide"]:
            sl = c["slide"]
            if en == "up":
                y += sl * self.dist
            elif en == "down":
                y -= sl * self.dist
            elif en == "left":
                x -= sl * self.dist * 1.6
            elif en == "right":
                x += sl * self.dist * 1.6
        if c["scale"] != 1.0:
            s = c["scale"]
            im = im.resize((max(1, int(im.width * s)), max(1, int(im.height * s))), Image.BILINEAR)
            x += (self.im.width - im.width) / 2
            y += (self.im.height - im.height) / 2
        if c["wipe"] is not None:
            cw = int(im.width * c["wipe"])
            if cw <= 0:
                return
            im = im.crop((0, 0, cw, im.height))
            put(fr, im, x, y, a)
            # 走査線
            edge = new(6, self.im.height)
            ImageDraw.Draw(edge).rectangle((2, 0, 3, self.im.height), fill=rgba(EDGE_COLOR, 0.9))
            edge = edge.filter(ImageFilter.GaussianBlur(1.2))
            put(fr, edge, x + cw - 3, y, a)
            return
        if c["glitch"]:
            im = glitch(im, k, self.key, int(t * FPS))
        put(fr, im, x, y, a)


def enter_curve(enter, k):
    """Sprite の入りの 1 時点の値（k: 0 = まだ、1 以上 = 出きっている）。
    alpha: 掛ける不透明度、scale: 倍率、slide: 動く距離のうち残っている割合（up/down/left/right）、
    wipe: 見えている割合（None = ワイプでない）、glitch: グリッチの強さ。
    Rust の render/trans.rs の `curve` と同じ値になる（見本 trans_curves.json。Sprite の名前 up/down/left/right は slide の行）。
    距離・向き・画像の縮め方・走査線の描き方は draw が決める。"""
    c = {"alpha": 1.0, "scale": 1.0, "slide": 0.0, "wipe": None, "glitch": 0.0}
    if k >= 1:
        return c
    k = clamp01(k)
    if enter in ("up", "down", "left", "right"):
        c["alpha"] = ease_out(k)
        c["slide"] = 1 - ease_out(k)
    elif enter == "fade":
        c["alpha"] = ease_out(k)
    elif enter == "pop":
        c["alpha"] = ease_out(k * 1.6)
        c["scale"] = lerp(0.82, 1.0, ease_back(k, 2.2))
    elif enter == "slam":
        c["alpha"] = ease_out(k * 2.5)
        c["scale"] = lerp(1.7, 1.0, ease_out(k))
    elif enter == "wipe":
        c["wipe"] = ease_in_out(k)
    elif enter == "glitch":
        c["alpha"] = ease_out(k * 2.0)
        c["glitch"] = (1 - k) ** 1.5
    return c


# ---- グリッチの画素（Rust の render/trans.rs の seed・hrand・glitch と一画素まで同じ。見本 glitch.json） ----
_M64 = 0xFFFFFFFFFFFFFFFF
_G0, _G1, _G2 = 0x9E3779B97F4A7C15, 0xBF58476D1CE4E5B9, 0x94D049BB133111EB


def glitch_seed(id, frame):
    """グリッチの乱数の種（クリップとコマで決まる）。FNV-1a ＋ frame × 0x9e3779b97f4a7c15、64 bit で切る。"""
    h = 0xCBF29CE484222325
    for b in str(id).encode("utf-8"):
        h = ((h ^ b) * 0x00000100000001B3) & _M64
    return h ^ ((int(frame) & _M64) * _G0 & _M64)


def _hrand(seed, i, tag):
    """決定的な乱数 [0, 1)（splitmix 風。Rust の trans.rs の hrand）。"""
    z = (seed ^ ((i * _G1) & _M64) ^ ((tag * _G2) & _M64)) & _M64
    z = (z + _G0) & _M64
    z = ((z ^ (z >> 30)) * _G1) & _M64
    z = ((z ^ (z >> 27)) * _G2) & _M64
    z ^= z >> 31
    return (z >> 11) / 2 ** 53


def _round_half_away(x):
    """Rust の f64::round（0.5 は 0 から遠い方へ。Python の round は偶数へ丸めるので使わない）。"""
    if x < 0:
        return -_round_half_away(-x)
    f = math.floor(x)
    return f + 1 if x - f >= 0.5 else f


def glitch_pixels(arr, amt, seed, unit=1.0):
    """画素の配列（h×w×ch の uint8）に横ずれ＋色ずれを掛けて返す（Rust の trans.rs の glitch と同じ）。
    amt: 強さ 0〜1、unit: 1px が何画素か。"""
    if amt < 0.03 or arr.shape[0] == 0 or arr.shape[1] == 0:
        return arr
    h, w = arr.shape[0], arr.shape[1]
    out = arr.copy()
    for i in range(6):
        if _hrand(seed, i, 1) > 0.55:
            continue
        y0 = int(_hrand(seed, i, 2) * h)
        bh = max(1, _round_half_away(4 * unit + _hrand(seed, i, 3) * h * 0.18))
        dx = _round_half_away((_hrand(seed, i, 4) - 0.5) * 70 * amt * unit)
        if dx == 0:
            continue
        out[y0:y0 + bh] = np.roll(arr[y0:y0 + bh], dx, axis=1)       # 行は元の画像から回す
    s = _round_half_away(6 * amt * unit)
    if s > 0 and arr.shape[2] >= 3:
        cur = out.copy()
        xs = np.arange(w)
        out[..., 0] = cur[:, np.clip(xs - s, 0, w - 1), 0]            # R を右へ（端で止める）
        out[..., 2] = cur[:, np.clip(xs + s, 0, w - 1), 2]            # B を左へ
    return out


def glitch(im, k, key, fi):
    """入りの瞬間だけ横ずれ＋色ずれ。k: 0→1。"""
    amt = (1 - k) ** 1.5
    if amt < 0.03:
        return im
    return Image.fromarray(glitch_pixels(np.asarray(im).copy(), amt, glitch_seed(repr(key), fi)))


class Func(El):
    """毎フレーム描く動的な要素。fn(fr, t, a) を呼ぶ。"""

    def __init__(self, fn, t0, t1=None, z=0, fade_in=0.25, out=0.22):
        super().__init__(t0, t1, z, out)
        self.fn, self.fade_in = fn, fade_in

    def draw(self, fr, t):
        a = self.k_out(t) * (ease_out((t - self.t0) / self.fade_in) if self.fade_in else 1.0)
        if a > 0.002:
            self.fn(fr, t, a)


# ---------------------------------------------------------------- cue の表と同期点
def load_cues(path):
    """AK Studio が書き出す script_cues.json（{"cues": [{n, start, end, text, speaker}]}。形は ak-proto の CueTable）を
    {n: {"start", "end", "text", "speaker"}} にして返す。srt は読まない（script.srt は Premiere 用と AK Studio の内部用）。"""
    if not os.path.exists(path):
        raise FileNotFoundError(f"{path} がありません。AK Studio で「台本 → 演出の入力」を書き出してください")
    with open(path, encoding="utf-8-sig") as f:
        js = json.load(f)
    return {int(c["n"]): {"start": c["start"], "end": c["end"], "text": c["text"], "speaker": c.get("speaker", "")}
            for c in js["cues"]}


def _cues_path(path):
    """Timeline の第 1 引数。.srt が渡されたら、同じフォルダの script_cues.json を読む（srt の中身は読まない）。"""
    path = os.fspath(path)
    if path.lower().endswith(".srt"):
        return os.path.join(os.path.dirname(os.path.abspath(path)), "script_cues.json")
    return path


class Timeline:
    def __init__(self, cues, sync=None, end=None):
        """cues: script_cues.json のパス（.srt のパスなら隣の script_cues.json）。sync: sync_points.json のパス"""
        self.cues = load_cues(_cues_path(cues))
        self.sync = {}
        self.sync_used = []
        if sync and os.path.exists(sync):
            js = json.load(open(sync, encoding="utf-8"))
            for r in js.get("sync", []):
                self.sync[(int(r["cue"]), r["keyword"])] = r
            for n, c in js.get("cues", {}).items():
                if int(n) in self.cues:
                    self.cues[int(n)]["onset"] = c.get("onset")
        # 手で入れた同期点（talk/ の音声が録り直し前で Whisper が合わない cue など）。sync_points.json より優先。
        # 1 件の形は ak-proto の SyncPoint（cue・keyword・t・t_end・method?。method が無ければ "manual"）
        manual = os.path.join(os.path.dirname(os.path.abspath(sync)), "sync_manual.json") if sync else None
        if manual and os.path.exists(manual):
            for r in json.load(open(manual, encoding="utf-8")).get("sync", []):
                self.sync[(int(r["cue"]), r["keyword"])] = dict(r, method=r.get("method", "manual"))
        self.els = []
        self.end = end

    @classmethod
    def blank(cls, end=None):
        """cue も同期点も無い空のタイムライン（台本の無い演出用。サブスクのお願い・攻略動画など）。"""
        tl = cls.__new__(cls)
        tl.cues, tl.sync, tl.sync_used, tl.els, tl.end = {}, {}, [], [], end
        return tl

    def c(self, n):
        """cue n の開始秒。"""
        return self.cues[n]["start"]

    def ce(self, n):
        return self.cues[n]["end"]

    def s(self, n, kw, end=False):
        """cue n のセリフで kw を言い始める時刻（end=True なら言い終わり）。"""
        r = self.sync.get((n, kw))
        cue = self.cues[n]
        if r:
            t = r["t_end"] if end else r["t"]
            method = r.get("method", "whisper")
        else:
            # フォールバック: 文字位置の割合 × セリフの長さ
            txt = cue["text"]
            i = txt.find(kw)
            if i < 0:
                raise KeyError(f"cue {n} に「{kw}」が無い")
            if end:
                i += len(kw)
            t = cue["start"] + (cue["end"] - cue["start"]) * i / max(1, len(txt))
            method = "ratio"
        self.sync_used.append((n, kw, end, t, method))
        return t

    def add(self, *els):
        for e in els:
            if isinstance(e, (list, tuple)):
                self.add(*e)
            elif e is not None:
                self.els.append(e)
        return els

    def frame(self, t):
        fr = new(W, H)
        for e in sorted((e for e in self.els if e.alive(t)), key=lambda e: e.z):
            e.draw(fr, t)
        return fr



# ---------------------------------------------------------------- 録画の窓の土台
def show_placeholder():
    """録画の仮表示を描くか。--no-placeholder で FX_NO_PLACEHOLDER=1 になる。
    モジュール変数にしないのは、並列レンダリングの子プロセス（Windows は spawn）に引き継がれないため。
    録画トラックの書き出し（FX_REC_VIDEO）を下に敷くときは、録画が入っている区間だけ仮表示を消す（rec_has_footage）。"""
    return os.environ.get("FX_NO_PLACEHOLDER") != "1"


def rec_has_footage(t0, t1, box=REC_BOX):
    """録画トラックの書き出しに、t0〜t1 の窓の中身（黒以外）があるか。結果は書き出しの更新時刻ごとにキャッシュする。"""
    import subprocess
    import tempfile
    path = os.environ.get("FX_REC_VIDEO")
    if not path or not os.path.exists(path):
        return False
    st = os.stat(path)
    key = hashlib.md5(f"{os.path.abspath(path)}|{st.st_mtime}|{st.st_size}".encode()).hexdigest()[:12]
    cache_p = os.path.join(tempfile.gettempdir(), f"fx_rec_present_{key}.json")
    try:
        cache = json.load(open(cache_p, encoding="utf-8"))
    except Exception:
        cache = {}
    ck = f"{t0:.2f}-{t1:.2f}"
    if ck in cache:
        return cache[ck]
    x0, y0, x1, y1 = box
    found = False
    for t in (t0 + 0.6, (t0 + t1) / 2, t1 - 0.6):
        r = subprocess.run([FFMPEG, "-v", "error", "-ss", f"{max(0.0, t):.3f}", "-i", path, "-frames:v", "1",
                            "-vf", f"crop={x1 - x0}:{y1 - y0}:{x0}:{y0},scale=64:36", "-f", "rawvideo", "-pix_fmt", "gray",
                            "-"], capture_output=True)
        if r.stdout and np.frombuffer(r.stdout, np.uint8).astype(float).std() > 8:
            found = True
            break
    cache[ck] = found
    tmp = f"{cache_p}.{os.getpid()}"
    json.dump(cache, open(tmp, "w", encoding="utf-8"))
    os.replace(tmp, cache_p)
    return found


def premiere_label(cx, cy, scale, note=""):
    """仮表示に添える「Premiere: 位置 (x, y)・スケール s%」のラベル（素の見た目: 黒い帯に白い文字）。
    kit は自分の見た目の版を作って、placeholder_image の中で使う。"""
    txt = f"Premiere  位置 ({cx:.0f}, {cy:.0f})"
    if scale is not None:
        txt += f"  スケール {scale:.3f}%".replace(".000%", "%")
    if note:
        txt += f"  （{note}）"
    t = text_img(txt, 22, DEFAULT_COLOR, 600, shadow=0)
    p = t.info["pad"]
    w, h = t.width - 2 * p + 32, t.height - 2 * p + 8
    im = new(w, h)
    ImageDraw.Draw(im).rectangle((0, 0, w - 1, h - 1), fill=(0, 0, 0, 200))
    im.alpha_composite(t, (16 - p, 4 - p))
    return im


class RecWindowBase(El):
    """録画を差し込む窓の土台（仕組みだけ）。placeholder=True なら中身に仮表示を描く（None なら show_placeholder()）。

    segs: [(t, rec_id, filename, 説明), ...] 同じ窓のまま中身だけ切り替える
    録画そのものは Premiere でこの動画の下のトラックに置く（位置は AK Studio の clips_list の nest）。
    入り・抜けの長さと緩急は ENTER・ENTER_EASE・out・OUT_EASE。AK Studio（ワーカー）が Rust に渡して、
    窓の中に差し込む録画の濃さを合わせる（値を変えるときはここだけ直す）。

    見た目は、次の 4 つを上書きして変える（素の版は細い枠・黒い帯・斜線の仮表示）。
    __init__ の中から呼ぶので、インスタンスの変数に頼らず、引数だけで描くこと。
      frame_image(w, h)                        窓の枠。中身が w×h のとき、四方に 8px の余白を足した (w+16)×(h+16) を返す
      label_image(idx, rid)                    区間 idx（録画の番号 rid）のラベル。窓の左上の縁に載る
      placeholder_image(w, h, fn, desc, box)   仮表示の中身（w×h。fn はファイル名、desc は説明、box は窓の位置）
      overlay(fr, t, a)                        枠とラベルの上に重ねるもの（REC 表示など）。a は入り・抜けの濃さ込み
    """
    ENTER = 0.35                    # 入りの長さ（秒）
    ENTER_EASE = "ease_out"         # 入りの緩急の名前（EASES）
    OUT_EASE = "ease_in_out"        # 抜けの緩急の名前（EASES）

    def __init__(self, segs, t1, placeholder=None, box=REC_BOX):
        super().__init__(segs[0][0], t1, z=10, out=0.25)
        placeholder = show_placeholder() if placeholder is None else placeholder
        self.segs, self.ph = segs, placeholder
        self.box = box
        x0, y0, x1, y1 = box
        w, h = x1 - x0, y1 - y0
        self.frame_im = self.frame_image(w, h)
        self.labels = []
        self.inner = []
        for i, (ts, rid, fn, desc) in enumerate(segs):
            self.labels.append(self.label_image(i, rid))
            ph = new(w, h)
            te = segs[i + 1][0] if i + 1 < len(segs) else t1
            if placeholder and not rec_has_footage(ts, te, box):     # 録画が届いている区間は、下に敷く録画を見せる
                ph = self.placeholder_image(w, h, fn, desc, box)
            self.inner.append(ph)

    # ---- 見た目（素の版）
    def frame_image(self, w, h):
        fr = new(w + 16, h + 16)
        ImageDraw.Draw(fr).rectangle((8, 8, w + 8 - 1, h + 8 - 1), outline=(255, 255, 255, 140), width=2)
        return fr

    def label_image(self, idx, rid):
        t = text_img(f"REC  {rid}", 20, DEFAULT_COLOR, 700, kind="ui", shadow=0)
        p = t.info["pad"]
        w, h = t.width - 2 * p + 28, t.height - 2 * p + 12
        im = new(w, h)
        ImageDraw.Draw(im).rectangle((0, 0, w - 1, h - 1), fill=(0, 0, 0, 200))
        im.alpha_composite(t, (14 - p, 6 - p))
        return im

    def placeholder_image(self, w, h, fn, desc, box):
        ph = new(w, h)
        d = ImageDraw.Draw(ph)
        d.rectangle((0, 0, w, h), fill=(0, 0, 0, 170))
        for i in range(-h, w, 34):
            d.line([(i, h), (i + h, 0)], fill=(255, 255, 255, 16), width=2)
        f1 = text_img(fn, 34, DEFAULT_COLOR, 400, kind="mono", a=0.9)
        f2 = text_img(desc, 40, DEFAULT_COLOR, 800)
        f3 = text_img("録画素材をここに差し込む（仮表示）", 24, (180, 180, 180), 600)
        for im, yy in ((f1, h / 2 - 70), (f2, h / 2 - 10), (f3, h / 2 + 60)):
            ph.alpha_composite(im, (int(w / 2 - im.width / 2), int(yy)))
        x0, y0, x1, y1 = box
        ph.alpha_composite(premiere_label((x0 + x1) / 2, (y0 + y1) / 2, w / 1920 * 100, "1920x1080 の録画"), (18, 26))
        return ph

    def overlay(self, fr, t, a):
        pass

    # ---- 時刻の扱い
    def seg_index(self, t):
        """t に出ている区間の番号（alive の中で呼ぶこと。最初の区間より前の t では ValueError）。"""
        return max(i for i, s in enumerate(self.segs) if t >= s[0])

    def draw(self, fr, t):
        a = self.k_out(t)
        x0, y0, x1, y1 = self.box
        k = (t - self.t0) / self.ENTER
        e = EASES[self.ENTER_EASE](k)
        idx = self.seg_index(t)
        ts = self.segs[idx][0]
        kk = ease_out((t - ts) / 0.3)
        # 中身
        if self.ph:
            if idx > 0 and kk < 1:
                put(fr, self.inner[idx - 1], x0, y0, a * (1 - kk))
            put(fr, self.inner[idx], x0, y0, a * e * (kk if idx > 0 else 1))
        # 枠（少し大きい状態から締まる）
        im = self.frame_im
        if k < 1:
            s = lerp(1.05, 1.0, e)
            im = im.resize((int(im.width * s), int(im.height * s)), Image.BILINEAR)
        put(fr, im, (x0 + x1) / 2 - im.width / 2, (y0 + y1) / 2 - im.height / 2, a * e)
        # ラベル
        lab = self.labels[idx]
        put(fr, lab, x0 + 18, y0 - lab.height / 2, a * e * (kk if idx > 0 else 1))
        self.overlay(fr, t, a * e)
