"""GitHub のソーシャルプレビュー（1280x640）を作る。

使い方: python tools/social_preview.py docs/images/main.png docs/images/chat.png docs/images/social-preview.png
フォントは Windows の Noto Sans JP（C:/Windows/Fonts/NotoSansJP-VF.ttf）。
"""
import sys
from PIL import Image, ImageDraw, ImageFilter, ImageFont

SHOT, CHAT, OUT = sys.argv[1], sys.argv[2], sys.argv[3]
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

def card(img, x, y, w, dim=0.0):
    """スクリーンショットを角丸・影・縁つきで置く。dim で暗くする（奥に置くもの）。"""
    img = img.convert("RGB")
    img = img.resize((w, int(img.height * w / img.width)), Image.LANCZOS)
    if dim:
        img = Image.blend(img, Image.new("RGB", img.size, (0, 0, 0)), dim)
    mask = Image.new("L", img.size, 0)
    ImageDraw.Draw(mask).rounded_rectangle([0, 0, img.width - 1, img.height - 1], 12 * S, fill=255)
    shadow = Image.new("L", im.size, 0)
    ImageDraw.Draw(shadow).rounded_rectangle([x, y + 14 * S, x + img.width, y + img.height + 14 * S], 12 * S, fill=190)
    im.paste((0, 0, 0), (0, 0), shadow.filter(ImageFilter.GaussianBlur(26 * S)))
    ImageDraw.Draw(im).rounded_rectangle([x - 2 * S, y - 2 * S, x + img.width + 2 * S, y + img.height + 2 * S], 14 * S,
                                         fill=(80, 80, 80))
    im.paste(img, (x, y), mask)


# 右: 奥に画面全体、手前に Claude とのチャット（このアプリの売り）
card(Image.open(SHOT), 590 * S, 170 * S, 820 * S, dim=0.35)
card(Image.open(CHAT), 880 * S, 64 * S, 330 * S)
d = ImageDraw.Draw(im)

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
