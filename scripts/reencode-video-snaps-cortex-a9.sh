#!/usr/bin/env bash
# Re-encode source video snaps into validated Cortex-A9-friendly MP4 assets.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SRC_DIR="${1:-${MISTER_VIDEO_SOURCE_DIR:-$HERE/private/test-fixtures/video-snaps-original}}"
OUT_DIR="${2:-${MISTER_VIDEO_OUT_DIR:-$HERE/build/video-snaps-neogeo-cortex-a9}}"
SSIM_MIN="${MISTER_VIDEO_SSIM_MIN:-0.995}"
LUMA_PSNR_MIN="${MISTER_VIDEO_LUMA_PSNR_MIN:-45}"
CRF="${MISTER_VIDEO_CRF:-10}"
PRESET="${MISTER_VIDEO_PRESET:-slow}"
GOP="${MISTER_VIDEO_GOP:-120}"
SOURCE_GEOMETRY="${MISTER_VIDEO_SOURCE_GEOMETRY:-640x480}"
OUTPUT_GEOMETRY="${MISTER_VIDEO_OUTPUT_GEOMETRY:-320x240}"

die() {
  echo "reencode-video-snaps-cortex-a9: $*" >&2
  exit 1
}

require_tool() {
  command -v "$1" >/dev/null 2>&1 || die "missing required tool: $1"
}

sha256_file() {
  shasum -a 256 "$1" | awk '{print $1}'
}

ffprobe_stream_values() {
  local stream="$1"
  local entries="$2"
  local file="$3"
  ffprobe -v error -select_streams "$stream" -show_entries "stream=$entries" \
    -of default=noprint_wrappers=1:nokey=1 "$file"
}

fraction_equal() {
  python3 - "$1" "$2" <<'PY'
from fractions import Fraction
import sys
raise SystemExit(0 if Fraction(sys.argv[1]) == Fraction(sys.argv[2]) else 1)
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

require_tool ffmpeg
require_tool ffprobe
require_tool python3
require_tool shasum
[[ -d "$SRC_DIR" ]] || die "source directory not found: $SRC_DIR"

shopt -s nullglob
sources=("$SRC_DIR"/*.mp4 "$SRC_DIR"/*.MP4 "$SRC_DIR"/*.mov "$SRC_DIR"/*.MOV)
shopt -u nullglob
[[ "${#sources[@]}" -gt 0 ]] || die "no .mp4/.mov sources in $SRC_DIR"
IFS=$'\n' sources=($(printf '%s\n' "${sources[@]}" | sort -f))
unset IFS

stage="$(mktemp -d "${OUT_DIR}.staging.XXXXXX")"
trap 'rm -rf "$stage"' EXIT
manifest="$stage/manifest.tsv"
printf 'file\tsource_sha256\toutput_sha256\tsource_geometry\toutput_geometry\tfps\tvideo_codec\taudio_codec\tpix_fmt\tprofile\tssim\tluma_psnr\n' >"$manifest"

for source in "${sources[@]}"; do
  base="$(basename "$source")"
  out_name="${base%.*}.mp4"
  out="$stage/$out_name"
  provenance="$stage/$out_name.provenance"
  echo "reencode-video-snaps-cortex-a9: $base -> $out_name"

  mapfile -t video < <(ffprobe_stream_values v:0 codec_name,width,height,pix_fmt,r_frame_rate "$source")
  mapfile -t audio < <(ffprobe_stream_values a:0 codec_name "$source")
  [[ "${#video[@]}" -eq 5 ]] || die "$base: could not read first video stream"
  [[ "${#audio[@]}" -ge 1 ]] || die "$base: missing audio stream; canonical assets require AAC audio copy"

  source_codec="${video[0]}"
  source_width="${video[1]}"
  source_height="${video[2]}"
  source_pix_fmt="${video[3]}"
  fps="${video[4]}"
  audio_codec="${audio[0]}"
  [[ "$audio_codec" == "aac" ]] || die "$base: audio codec is $audio_codec; expected AAC so the canonical encode can copy audio"
  [[ "${source_width}x${source_height}" == "$SOURCE_GEOMETRY" ]] || die "$base: source geometry is ${source_width}x${source_height}; expected $SOURCE_GEOMETRY"
  [[ $((source_width % 2)) -eq 0 && $((source_height % 2)) -eq 0 ]] || die "$base: source geometry must be even, got ${source_width}x${source_height}"
  out_width=$((source_width / 2))
  out_height=$((source_height / 2))
  [[ "$out_width" -gt 0 && "$out_height" -gt 0 ]] || die "$base: invalid half geometry ${out_width}x${out_height}"
  [[ "${out_width}x${out_height}" == "$OUTPUT_GEOMETRY" ]] || die "$base: output geometry would be ${out_width}x${out_height}; expected $OUTPUT_GEOMETRY"

  ffmpeg -hide_banner -y -i "$source" \
    -map 0:v:0 -map 0:a:0 \
    -vf "scale=${out_width}:${out_height}:flags=lanczos,format=yuv420p" \
    -c:v libx264 -preset "$PRESET" -crf "$CRF" \
    -profile:v baseline -level:v 3.0 \
    -x264-params "cabac=0:bframes=0:ref=1:keyint=${GOP}:min-keyint=${GOP}:scenecut=0:open-gop=0" \
    -pix_fmt yuv420p \
    -c:a copy \
    -movflags +faststart \
    "$out"

  mapfile -t out_video < <(ffprobe_stream_values v:0 codec_name,profile,width,height,pix_fmt,r_frame_rate,has_b_frames "$out")
  mapfile -t out_audio < <(ffprobe_stream_values a:0 codec_name "$out")
  [[ "${#out_video[@]}" -eq 7 ]] || die "$out_name: could not read encoded video stream"
  [[ "${#out_audio[@]}" -ge 1 ]] || die "$out_name: missing encoded audio stream"

  out_codec="${out_video[0]}"
  profile="${out_video[1]}"
  probed_width="${out_video[2]}"
  probed_height="${out_video[3]}"
  has_b_frames="${out_video[4]}"
  pix_fmt="${out_video[5]}"
  probed_fps="${out_video[6]}"
  probed_audio="${out_audio[0]}"

  [[ "$out_codec" == "h264" ]] || die "$out_name: video codec is $out_codec"
  [[ "$profile" == "Constrained Baseline" || "$profile" == "Baseline" ]] || die "$out_name: profile is $profile"
  [[ "${probed_width}x${probed_height}" == "${out_width}x${out_height}" ]] || die "$out_name: geometry is ${probed_width}x${probed_height}"
  [[ "$pix_fmt" == "yuv420p" ]] || die "$out_name: pixel format is $pix_fmt"
  [[ "$has_b_frames" == "0" ]] || die "$out_name: has_b_frames is $has_b_frames"
  [[ "$probed_audio" == "aac" ]] || die "$out_name: audio codec is $probed_audio"
  fraction_equal "$probed_fps" "$fps" || die "$out_name: fps is $probed_fps, source fps is $fps"
  validate_h264_policy "$out" "$GOP" || die "$out_name: H.264 policy validation failed"

  ssim_log="$(mktemp "${TMPDIR:-/tmp}/video-ssim.XXXXXX")"
  psnr_log="$(mktemp "${TMPDIR:-/tmp}/video-psnr.XXXXXX")"
  ffmpeg -hide_banner -v info -i "$source" -i "$out" \
    -filter_complex "[0:v]scale=${out_width}:${out_height}:flags=lanczos,format=yuv420p[ref];[1:v]format=yuv420p[dist];[ref][dist]ssim" \
    -an -f null - >"$ssim_log" 2>&1
  ffmpeg -hide_banner -v info -i "$source" -i "$out" \
    -filter_complex "[0:v]scale=${out_width}:${out_height}:flags=lanczos,format=yuv420p[ref];[1:v]format=yuv420p[dist];[ref][dist]psnr" \
    -an -f null - >"$psnr_log" 2>&1

  ssim="$(python3 - "$ssim_log" <<'PY'
import re
import sys
text = open(sys.argv[1], encoding="utf-8", errors="replace").read()
matches = re.findall(r"All:([0-9.]+)", text)
if not matches:
    raise SystemExit("missing SSIM All")
print(matches[-1])
PY
)"
  luma_psnr="$(python3 - "$psnr_log" <<'PY'
import re
import sys
text = open(sys.argv[1], encoding="utf-8", errors="replace").read()
matches = re.findall(r"(?:^|\s)y:([0-9.]+|inf)", text)
if not matches:
    raise SystemExit("missing PSNR y")
print(matches[-1])
PY
)"
  rm -f "$ssim_log" "$psnr_log"

  quality_ge "$ssim" "$SSIM_MIN" || die "$out_name: SSIM $ssim below $SSIM_MIN"
  quality_ge "$luma_psnr" "$LUMA_PSNR_MIN" || die "$out_name: luma PSNR $luma_psnr below $LUMA_PSNR_MIN"

  source_sha="$(sha256_file "$source")"
  output_sha="$(sha256_file "$out")"
  {
    printf 'source=%s\n' "$source"
    printf 'output=%s\n' "$out_name"
    printf 'source_sha256=%s\n' "$source_sha"
    printf 'output_sha256=%s\n' "$output_sha"
    printf 'source_codec=%s\n' "$source_codec"
    printf 'source_pix_fmt=%s\n' "$source_pix_fmt"
    printf 'source_geometry=%sx%s\n' "$source_width" "$source_height"
    printf 'output_geometry=%sx%s\n' "$out_width" "$out_height"
    printf 'fps=%s\n' "$fps"
    printf 'downscale=lanczos-half\n'
    printf 'video_codec=h264\n'
    printf 'profile=%s\n' "$profile"
    printf 'pix_fmt=yuv420p\n'
    printf 'audio_codec=aac-copy\n'
    printf 'x264=crf:%s,preset:%s,cabac:0,bframes:0,ref:1,keyint:%s,min-keyint:%s,scenecut:0,open-gop:0\n' "$CRF" "$PRESET" "$GOP" "$GOP"
    printf 'ssim=%s\n' "$ssim"
    printf 'luma_psnr=%s\n' "$luma_psnr"
  } >"$provenance"

  printf '%s\t%s\t%s\t%sx%s\t%sx%s\t%s\th264\taac\tyuv420p\t%s\t%s\t%s\n' \
    "$out_name" "$source_sha" "$output_sha" "$source_width" "$source_height" \
    "$out_width" "$out_height" "$fps" "$profile" "$ssim" "$luma_psnr" >>"$manifest"
done

mkdir -p "$OUT_DIR"
find "$OUT_DIR" -maxdepth 1 -type f \( -name '*.mp4' -o -name '*.MP4' -o -name '*.provenance' -o -name 'manifest.tsv' -o -name '.sync-sha256' \) -delete
find "$stage" -maxdepth 1 -type f -exec mv {} "$OUT_DIR/" \;
trap - EXIT
rmdir "$stage"
echo "reencode-video-snaps-cortex-a9: wrote validated assets to $OUT_DIR"
