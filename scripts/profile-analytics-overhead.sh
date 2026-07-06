#!/usr/bin/env bash
# Measure live frame analytics overhead on the MiSTer.
#
# This script uses scripts/mister only. It toggles the volatile analytics lease
# under /tmp so the launcher exercises the same frame-loop instrumentation that
# the telemetry stream enables, then samples MagiK and agent CPU jiffies.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MISTER="$HERE/scripts/mister"
LABEL="${1:-analytics-overhead-$(date -u +%Y%m%dT%H%M%SZ)}"
OUT_DIR="$HERE/build/analytics-overhead/$LABEL"
SAMPLE_SECS="${MISTER_ANALYTICS_OVERHEAD_SECS:-20}"
CPU_DELTA_LIMIT="${MISTER_ANALYTICS_OVERHEAD_LIMIT:-0.25}"
LEASE="/tmp/mister-magik/realtime-frame-analytics"
TSV="$OUT_DIR/analytics-overhead.tsv"

mkdir -p "$OUT_DIR"

remote_quote() {
  printf "'%s'" "${1//\'/\'\\\'\'}"
}

clear_lease() {
  "$MISTER" run "rm -f $(remote_quote "$LEASE")" >/dev/null
}

set_lease() {
  local mode="$1"
  "$MISTER" run "mkdir -p /tmp/mister-magik; printf '%s\n' '$mode' > $(remote_quote "$LEASE")" >/dev/null
}

sample_remote() {
  local scenario="$1"
  local mode="$2"
  local out="$OUT_DIR/$scenario.remote.txt"
  "$MISTER" run "
hz=\$(getconf CLK_TCK 2>/dev/null || echo 100)
dur=$SAMPLE_SECS
sample_proc() {
  name=\"\$1\"
  pid=\$(pidof \"\$name\" 2>/dev/null | awk '{print \$1}')
  if [ -z \"\$pid\" ] || [ ! -r \"/proc/\$pid/stat\" ]; then
    printf 'process\t%s\tmissing\t0\t0\t0\n' \"\$name\"
    return
  fi
  j0=\$(awk '{print \$14+\$15}' \"/proc/\$pid/stat\")
  sleep \"\$dur\"
  if [ ! -r \"/proc/\$pid/stat\" ]; then
    printf 'process\t%s\texited\t%s\t0\t0\n' \"\$name\" \"\$pid\"
    return
  fi
  j1=\$(awk '{print \$14+\$15}' \"/proc/\$pid/stat\")
  dj=\$((j1-j0))
  pct100=\$((dj*10000/(hz*dur)))
  printf 'process\t%s\tok\t%s\t%s\t%d.%02d\n' \"\$name\" \"\$pid\" \"\$dj\" \$((pct100/100)) \$((pct100%100))
}
echo scenario=$scenario mode=$mode hz=\$hz dur=$SAMPLE_SECS ts=\$(date +%s)
sample_proc mister-magik-fb &
p1=\$!
sample_proc mister-magik-agent &
p2=\$!
wait \$p1 \$p2
echo status_json
sed -n '1p' /tmp/mister-magik/status.json 2>/dev/null || true
" >"$out"
}

append_tsv() {
  local scenario="$1"
  local mode="$2"
  local remote="$OUT_DIR/$scenario.remote.txt"
  python3 - "$scenario" "$mode" "$remote" "$TSV" <<'PY'
import json
import sys
from pathlib import Path

scenario, mode, remote_path, tsv_path = sys.argv[1:]
text = Path(remote_path).read_text(encoding="utf-8", errors="replace")
status = {}
if "status_json\n" in text:
    candidate = text.split("status_json\n", 1)[1].strip().splitlines()
    if candidate:
        try:
            status = json.loads(candidate[0])
        except json.JSONDecodeError:
            status = {}
frame_budget = status.get("frame_budget") or {}
recent = frame_budget.get("recent_frames") or []
payload = json.dumps(frame_budget, separators=(",", ":"))
rows = []
for line in text.splitlines():
    if not line.startswith("process\t"):
        continue
    _, process, state, pid, jiffies, cpu = line.split("\t")
    rows.append(
        [
            scenario,
            mode,
            process,
            state,
            pid,
            jiffies,
            cpu,
            str(len(recent)),
            str(len(payload.encode("utf-8"))),
        ]
    )

path = Path(tsv_path)
new = not path.exists()
with path.open("a", encoding="utf-8") as f:
    if new:
        f.write("scenario\tmode\tprocess\tstate\tpid\tjiffies\tcpu_pct_one_core\trecent_frames\tframe_budget_json_bytes\n")
    for row in rows:
        f.write("\t".join(row) + "\n")
PY
}

run_scenario() {
  local scenario="$1"
  local mode="$2"
  echo "==> $scenario mode=$mode"
  if [[ "$mode" == "off" ]]; then
    clear_lease
  else
    set_lease "$mode"
  fi
  sleep 2
  sample_remote "$scenario" "$mode"
  append_tsv "$scenario" "$mode"
}

cleanup() {
  clear_lease >/dev/null 2>&1 || true
}
trap cleanup EXIT

echo "analytics_overhead_out=$OUT_DIR"
run_scenario baseline off
run_scenario wall wall
run_scenario thread thread
run_scenario process process
clear_lease

python3 - "$TSV" "$CPU_DELTA_LIMIT" <<'PY'
import csv
import sys
from pathlib import Path

path = Path(sys.argv[1])
limit = float(sys.argv[2])
rows = list(csv.DictReader(path.open(encoding="utf-8"), delimiter="\t"))
baseline = {
    row["process"]: float(row["cpu_pct_one_core"])
    for row in rows
    if row["scenario"] == "baseline" and row["state"] == "ok"
}
print(f"analytics_overhead_summary tsv={path}")
failed = False
for row in rows:
    if row["scenario"] == "baseline" or row["state"] != "ok":
        continue
    base = baseline.get(row["process"], 0.0)
    cpu = float(row["cpu_pct_one_core"])
    delta = cpu - base
    print(
        "analytics_delta scenario={scenario} process={process} cpu={cpu:.2f}% "
        "baseline={base:.2f}% delta={delta:.2f}% recent_frames={recent_frames} bytes={frame_budget_json_bytes}".format(
            cpu=cpu, base=base, delta=delta, **row
        )
    )
    if row["process"] == "mister-magik-fb" and delta > limit:
        failed = True
if failed:
    print(f"FAIL: MagiK analytics CPU delta exceeded {limit:.2f}% of one core", file=sys.stderr)
    sys.exit(1)
PY
