#!/usr/bin/env bash
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
. "$HERE/scripts/apple-container-resources.sh"
VERSION="${MISTER_FFMPEG_VERSION:-8.1.2}"
VIDEO_LAB="${MISTER_FFMPEG_VIDEO_LAB:-0}"
WORK="$HERE/target/ffmpeg-minimal/armv7"
SRC="$WORK/ffmpeg-$VERSION"
DIST="$WORK/dist"
if [[ "$VIDEO_LAB" =~ ^(1|true|yes)$ ]]; then
  FFMPEG_MODE="video-lab-swscale"
else
  FFMPEG_MODE="video-fast-noswscale"
fi
STAMP="$DIST/.mister-minimal-ffmpeg-$VERSION-h264-aac-s16le-swresample-$FFMPEG_MODE-cortex-a9-o3"
DOCKER_IMAGE="${MISTER_CROSS_IMAGE:-cross-custom-rust:armv7-unknown-linux-gnueabihf-b52a5}"
APPLE_IMAGE="${MISTER_APPLE_CONTAINER_IMAGE:-mister-magik-cross-armv7:ubuntu20-arm64}"
BACKEND="${MISTER_FFMPEG_BUILD_BACKEND:-auto}"

REQUIRED_DIST_FILES=(
  "$STAMP"
  "$DIST/include/libavcodec/avcodec.h"
  "$DIST/include/libavcodec/version_major.h"
  "$DIST/include/libavformat/avformat.h"
  "$DIST/include/libavutil/avutil.h"
  "$DIST/include/libswresample/swresample.h"
  "$DIST/lib/libavcodec.a"
  "$DIST/lib/libavformat.a"
  "$DIST/lib/libavutil.a"
  "$DIST/lib/libswresample.a"
  "$DIST/lib/pkgconfig/libavcodec.pc"
  "$DIST/lib/pkgconfig/libswresample.pc"
)

if [ "$FFMPEG_MODE" = "video-lab-swscale" ]; then
  REQUIRED_DIST_FILES+=(
    "$DIST/include/libswscale/swscale.h"
    "$DIST/lib/libswscale.a"
  )
fi

dist_is_complete() {
  local file
  for file in "${REQUIRED_DIST_FILES[@]}"; do
    if [ ! -f "$file" ]; then
      return 1
    fi
  done
}

case "$BACKEND" in
  auto|apple-container|docker) ;;
  *)
    echo "ERROR: invalid MISTER_FFMPEG_BUILD_BACKEND=$BACKEND (expected auto, apple-container, or docker)" >&2
    exit 2
    ;;
esac

if dist_is_complete; then
  echo "==> minimal FFmpeg already built: $DIST"
  exit 0
fi
if [ -e "$DIST" ]; then
  echo "==> minimal FFmpeg cache is incomplete; rebuilding: $DIST"
  rm -rf "$DIST"
fi

mkdir -p "$WORK"
if [ ! -d "$SRC/.git" ]; then
  rm -rf "$SRC"
  echo "==> fetching FFmpeg n$VERSION"
  git clone --depth=1 -b "n$VERSION" "${FFMPEG_GIT_URL:-https://github.com/FFmpeg/FFmpeg}" "$SRC"
fi

if [ "$BACKEND" = auto ]; then
  if [ "$(uname -s)" = Darwin ] && [ "$(uname -m)" = arm64 ] && command -v container >/dev/null 2>&1; then
    BACKEND=apple-container
  else
    BACKEND=docker
  fi
fi

if [ "$BACKEND" = apple-container ]; then
  if [ "$(uname -s)" != Darwin ] || [ "$(uname -m)" != arm64 ]; then
    echo "ERROR: Apple-container FFmpeg backend requires arm64 macOS; got $(uname -s)/$(uname -m)." >&2
    exit 1
  fi
  if ! command -v container >/dev/null 2>&1; then
    echo "ERROR: Apple container is not installed or not on PATH." >&2
    exit 1
  fi
  CONTAINER_CPUS="$(apple_container_cpus)"
  CONTAINER_MEMORY="$(apple_container_memory)"
  echo "==> building Apple-container FFmpeg helper image $APPLE_IMAGE"
  container build --arch arm64 --file "$HERE/Dockerfile.cross-armv7" --tag "$APPLE_IMAGE" "$HERE"
  RUNNER=(
    container run --arch arm64 --rm
    --cpus "$CONTAINER_CPUS"
    --memory "$CONTAINER_MEMORY"
    --env MAKEFLAGS="-j$CONTAINER_CPUS"
    --volume "$HERE:/project"
    --workdir "/project/target/ffmpeg-minimal/armv7/ffmpeg-$VERSION"
    "$APPLE_IMAGE"
  )
else
  export DOCKER_DEFAULT_PLATFORM="${DOCKER_DEFAULT_PLATFORM:-linux/amd64}"
  if ! docker image inspect "$DOCKER_IMAGE" >/dev/null 2>&1; then
    echo "==> building cross helper image $DOCKER_IMAGE"
    docker build -t "$DOCKER_IMAGE" -f "$HERE/Dockerfile.cross-armv7" "$HERE"
  fi
  RUNNER=(
    docker run --rm
    --platform linux/amd64
    --user "$(id -u):$(id -g)"
    -v "$HERE:/project"
    -w "/project/target/ffmpeg-minimal/armv7/ffmpeg-$VERSION"
    "$DOCKER_IMAGE"
  )
fi

echo "==> configuring minimal FFmpeg n$VERSION mode=$FFMPEG_MODE"
CONFIGURE_SW_SCALE=()
if [ "$FFMPEG_MODE" = "video-lab-swscale" ]; then
  CONFIGURE_SW_SCALE=(--enable-swscale)
else
  CONFIGURE_SW_SCALE=(--disable-swscale)
fi
CONFIGURE_SW_SCALE_FLAG="${CONFIGURE_SW_SCALE[0]}"
"${RUNNER[@]}" \
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
  --enable-swresample \
  --enable-avcodec \
  --enable-avformat \
  --enable-avutil \
  "$1" \
  --enable-decoder=h264 \
  --enable-decoder=aac \
  --enable-decoder=pcm_s16le \
  --enable-parser=aac \
  --enable-parser=h264 \
  --enable-demuxer=mov \
  --enable-protocol=file
grep -q "^#define CONFIG_GPL 0$" config.h
grep -q "^#define CONFIG_VERSION3 0$" config.h
grep -q "^#define CONFIG_NONFREE 0$" config.h
make -j"$(nproc)" install
' bash "$CONFIGURE_SW_SCALE_FLAG"

touch "$STAMP"
echo "==> minimal FFmpeg built: $DIST"
find "$DIST/lib" -name "*.a" -exec ls -lh {} +
