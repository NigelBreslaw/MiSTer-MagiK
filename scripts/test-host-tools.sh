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
  "$ROOT/scripts/check-scanout-slots-contract.sh" \
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

if command -v sqlite3 >/dev/null 2>&1 && command -v zip >/dev/null 2>&1; then
  package_tmp="$TMP/package-distribution"
  mkdir -p "$package_tmp/out"
  printf '#!/bin/sh\nexit 0\n' >"$package_tmp/mister-magik-fb"
  chmod 755 "$package_tmp/mister-magik-fb"
  sqlite3 "$package_tmp/mame.sqlite3" \
    "CREATE TABLE release_check(name TEXT PRIMARY KEY, value TEXT NOT NULL); INSERT INTO release_check VALUES('kind','package-self-test');"
  sqlite3 "$package_tmp/tiny-hbmame.sqlite3" \
    "CREATE TABLE mame_machines(setname TEXT PRIMARY KEY, parent_setname TEXT, title TEXT NOT NULL) WITHOUT ROWID; INSERT INTO mame_machines VALUES('mappyj','mappy','Mappy');"
  if "$ROOT/scripts/package-distribution.sh" \
      --binary "$package_tmp/mister-magik-fb" \
      --mame-sqlite "$package_tmp/mame.sqlite3" \
      --name missing-provenance \
      --out-dir "$package_tmp/out" >/dev/null 2>&1; then
    echo "expected package without MAME provenance to fail" >&2
    exit 1
  fi
  if "$ROOT/scripts/package-distribution.sh" \
      --binary "$package_tmp/mister-magik-fb" \
      --mame-sqlite "$package_tmp/mame.sqlite3" \
      --mame-source-ref test-fixture \
      --hbmame-sqlite "$package_tmp/tiny-hbmame.sqlite3" \
      --hbmame-source-revision test-fixture \
      --name tiny-hbmame \
      --out-dir "$package_tmp/out" >/dev/null 2>&1; then
    echo "expected tiny HBMAME metadata DB package to fail" >&2
    exit 1
  fi

  sqlite3 "$package_tmp/hbmame.sqlite3" <<'SQL'
CREATE TABLE mame_machines(
  setname TEXT PRIMARY KEY,
  parent_setname TEXT,
  title TEXT NOT NULL,
  source_version TEXT NOT NULL
) WITHOUT ROWID;
WITH RECURSIVE seq(i) AS (
  VALUES(1)
  UNION ALL
  SELECT i + 1 FROM seq WHERE i < 5000
)
INSERT INTO mame_machines
SELECT 'dummy' || i, '', 'Dummy ' || i, 'self-test' FROM seq;
INSERT INTO mame_machines VALUES('marpy', 'mappy', 'Marpy', 'self-test');
CREATE TABLE package_padding(data BLOB NOT NULL);
INSERT INTO package_padding VALUES(zeroblob(1048576));
SQL
  "$ROOT/scripts/package-distribution.sh" \
    --binary "$package_tmp/mister-magik-fb" \
    --mame-sqlite "$package_tmp/mame.sqlite3" \
    --mame-source-ref test-fixture \
    --hbmame-sqlite "$package_tmp/hbmame.sqlite3" \
    --hbmame-source-revision test-fixture \
    --name valid-hbmame \
    --out-dir "$package_tmp/out" >/dev/null
  python3 - "$package_tmp/out/valid-hbmame.zip" <<'PY'
import sys
import zipfile

with zipfile.ZipFile(sys.argv[1]) as archive:
    names = set(archive.namelist())
    required = {
        "THIRD-PARTY-NOTICES.txt",
        "SOURCE-OFFER.txt",
        "licenses/MiSTer-MagiK-GPL-3.0-or-later.txt",
        "licenses/RUST-LIBRARIES.txt",
        "licenses/FFMPEG-LGPL-2.1-or-later.txt",
        "licenses/PRESS-START-2P-OFL-1.1.txt",
    }
    missing = sorted(required - names)
    if missing:
        raise SystemExit(f"distribution missing legal files: {', '.join(missing)}")
    notices = archive.read("THIRD-PARTY-NOTICES.txt").decode()
    source_offer = archive.read("SOURCE-OFFER.txt").decode()
    for expected in ("test-fixture", "not ROM, BIOS, firmware, or game media"):
        if expected not in notices:
            raise SystemExit(f"distribution notices missing: {expected}")
    if "FFmpeg 8.1.2 source" not in source_offer:
        raise SystemExit("distribution source offer is missing FFmpeg source")
PY
fi

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
"$ROOT/scripts/check-scanout-slots-contract.sh"
python3 "$ROOT/scripts/test-scanout-platform-contract.py"
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
