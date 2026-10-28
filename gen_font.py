#!/opt/python/3.15.1/bin/python3
"""把界面里用到的字预先烤成位图字模图集，Rust 那边就不需要字体引擎了。"""
import json
import os
import sys

from PIL import Image, ImageDraw, ImageFont

OUT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "assets")
FONTS = [
    ("/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf", [13, 10]),
]
CJK = "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc"

import glob as _glob


def _scan_cjk():
    """把 Rust 源码和 Python 里出现过的汉字全收集起来，免得漏字变成方块。"""
    chars = set()
    for pat in ("src/**/*.rs", "../display/*.py", "gen_font.py"):
        for f in _glob.glob(pat, recursive=True):
            try:
                txt = open(f, encoding="utf-8").read()
            except OSError:
                continue
            for ch in txt:
                if 0x2E80 <= ord(ch) <= 0x9FFF:
                    chars.add(ch)
    extra = os.path.join(OUT, "extra_chars.txt")
    if os.path.exists(extra):
        try:
            for ch in open(extra, encoding="utf-8").read():
                if 0x2E80 <= ord(ch) <= 0x9FFF or ch in "，。！？：、":
                    chars.add(ch)
        except OSError:
            pass
    return "".join(sorted(chars))


CHARS = ("0123456789.%℃/·:- " + "CPU RAM MEM DISK LOAD TEMP SVC OK " + _scan_cjk())
import string
ASCII = string.digits + string.ascii_letters + string.punctuation.replace('"', '') + " °·℃"

def find_cjk():
    for p in (CJK, "/usr/share/fonts/truetype/noto/NotoSansCJK-Regular.ttc",
              "/usr/share/fonts/opentype/noto/NotoSansCJK-Bold.ttc"):
        if os.path.exists(p):
            return p
    return None

def main():
    os.makedirs(OUT, exist_ok=True)
    faces = {}
    # 数字/英文：等宽
    mono = "/usr/share/fonts/truetype/dejavu/DejaVuSansMono-Bold.ttf"
    if not os.path.exists(mono):
        mono = "/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf"
    faces["mono13"] = ImageFont.truetype(mono, 13)
    faces["mono10"] = ImageFont.truetype(mono, 10)
    cjk = find_cjk()
    if cjk:
        faces["cjk13"] = ImageFont.truetype(cjk, 13)
        faces["cjk10"] = ImageFont.truetype(cjk, 10)
    else:
        print("no CJK font found", file=sys.stderr)
    atlas = {}
    tiles = []
    x = y = 0
    row_h = 0
    W = 512
    ascent_meta = {}
    for font_name, font in faces.items():
        ascent, descent = font.getmetrics()
        ascent_meta[font_name] = ascent
        for ch in sorted(set(CHARS + ASCII)):
            if ch == " ":
                atlas[f"{font_name}:{ch}"] = {"w": 0, "h": 0, "x": 0, "y": 0,
                                              "adv": int(round(font.getlength(ch)))}
                continue
            w = int(round(font.getlength(ch))) + 2
            h = ascent + descent
            img = Image.new("L", (w, h), 0)
            d = ImageDraw.Draw(img)
            d.text((1, 0), ch, font=font, fill=255)
            if x + w > W:
                x = 0
                y += row_h + 1
                row_h = 0
            tiles.append((img, x, y))
            atlas[f"{font_name}:{ch}"] = {"w": w, "h": h, "x": x, "y": y,
                                          "adv": int(round(font.getlength(ch)))}
            x += w + 1
            row_h = max(row_h, h)
    H = y + row_h + 1
    page = Image.new("L", (W, H), 0)
    for img, px, py in tiles:
        page.paste(img, (px, py))
    page.save(os.path.join(OUT, "font.png"))
    for name, asc in ascent_meta.items():
        atlas[f"{name}:__ascent"] = {"w": 0, "h": 0, "x": 0, "y": 0, "adv": int(asc)}
    json.dump(atlas, open(os.path.join(OUT, "font.json"), "w"), ensure_ascii=False)
    print("font atlas", W, "x", H, "glyphs", len(atlas))

if __name__ == "__main__":
    main()
