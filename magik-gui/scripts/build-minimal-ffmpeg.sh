#!/usr/bin/env bash
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
VERSION="${MISTER_FFMPEG_VERSION:-8.1.1}"
WORK="$HERE/target/ffmpeg-minimal/armv7"
SRC="$WORK/ffmpeg-$VERSION"
DIST="$WORK/dist"
STAMP="$DIST/.mister-minimal-ffmpeg-$VERSION-h264-pcm-s16le-cortex-a9-o3"
IMAGE="${MISTER_CROSS_IMAGE:-cross-custom-rust:armv7-unknown-linux-gnueabihf-b52a5}"

REQUIRED_DIST_FILES=(
  "$STAMP"
  "$DIST/include/libavcodec/avcodec.h"
  "$DIST/include/libavcodec/version_major.h"
  "$DIST/include/libavformat/avformat.h"
  "$DIST/include/libavutil/avutil.h"
  "$DIST/include/libswscale/swscale.h"
  "$DIST/lib/libavcodec.a"
  "$DIST/lib/libavformat.a"
  "$DIST/lib/libavutil.a"
  "$DIST/lib/libswscale.a"
  "$DIST/lib/pkgconfig/libavcodec.pc"
)

dist_is_complete() {
  local file
  for file in "${REQUIRED_DIST_FILES[@]}"; do
    if [ ! -f "$file" ]; then
      return 1
    fi
  done
}

if dist_is_complete; then
  echo "==> minimal FFmpeg already built: $DIST"
  exit 0
fi
if [ -e "$DIST" ]; then
  echo "==> minimal FFmpeg cache is incomplete; rebuilding: $DIST"
  rm -rf "$DIST"
fi

export DOCKER_DEFAULT_PLATFORM="${DOCKER_DEFAULT_PLATFORM:-linux/amd64}"

if ! docker image inspect "$IMAGE" >/dev/null 2>&1; then
  echo "==> building cross helper image $IMAGE"
  docker build -t "$IMAGE" -f "$HERE/Dockerfile.cross-armv7" "$HERE"
fi

mkdir -p "$WORK"
if [ ! -d "$SRC/.git" ]; then
  rm -rf "$SRC"
  echo "==> fetching FFmpeg n$VERSION"
  git clone --depth=1 -b "n$VERSION" "${FFMPEG_GIT_URL:-https://github.com/FFmpeg/FFmpeg}" "$SRC"
fi

echo "==> configuring minimal FFmpeg n$VERSION"
docker run --rm \
  --platform linux/amd64 \
  --user "$(id -u):$(id -g)" \
  -v "$HERE:/project" \
  -w "/project/target/ffmpeg-minimal/armv7/ffmpeg-$VERSION" \
  "$IMAGE" \
  bash -lc '
set -euo pipefail
rm -rf ../dist
./configure \
  --prefix=/project/target/ffmpeg-minimal/armv7/dist \
  --cross-prefix=arm-linux-gnueabihf- \
  --arch=arm \
  --cpu=cortex-a9 \
  --target-os=linux \
  --enable-cross-compile \
  --extra-cflags="-O3 -mcpu=cortex-a9 -mfpu=neon-vfpv3 -mfloat-abi=hard" \
  --extra-cxxflags="-O3 -mcpu=cortex-a9 -mfpu=neon-vfpv3 -mfloat-abi=hard" \
  --enable-static \
  --disable-shared \
  --enable-pic \
  --disable-autodetect \
  --disable-programs \
  --disable-doc \
  --disable-debug \
  --enable-stripping \
  --disable-everything \
  --disable-avdevice \
  --disable-avfilter \
  --disable-swresample \
  --enable-avcodec \
  --enable-avformat \
  --enable-avutil \
  --enable-swscale \
  --enable-decoder=h264 \
  --enable-decoder=pcm_s16le \
  --enable-parser=h264 \
  --enable-demuxer=mov \
  --enable-protocol=file
make -j"$(nproc)" install
'

touch "$STAMP"
echo "==> minimal FFmpeg built: $DIST"
find "$DIST/lib" -name "*.a" -exec ls -lh {} +
