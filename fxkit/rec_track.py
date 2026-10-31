#!/usr/bin/env python3
"""
録画トラックの書き出し（Premiere で録画トラックだけを書き出した mp4）から、画面上の1つの物（敵など）の位置を追う。
FocusMark（追従するマーキング）に渡す軌跡 JSON を作る。

    python rec_track.py ../material_track.mp4 --t0 344.9 --t1 351.4 --box 1555 150 90 110 -o ../files/tracks/rec06_robot.json

--box は t0 のコマで追う物を囲む枠（画面の座標 x y w h、1920x1080）。確認画像（--check、既定で出す）で
枠が最後まで物に乗っているかを必ず目で見る。score が 0.5 を切るコマが続くなら、枠を小さく（体だけに）するか区間を分ける。

時刻はオーバーレイと同じ 0 秒始まり（イントロのぶんを抜いて書き出してもらう。抜いていなければ --offset で引く）。
出力: {"src", "fps", "box": [w, h], "points": [[t, cx, cy, score], ...]}（中心の座標。軽く平滑化済み）
OpenCV（pip install "opencv-python-headless<4.11"。4.11 以降は numpy 2 を入れて scipy を壊すので避ける）。
"""

import argparse
import json
import os
import subprocess

import cv2
import imageio_ffmpeg
import numpy as np
from PIL import Image, ImageDraw

FFMPEG = imageio_ffmpeg.get_ffmpeg_exe()


def read_frames(src, t0, t1, crop, fps):
    x, y, w, h = crop
    cmd = [FFMPEG, "-loglevel", "error", "-ss", str(t0), "-t", str(t1 - t0), "-i", src,
           "-vf", f"fps={fps},crop={w}:{h}:{x}:{y}", "-f", "rawvideo", "-pix_fmt", "rgb24", "-"]
    raw = subprocess.run(cmd, capture_output=True, check=True).stdout
    return np.frombuffer(raw, np.uint8).reshape(-1, h, w, 3)


def track(frames, box, search=40, learn=0.1, keep=0.6):
    """テンプレート照合で追う。照合がよいコマ（score > keep）だけ少しずつテンプレートを更新する（エフェクトに強くする）。"""
    bx, by, bw, bh = box
    gray = [cv2.cvtColor(f, cv2.COLOR_RGB2GRAY) for f in frames]
    H, W = gray[0].shape
    tmpl = gray[0][by:by + bh, bx:bx + bw].astype(np.float32)
    px, py = bx, by
    out = []
    for g in gray:
        sx0, sy0 = max(0, px - search), max(0, py - search)
        sx1, sy1 = min(W, px + bw + search), min(H, py + bh + search)
        res = cv2.matchTemplate(g[sy0:sy1, sx0:sx1], tmpl.astype(np.uint8), cv2.TM_CCOEFF_NORMED)
        _, score, _, loc = cv2.minMaxLoc(res)
        px, py = sx0 + loc[0], sy0 + loc[1]
        if score > keep:
            tmpl = (1 - learn) * tmpl + learn * g[py:py + bh, px:px + bw]
        out.append((px + bw / 2, py + bh / 2, float(score)))
    return out


def smooth(v, k=5):
    if len(v) < k:
        return np.asarray(v, float)
    pad = k // 2
    vv = np.pad(np.asarray(v, float), pad, mode="edge")
    return np.convolve(vv, np.ones(k) / k, mode="valid")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("src")
    ap.add_argument("--t0", type=float, required=True)
    ap.add_argument("--t1", type=float, required=True)
    ap.add_argument("--box", type=int, nargs=4, required=True, metavar=("X", "Y", "W", "H"))
    ap.add_argument("--offset", type=float, default=0.0, help="書き出しの頭にイントロが残っているときの秒数")
    ap.add_argument("--fps", type=int, default=30)
    ap.add_argument("--margin", type=int, default=260, help="追う範囲（枠のまわり何 px まで読むか）")
    ap.add_argument("-o", "--out", required=True)
    ap.add_argument("--no-check", action="store_true")
    a = ap.parse_args()

    x, y, w, h = a.box
    cx0, cy0 = max(0, x - a.margin), max(0, y - a.margin)
    cx1, cy1 = min(1920, x + w + a.margin), min(1080, y + h + a.margin)
    crop = (cx0, cy0, cx1 - cx0, cy1 - cy0)
    frames = read_frames(a.src, a.t0 + a.offset, a.t1 + a.offset, crop, a.fps)
    tr = track(frames, (x - cx0, y - cy0, w, h))
    xs, ys = smooth([p[0] for p in tr]), smooth([p[1] for p in tr])
    pts = [[round(a.t0 + i / a.fps, 4), round(float(xs[i]) + cx0, 1), round(float(ys[i]) + cy0, 1), round(tr[i][2], 3)]
           for i in range(len(tr))]
    os.makedirs(os.path.dirname(os.path.abspath(a.out)), exist_ok=True)
    json.dump({"src": os.path.basename(a.src), "fps": a.fps, "box": [w, h], "points": pts},
              open(a.out, "w", encoding="utf-8"), indent=0)
    low = [p for p in pts if p[3] < 0.5]
    print(a.out, f"{len(pts)} コマ、score 最小 {min(p[3] for p in pts):.2f}" + (f"、0.5 未満 {len(low)} コマ" if low else ""))

    if not a.no_check:
        n = 9
        idx = [round(i * (len(frames) - 1) / (n - 1)) for i in range(n)]
        cw, ch = crop[2], crop[3]
        sc = min(1.0, 640 / cw)
        tw, th = int(cw * sc), int(ch * sc)
        sheet = Image.new("RGB", (tw * 3, th * 3))
        for k, i in enumerate(idx):
            im = Image.fromarray(frames[i])
            d = ImageDraw.Draw(im)
            t, px, py, s = pts[i]
            d.rectangle((px - cx0 - w / 2, py - cy0 - h / 2, px - cx0 + w / 2, py - cy0 + h / 2), outline=(0, 255, 255),
                        width=3)
            d.rectangle((0, 0, 150, 18), fill=(0, 0, 0))
            d.text((4, 4), f"{t:.2f}s  score {s:.2f}", fill=(255, 255, 0))
            sheet.paste(im.resize((tw, th)), ((k % 3) * tw, (k // 3) * th))
        p = os.path.splitext(a.out)[0] + "_check.png"
        sheet.save(p)
        print(p)


if __name__ == "__main__":
    main()
