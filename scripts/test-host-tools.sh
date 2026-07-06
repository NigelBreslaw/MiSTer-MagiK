#!/usr/bin/env bash
# Cheap host-side checks for retained shell/Rust tooling.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

cat >"$TMP/MiSTer.ini" <<'EOF'
[MiSTer]
direct_video=1
main=mister-magik-fb

[arcade_vertical]
direct_video=0
video_mode=14

[Menu]
video_mode=4

[MiSTer]
fb_terminal=1
EOF

cargo run --manifest-path "$ROOT/tools/mister/Cargo.toml" --quiet -- \
  ini-edit-local magik-boot "$TMP/MiSTer.ini" "$TMP/repaired.ini"
grep -q '^direct_video=0$' "$TMP/repaired.ini"
grep -q '^main=MiSTer_MagiK$' "$TMP/repaired.ini"
grep -q '^\[arcade_vertical\]$' "$TMP/repaired.ini"
grep -q '^video_mode=14$' "$TMP/repaired.ini"
grep -q '^direct_video=0$' "$TMP/repaired.ini"
grep -q '^video_mode=8$' "$TMP/repaired.ini"

for script in \
  "$ROOT/scripts/check-no-main-kill.sh" \
  "$ROOT/scripts/check-no-direct-arcade-scene.sh" \
  "$ROOT/scripts/bench-context-lib.sh" \
  "$ROOT/scripts/bench-toolchain.sh" \
  "$ROOT/scripts/build-mister-agent.sh" \
  "$ROOT/scripts/deploy-rust.sh" \
  "$ROOT/scripts/device-catalog-acceptance.sh" \
  "$ROOT/scripts/device-catalog-destruction.sh" \
  "$ROOT/scripts/device-library-change-flow.sh" \
  "$ROOT/scripts/device-launch-return-smoke.sh" \
  "$ROOT/scripts/device-release-acceptance.sh" \
  "$ROOT/scripts/dev-rust" \
  "$ROOT/scripts/install-slint-boot.sh" \
  "$ROOT/scripts/library-sql-output-lib.sh" \
  "$ROOT/scripts/mister" \
  "$ROOT/scripts/mister-asset-diagnostics.sh" \
  "$ROOT/scripts/mister-fifo-lib.sh" \
  "$ROOT/scripts/mister-magik-agent.sh" \
  "$ROOT/scripts/mister-shutdown-trace.sh" \
  "$ROOT/scripts/mister-supervision-lib.sh" \
  "$ROOT/scripts/profile-first-scan.sh" \
  "$ROOT/scripts/profile-media-cold-boot.sh" \
  "$ROOT/scripts/profile-preview-index-refresh.sh" \
  "$ROOT/scripts/profile-preview-pack-decode.sh" \
  "$ROOT/scripts/profile-preview-scroll.sh" \
  "$ROOT/scripts/profile-screenshot-download.sh" \
  "$ROOT/scripts/reboot-wait-lib.sh" \
  "$ROOT/scripts/restore-stock-boot.sh" \
  "$ROOT/magik-gui/build-arm.sh"; do
  bash -n "$script"
done

while IFS= read -r script; do
  bash -n "$script"
done < <(find "$ROOT/scripts/experiments" -type f -name '*.sh' | sort)

if grep -R -E 'scripts/(bench-effects|profile-camera-effects|profile-sprite-effects|profile-text-effects|profile-raster-effects|profile-transition-effects)\.sh|scripts/experiments/(profile-preview-transition-mega|bench-effects|profile-camera-effects|profile-sprite-effects|profile-text-effects|profile-raster-effects|profile-transition-effects)\.sh' \
  "$ROOT/AGENTS.md" "$ROOT/docs/benchmarking.md" "$ROOT/magik-gui/BUILD.md" "$ROOT/magik-gui/ui/bench/README.md" >/dev/null; then
  echo "old effect experiment script path found in current benchmark docs" >&2
  exit 1
fi

"$ROOT/scripts/device-release-acceptance.sh" --help >/dev/null
if "$ROOT/scripts/device-release-acceptance.sh" --tiers nope >/dev/null 2>&1; then
  echo "expected invalid tier to fail" >&2
  exit 1
fi
if "$ROOT/scripts/device-release-acceptance.sh" --fast --tiers health >/dev/null 2>&1; then
  echo "expected --fast plus --tiers to fail" >&2
  exit 1
fi
python3 - "$ROOT/scripts/device-release-acceptance.sh" <<'PY'
import re
import sys

path = sys.argv[1]
text = open(path, encoding="utf-8").read()
for needle in [
    "SUMMARY=\"$OUT/summary.json\"",
    "RESULTS_TSV=\"$OUT/results.tsv\"",
    "record_skip()",
    "append_result_table()",
    "write_summary_json()",
    'write_summary_json "PASS"',
    'write_summary_json "FAIL"',
]:
    if needle not in text:
        print(f"device acceptance reporting contract missing {needle!r}", file=sys.stderr)
        sys.exit(1)
if re.search(r"record_ok [^\n]*skipp?ed", text, re.I):
    print("device acceptance skipped checks must use record_skip, not record_ok", file=sys.stderr)
    sys.exit(1)
match = re.search(
    r"run_tier_launcher_lifecycle\(\) \{(?P<body>.*?)\n\}\n\nrun_tier_handoff\(\)",
    text,
    re.S,
)
if not match:
    print("could not find launcher lifecycle tier body", file=sys.stderr)
    sys.exit(1)
body = match.group("body")
if "run_supervised_reboot_soak" not in body:
    print("launcher lifecycle no longer references supervised reboot soak", file=sys.stderr)
    sys.exit(1)
gate = re.search(r'if tier_selected "soak"; then\s+run_supervised_reboot_soak', body)
if not gate:
    print("launcher lifecycle supervised reboot soak is not gated by selected soak tier", file=sys.stderr)
    sys.exit(1)
PY

python3 - "$ROOT/scripts/device-fs-fault-reset.sh" <<'PY'
import re
import sys

path = sys.argv[1]
text = open(path, encoding="utf-8").read()
match = re.search(r"cleanup_on_exit\(\) \{(?P<body>.*?)\n\}", text, re.S)
if not match:
    print("could not find fs-fault cleanup_on_exit body", file=sys.stderr)
    sys.exit(1)
body = match.group("body")
for needle in [
    "REMOTE_ENV",
    "REMOTE_FAULT_ENV",
    "REMOTE_MARKER",
    "REMOTE_SESSION",
    "REMOTE_REBUILD_MARKER",
]:
    if needle not in body:
        print(f"fs-fault cleanup_on_exit missing {needle}", file=sys.stderr)
        sys.exit(1)
PY

"$ROOT/scripts/check-no-main-kill.sh"
"$ROOT/scripts/check-no-direct-arcade-scene.sh"
if rg -n '(^|[^[:alnum:]_])(println!|eprintln!|print!|eprint!)' "$ROOT/magik-gui/src" "$ROOT/magik-gui/catalog/src" \
  -g '*.rs' \
  -g '!fallible_log.rs' >/dev/null; then
  echo "standard Rust stdio macros panic on output errors; use ui_log*/ui_errln* instead" >&2
  exit 1
fi
source "$ROOT/scripts/bench-context-lib.sh"
verified_context="$(bench_context_binary_fields release-device launcher ui "$ROOT/does-not-exist" production verified)"
case "$verified_context" in
  *$'binary_scope=launcher-scope'*$'runtime_type=production'*$'deployment_state=verified'*$'production_restore_required=0'*) ;;
  *) echo "unexpected verified benchmark context: $verified_context" >&2; exit 1 ;;
esac
unknown_context="$(bench_context_binary_fields release-device launcher ui "$ROOT/does-not-exist" production unverified-skip-build)"
case "$unknown_context" in
  *$'binary_scope=deployed-unknown'*$'runtime_type=deployed-unknown'*$'deployment_state=unverified-skip-build'*$'production_restore_required=unknown'*) ;;
  *) echo "unexpected unverified benchmark context: $unknown_context" >&2; exit 1 ;;
esac
profile_context="$(bench_context_binary_fields release-device-profile launcher ui,profile "$ROOT/does-not-exist" profile verified)"
case "$profile_context" in
  *$'binary_scope=profile-launcher-scope'*$'runtime_type=profile'*$'deployment_state=verified'*$'production_restore_required=1'*) ;;
  *) echo "unexpected profile benchmark context: $profile_context" >&2; exit 1 ;;
esac
"$ROOT/scripts/bench-toolchain.sh" --self-test
"$ROOT/scripts/library-sql-output-lib.sh"
"$ROOT/scripts/reboot-wait-lib.sh"
"$ROOT/scripts/mister-fifo-lib.sh"
"$ROOT/scripts/profile-first-scan.sh" --self-test
"$ROOT/scripts/profile-media-cold-boot.sh" --self-test
"$ROOT/scripts/profile-preview-pack-decode.sh" --self-test
"$ROOT/scripts/profile-preview-scroll.sh" --self-test
python3 -m py_compile "$ROOT/scripts/reboot-shutdown-summary.py"
env RUSTC_WRAPPER= cargo test --manifest-path "$ROOT/tools/mister/Cargo.toml" --quiet

echo "host tool checks ok"
