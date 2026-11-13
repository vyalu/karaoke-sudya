#!/bin/sh
# whisper-cli.exe for the «Нейросети» add-on: whisper.cpp v1.7.4, CPU only, static (only system DLLs).
#   tools/whisper-build.sh <whisper.cpp source> <out dir>
# Needs cmake and mingw-w64 (posix threads).
set -e
SRC=$1; OUT=$2
mkdir -p "$OUT"; OUT=$(cd "$OUT" && pwd)
mkdir -p "$OUT"
TC="$OUT/mingw.cmake"
cat > "$TC" <<'T'
set(CMAKE_SYSTEM_NAME Windows)
set(CMAKE_SYSTEM_PROCESSOR x86_64)
set(CMAKE_C_COMPILER x86_64-w64-mingw32-gcc-posix)
set(CMAKE_CXX_COMPILER x86_64-w64-mingw32-g++-posix)
set(CMAKE_RC_COMPILER x86_64-w64-mingw32-windres)
set(CMAKE_FIND_ROOT_PATH /usr/x86_64-w64-mingw32)
set(CMAKE_FIND_ROOT_PATH_MODE_PROGRAM NEVER)
set(CMAKE_FIND_ROOT_PATH_MODE_LIBRARY ONLY)
set(CMAKE_FIND_ROOT_PATH_MODE_INCLUDE ONLY)
T
cmake -S "$SRC" -B "$OUT/b" -DCMAKE_TOOLCHAIN_FILE="$TC" -DCMAKE_BUILD_TYPE=Release \
  -DBUILD_SHARED_LIBS=OFF -DGGML_NATIVE=OFF -DGGML_OPENMP=OFF \
  -DGGML_AVX=ON -DGGML_AVX2=ON -DGGML_FMA=ON -DGGML_F16C=ON \
  -DWHISPER_BUILD_TESTS=OFF -DWHISPER_BUILD_SERVER=OFF -DWHISPER_SDL2=OFF \
  -DCMAKE_EXE_LINKER_FLAGS="-static -static-libgcc -static-libstdc++"
cmake --build "$OUT/b" --config Release -j"$(nproc)" --target whisper-cli
x86_64-w64-mingw32-strip -o "$OUT/whisper-cli.exe" "$OUT/b/bin/whisper-cli.exe"
ls -l "$OUT/whisper-cli.exe"
