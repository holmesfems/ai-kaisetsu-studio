#!/usr/bin/env python3
"""
効果音（SE）の仕組み（公開。芯 fxcore だけを使う。部品（fxparts）・kit・fxsound は読まない）。

- 合成の音（numpy・scipy）と、素材（wav）を並べて動画と同じ 0 秒始まりのステレオ wav にする書き出し（render_se・render_all）
- 素材の割り当て se/bank.json の読み書き（load_bank・bank_info・bank_set）と、画面の「効果音の設定」の裏側（試聴 audition・書き出し render）
- どの動きにどの種類の音を当てるか（collect）は kit の fxsound.py が持つ。ここは「候補（時刻・種類・位置）の並び」を受け取るだけ

    import fxse
    bank = fxse.load_bank("se/bank.json")
    fxse.render_all(ev, total, "out", "xx", bank=bank)      # ev = [{"t": 秒, "kind": 種類, "x": 画面の x}, ...]

鳴らす種類は profile で選ぶ。"full"（既定。パネルの開きや行の出入りも鳴らす）/ "lite"（要所だけ）。
要素ごとの指定: with_se(要素, "zap")（Func などに音を付ける）/ with_se(要素, False)（無音）。
自作の要素は se_events() -> [(t, 種類[, x[, {"dur": .., "ease": ..}]]), ...] を書くと鳴る。
素材を使う種類は se/bank.json に書く（{"chapter": ["synth", "T12.wav"], ...}。複数なら出てくる順に順番に使う）。
素材の音量は、同じ種類の合成音の大きさに自動でそろえる。

kit が無い（素の kit）ときは、候補が無く、bank も空。画面から自分の wav を足して鳴らせる。
"""

import json
import math
import os
import shutil
import time
import wave

import numpy as np
from scipy import signal

from fxcore import STAGE_CX, ease_in_out, ease_out, hrand

SR = 48000

# 種類ごとの音量（ピーク dBFS）と優先度。セリフ（実効値 −20 dB 前後）の邪魔をしない大きさにしてある。
LEVEL = {
    "chapter": -13, "slam": -14, "rec_open": -21, "rec_switch": -26, "glitch": -24, "glitch_s": -30,
    "lockon": -25, "ring": -25, "zap": -26, "count": -33, "confirm": -26, "confirm_neg": -25,
    "wipe": -30, "blip": -27, "tick": -33, "move": -33, "hud": -30, "draw": -35, "bar": -31,
    # 攻略動画の操作（予備は控えめ・精密の予備は少し大きめ）
    "guide_pre": -28, "guide_done": -26, "guide_precise_pre": -24, "guide_precise_done": -24,
}
PRIO = {
    "chapter": 100, "slam": 90, "rec_open": 80, "glitch": 70, "lockon": 65, "ring": 60, "count": 55, "zap": 55,
    "rec_switch": 50, "bar": 45, "draw": 40, "wipe": 35, "glitch_s": 30, "blip": 25, "move": 20, "hud": 15,
    "tick": 10, "guide_pre": 26, "guide_done": 66, "guide_precise_pre": 62, "guide_precise_done": 67,
}
# 攻略動画の操作の効果音の 4 つのグループ。素材は回の se/ ではなくアプリの効果音の棚（全部の回で使う）から読む
# （回の se/bank.json にその種類があれば回のほうが勝つ。AK Studio の akfx_worker.py が棚の場所を渡す）
SHELF_KINDS = ("guide_pre", "guide_done", "guide_precise_pre", "guide_precise_done")
PROFILES = {
    "lite": {"chapter", "slam", "rec_open", "rec_switch", "glitch", "lockon", "ring", "count", "zap"},
    "full": set(PRIO),
}


# ======================================================================= 拾う
def with_se(el, kind):
    """要素に鳴らす音の種類を付ける（Func や自作の要素用）。kind=False で無音。"""
    el.se = kind
    return el


def _pan_x(x):
    """画面の x → 定位 −1..1（ステージの中央を真ん中に、控えめに振る）。"""
    return float(np.clip((x - STAGE_CX) / 640, -1, 1) * 0.35)



def select(ev, profile="lite", t0=None, t1=None):
    """種類で絞り、重なりを間引く。"""
    kinds = PROFILES[profile] if isinstance(profile, str) else set(profile)
    # 候補はクリップごとに集めるので時刻の順とは限らない。まとめ・間引きは時刻の順に見る
    # （順でないと、ずっと後の同じ種類に「0.07 秒以内」としてまとめられて消える）
    ev = sorted((d for d in ev if d["kind"] in kinds and (t0 is None or d["t"] >= t0) and (t1 is None or d["t"] < t1)),
                key=lambda d: d["t"])
    # 同じ種類が 0.07 秒以内に続いたら1つにまとめる（数え上げは長い方を残す）
    out = []
    for d in ev:
        same = [o for o in out[-6:] if o["kind"] == d["kind"] and 0 <= d["t"] - o["t"] < 0.07]
        if same:
            o = same[-1]
            if d.get("dur", 0) > o.get("dur", 0):
                o.update(dur=d["dur"], ease=d.get("ease"))
            o["neg"] = o.get("neg", False) or d.get("neg", False)
            continue
        out.append(dict(d))
    # 大きい音（章タイトル・ハンコ・録画の窓）の直後は、小さい音を鳴らさない
    big = [(d["t"], d["kind"]) for d in out if d["kind"] in ("chapter", "slam", "rec_open")]
    keep = []
    for d in out:
        sup = False
        for tb, kb in big:
            if kb == d["kind"] and tb == d["t"]:
                continue
            win = 0.6 if kb == "chapter" else 0.18
            if -0.05 <= d["t"] - tb <= win and PRIO[d["kind"]] < min(PRIO[kb], 60):
                sup = True
                break
        if not sup:
            keep.append(d)
    # 0.1 秒の中に重ねるのは3つまで（優先度の高い順）
    final = []
    for d in keep:
        near = [o for o in final if abs(o["t"] - d["t"]) < 0.1]
        if len(near) >= 3:
            low = min(near, key=lambda o: PRIO[o["kind"]])
            if PRIO[low["kind"]] < PRIO[d["kind"]]:
                final.remove(low)
            else:
                continue
        final.append(d)
    return sorted(final, key=lambda d: d["t"])


# ======================================================================= 合成の道具
def _n(d):
    return max(1, int(round(d * SR)))


def _tt(n):
    return np.arange(n) / SR


def _env(n, a=0.002, tau=0.1, hold=0.0):
    t = _tt(n)
    return np.where(t < a, t / max(a, 1e-6), np.exp(-np.clip(t - a - hold, 0, None) / tau))


def _ph(freq, n):
    f = np.broadcast_to(np.asarray(freq, dtype=np.float64), (n,))
    return 2 * np.pi * np.cumsum(f) / SR


def _glide(f0, f1, n, d=None):
    """指数カーブで f0→f1（d 秒で到達して以後保持）。"""
    k = np.clip(_tt(n) / (d or n / SR), 0, 1)
    return f0 * (f1 / f0) ** k


def _noise(n, seed):
    return np.random.default_rng(seed).standard_normal(n)


def _sos(kind, f, order=2):
    return signal.butter(order, f, kind, fs=SR, output="sos")


def _bp(x, lo, hi, order=2):
    return signal.sosfilt(_sos("bandpass", [lo, min(hi, SR / 2 - 100)], order), x)


def _hp(x, f, order=2):
    return signal.sosfilt(_sos("highpass", f, order), x)


def _lp(x, f, order=2):
    return signal.sosfilt(_sos("lowpass", min(f, SR / 2 - 100), order), x)


def _fade(x, fi=0.001, fo=0.006):
    x = x.copy()
    a, b = _n(fi), _n(fo)
    x[:a] *= np.linspace(0, 1, a)
    x[-b:] *= np.linspace(1, 0, b)
    return x


def _norm(x):
    m = np.abs(x).max()
    return x / m if m > 0 else x


def _put(buf, x, i):
    if i >= len(buf) or i + len(x) <= 0:
        return
    if i < 0:
        x, i = x[-i:], 0
    m = min(len(x), len(buf) - i)
    buf[i:i + m] += x[:m]


def _sweep_noise(n, f0, f1, seed, bw=0.7, env=None):
    """帯域を f0→f1 へ動かしたノイズ（シュッ）。bw はオクターブ幅。"""
    x = _noise(n + 2048, seed)
    f, _, Z = signal.stft(x, SR, nperseg=1024, noverlap=768)
    m = Z.shape[1]
    k = np.linspace(0, 1, m)
    fc = f0 * (f1 / f0) ** k
    lf = np.log2(np.maximum(f, 20))[:, None]
    mask = np.exp(-0.5 * ((lf - np.log2(fc)[None, :]) / (bw / 2)) ** 2)
    _, y = signal.istft(Z * mask, SR, nperseg=1024, noverlap=768)
    y = y[:n]
    if env is not None:
        y = y * env
    return y


def _seed(*key):
    return int(hrand("se", *key) * 2 ** 31)


# ======================================================================= 音
def s_tick(ev):
    n = _n(0.06)
    r = hrand("tick", ev["key"])
    x = np.sin(_ph(2300 + 400 * r, n)) * _env(n, 0.0005, 0.012)
    x += 0.3 * _hp(_noise(n, _seed(ev["key"])), 3000) * _env(n, 0.0, 0.003)
    return _fade(_norm(x))


def s_blip(ev):
    n = _n(0.2)
    f0 = 1400 * (1 + 0.1 * (hrand("blip", ev["key"]) - 0.5))
    ph = _ph(_glide(f0, f0 * 1.35, n, 0.03), n)
    x = (np.sin(ph) + 0.25 * np.sin(2 * ph)) * _env(n, 0.002, 0.045)
    return _fade(_norm(x))


def s_lockon(ev):
    n = _n(0.3)
    x = np.zeros(n)
    for dt, f, tau in ((0.0, 2093, 0.02), (0.065, 2794, 0.05)):
        m = _n(0.2)
        b = np.tanh(2 * np.sin(_ph(f, m))) * _env(m, 0.001, tau)
        _put(x, b, _n(dt))
    return _fade(_norm(_lp(x, 8000)))


def s_count_tick(f):
    n = _n(0.035)
    x = np.sin(_ph(f, n)) * _env(n, 0.0004, 0.006)
    x += 0.15 * _hp(_noise(n, int(f)), 4000) * _env(n, 0.0, 0.002)
    return _fade(_norm(x), 0.0005, 0.004)


def s_confirm(ev):
    n = _n(0.45)
    t = _tt(n)
    x = (np.sin(2 * np.pi * 1760 * t) + 0.7 * np.sin(2 * np.pi * 2637 * t)) * _env(n, 0.002, 0.12)
    return _fade(_norm(x))


def s_confirm_neg(ev):
    n = _n(0.35)
    x = np.zeros(n)
    for dt, f in ((0.0, 740), (0.085, 523)):
        m = _n(0.2)
        b = np.tanh(2.2 * np.sin(_ph(f, m))) * _env(m, 0.002, 0.06)
        _put(x, b, _n(dt))
    return _fade(_norm(_lp(x, 5000)))


def s_glitch(ev):
    rng = np.random.default_rng(_seed("glitch", ev["key"]))
    n = _n(0.32)
    out = np.zeros(n)
    pos, amp = 0, 1.0
    for _ in range(int(rng.integers(4, 7))):
        L = _n(rng.uniform(0.012, 0.035))
        t = _tt(L)
        typ = int(rng.integers(0, 4))
        if typ == 0:
            s = np.sign(np.sin(2 * np.pi * rng.uniform(300, 1800) * t)) * 0.6
        elif typ == 1:
            s = _bp(rng.standard_normal(L), 2000, 9000)
        elif typ == 2:
            step = int(SR / rng.uniform(1500, 5000))
            s = np.repeat(rng.standard_normal(L // step + 1), step)[:L] * 0.7
        else:
            s = np.sin(_ph(_glide(rng.uniform(800, 3000), rng.uniform(200, 800), L), L))
        q = float(rng.choice([4, 8, 16]))
        s = np.round(s * q) / q
        s = _fade(s, 0.001, 0.002) * amp
        _put(out, s, pos)
        pos += L + _n(rng.uniform(0.004, 0.02))
        amp *= rng.uniform(0.65, 0.9)
    return _fade(_norm(_lp(out, 10000)))


def s_wipe(ev):
    d = ev.get("dur", 0.3)
    n = _n(d + 0.12)
    t = _tt(n)
    env = np.clip(t / (d * 0.3), 0, 1) * np.exp(-np.clip(t - d * 0.3, 0, None) / (d * 0.45))
    x = _sweep_noise(n, 1200, 7000, _seed("wipe", ev["key"]), 0.8, env)
    x += 0.15 * np.sin(_ph(_glide(900, 2400, n, d), n)) * env
    return _fade(_norm(x))


def s_move(ev):
    d = ev.get("dur", 0.5)
    n = _n(d + 0.05)
    k = np.clip(_tt(n) / d, 0, 1)
    env = np.sin(np.pi * k) ** 1.5
    return _fade(_norm(_sweep_noise(n, 500, 1800, _seed("move", ev["key"]), 1.0, env)))


def s_ring(ev):
    n = _n(0.4)
    ph = _ph(_glide(1175, 1150, n, 0.3), n)
    x = (np.sin(ph) + 0.15 * np.sin(2 * ph)) * _env(n, 0.003, 0.14)
    y = np.zeros(n + _n(0.25))
    for dt, g in ((0.0, 1.0), (0.11, 0.35), (0.22, 0.12)):
        _put(y, x * g, _n(dt))
    return _fade(_norm(y))


def s_zap(ev):
    n = _n(0.22)
    t = _tt(n)
    f = _glide(3000, 500, n, 0.14) * (1 + 0.03 * np.sin(2 * np.pi * 60 * t))
    x = np.sin(_ph(f, n)) * _env(n, 0.001, 0.07)
    x += 0.2 * _hp(_noise(n, _seed("zap", ev["key"])), 5000) * _env(n, 0.0, 0.02)
    return _fade(_norm(x))


def s_slam(ev):
    n = _n(0.6)
    body = np.sin(_ph(_glide(150, 55, n, 0.09), n)) * _env(n, 0.001, 0.16)
    body = np.tanh(1.8 * body) / np.tanh(1.8)
    sd = _seed("slam", ev["key"])
    click = 0.5 * _hp(_noise(n, sd), 1500) * _env(n, 0.0, 0.004)
    thock = 0.6 * _bp(_noise(n, sd + 1), 250, 1400) * _env(n, 0.0, 0.035)
    return _fade(_norm(body + click + thock))


def s_chapter(ev):
    n = _n(1.8)
    t = _tt(n)
    sd = _seed("chapter", ev["key"])
    imp = np.sin(_ph(_glide(95, 40, n, 0.3), n)) * _env(n, 0.002, 0.45)
    imp = np.tanh(1.5 * imp) / np.tanh(1.5)
    whoosh = _sweep_noise(n, 6000, 700, sd, 1.2, _env(n, 0.003, 0.3))
    whoosh = whoosh / (np.abs(whoosh).max() + 1e-9)
    chime = np.zeros(n)
    for dt, f, tau in ((0.0, 1318.5, 0.18), (0.07, 1975.5, 0.18), (0.14, 2637.0, 0.55)):
        m = n - _n(dt)
        tm = _tt(m)
        nt = np.zeros(m)
        for det in (1.0, 1.0017):
            ph = 2 * np.pi * f * det * tm
            nt += np.sin(ph) + 0.2 * np.sin(2 * ph) + 0.08 * np.sin(3 * ph)
        _put(chime, nt * _env(m, 0.002, tau), _n(dt))
    chime /= np.abs(chime).max()
    gl = np.zeros(n)
    _put(gl, s_glitch(ev), 0)
    x = 0.9 * imp + 0.35 * whoosh + 0.32 * chime + 0.22 * gl
    return _fade(_norm(x), 0.001, 0.05)


def s_rec_open(ev):
    n = _n(0.5)
    sd = _seed("rec", ev["key"])
    click = 0.6 * _hp(_noise(n, sd), 2000) * _env(n, 0.0, 0.003)
    step = int(SR / 3000)
    sh = np.repeat(_noise(n // step + 1, sd + 1), step)[:n]
    static = 0.5 * _bp(sh, 1000, 7000) * _env(n, 0.004, 0.09)
    bwip = 0.6 * np.sin(_ph(_glide(350, 1500, n, 0.12), n)) * _env(n, 0.003, 0.09)
    t = _tt(n)
    hum = 0.25 * _lp(signal.sawtooth(2 * np.pi * 110 * t), 1000) * _env(n, 0.02, 0.2)
    return _fade(_norm(click + static + bwip + hum))


def s_rec_switch(ev):
    n = _n(0.25)
    sd = _seed("recsw", ev["key"])
    step = int(SR / 3500)
    sh = np.repeat(_noise(n // step + 1, sd), step)[:n]
    x = _bp(sh, 1000, 7000) * _env(n, 0.003, 0.05) + 0.5 * _hp(_noise(n, sd + 1), 2000) * _env(n, 0.0, 0.003)
    return _fade(_norm(x))


def s_hud(ev):
    n = _n(0.2)
    x = np.zeros(n)
    for dt, f in ((0.0, 1760), (0.07, 2093)):
        m = _n(0.08)
        _put(x, np.sin(_ph(f, m)) * _env(m, 0.001, 0.015), _n(dt))
    return _fade(_norm(x))


def s_draw(ev):
    d = ev.get("dur", 1.4)
    n = _n(d + 0.15)
    t = _tt(n)
    env = np.clip(t / 0.1, 0, 1) * np.clip((d + 0.15 - t) / 0.25, 0, 1)
    tone = np.sin(_ph(_glide(330, 660, n, d), n))
    rng = np.random.default_rng(_seed("draw", ev["key"]))
    scale = np.array([1200, 1350, 1600, 1800, 2400])
    step = int(SR / 24)
    fr = np.repeat(rng.choice(scale, n // step + 1), step)[:n]
    chat = 0.35 * np.sin(_ph(fr, n)) * (0.5 + 0.5 * np.sign(np.sin(2 * np.pi * 24 * t)))
    return _fade(_norm(_lp((tone + chat) * env, 6000)))


def s_bar(ev):
    d = ev.get("dur", 0.55)
    n = _n(d + 0.2)
    k = np.clip(_tt(n) / d, 0, 1)
    env = np.sin(np.pi * np.minimum(k, 1) * 0.5 + 0.0) * np.exp(-np.clip(_tt(n) - d, 0, None) / 0.06)
    x = np.sin(_ph(_glide(260, 780, n, d), n)) * env
    _put(x, 0.6 * s_count_tick(2600), _n(d))
    return _fade(_norm(x))


def s_guide_pre(ev):
    """通常の予備（棚に音が無いときの合成音。blip に近い低めの音）"""
    return s_blip(dict(ev, key=("guide_pre", ev["key"])))


def s_guide_done(ev):
    """通常の完了（lockon に近い確定の音）"""
    return s_lockon(dict(ev, key=("guide_done", ev["key"])))


def s_guide_precise_pre(ev):
    """精密の予備: ピーピーピーの警告（同じ高さの短い音 3 つ）"""
    n = _n(0.62)
    x = np.zeros(n)
    for i in range(3):
        m = _n(0.11)
        b = np.tanh(1.6 * np.sin(_ph(1568, m))) * _env(m, 0.002, 0.05, 0.03)
        _put(x, b, _n(0.2 * i))
    return _fade(_norm(_lp(x, 6000)))


def s_guide_precise_done(ev):
    """精密の完了: lockon に低い響きを重ねた派手めの確定"""
    x = s_lockon(dict(ev, key=("guide_precise_done", ev["key"])))
    n = len(x)
    low = np.sin(_ph(_glide(330, 165, n, 0.12), n)) * _env(n, 0.002, 0.08)
    return _fade(_norm(x + 0.5 * _norm(low)))


SYNTH = {
    "tick": s_tick, "blip": s_blip, "lockon": s_lockon, "glitch": s_glitch, "glitch_s": s_glitch, "wipe": s_wipe,
    "move": s_move, "ring": s_ring, "zap": s_zap, "slam": s_slam, "chapter": s_chapter, "rec_open": s_rec_open,
    "rec_switch": s_rec_switch, "hud": s_hud, "draw": s_draw, "bar": s_bar,
}


def _count_ticks(ev):
    """数え上げの間のカチカチ。数字の進み（イージング）に合わせて間隔が開いていき、音程は少しずつ上がる。"""
    d, ease = ev.get("dur", 0.5), ev.get("ease") or ease_out
    N = int(np.clip(d * 22, 5, 14))
    ks = np.linspace(0, 1, 2001)
    es = np.array([ease(k) for k in ks])
    out = []
    for i in range(N):
        target = i / N
        k = ks[np.searchsorted(es, target)]
        out.append((ev["t"] + k * d, 2600 + 1400 * i / N))
    t_done = ev["t"] + ks[np.searchsorted(es, 0.97)] * d
    return out, t_done


# ======================================================================= 書き出す
def _ir(seed, length=0.6, tau=0.12, pre=0.012):
    n = _n(length)
    t = _tt(n)
    irs = []
    for ch in range(2):
        x = _noise(n, seed + ch) * np.exp(-t / tau)
        x = _lp(x, 6000)
        x[:_n(pre)] = 0
        irs.append(x / np.sqrt((x ** 2).sum()))
    return irs


SYNTH.update(confirm=s_confirm, confirm_neg=s_confirm_neg, guide_pre=s_guide_pre, guide_done=s_guide_done,
             guide_precise_pre=s_guide_precise_pre, guide_precise_done=s_guide_precise_done)
ALIGN_PEAK = {"chapter", "slam"}   # 素材のいちばん大きい所を、合成音のいちばん大きい所に合わせる（前ふり付きの音のため）


def _loud(m):
    """50ms 窓の最大実効値（音量をそろえる目安）。"""
    w = _n(0.05)
    if len(m) < w:
        return float(np.sqrt((m ** 2).mean()))
    k = len(m) // w
    return float(np.sqrt((m[:k * w].reshape(k, w) ** 2).mean(1)).max())


def bank_from_json(js, dirs):
    """素材の割り当て（bank.json の中身）から、種類ごとの素材の並びを作る。wav は dirs（先に見つかったほう）から読む。

    {"chapter": ["synth", "T12.wav"], "slam": ["synth", "H7.wav", "H9.wav"], ...}
    種類ごとに、出てくる順に順番に使う。"synth" は合成の音。"_" で始まるキーはメモ。リストでない値は読み飛ばす。"""
    cache, bank = {}, {}
    for kind, lst in js.items():
        if kind.startswith("_") or not isinstance(lst, list):
            continue
        pool = []
        for s in lst:
            if s == "synth":
                pool.append("synth")
            else:
                if s not in cache:
                    p = next((q for q in (os.path.join(d, s) for d in dirs) if os.path.exists(q)), os.path.join(dirs[0], s))
                    cache[s] = (os.path.splitext(s)[0], read_wav(p))
                pool.append(cache[s])
        bank[kind] = pool
    return bank


def load_bank(path):
    """素材の割り当て（se/bank.json）を読む。wav は bank.json と同じフォルダから読む。形は bank_from_json。"""
    with open(path, encoding="utf-8") as f:
        js = json.load(f)
    return bank_from_json(js, [os.path.dirname(os.path.abspath(path))])


def _pan_arr(pan, n):
    if np.ndim(pan) == 0:
        return np.full(n, float(pan))
    m = min(n, pan[2])
    return np.concatenate([np.linspace(pan[0], pan[1], m), np.full(max(0, n - m), pan[1])])


STEMS = [  # Premiere で種類ごとに音量を触れるように分けて書き出す
    ("1_title_stamp", "章タイトル・ハンコ", {"chapter", "slam"}),
    ("2_glitch", "グリッチ", {"glitch", "glitch_s"}),
    ("3_ui", "ピピッ・ポッ・カチッ・カウント", {"lockon", "blip", "tick", "hud", "count", "guide_pre", "guide_done", "guide_precise_pre",
                                        "guide_precise_done"}),
    ("4_motion", "シュッ・移動・攻撃の線・グラフ", {"wipe", "move", "zap", "draw", "bar"}),
    ("5_rec_ring", "録画の窓・波紋", {"rec_open", "rec_switch", "ring"}),
]


def render_se(ev, total, out_wav, profile="full", bank=None, t0=None, t1=None, wet_db=-14.0, gain_db=0.0,
              kinds=None):
    """候補 ev から profile の音を選んで、0 秒始まり・長さ total 秒のステレオ wav に書く。

    bank（load_bank の戻り値）があれば、その種類は素材を順番に鳴らす。素材の音量は、同じ種類の合成音に合わせる。
    kinds を渡すと、その種類だけを書く（STEMS の分け書き用。順番に使う素材の割り当ては全体と同じになる）。
    選んだ候補を返す（各候補の "src" に、鳴らした音の名前が入る）。"""
    sel = select(ev, profile, t0, t1)
    if kinds is not None:
        sel = [d for d in sel if d["kind"] in kinds]
    n = _n(total + 2.0)
    L, R = np.zeros(n), np.zeros(n)
    bank = bank or {}
    counters, refs = {}, {}

    def add_st(l, r, at, pan):
        th = (_pan_arr(pan, len(l)) + 1) * np.pi / 4
        i = _n(at) if at >= 0 else -_n(-at)
        _put(L, l * np.cos(th) * math.sqrt(2), i)
        _put(R, r * np.sin(th) * math.sqrt(2), i)

    def play(kind, d, at, pan):
        pool = bank.get(kind) or ["synth"]
        i = counters.get(kind, 0)
        counters[kind] = i + 1
        src = pool[i % len(pool)]
        if src == "synth":
            x = SYNTH[kind](d) * 10 ** ((LEVEL[kind] + gain_db) / 20)
            add_st(x, x, at, pan)
            return "synth"
        name, x2 = src
        rk = (kind, round(d.get("dur", 0.3), 2))
        if rk not in refs:
            refs[rk] = SYNTH[kind](dict(d, key=("ref", kind)))
        ref = refs[rk]
        g = _loud(ref) * 10 ** ((LEVEL[kind] + gain_db) / 20) / max(_loud(x2.mean(1)), 1e-9)
        if kind in ALIGN_PEAK:
            at += np.argmax(np.abs(ref)) / SR - np.argmax(np.abs(x2).max(1)) / SR
        add_st(x2[:, 0] * g, x2[:, 1] * g, at, pan)
        return name

    ticks, confirms = [], []
    for d in sel:
        k = d["kind"]
        if k == "count":
            tk, t_done = _count_ticks(d)
            ticks += [(t, f, _pan_x(d["x"])) for t, f in tk]
            confirms.append((t_done, d.get("neg", False), _pan_x(d["x"]), d))
            continue
        if isinstance(d["x"], tuple):
            pan = (_pan_x(d["x"][0]), _pan_x(d["x"][1]), _n(d.get("dur", 0.3)))
        else:
            pan = _pan_x(d["x"])
        d["src"] = play(k, d, d["t"], pan)
    # 同時に数える行が複数あっても、カチカチと確定音は1本にまとめる
    last = -1
    for t, f, p in sorted(ticks):
        if t - last >= 0.03:
            x = s_count_tick(f) * 10 ** ((LEVEL["count"] + gain_db) / 20)
            add_st(x, x, t, p)
            last = t
    last = -1
    for t, neg, p, d in sorted(confirms, key=lambda c: c[0]):
        if t - last >= 0.15:
            d["src"] = play("confirm_neg" if neg else "confirm", {"key": ("confirm", t)}, t, p)
            last = t
    # 空間（短い残響）で全体をなじませる
    irL, irR = _ir(7)
    wet = 10 ** (wet_db / 20)
    mono = (L + R) / 2
    L = L + wet * signal.oaconvolve(mono, irL)[:n]
    R = R + wet * signal.oaconvolve(mono, irR)[:n]
    st = np.stack([L, R], 1)
    peak = np.abs(st).max()
    if peak > 0.89:
        st = np.tanh(st / 0.89) * 0.89
    write_wav(out_wav, st)
    return sel


def render_all(ev, total, out_dir, name, profile="full", bank=None):
    """全部入り（<name>_se.wav）と、種類別（se_stems/<name>_se_<番号>.wav）を書く。全部入りの候補一覧を返す。"""
    sel = render_se(ev, total, os.path.join(out_dir, f"{name}_se.wav"), profile, bank)
    sd = os.path.join(out_dir, "se_stems")
    os.makedirs(sd, exist_ok=True)
    for key, _, kinds in STEMS:
        render_se(ev, total, os.path.join(sd, f"{name}_se_{key}.wav"), profile, bank, kinds=kinds)
    return sel


def write_wav(path, st):
    st = np.atleast_2d(st.T).T if st.ndim == 1 else st
    data = (np.clip(st, -1, 1) * 32767).astype(np.int16)
    with wave.open(path, "wb") as w:
        w.setnchannels(data.shape[1])
        w.setsampwidth(2)
        w.setframerate(SR)
        w.writeframes(data.tobytes())


def read_wav(path):
    with wave.open(path) as w:
        sr, ch = w.getframerate(), w.getnchannels()
        x = np.frombuffer(w.readframes(w.getnframes()), np.int16).astype(np.float64) / 32768
    x = x.reshape(-1, ch)
    if ch == 1:
        x = np.repeat(x, 2, axis=1)
    if sr != SR:
        x = signal.resample_poly(x, SR, sr, axis=0)
    return x


def mix(voice_wav, se_wav, out_wav, voice_db=-1.0, se_db=0.0):
    """確認用: セリフと SE を重ねた wav（ピークは軽く抑える）。"""
    v, s = read_wav(voice_wav), read_wav(se_wav)
    n = max(len(v), len(s))
    m = np.zeros((n, 2))
    m[:len(v)] += v * 10 ** (voice_db / 20)
    m[:len(s)] += s * 10 ** (se_db / 20)
    over = np.abs(m) > 0.9
    m[over] = np.sign(m[over]) * (0.9 + 0.1 * np.tanh((np.abs(m[over]) - 0.9) / 0.1))
    write_wav(out_wav, m)
    return out_wav




# ======================================================================= 効果音の設定（se/bank.json）の管理
# AK Studio の「効果音の設定」の裏側。ワーカー（akfx_worker.py）は SeCtx を作って、ここの関数を呼ぶだけ。
#
# se/bank.json は load_bank と同じ形に、AK Studio の「効果音の設定」で広げて使う。
#   {"slam": ["H7.wav"],                       ← 種類ごとに鳴らす音（今まで通り。複数なら出てくる順に順番に使う）
#    "_studio": {"cands": {"slam": ["H9.wav"]}, ← 鳴らさないが候補として並べておく音
#                "db": {"slam": {"H7.wav": -3, "synth": 2}}}}   ← 候補ごとの音量（標準の大きさからの dB）
# "_" で始まるキーは load_bank が読み飛ばすので、fxse を直接使っても読める（音量の dB だけは効かない）。
# 候補の音は se/ の下の .wav（16bit PCM）。se/<種類>/ に置いた .wav は、その種類の候補に自動で出る。
#
# 置き場所は 2 つ。回の fx/se/（書き込み先。あれば回のほうが勝つ）と、kit の既定の棚 default_dir（ak_kit.SE_DIR。読むだけ）。
# 回に bank.json が無いときは既定の棚の bank.json を元にし、候補の wav は回に無ければ既定の棚から読む。素の kit は default_dir が None（空の bank）。
# 攻略動画の操作の 4 つの種類（SHELF_KINDS）の音は、回の se/ ではなくアプリの効果音の棚（shelf_dir。全部の回で使う）から読む。
# どれを鳴らすか・音量は settings.json の guide_se（shelf_sel: {種類: {"src": "<種類>/<名前>.wav" か "synth", "db": 数}}）。
# 回の bank.json にその種類の並びがあれば回のほうが勝つ。
SE_DB_RANGE = (-30.0, 24.0)


class SeError(ValueError):
    """効果音の設定の誤り（トレースバックを付けずに画面へ返す）。"""


class SeCtx:
    """効果音の設定を扱う 1 回分の前提。

    fx_dir: 回の fx/（se/bank.json の保存先）。default_dir: kit の既定の棚（無ければ None）。
    shelf_dir・shelf_sel: アプリの効果音の棚の場所と、種類ごとの選び（無ければ棚は使わない）。
    label(kind) -> 表示名（無ければ種類の名前）。events() -> この回で鳴る候補 [{"t", "kind", …}]（無ければ None。間引く前）。
    shelf_warn: 棚の音を使えなかったときの警告（作り直しの返事に付ける）"""

    def __init__(self, fx_dir, default_dir=None, shelf_dir=None, shelf_sel=None, label=None, events=None):
        self.fx_dir = os.path.abspath(fx_dir)
        self.default_dir = os.path.abspath(default_dir) if default_dir else None
        self.shelf_dir = os.path.abspath(shelf_dir) if shelf_dir else None
        self.shelf_sel = shelf_sel if isinstance(shelf_sel, dict) else {}
        self.label = label or (lambda kind: kind)
        self.events = events
        self.shelf_warn = []

    @property
    def se_dir(self):
        return os.path.join(self.fx_dir, "se")

    @property
    def bank_path(self):
        return os.path.join(self.se_dir, "bank.json")


def _bytes(p):
    """ファイルの中身のバイト列（無ければ None）。"""
    try:
        with open(p, "rb") as f:
            return f.read()
    except FileNotFoundError:
        return None


def bank_load(ctx):
    """bank.json を読む。(中身, 読んだバイト列（回の bank.json が無ければ None）)。バイト列は bank_write で外の変更に気づくのに使う。
    回に無ければ、kit の既定の棚の bank.json（あれば）を元にする（棚の種類の並びは回ごとの決まりなので持ち込まない）。"""
    raw = _bytes(ctx.bank_path)
    if raw is None:
        raw_d = _bytes(os.path.join(ctx.default_dir, "bank.json")) if ctx.default_dir else None
        if raw_d is None:
            return {}, None
        try:
            js = json.loads(raw_d.decode("utf-8"))
        except ValueError as e:
            raise SeError(f"既定の効果音の設定（{os.path.join(ctx.default_dir, 'bank.json')}）を読めませんでした: {e}")
        js = js if isinstance(js, dict) else {}
        return {k: v for k, v in js.items() if k not in SHELF_KINDS}, None
    js = json.loads(raw.decode("utf-8"))
    return (js if isinstance(js, dict) else {}), raw


def bank_read(ctx):
    return bank_load(ctx)[0]


def bank_write(ctx, js, seen):
    """bank.json に原子的に書く。読んだ後（seen）に外で書き換えられていたら、外の変更を消さないように書かずに SeError。"""
    os.makedirs(ctx.se_dir, exist_ok=True)
    p = ctx.bank_path
    tmp = p + ".tmp"
    with open(tmp, "w", encoding="utf-8") as f:
        json.dump(js, f, ensure_ascii=False, indent=1)
    if _bytes(p) != seen:
        os.remove(tmp)
        raise SeError("効果音の設定（se/bank.json）が保存の途中で外で変更されたので、保存しませんでした。もう一度操作してください")
    os.replace(tmp, p)


def _studio(js):
    st = js.get("_studio")
    st = st if isinstance(st, dict) else {}
    cands = st.get("cands") if isinstance(st.get("cands"), dict) else {}
    db = st.get("db") if isinstance(st.get("db"), dict) else {}
    return cands, db


def _is_shelf(ctx, js, k):
    """種類 k の素材は棚から読むか（棚の種類で、棚の場所が分かっていて、回の bank.json に無い）。"""
    return k in SHELF_KINDS and ctx.shelf_dir is not None and not isinstance(js.get(k), list)


def _dirs(ctx, shelf=False):
    """候補（相対パス）を探すフォルダ（先に見つかったほう）。棚の種類は棚だけ。"""
    if shelf:
        return [ctx.shelf_dir]
    return [ctx.se_dir] + ([ctx.default_dir] if ctx.default_dir else [])


def _path(ctx, src, shelf=False):
    """候補 src の実際のファイル（回の se/ → 既定の棚の順。どちらにも無ければ回の se/ の下の場所）。"""
    ds = _dirs(ctx, shelf)
    for d in ds:
        p = os.path.join(d, src)
        if os.path.exists(p):
            return p
    return os.path.join(ds[0], src)


def _shelf_sel(ctx, k):
    """棚の種類 k の選び（src, db）。選んでいなければ（"synth", 0.0）。"""
    sel = ctx.shelf_sel.get(k)
    if not isinstance(sel, dict) or not isinstance(sel.get("src"), str) or not sel["src"]:
        return "synth", 0.0
    try:
        db = float(sel.get("db") or 0.0)
    except (TypeError, ValueError):
        db = 0.0
    return sel["src"], (db if db == db else 0.0)


def _shelf_files(ctx, kind):
    """棚の <種類>/ の下の .wav（相対パス、区切りは /）。"""
    base = os.path.join(ctx.shelf_dir, kind) if ctx.shelf_dir else None
    if not base or not os.path.isdir(base):
        return []
    return [f"{kind}/{fn}" for fn in sorted(os.listdir(base)) if fn.lower().endswith(".wav")]


def _src_ok(src, base):
    """候補の名前（"synth" か base の下の相対パスの .wav）か。base の外は指せない。"""
    if src == "synth":
        return True
    if not isinstance(src, str) or not src.lower().endswith(".wav"):
        return False
    base = os.path.normcase(os.path.abspath(base))
    p = os.path.normcase(os.path.abspath(os.path.join(base, src)))
    return p.startswith(base + os.sep)


def _src_ok_ctx(ctx, src):
    """回の se/ か既定の棚の下の候補か。"""
    return any(_src_ok(src, d) for d in _dirs(ctx))


def wav_bad(path):
    """read_wav で読めない .wav なら、その理由（16bit PCM だけ読める）。読めるなら None。"""
    try:
        with wave.open(path) as w:
            if w.getsampwidth() != 2:
                return f"{w.getsampwidth() * 8}bit の wav は使えません（16bit PCM で書き出し直してください）"
            if w.getnframes() <= 0:
                return "音が入っていない wav です"
    except FileNotFoundError:
        return "ファイルがありません"
    except Exception as e:  # 浮動小数点の wav などは wave が開けない
        return f"読めない wav です（使えるのは 16bit PCM だけです）: {e}"
    return None


def _walk_wavs(base):
    out = []
    if not os.path.isdir(base):
        return out
    for root, dirs, files in os.walk(base):
        dirs[:] = sorted(d for d in dirs if not d.startswith((".", "_")))
        for fn in sorted(files):
            if fn.lower().endswith(".wav"):
                out.append(os.path.relpath(os.path.join(root, fn), base).replace(os.sep, "/"))
    return out


def se_files(ctx):
    """回の se/ と既定の棚の下の .wav（相対パス、区切りは /。同じ名前は 1 つ）。"""
    out = []
    for d in _dirs(ctx):
        out += [f for f in _walk_wavs(d) if f not in out]
    return out


class _SeLevel(dict):
    """LEVEL の代わり。鳴らす直前に選ばれた候補の音量（dB）を足して返す。
    （render_se は LEVEL[種類] で音の大きさを決めるので、ここで候補ごとの音量を足す）"""

    def __init__(self, base, db):
        super().__init__(base)
        self.db = db
        self.cur = {}

    def __getitem__(self, k):
        add = self.cur.get(k)
        if add is None:
            add = (self.db.get(k) or {}).get("synth", 0.0)
        return dict.__getitem__(self, k) + float(add or 0.0)


class _SePool(list):
    """種類ごとの素材の並び（load_bank の戻り値の 1 つ）。取り出されたら、その候補の音量を _SeLevel に知らせる。"""

    def __init__(self, items, names, kind, level):
        super().__init__(items)
        self.names, self.kind, self.level = list(names), kind, level

    def __getitem__(self, i):
        x = list.__getitem__(self, i)
        if isinstance(i, int) and self.names:
            nm = self.names[i % len(self.names)]
            self.level.cur[self.kind] = float((self.level.db.get(self.kind) or {}).get(nm, 0.0) or 0.0)
        return x


def build_bank(ctx, override=None, used=None):
    """書き出しに使う bank と、候補ごとの音量を入れた LEVEL を作る。
    override: {"kind": 種類, "src": 候補} のとき、その種類だけをその候補 1 つ・0 dB にする（試聴用）。
    used: 鳴らす種類の集合。棚の種類は、これに入っているものだけ棚から読む（None は全部。攻略動画でない回で棚の wav を毎回読まない・警告も出さない）"""
    js = bank_read(ctx)
    _, db = _studio(js)
    db = {k: dict(v) for k, v in db.items() if isinstance(v, dict)}
    raw = bank_from_json(js, _dirs(ctx))
    names = {k: list(v) for k, v in js.items() if not k.startswith("_") and isinstance(v, list)}
    # 棚の種類（回の bank.json に無いもの）は、設定の選びの音を棚から読む。読めなければ合成の音で鳴らして警告に出す
    for k in SHELF_KINDS:
        if not _is_shelf(ctx, js, k) or (used is not None and k not in used):
            continue
        src, kdb = _shelf_sel(ctx, k)
        raw.pop(k, None)
        db[k] = {src: kdb} if kdb else {}
        if src == "synth":
            continue
        path = os.path.join(ctx.shelf_dir, src)
        bad = None if _src_ok(src, ctx.shelf_dir) else "棚の外は指せません"
        bad = bad or wav_bad(path)
        if bad:
            msg = f"効果音の棚の {src} を使えないので、合成の音で鳴らします: {bad}"
            if msg not in ctx.shelf_warn:
                ctx.shelf_warn.append(msg)
            db[k] = {}
            continue
        raw[k] = [(os.path.splitext(os.path.basename(src))[0], read_wav(path))]
        names[k] = [src]
    if override:
        k, src = override["kind"], override["src"]
        if src == "synth":
            raw[k] = ["synth"]
        else:
            raw[k] = [(os.path.splitext(os.path.basename(src))[0],
                       read_wav(_path(ctx, src, _is_shelf(ctx, js, k))))]
        names[k] = [src]
        db[k] = {src: 0.0}
    level = _SeLevel(LEVEL, db)
    bank = {k: _SePool(pool, names.get(k, []), k, level) for k, pool in raw.items()}
    return bank, level


class _LevelPatch:
    """書き出しの間だけ LEVEL を差し替える（render_se は LEVEL をモジュールの変数から読む）。"""

    def __init__(self, level):
        self.level = level

    def __enter__(self):
        global LEVEL
        self.old = LEVEL
        LEVEL = self.level

    def __exit__(self, *a):
        global LEVEL
        LEVEL = self.old


def _clean_tmp(out_dir, older_than=3600):
    """前の作り直しが途中で止まって残った .se_tmp_*（ほかのプロセスが作業中のものは消さないよう、古いものだけ）。"""
    now = time.time()
    try:
        names = os.listdir(out_dir)
    except OSError:
        return
    for n in names:
        p = os.path.join(out_dir, n)
        if not n.startswith(".se_tmp_") or not os.path.isdir(p):
            continue
        try:
            if now - os.path.getmtime(p) > older_than:
                shutil.rmtree(p, ignore_errors=True)
        except OSError:
            pass


def render(ctx, ev, total, out_dir, name, profile="full"):
    """候補 ev から効果音を作り直す（全部入り <name>_se.wav と、種類別 se_stems/）。{wav, count, events, warnings} を返す。
    途中で止められても前の wav を壊さないよう、一時フォルダに作ってから置き換える（全部入りを最後に）。"""
    bank, level = build_bank(ctx, used={d["kind"] for d in ev})
    os.makedirs(out_dir, exist_ok=True)
    _clean_tmp(out_dir)
    tmp = os.path.join(out_dir, f".se_tmp_{os.getpid()}")
    shutil.rmtree(tmp, ignore_errors=True)
    os.makedirs(tmp)
    try:
        with _LevelPatch(level):
            sel = render_all(ev, total, tmp, name, profile, bank)
        # アプリが使う全部入りを先に置く（種類別は Premiere 用）。ほかで開いていて置き換えられないときは理由を出す
        pairs = [(os.path.join(tmp, f"{name}_se.wav"), os.path.join(out_dir, f"{name}_se.wav"))]
        stems = os.path.join(tmp, "se_stems")
        if os.path.isdir(stems):
            dst = os.path.join(out_dir, "se_stems")
            os.makedirs(dst, exist_ok=True)
            pairs += [(os.path.join(stems, fn), os.path.join(dst, fn)) for fn in sorted(os.listdir(stems))]
        for a, b in pairs:
            try:
                os.replace(a, b)
            except PermissionError as e:
                raise SeError(f"効果音の wav を置き換えられませんでした（ほかのアプリで開いていないか確かめてください）: {b} ({e})")
    finally:
        shutil.rmtree(tmp, ignore_errors=True)
    return {"wav": os.path.join(out_dir, f"{name}_se.wav"), "count": len(sel),
            "events": [{"t": round(d["t"], 3), "kind": d["kind"]} for d in sel], "warnings": list(ctx.shelf_warn)}


def bank_info(ctx):
    """効果音の設定の一覧: 種類ごとの候補・鳴らす音・音量・この回で鳴る時刻（ak_proto の SeBank と同じ形）。"""
    js = bank_read(ctx)
    cands_x, db = _studio(js)
    files = se_files(ctx)
    sources = js.get("_sources") if isinstance(js.get("_sources"), dict) else {}
    # この回で鳴る時刻（間引く前の候補。数え上げの確定音は数え上げの時刻）
    times = {}
    if ctx.events is not None:
        try:
            for d in ctx.events() or []:
                times.setdefault(d["kind"], []).append(round(d["t"], 3))
        except Exception:
            times = {}
    stems = {}
    for key, label, kinds in STEMS:
        for k in kinds:
            stems[k] = f"{key}（{label}）"
    lite = PROFILES.get("lite", set())
    out = []
    for k in LEVEL:
        bankable = k in SYNTH
        shelf = _is_shelf(ctx, js, k)
        listed = js.get(k) if isinstance(js.get(k), list) else None
        if shelf:
            # 棚の種類: 選びは settings.json の guide_se、候補は棚の <種類>/ の .wav（全部の回で同じ）
            sel_src, sel_db = _shelf_sel(ctx, k)
            use = [sel_src]
            listed = [sel_src] if sel_src != "synth" else None
            cl = ["synth"] + [f for f in [sel_src] + _shelf_files(ctx, k) if f != "synth"]
            cl = list(dict.fromkeys(cl))
            kdb = {sel_src: sel_db}
            extra = set()
        else:
            use = [s for s in (listed or []) if isinstance(s, str)] if bankable else []
            if not use:
                use = ["synth"]
            cl = ["synth"]
            if bankable:
                for s in use + list(cands_x.get(k) or []) + [f for f in files if f.startswith(k + "/")]:
                    if isinstance(s, str) and s not in cl:
                        cl.append(s)
            kdb = db.get(k) if isinstance(db.get(k), dict) else {}
            extra = set(cands_x.get(k) or []) if bankable else set()
        out.append({
            "kind": k, "label": ctx.label(k), "level": LEVEL[k], "prio": PRIO.get(k, 0), "bankable": bankable, "lite": k in lite,
            "stem": stems.get(k, ""), "use": use, "explicit": listed is not None, "shelf": shelf,
            "times": sorted(times.get("count" if k.startswith("confirm") else k, [])),
            "cands": [{"src": s, "db": float(kdb.get(s, 0) or 0),
                       "exists": s == "synth" or os.path.exists(_path(ctx, s, shelf)),
                       "bad": (None if s == "synth" else wav_bad(_path(ctx, s, shelf))),
                       "extra": s in extra, "folder": s.startswith(k + "/"),
                       "source": sources.get(s) or sources.get(os.path.basename(s)) or ""} for s in cl],
        })
    out.sort(key=lambda x: (-x["prio"], x["kind"]))
    return {"path": ctx.bank_path, "exists": bool(js), "files": files, "shelf_dir": ctx.shelf_dir,
            "db_range": list(SE_DB_RANGE), "kinds": out}


def bank_set(ctx, kinds):
    """種類ごとの設定を変えて se/bank.json に書く。変えたあとの bank_info を返す。
    kinds: {種類: {"use": [候補, …], "db": {候補: dB か null}, "add": [候補], "remove": [候補]}}（変えるものだけ）"""
    # 読み → 変える → 書くをこの 1 回で済ませる（外で直したほかの種類・項目は残る）。書く直前に読んだ版と比べる
    js, seen = bank_load(ctx)
    cands_x, db = _studio(js)
    cands_x = {k: list(v) for k, v in cands_x.items() if isinstance(v, list)}
    db = {k: dict(v) for k, v in db.items() if isinstance(v, dict)}
    if not isinstance(kinds, dict):
        raise SeError("kinds は {種類: {use, db, add, remove}} の形で指定してください")
    # 先に全部を確かめてから書く（途中で誤りがあっても bank.json は変えない）
    for k, patch in kinds.items():
        if k not in LEVEL:
            raise SeError(f"不明な効果音の種類です: {k}")
        if patch is None:
            continue
        if not isinstance(patch, dict):
            raise SeError(f"{k} の設定は {{use, db, add, remove}} の形で指定してください")
        for f in ("use", "add", "remove"):
            if f in patch and not (patch[f] is None or isinstance(patch[f], list)):
                raise SeError(f"{k}.{f} は候補の並び（[\"H9.wav\", …]）で指定してください")
        if "db" in patch and not (patch["db"] is None or isinstance(patch["db"], dict)):
            raise SeError(f"{k}.db は {{候補: dB}} の形で指定してください")
        for s, v in (patch.get("db") or {}).items():
            if v is not None and (isinstance(v, bool) or not isinstance(v, (int, float)) or v != v):
                raise SeError(f"{k}.db の {s} は数（dB）か null で指定してください: {v!r}")
        bankable = k in SYNTH
        for s in list(patch.get("use") or []) + list(patch.get("add") or []) + list((patch.get("db") or {}).keys()):
            if not _src_ok_ctx(ctx, s):
                raise SeError(f"候補は \"synth\" か se/ の下の .wav にしてください: {s}")
            if s != "synth" and not bankable:
                raise SeError(f"{k} は合成の音だけです（音量だけ変えられます）")
            if s != "synth" and not os.path.exists(_path(ctx, s)):
                raise SeError(f"ファイルがありません: se/{s}")
        for s in list(patch.get("use") or []) + list(patch.get("add") or []):
            bad = s != "synth" and wav_bad(_path(ctx, s))
            if bad:
                raise SeError(f"se/{s}: {bad}")
    for k, patch in kinds.items():
        patch = patch or {}
        bankable = k in SYNTH
        if "add" in patch or "remove" in patch:
            lst = cands_x.get(k, [])
            for s in patch.get("add") or []:
                if s != "synth" and s not in lst:
                    lst.append(s)
            rm = set(patch.get("remove") or []) - {"synth"}
            lst = [s for s in lst if s not in rm]
            if lst:
                cands_x[k] = lst
            else:
                cands_x.pop(k, None)
            # 外した候補は鳴らす音・音量からも外す
            if rm and isinstance(js.get(k), list):
                js[k] = [s for s in js[k] if s not in rm]
                if not js[k]:
                    js.pop(k)
            for s in rm:
                (db.get(k) or {}).pop(s, None)
        if "use" in patch:
            use = [s for s in (patch.get("use") or []) if isinstance(s, str)]
            old_use = js.get(k) if isinstance(js.get(k), list) else []
            if not bankable or not use or use == ["synth"]:
                js.pop(k, None)    # 合成の音だけ（既定）
            else:
                js[k] = use
            # 鳴らすのをやめた音も候補の一覧には残す（選び直せるように）。鳴らす音と重なる分は cands に持たない
            now = js.get(k) if isinstance(js.get(k), list) else []
            lst = cands_x.get(k, [])
            for s in old_use:
                if isinstance(s, str) and s != "synth" and s not in now and s not in lst:
                    lst.append(s)
            lst = [s for s in lst if s not in now]
            if lst:
                cands_x[k] = lst
            else:
                cands_x.pop(k, None)
        if "db" in patch:
            kdb = db.get(k, {})
            for s, v in (patch.get("db") or {}).items():
                if v is None or abs(float(v)) < 1e-6:
                    kdb.pop(s, None)
                else:
                    kdb[s] = round(max(SE_DB_RANGE[0], min(SE_DB_RANGE[1], float(v))), 2)
            db[k] = kdb
        if not db.get(k, True):
            db.pop(k, None)
    st = js.get("_studio") if isinstance(js.get("_studio"), dict) else {}
    st["_note"] = ("AK Studio の「効果音の設定」。cands は鳴らさない候補、db は候補ごとの音量"
                   "（標準の大きさからの dB。AK Studio の書き出しで効く）")
    st["cands"], st["db"] = cands_x, db
    if not cands_x and not db:
        js.pop("_studio", None)
    else:
        js["_studio"] = st
    bank_write(ctx, js, seen)
    return bank_info(ctx)


# 試聴の 1 回分の長さ（dur を使う種類）
SE_AUDITION_DUR = {"wipe": 0.35, "move": 0.5, "draw": 1.4, "bar": 0.55, "count": 0.8}


def audition(ctx, kind, src, out_wav):
    """1 つの候補を 0 dB（標準の大きさ）で鳴らした wav を書く。候補ごとの音量は再生側でかける。"""
    if kind not in LEVEL:
        raise SeError(f"不明な効果音の種類です: {kind}")
    shelf = _is_shelf(ctx, bank_read(ctx), kind)
    ok = _src_ok(src, ctx.shelf_dir) if shelf else _src_ok_ctx(ctx, src)
    if not ok:
        raise SeError(f"候補は \"synth\" か {'効果音の棚' if shelf else 'se/'} の下の .wav にしてください: {src}")
    bankable = kind in SYNTH
    if src != "synth" and not bankable:
        raise SeError(f"{kind} は合成の音だけです")
    bad = src != "synth" and wav_bad(_path(ctx, src, shelf))
    if bad:
        raise SeError(f"{src}: {bad}")
    bank, level = build_bank(ctx, override={"kind": kind, "src": src} if bankable else None, used=())
    if not bankable:
        level.db[kind] = {"synth": 0.0}   # 数え上げのカチカチ（確定音は今の設定のまま鳴る）
    t0 = 0.15
    d = {"t": t0, "kind": kind, "x": STAGE_CX, "key": ("audition", kind, src), "neg": False}
    if kind in SE_AUDITION_DUR:
        d["dur"] = SE_AUDITION_DUR[kind]
    total = t0 + d.get("dur", 0.3) + 1.6
    os.makedirs(os.path.dirname(os.path.abspath(out_wav)), exist_ok=True)
    with _LevelPatch(level):
        render_se([d], total, out_wav, {kind}, bank)
    return {"wav": out_wav, "kind": kind, "src": src}
