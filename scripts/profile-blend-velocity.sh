#!/usr/bin/env bash
# Capture the synthetic blend/fade velocity-scroll benchmark on the MiSTer.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MISTER="$HERE/scripts/mister"
OUT_DIR="$HERE/build/blend-velocity-profiles"
REMOTE="/media/fat/mister-magik/mister-magik-fb"

usage() {
  cat <<'EOF'
Usage: scripts/profile-blend-velocity.sh [SECS] [LABEL] [VARIANT] [--skip-build|--deploy-fast|--deploy-device]

Runs `ui blend_velocity` with `MISTER_BLEND_BENCH_TRACE`, pulls the raw TSV/log,
then prints split phase timing summaries.

Optional host env:
  MISTER_BLEND_BENCH_FADE_H=<px>  override fade band height for this run

VARIANT:
  baseline   real fade blend + fade/body/selection copies (default)
  real-text  same fade/copy path, with cached text rows instead of synthetic rows
  copy-only  copy fade rows without blending, isolating framebuffer writes
  no-fade    copy only the moving body + selection frame

Default: --skip-build, useful when the desired binary is already deployed.
EOF
}

secs="15"
label="blend-velocity-$(date -u +%Y%m%dT%H%M%SZ)"
variant="baseline"
deploy="skip"
positionals=()

while [[ $# -gt 0 ]]; do
  case "$1" in
    --skip-build) deploy="skip"; shift ;;
    --deploy-fast) deploy="fast"; shift ;;
    --deploy-device) deploy="device"; shift ;;
    -h|--help) usage; exit 0 ;;
    *)
      positionals+=("$1")
      shift
      ;;
  esac
done

if [[ "${#positionals[@]}" -gt 3 ]]; then
  echo "unexpected argument: ${positionals[3]}" >&2
  usage >&2
  exit 2
fi
if [[ "${#positionals[@]}" -ge 1 ]]; then
  secs="${positionals[0]}"
fi
if [[ "${#positionals[@]}" -ge 2 ]]; then
  label="${positionals[1]}"
fi
if [[ "${#positionals[@]}" -ge 3 ]]; then
  variant="${positionals[2]}"
fi

if [[ ! "$secs" =~ ^[0-9]+$ ]]; then
  echo "secs must be an integer number of seconds" >&2
  exit 2
fi
if [[ ! "$label" =~ ^[A-Za-z0-9_.-]+$ ]]; then
  echo "label must contain only letters, numbers, _, ., or -" >&2
  exit 2
fi
case "$variant" in
  baseline|real-text|real_text|text|copy-only|copy|no-fade|nofade|body-only) ;;
  *)
    echo "variant must be baseline, real-text, copy-only, or no-fade" >&2
    exit 2
    ;;
esac
fade_env=""
if [[ -n "${MISTER_BLEND_BENCH_FADE_H:-}" ]]; then
  if [[ ! "$MISTER_BLEND_BENCH_FADE_H" =~ ^[0-9]+$ ]] || [[ "$MISTER_BLEND_BENCH_FADE_H" -eq 0 ]]; then
    echo "MISTER_BLEND_BENCH_FADE_H must be a positive integer" >&2
    exit 2
  fi
  fade_env="MISTER_BLEND_BENCH_FADE_H=$MISTER_BLEND_BENCH_FADE_H "
fi

mkdir -p "$OUT_DIR"
remote_tsv="/tmp/${label}-blend-velocity.tsv"
remote_log="/tmp/${label}-blend-velocity.log"
local_tsv="$OUT_DIR/${label}-blend-velocity.tsv"
local_log="$OUT_DIR/${label}-blend-velocity.log"

case "$deploy" in
  fast) "$HERE/scripts/deploy-rust.sh" --fast ;;
  device) "$HERE/scripts/deploy-rust.sh" --device ;;
  skip) ;;
esac

echo "==> Capture blend_velocity secs=$secs label=$label variant=$variant deploy=$deploy"
if ! "$MISTER" run "kill -9 \$(pidof mister-magik-fb) 2>/dev/null || true; kill -9 \$(pidof MiSTer_MagiK) 2>/dev/null || true; kill -9 \$(pidof MiSTer) 2>/dev/null || true; rm -f $remote_tsv $remote_log; sleep 5; ${fade_env}MISTER_BLEND_BENCH_VARIANT=$variant MISTER_BLEND_BENCH_TRACE=$remote_tsv $REMOTE ui blend_velocity $secs >$remote_log 2>&1; status=\$?; grep -E 'blend_velocity|done:' $remote_log || true; test -s $remote_tsv || status=1; exit \$status"; then
  "$MISTER" get "$remote_log" "$local_log" || true
  echo "blend velocity profile failed; see $local_log" >&2
  exit 1
fi

"$MISTER" get "$remote_tsv" "$local_tsv"
"$MISTER" get "$remote_log" "$local_log"
echo "wrote $local_tsv"
echo "wrote $local_log"
echo
python3 - "$local_tsv" <<'PY'
import csv
import statistics
import sys

path = sys.argv[1]
with open(path, newline="") as f:
    rows = list(csv.DictReader(f, delimiter="\t"))

if len(rows) > 1:
    rows = rows[1:]

print(f"{path}: frames={len(rows)} include_first=False")
if not rows:
    raise SystemExit(0)

def pct(values, q):
    values = sorted(values)
    idx = max(0, min(len(values) - 1, int(len(values) * q) - 1))
    return values[idx]

for key in [
    "surface_us",
    "fade_blend_us",
    "fade_copy_us",
    "body_copy_us",
    "selection_copy_us",
    "vsync_us",
    "wall_us",
    "rows",
    "px",
]:
    values = [int(float(row[key])) for row in rows]
    print(
        f"  {key:<18} "
        f"p50={int(statistics.median(values)):>6} "
        f"p95={pct(values, 0.95):>6} "
        f"p99={pct(values, 0.99):>6} "
        f"max={max(values):>6} "
        f"avg={statistics.mean(values):>8.1f}"
    )
PY
