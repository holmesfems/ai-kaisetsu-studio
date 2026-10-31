"""GitHub のソーシャルプレビュー（1280x640）を作る。

使い方: python tools/social_preview.py docs/images/main.png docs/images/social-preview.png
フォントは Windows の Noto Sans JP（C:/Windows/Fonts/NotoSansJP-VF.ttf）。
"""
import sys
from PIL import Image, ImageDraw, ImageFilter, ImageFont

SHOT, OUT = sys.argv[1], sys.argv[2]
W, H, S = 1280, 640, 2  # 2 倍で描いて縮める
BG, PANEL, TEXT, DIM, ACCENT = (24, 24, 24), (38, 38, 38), (230, 230, 230), (150, 150, 150), (45, 140, 235)


def jp(size, wght):
    f = ImageFont.truetype("C:/Windows/Fonts/NotoSansJP-VF.ttf", size * S)
    try:
        f.set_variation_by_axes([wght])
    except Exception:
        pass
    return f


im = Image.new("RGB", (W * S, H * S), BG)
d = ImageDraw.Draw(im)

# 右: 画面のスクリーンショット（右下にはみ出させる）
shot = Image.open(SHOT).convert("RGB")
sw = 900 * S
shot = shot.resize((sw, int(shot.height * sw / shot.width)), Image.LANCZOS)
sx, sy = 606 * S, 150 * S
mask = Image.new("L", shot.size, 0)
ImageDraw.Draw(mask).rounded_rectangle([0, 0, shot.width - 1, shot.height - 1], 14 * S, fill=255)
shadow = Image.new("L", (im.width, im.height), 0)
ImageDraw.Draw(shadow).rounded_rectangle([sx, sy + 12 * S, sx + shot.width, sy + shot.height + 12 * S], 14 * S, fill=170)
shadow = shadow.filter(ImageFilter.GaussianBlur(28 * S))
im.paste((0, 0, 0), (0, 0), shadow)
d = ImageDraw.Draw(im)
d.rounded_rectangle([sx - 2 * S, sy - 2 * S, sx + shot.width + 2 * S, sy + shot.height + 2 * S], 16 * S, fill=(70, 70, 70))
im.paste(shot, (sx, sy), mask)

# 左: 文字
x = 72 * S
d.rounded_rectangle([x, 92 * S, x + 64 * S, 156 * S], 12 * S, fill=ACCENT)
d.text((x + 32 * S, 124 * S), "AK", font=jp(28, 900), fill=(255, 255, 255), anchor="mm")
d.text((x, 178 * S), "AK Studio", font=jp(64, 900), fill=TEXT)
d.text((x, 262 * S), "AI Kaisetsu Studio", font=jp(26, 600), fill=ACCENT)
for i, line in enumerate(["解説動画の台本・声・演出・合成を", "ひとつのアプリで、Claude と一緒に。"]):
    d.text((x, (330 + i * 44) * S), line, font=jp(27, 700), fill=TEXT)

# 技術のチップ
cx, cy = x, 452 * S
for word in ["Tauri v2", "Rust", "React", "Python", "MCP"]:
    f = jp(18, 600)
    tw = d.textlength(word, font=f)
    w = tw + 28 * S
    if cx + w > 520 * S:
        cx, cy = x, cy + 44 * S
    d.rounded_rectangle([cx, cy, cx + w, cy + 34 * S], 17 * S, fill=PANEL, outline=(70, 70, 70), width=S)
    d.text((cx + w / 2, cy + 17 * S), word, font=f, fill=DIM, anchor="mm")
    cx += w + 10 * S

d.text((x, 572 * S), "github.com/holmesfems/ai-kaisetsu-studio", font=jp(17, 500), fill=DIM)
im.resize((W, H), Image.LANCZOS).save(OUT, optimize=True)
print(OUT)
