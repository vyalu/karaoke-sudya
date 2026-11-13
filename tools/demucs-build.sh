#!/bin/sh
# kj-demucs.exe for the «Нейросети» add-on: tools/demucs/kj-demucs.cpp + demucs.cpp (MIT), static.
#   tools/demucs-build.sh <demucs.cpp source> <out dir> [eigen include dir]
# demucs.cpp commit 4edae497a114de1ccb17ac9b4a2b8fc0144714d7; Eigen 3.4 headers (libeigen3-dev).
set -e
SRC=$1; OUT=$2; EIGEN=${3:-/usr/include/eigen3}
HERE=$(cd "$(dirname "$0")" && pwd)
mkdir -p "$OUT"
x86_64-w64-mingw32-g++-posix -std=c++17 -O3 -DNDEBUG -mavx2 -mfma -mf16c \
  -fno-unsafe-math-optimizations -freciprocal-math -fno-signed-zeros \
  -I"$EIGEN" -I"$SRC/src" -I"$SRC/cli-apps" "$SRC"/src/*.cpp "$HERE/demucs/kj-demucs.cpp" \
  -o "$OUT/kj-demucs.exe" -static -static-libgcc -static-libstdc++ -pthread
x86_64-w64-mingw32-strip "$OUT/kj-demucs.exe"
ls -l "$OUT/kj-demucs.exe"
