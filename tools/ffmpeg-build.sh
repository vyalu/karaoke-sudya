#!/bin/sh
# Minimal LGPL ffmpeg: cut clips frame-accurately (decode + re-encode with Windows' own
# H.264 encoder, Media Foundation — no encoder of ours, no patent questions), put the dub (AAC)
# onto the picture, read the usual phone and web clips (mp4/mov/mkv/webm, H.264/H.265/VP8/VP9,
# AAC/MP3/Opus). The Linux build is for tests and uses the native mpeg4 encoder instead.
#
#   tools/ffmpeg-build.sh <ffmpeg source> <win|linux> <build dir>
#   source: git clone --depth 1 -b n7.1 https://github.com/FFmpeg/FFmpeg.git
# Result: <build dir>/ffmpeg.exe (win) or <build dir>/ffmpeg (linux).
set -e
SRC=$(cd "$1" && pwd); KIND="$2"; OUT="$3"
COMMON="--disable-everything --disable-autodetect --disable-network --disable-doc --disable-debug --disable-ffplay --disable-ffprobe --enable-ffmpeg --enable-small
 --enable-protocol=file,pipe
 --enable-demuxer=mov,matroska,mp3,wav,aac,ogg,flac,mpegts,concat
 --enable-muxer=mp4,mov,matroska,webm,wav,ipod
 --enable-decoder=aac,aac_fixed,mp3,mp3float,opus,vorbis,flac,pcm_s16le,pcm_s24le,pcm_f32le,h264,hevc,vp8,vp9,mpeg4
 --enable-encoder=aac,pcm_s16le
 --enable-parser=h264,hevc,aac,vp8,vp9,av1,opus,mpegaudio,flac,vorbis
 --enable-bsf=h264_mp4toannexb,hevc_mp4toannexb,aac_adtstoasc,extract_extradata,vp9_superframe,null
 --enable-filter=aresample,anull,null,copy,volume,aformat,atrim,asetpts,amix,trim,setpts,concat,format,scale,fps
 --enable-swresample --enable-swscale"
mkdir -p "$OUT" && cd "$OUT"
case "$KIND" in
  # (the extra encoders go after $COMMON: --disable-everything would switch them off again)
  win) "$SRC/configure" --target-os=mingw32 --arch=x86_64 --cross-prefix=x86_64-w64-mingw32- --enable-static --disable-shared --extra-ldflags="-static" \
         $COMMON --enable-mediafoundation --enable-encoder=h264_mf ;;
  linux) "$SRC/configure" $COMMON --enable-encoder=mpeg4 ;;
  *) echo "usage: $0 <src> <win|linux> <out>"; exit 1 ;;
esac
make -j"$(nproc)"
