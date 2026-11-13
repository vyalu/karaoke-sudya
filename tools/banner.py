"""README pictures: docs/banner.webp (3200x1120) and docs/social-preview.jpg (1280x640).
Needs Pillow; takes the home screenshot from docs/screenshots/home.webp (see tools/screens.sh)."""
import sys
from pathlib import Path
from PIL import Image, ImageDraw, ImageFont, ImageFilter

ROOT = Path(__file__).resolve().parent.parent
FONTS = ROOT / "assets" / "fonts"
BG = (13, 10, 20)
PINK = (255, 77, 141)
AMBER = (255, 181, 71)
TEAL = (61, 214, 198)
VIOLET = (155, 140, 255)
TEXT = (244, 238, 248)
MUTED = (170, 160, 185)

def font(name, size): return ImageFont.truetype(str(FONTS / name), size)

def perspective(img, tilt=0.06):
    """Lean the screenshot back a little, like a window standing on a desk."""
    w, h = img.size
    d = int(h * tilt)
    # coefficients for PIL's PERSPECTIVE: map output → input
    src = [(0, 0), (w, 0), (w, h), (0, h)]
    dst = [(0, d), (w, 0), (w, h), (0, h - d)]
    import numpy as np
    A = []
    for (x, y), (u, v) in zip(dst, src):
        A.append([x, y, 1, 0, 0, 0, -u * x, -u * y]); A.append([0, 0, 0, x, y, 1, -v * x, -v * y])
    B = [c for p in src for c in p]
    coef = np.linalg.solve(np.array(A, float), np.array(B, float))
    return img.transform((w, h), Image.PERSPECTIVE, coef, Image.BICUBIC, fillcolor=(0, 0, 0, 0))

def window(shot, scale):
    """The screenshot inside a dark window frame with three dots."""
    s = shot.resize((int(shot.width * scale), int(shot.height * scale)), Image.LANCZOS)
    bar = int(64 * scale)
    win = Image.new("RGBA", (s.width, s.height + bar), (0, 0, 0, 0))
    m = Image.new("L", win.size, 0)
    ImageDraw.Draw(m).rounded_rectangle([0, 0, win.width - 1, win.height - 1], radius=int(28 * scale), fill=255)
    body = Image.new("RGBA", win.size, (34, 28, 46, 255))
    d = ImageDraw.Draw(body)
    for i, c in enumerate([(255, 95, 87), (254, 188, 46), (40, 200, 64)]):
        cx = int((40 + i * 34) * scale); cy = bar // 2
        d.ellipse([cx - 9 * scale, cy - 9 * scale, cx + 9 * scale, cy + 9 * scale], fill=c)
    body.paste(s, (0, bar))
    win.paste(body, (0, 0), m)
    return win

def pill(d, x, y, text, color, f):
    w = d.textlength(text, font=f) + 56
    d.rounded_rectangle([x, y, x + w, y + 72], radius=36, outline=color, width=4, fill=tuple(int(c * 0.14) for c in color))
    d.text((x + 28, y + 36), text, font=f, fill=color, anchor="lm")
    return x + w + 22

def make_banner(out_banner, out_social):
    W, H = 3200, 1120
    img = Image.new("RGB", (W, H), BG)
    d = ImageDraw.Draw(img)
    for x in range(0, W, 64): d.line([(x, 0), (x, H)], fill=(22, 18, 32), width=2)
    for y in range(0, H, 64): d.line([(0, y), (W, y)], fill=(22, 18, 32), width=2)
    # a soft pink glow behind the window
    glow = Image.new("RGB", (W, H), (0, 0, 0))
    ImageDraw.Draw(glow).ellipse([1900, 150, 3300, 1250], fill=(120, 30, 80))
    img = Image.blend(img, Image.composite(glow, img, glow.convert("L").point(lambda v: 255 if v else 0)).filter(ImageFilter.GaussianBlur(220)), 0.55)
    d = ImageDraw.Draw(img)
    # right: the program
    shot = Image.open(ROOT / "docs" / "screenshots" / "home.webp").convert("RGBA")
    win = perspective(window(shot, 1.25))
    img.paste(win, (1500, 130), win)
    # left: title
    icon = Image.open(ROOT / "assets" / "icon256.png").convert("RGBA").resize((150, 150), Image.LANCZOS)
    img.paste(icon, (144, 150), icon)
    d.text((330, 225), "Караоке-судья", font=font("Unbounded-Bold.ttf", 66), fill=TEXT, anchor="lm")
    big = font("Unbounded-ExtraBold.ttf", 132)
    y = 330
    d.text((144, y), "Пой —", font=big, fill=TEXT)
    d.text((144 + d.textlength("Пой — ", font=big), y), "судья", font=big, fill=PINK)
    d.text((144, y + 160), "уже ", font=big, fill=TEXT)
    d.text((144 + d.textlength("уже ", font=big), y + 160), "слушает", font=big, fill=AMBER)
    sub = font("Manrope-Regular.ttf", 50)
    d.text((144, 720), "Караоке с YouTube, честная оценка пения,", font=sub, fill=MUTED)
    d.text((144, 784), "болельщик Бориска и мини-игры для компании.", font=sub, fill=MUTED)
    pf = font("Manrope-Bold.ttf", 38)
    x = 144
    for t, c in [("Караоке", PINK), ("Пародист", AMBER), ("Озвучка", TEAL), ("АПОЖ", VIOLET), ("Windows", (120, 200, 255))]:
        x = pill(d, x, 900, t, c, pf)
    img.save(out_banner, "WEBP", quality=88, method=6)
    # social preview 1280x640: the same picture, cropped and scaled
    sp = img.crop((0, 0, 2240, 1120)).resize((1280, 640), Image.LANCZOS)
    sp.save(out_social, "JPEG", quality=88)

if __name__ == "__main__":
    make_banner(ROOT / "docs" / "banner.webp", ROOT / "docs" / "social-preview.jpg")
    print("docs/banner.webp, docs/social-preview.jpg")
