#!/usr/bin/env bash
# Validate and atomically sync canonical Cortex-A9 MP4 video snaps to the MiSTer.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MISTER="$HERE/scripts/mister"

SRC_DIR="${1:-${MISTER_VIDEO_SRC_DIR:-$HERE/build/video-snaps-neogeo-cortex-a9}}"
REMOTE_DIR="${2:-${MISTER_VIDEO_REMOTE_DIR:-/media/fat/mister-magik/video-snaps/neogeo}}"
MANIFEST="$SRC_DIR/manifest.tsv"
SSIM_MIN="${MISTER_VIDEO_SSIM_MIN:-0.995}"
LUMA_PSNR_MIN="${MISTER_VIDEO_LUMA_PSNR_MIN:-45}"
GOP="${MISTER_VIDEO_GOP:-120}"
SOURCE_GEOMETRY="${MISTER_VIDEO_SOURCE_GEOMETRY:-640x480}"
OUTPUT_GEOMETRY="${MISTER_VIDEO_OUTPUT_GEOMETRY:-320x240}"

die() {
  echo "sync-video-snaps: $*" >&2
  exit 1
}

require_tool() {
  command -v "$1" >/dev/null 2>&1 || die "missing required tool: $1"
}

sha256_file() {
  shasum -a 256 "$1" | awk '{print $1}'
}

ffprobe_field() {
  ffprobe -v error "$@"
}

validate_fraction_match() {
  local got="$1"
  local expected="$2"
  python3 - "$got" "$expected" <<'PY'
from fractions import Fraction
import sys
got = Fraction(sys.argv[1])
expected = Fraction(sys.argv[2])
if got != expected:
    raise SystemExit(1)
PY
}

quality_ge() {
  python3 - "$1" "$2" <<'PY'
import math
import sys
value = float(sys.argv[1])
minimum = float(sys.argv[2])
raise SystemExit(0 if value >= minimum or math.isinf(value) else 1)
PY
}

validate_h264_policy() {
  local file="$1"
  local gop="$2"
  local frames_log trace_log
  frames_log="$(mktemp "${TMPDIR:-/tmp}/video-frames.XXXXXX")"
  trace_log="$(mktemp "${TMPDIR:-/tmp}/video-h264-trace.XXXXXX")"
  ffprobe -v error -select_streams v:0 -show_entries frame=key_frame,pict_type \
    -of csv=p=0 "$file" >"$frames_log"
  ffmpeg -hide_banner -v debug -i "$file" -map 0:v:0 -c copy \
    -bsf:v trace_headers -f null - >"$trace_log" 2>&1
  if ! python3 - "$frames_log" "$trace_log" "$gop" <<'PY'
import re
import sys

frames_path, trace_path, gop_text = sys.argv[1:]
gop = int(gop_text)
keys = []
for index, raw in enumerate(open(frames_path, encoding="utf-8", errors="replace")):
    fields = [part.strip() for part in raw.strip().split(",") if part.strip()]
    if len(fields) < 2:
        continue
    key_frame, pict_type = fields[0], fields[1]
    if pict_type == "B":
        raise SystemExit("B-frame found")
    if key_frame == "1":
        keys.append(index)
if not keys:
    raise SystemExit("no keyframes found")
if keys[0] != 0:
    raise SystemExit(f"first keyframe is frame {keys[0]}, expected 0")
for previous, current in zip(keys, keys[1:]):
    if current - previous != gop:
        raise SystemExit(f"GOP interval {current - previous}, expected {gop}")

trace = open(trace_path, encoding="utf-8", errors="replace").read()
entropy = re.findall(r"entropy_coding_mode_flag[^\n]*=\s*([01])", trace)
if not entropy:
    raise SystemExit("could not validate CABAC flag")
if any(value != "0" for value in entropy):
    raise SystemExit("CABAC enabled")
refs = re.findall(r"\b(?:max_)?num_ref_frames\b[^\n]*=\s*(\d+)", trace)
if not refs:
    raise SystemExit("could not validate reference frame count")
if any(int(value) > 1 for value in refs):
    raise SystemExit(f"multiple reference frames found: {refs}")
PY
  then
    rm -f "$frames_log" "$trace_log"
    return 1
  fi
  rm -f "$frames_log" "$trace_log"
}

[[ -d "$SRC_DIR" ]] || die "source directory not found: $SRC_DIR"
[[ -f "$MANIFEST" ]] || die "validated manifest not found: $MANIFEST; run scripts/reencode-video-snaps-cortex-a9.sh first"
require_tool ffprobe
require_tool ffmpeg
require_tool python3
require_tool shasum

SYNC_SHA="$SRC_DIR/.sync-sha256"
tmp_sync_sha="$(mktemp "${TMPDIR:-/tmp}/video-snaps-sync.XXXXXX")"
trap 'rm -f "$tmp_sync_sha"' EXIT
: >"$tmp_sync_sha"

files=()
while IFS=$'\t' read -r file source_sha output_sha source_geometry output_geometry fps video_codec audio_codec pix_fmt profile ssim luma_psnr; do
  [[ -n "$file" ]] || continue
  [[ "$file" != */* ]] || die "manifest file must be a basename: $file"
  path="$SRC_DIR/$file"
  provenance="$SRC_DIR/$file.provenance"
  [[ -f "$path" ]] || die "manifest file missing: $path"
  [[ -f "$provenance" ]] || die "provenance missing: $provenance"
  [[ "$(sha256_file "$path")" == "$output_sha" ]] || die "sha256 mismatch for $file"
  grep -qx "output_sha256=$output_sha" "$provenance" || die "provenance output hash mismatch for $file"
  grep -qx "source_geometry=$source_geometry" "$provenance" || die "provenance source geometry mismatch for $file"
  grep -qx "output_geometry=$output_geometry" "$provenance" || die "provenance output geometry mismatch for $file"
  grep -qx "ssim=$ssim" "$provenance" || die "provenance SSIM mismatch for $file"
  grep -qx "luma_psnr=$luma_psnr" "$provenance" || die "provenance luma PSNR mismatch for $file"
  [[ "$source_geometry" == "$SOURCE_GEOMETRY" ]] || die "$file source geometry is $source_geometry, expected $SOURCE_GEOMETRY"
  [[ "$output_geometry" == "$OUTPUT_GEOMETRY" ]] || die "$file output geometry is $output_geometry, expected $OUTPUT_GEOMETRY"
  quality_ge "$ssim" "$SSIM_MIN" || die "$file SSIM $ssim below $SSIM_MIN"
  quality_ge "$luma_psnr" "$LUMA_PSNR_MIN" || die "$file luma PSNR $luma_psnr below $LUMA_PSNR_MIN"

  mapfile -t probed_video < <(
    ffprobe_field -select_streams v:0 \
      -show_entries stream=codec_name,profile,width,height,pix_fmt,r_frame_rate \
      -of default=noprint_wrappers=1:nokey=1 "$path"
  )
  [[ "${#probed_video[@]}" -eq 6 ]] || die "$file video probe returned ${#probed_video[@]} fields"
  probed_codec="${probed_video[0]}"
  probed_profile="${probed_video[1]}"
  probed_width="${probed_video[2]}"
  probed_height="${probed_video[3]}"
  probed_pix_fmt="${probed_video[4]}"
  probed_fps="${probed_video[5]}"
  probed_audio="$(ffprobe_field -select_streams a:0 -show_entries stream=codec_name -of default=noprint_wrappers=1:nokey=1 "$path")"

  [[ "$probed_codec" == "$video_codec" ]] || die "$file video codec is $probed_codec, manifest says $video_codec"
  [[ "$probed_profile" == "$profile" ]] || die "$file profile is $probed_profile, manifest says $profile"
  [[ "${probed_width}x${probed_height}" == "$output_geometry" ]] || die "$file geometry is ${probed_width}x${probed_height}, manifest says $output_geometry"
  [[ "$probed_pix_fmt" == "$pix_fmt" ]] || die "$file pix_fmt is $probed_pix_fmt, manifest says $pix_fmt"
  [[ "$probed_audio" == "$audio_codec" ]] || die "$file audio codec is $probed_audio, manifest says $audio_codec"
  validate_fraction_match "$probed_fps" "$fps" || die "$file fps is $probed_fps, manifest says $fps"
  validate_h264_policy "$path" "$GOP" || die "$file H.264 policy validation failed"

  printf '%s  %s\n' "$output_sha" "$file" >>"$tmp_sync_sha"
  prov_sha="$(sha256_file "$provenance")"
  printf '%s  %s\n' "$prov_sha" "$file.provenance" >>"$tmp_sync_sha"
done < <(tail -n +2 "$MANIFEST")

if [[ ! -s "$tmp_sync_sha" ]]; then
  die "manifest contains no assets"
fi

manifest_sha="$(sha256_file "$MANIFEST")"
printf '%s  manifest.tsv\n' "$manifest_sha" >>"$tmp_sync_sha"
cp "$tmp_sync_sha" "$SYNC_SHA"

mapfile -t files < <(awk 'NR > 1 && $1 != "" { print $1 }' "$MANIFEST")
echo "sync-video-snaps: validated ${#files[@]} file(s) from $SRC_DIR"

stamp="$(date +%Y%m%d%H%M%S)-$$"
stage="$REMOTE_DIR.__staging__.$stamp"
backup="$REMOTE_DIR.__backup__.$stamp"

"$MISTER" run "rm -rf '$stage' '$backup'; mkdir -p '$stage'"
for file in "${files[@]}"; do
  echo "sync-video-snaps: upload $file"
  "$MISTER" put "$SRC_DIR/$file" "$stage/$file"
  "$MISTER" put "$SRC_DIR/$file.provenance" "$stage/$file.provenance"
done
"$MISTER" put "$MANIFEST" "$stage/manifest.tsv"
"$MISTER" put "$SYNC_SHA" "$stage/.sync-sha256"

"$MISTER" run "cd '$stage' && sha256sum -c .sync-sha256"
"$MISTER" run "set -e; if [ -d '$REMOTE_DIR' ]; then mv '$REMOTE_DIR' '$backup'; fi; if mv '$stage' '$REMOTE_DIR'; then rm -rf '$backup'; else if [ -d '$backup' ]; then mv '$backup' '$REMOTE_DIR'; fi; exit 1; fi"

echo "sync-video-snaps: synced ${#files[@]} validated file(s) to $REMOTE_DIR"
