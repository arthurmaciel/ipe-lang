#!/usr/bin/env python3
# tools/scripts/lib/visual_diff.py — perceptual diff helper for visual parity.
#
# Computes the RMS pixel difference between two screenshots on a 0-255 scale.
# Both images are auto-cropped to remove xvfb black borders, then resized to
# the smaller of the two before comparison (so different capture resolutions
# do not produce spurious diffs from resize alone).
#
# Cross-engine note: Chromium (Playwright) and WebKitGTK (xvfb webview) render
# the same HTML with ~40-50 RMS difference due to font metrics, antialiasing,
# and shadow rendering. The default threshold of 8.0 is only appropriate for
# same-engine comparisons (e.g. Playwright sky vs Playwright ipe-web).  For
# cross-engine comparisons raise the threshold or use structural DOM checks
# instead. The --threshold flag makes this explicit per-port.
#
# Usage (standalone):
#   python3 visual_diff.py <image-a> <image-b> [x0,y0,x1,y1] [--threshold N]
#   Exits 0 if RMS <= threshold (default 8.0), 1 if above, 2 on error.
#
# Usage (imported):
#   from visual_diff import visual_rms, auto_crop_black
#   rms = visual_rms("sky.png", "ipe.png", crop=(0, 70, 960, 720))

from PIL import Image, ImageChops, ImageStat, ImageOps
import sys
import os

DEFAULT_THRESHOLD = 8.0
_BLACK_THRESHOLD = 15   # pixel luminance below which a pixel counts as "black border"


def auto_crop_black(img: Image.Image, threshold: int = _BLACK_THRESHOLD) -> Image.Image:
    """Remove trailing black rows/columns (xvfb virtual-display background).

    Converts to greyscale, thresholds at `threshold`, finds the bounding box
    of non-zero pixels, and crops to that box.  No-ops on images with no black
    border (returns the original unchanged).
    """
    grey = ImageOps.grayscale(img)
    mask = grey.point(lambda p: 255 if p > threshold else 0)
    bbox = mask.getbbox()
    return img.crop(bbox) if bbox else img


def visual_rms(path_a: str, path_b: str, crop=None) -> float:
    """Return RMS pixel difference (0-255 greyscale scale).

    Opens both images, converts to RGB, auto-crops black borders, resizes
    both to the smaller of the two dimensions, optionally further crops,
    then returns the RMS of the per-pixel greyscale difference.

    Score guide:
      0        — pixel-identical
      1-10     — same-engine rendering noise (AA, subpixel)
      10-20    — minor layout differences (border-radius, shadow)
      40-60    — cross-engine differences (Chromium vs WebKitGTK on same HTML)
      >80      — visible layout regression (missing element, wrong colour)
    """
    a = Image.open(path_a).convert("RGB")
    b = Image.open(path_b).convert("RGB")
    a = auto_crop_black(a)
    b = auto_crop_black(b)
    w = min(a.width, b.width)
    h = min(a.height, b.height)
    if (a.width, a.height) != (w, h):
        a = a.resize((w, h), Image.LANCZOS)
    if (b.width, b.height) != (w, h):
        b = b.resize((w, h), Image.LANCZOS)
    if crop is not None:
        x0, y0, x1, y1 = crop
        box = (x0, y0, min(x1, w), min(y1, h))
        a = a.crop(box)
        b = b.crop(box)
    diff = ImageChops.difference(a, b)
    grey = ImageOps.grayscale(diff)
    stat = ImageStat.Stat(grey)
    return stat.rms[0]


def main() -> int:
    if len(sys.argv) < 3:
        print(
            f"usage: {sys.argv[0]} <image-a> <image-b> [x0,y0,x1,y1] [--threshold N]",
            file=sys.stderr,
        )
        return 2

    path_a, path_b = sys.argv[1], sys.argv[2]
    crop = None
    threshold = DEFAULT_THRESHOLD

    args = sys.argv[3:]
    i = 0
    while i < len(args):
        if args[i] == "--threshold" and i + 1 < len(args):
            try:
                threshold = float(args[i + 1])
            except ValueError:
                print(f"invalid threshold: {args[i+1]}", file=sys.stderr)
                return 2
            i += 2
        elif "," in args[i]:
            try:
                parts = [int(x) for x in args[i].split(",")]
                if len(parts) != 4:
                    raise ValueError
                crop = tuple(parts)
            except ValueError:
                print(f"invalid crop '{args[i]}' — expect x0,y0,x1,y1", file=sys.stderr)
                return 2
            i += 1
        else:
            print(f"unknown argument: {args[i]}", file=sys.stderr)
            return 2

    for p in (path_a, path_b):
        if not os.path.isfile(p):
            print(f"file not found: {p}", file=sys.stderr)
            return 2

    try:
        rms = visual_rms(path_a, path_b, crop=crop)
    except Exception as e:
        print(f"error: {e}", file=sys.stderr)
        return 2

    verdict = "PASS" if rms <= threshold else "FAIL"
    crop_note = f" crop={crop}" if crop else ""
    print(f"{verdict}  rms={rms:.2f}  threshold={threshold:.1f}{crop_note}")
    print(f"  a: {path_a}")
    print(f"  b: {path_b}")
    return 0 if rms <= threshold else 1


if __name__ == "__main__":
    sys.exit(main())
