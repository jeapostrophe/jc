# /// script
# dependencies = ["pillow"]
# ///
"""Make jcc's icon from jc's: icon.png's grey ramp remapped onto a blue one.

Usage: uv run scripts/tint-icon.py icon.png icon-jcc.png
The source is near-greyscale, so each pixel's luminance picks a colour on the
ramp below; the white key stays near white and the slate background turns blue.
"""
import sys

from PIL import Image

# (luminance, RGB) stops; the background sits around luminance 55-65.
RAMP = [(0, (5, 14, 30)), (60, (28, 72, 135)), (255, (244, 248, 255))]


def lut(channel):
  out = []
  for v in range(256):
    for (a, ca), (b, cb) in zip(RAMP, RAMP[1:]):
      if v <= b:
        t = (v - a) / (b - a)
        out.append(round(ca[channel] + (cb[channel] - ca[channel]) * t))
        break
  return out


src = Image.open(sys.argv[1])
alpha = src.getchannel("A") if src.mode in ("RGBA", "LA") else None
grey = src.convert("L")
tinted = Image.merge("RGB", [grey.point(lut(c)) for c in range(3)])
if alpha is not None:
  tinted.putalpha(alpha)
tinted.save(sys.argv[2])
