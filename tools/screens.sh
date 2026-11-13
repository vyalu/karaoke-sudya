#!/bin/sh
# README screenshots from the demo stand (Linux build, made-up players and sounds):
#   cargo build --release --target x86_64-unknown-linux-gnu && tools/screens.sh && python3 tools/banner.py
# Needs Xvfb, ImageMagick (import) and Pillow.
set -e
BIN=target/x86_64-unknown-linux-gnu/release/karaoke
TMP=$(mktemp -d)
Xvfb :77 -screen 0 1600x1000x24 >/dev/null 2>&1 & XV=$!
sleep 1
for n in home games result parodist dub editor apozh; do
  DISPLAY=:77 LIBGL_ALWAYS_SOFTWARE=1 KJ_DEMO=$n $BIN >/dev/null 2>&1 & P=$!
  sleep 6
  DISPLAY=:77 import -window root "$TMP/$n.png"
  kill $P 2>/dev/null || true; sleep 1
done
kill $XV
python3 -c "
import sys
from PIL import Image
for n in 'home games result parodist dub editor apozh'.split():
    Image.open(sys.argv[1] + '/' + n + '.png').convert('RGB').save('docs/screenshots/' + n + '.webp', 'WEBP', quality=88, method=6)
" "$TMP"
echo docs/screenshots/*.webp
