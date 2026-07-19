#!/usr/bin/env bash
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

# Cheap host-side checks for retained shell/Rust tooling.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

MODE=full
case "${1:-}" in
  ""|--full) ;;
  --fast) MODE=fast ;;
  -h|--help)
    echo "usage: scripts/test-host-tools.sh [--fast|--full]"
    exit 0
    ;;
  *)
    echo "unknown argument: $1" >&2
    exit 2
    ;;
esac

python3 "$ROOT/scripts/checks/check-license-headers.py"
python3 "$ROOT/scripts/checks/check-agent-guidance.py"
python3 "$ROOT/scripts/checks/check-repository-layout.py"
python3 "$ROOT/scripts/checks/check-catalog-contention.py" --self-test
python3 "$ROOT/scripts/checks/check-catalog-rebuild.py" --self-test
python3 "$ROOT/scripts/tests/test-doctor.py"

if [ "$MODE" = full ]; then
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

cargo run --manifest-path "$ROOT/mister/tools/host/Cargo.toml" --quiet -- \
  ini-edit-local magik-boot "$TMP/MiSTer.ini" "$TMP/repaired.ini"
grep -q '^direct_video=0$' "$TMP/repaired.ini"
grep -q '^main=MiSTer_MagiK$' "$TMP/repaired.ini"
grep -q '^\[arcade_vertical\]$' "$TMP/repaired.ini"
grep -q '^video_mode=14$' "$TMP/repaired.ini"
grep -q '^direct_video=0$' "$TMP/repaired.ini"
grep -q '^video_mode=8$' "$TMP/repaired.ini"
fi

for script in \
  "$ROOT/scripts/checks/check-no-main-kill.sh" \
  "$ROOT/scripts/checks/check-no-direct-arcade-scene.sh" \
  "$ROOT/scripts/checks/check-scanout-slots-contract.sh" \
  "$ROOT/scripts/lib/bench-context-lib.sh" \
  "$ROOT/scripts/lib/arming-state-lib.sh" \
  "$ROOT/scripts/lib/benchmark-cleanup-lib.sh" \
  "$ROOT/scripts/lib/catalog-device-test-lib.sh" \
  "$ROOT/scripts/bench-catalog-rebuild.sh" \
  "$ROOT/scripts/bench-toolchain.sh" \
  "$ROOT/scripts/build-mister-agent.sh" \
  "$ROOT/scripts/deploy-rust.sh" \
  "$ROOT/scripts/device-catalog-acceptance.sh" \
  "$ROOT/scripts/device-catalog-destruction.sh" \
  "$ROOT/scripts/device-library-change-flow.sh" \
  "$ROOT/scripts/device-launch-return-smoke.sh" \
  "$ROOT/scripts/device-release-acceptance.sh" \
  "$ROOT/scripts/dev-rust" \
  "$ROOT/scripts/doctor" \
  "$ROOT/scripts/install-slint-boot.sh" \
  "$ROOT/scripts/lib/magik-layout.sh" \
  "$ROOT/scripts/lib/platform-manifest-lib.sh" \
  "$ROOT/scripts/magik-mode.sh" \
  "$ROOT/scripts/lib/library-sql-output-lib.sh" \
  "$ROOT/scripts/mister" \
  "$ROOT/scripts/mister-asset-diagnostics.sh" \
  "$ROOT/scripts/mister-magik-agent.sh" \
  "$ROOT/scripts/mister-shutdown-trace.sh" \
  "$ROOT/scripts/lib/mister-supervision-lib.sh" \
  "$ROOT/scripts/profile-first-scan.sh" \
  "$ROOT/scripts/profile-library-io.sh" \
  "$ROOT/scripts/profile-media-cold-boot.sh" \
  "$ROOT/scripts/profile-media-arcade-contention.sh" \
  "$ROOT/scripts/profile-arcade-scroll.sh" \
  "$ROOT/scripts/profile-catalog-contention.sh" \
  "$ROOT/scripts/profile-preview-index-refresh.sh" \
  "$ROOT/scripts/profile-preview-pack-decode.sh" \
  "$ROOT/scripts/profile-preview-scroll.sh" \
  "$ROOT/scripts/profile-screenshot-download.sh" \
  "$ROOT/scripts/regression-arm-noop.sh" \
  "$ROOT/scripts/lib/reboot-wait-lib.sh" \
  "$ROOT/scripts/restore-stock-boot.sh" \
  "$ROOT/scripts/switch-ui.sh" \
  "$ROOT/scripts/tests/test-magik-mode.sh" \
  "$ROOT/scripts/validate" \
  "$ROOT/apps/mister/build-arm.sh" \
  "$ROOT/apps/mister/build-arm64-apple-container.sh"; do
  bash -n "$script"
done

while IFS= read -r script; do
  bash -n "$script"
done < <(find "$ROOT/scripts/experiments" -type f -name '*.sh' | sort)

if [ "$MODE" = fast ]; then
  "$ROOT/scripts/checks/check-no-main-kill.sh"
  "$ROOT/scripts/checks/check-no-direct-arcade-scene.sh"
  "$ROOT/scripts/checks/check-scanout-slots-contract.sh"
  python3 "$ROOT/scripts/tests/test-kernel-scanout-workflows.py"
  python3 "$ROOT/scripts/tests/test-platform-bundle-workflow.py"
  python3 "$ROOT/scripts/tests/test-select-published-release.py"
  python3 "$ROOT/scripts/tests/test-game-databases-workflow.py"
  python3 "$ROOT/scripts/tests/test-distribution-workflow.py"
  python3 "$ROOT/scripts/tests/test-arm-build-contract.py"
  python3 "$ROOT/scripts/tests/test-ci-cache-identity.py"
  python3 "$ROOT/scripts/tests/test-ci-cache-contract.py"
  "$ROOT/scripts/tests/test-quartus-r2-cache.sh"
  "$ROOT/scripts/tests/test-apple-container-resources.sh"
  echo "fast host tool checks ok"
  exit 0
fi

python3 "$ROOT/scripts/tests/test-ci-cache-identity.py"
python3 "$ROOT/scripts/tests/test-ci-cache-contract.py"
"$ROOT/scripts/tests/test-quartus-r2-cache.sh"
"$ROOT/scripts/tests/test-apple-container-resources.sh"

switch_log="$TMP/switch-ui-calls.log"
cat >"$TMP/fake-mister" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
printf '%s\n' "$*" >>"$SWITCH_UI_TEST_LOG"
EOF
chmod +x "$TMP/fake-mister"
MISTER="$TMP/fake-mister" SWITCH_UI_TEST_LOG="$switch_log" \
  "$ROOT/scripts/switch-ui.sh" -stock >/dev/null
if ! grep -qx 'reboot-wait' "$switch_log"; then
  echo "frontend switch must delegate to the normal mode-switch reboot" >&2
  exit 1
fi

"$ROOT/scripts/tests/test-magik-mode.sh"
"$ROOT/scripts/tests/test-mister-magik-installer.sh"
python3 "$ROOT/scripts/tests/test-platform-component-id.py"
python3 "$ROOT/scripts/tests/test-kernel-scanout-workflows.py"
python3 "$ROOT/scripts/tests/test-platform-bundle.py"
python3 "$ROOT/scripts/tests/test-platform-bundle-workflow.py"
python3 "$ROOT/scripts/tests/test-embedded-catalog-release.py"
python3 "$ROOT/scripts/tests/test-game-databases-bundle.py"
python3 "$ROOT/scripts/tests/test-select-published-release.py"
python3 "$ROOT/scripts/tests/test-game-databases-workflow.py"
python3 "$ROOT/scripts/tests/test-arm-build-contract.py"

if command -v sqlite3 >/dev/null 2>&1 && command -v zip >/dev/null 2>&1; then
  package_tmp="$TMP/package-distribution"
  mkdir -p "$package_tmp/out"
  printf '#!/bin/sh\nexit 0\n' >"$package_tmp/mister-magik-fb"
  chmod 755 "$package_tmp/mister-magik-fb"
  printf 'ui\n' >"$package_tmp/mister-magik-fb.features"
  MISTER_MAGIK_BUILD_NUMBER=42 MISTER_MAGIK_VERSION=0.2.42 \
    bash -c 'source "$1/scripts/lib/bench-context-lib.sh"; bench_context_write_build_receipt "$2" "$1" release-device ui all' \
    _ "$ROOT" "$package_tmp/mister-magik-fb"
  cp "$package_tmp/mister-magik-fb" "$package_tmp/MiSTer_MagiK"
  printf 'module fixture\n' >"$package_tmp/mister_magik_scanout_slots.ko"
  printf 'rbf fixture\n' >"$package_tmp/menu-magik-vblank-latch.rbf"
  fixture_contract="$(printf contract | sha256sum | awk '{print $1}')"
  fixture_magik="$(git -C "$ROOT" rev-parse HEAD)"
  fixture_main="1111111111111111111111111111111111111111"
  fixture_menu="3333333333333333333333333333333333333333"
  printf 'platform_contract_sha256=%s\nmodule_sha256=%s\nvermagic=5.15.1-MiSTer fixture\nsource_revision=%s\n' \
    "$fixture_contract" "$(sha256sum "$package_tmp/mister_magik_scanout_slots.ko" | awk '{print $1}')" "$fixture_magik" \
    >"$package_tmp/scanout.metadata.txt"
  printf 'format=mister-magik-fpga-release-v1\nplatform_contract_sha256=%s\nmagik_commit=%s\nsource_commit=%s\nrbf_sha256=%s\n' \
    "$fixture_contract" "$fixture_magik" "$fixture_menu" \
    "$(sha256sum "$package_tmp/menu-magik-vblank-latch.rbf" | awk '{print $1}')" \
    >"$package_tmp/latch.metadata.txt"
  "$ROOT/scripts/release/platform/platform-manifest.py" generate \
    --output "$package_tmp/platform-v2.manifest" \
    --main "$package_tmp/MiSTer_MagiK" \
    --gui "$package_tmp/mister-magik-fb" \
    --scanout-module "$package_tmp/mister_magik_scanout_slots.ko" \
    --scanout-metadata "$package_tmp/scanout.metadata.txt" \
    --latch-rbf "$package_tmp/menu-magik-vblank-latch.rbf" \
    --latch-metadata "$package_tmp/latch.metadata.txt" \
    --main-revision "$fixture_main" --magik-revision "$fixture_magik" >/dev/null
  printf '{"format":"mister-magik-platform-bundle-v0.1","bundle_id":"%064d"}\n' 0 \
    >"$package_tmp/platform-bundle-v0.1.json"
  platform_args=(
    --version 0.2.42
    --build-number 42
    --main-bin "$package_tmp/MiSTer_MagiK"
    --main-source-revision "$fixture_main"
    --scanout-module "$package_tmp/mister_magik_scanout_slots.ko"
    --scanout-metadata "$package_tmp/scanout.metadata.txt"
    --latch-rbf "$package_tmp/menu-magik-vblank-latch.rbf"
    --latch-metadata "$package_tmp/latch.metadata.txt"
    --platform-manifest "$package_tmp/platform-v2.manifest"
    --platform-bundle-manifest "$package_tmp/platform-bundle-v0.1.json"
  )
  sqlite3 "$package_tmp/mame.sqlite3" \
    "CREATE TABLE mame_machines(setname TEXT PRIMARY KEY,parent_setname TEXT,title TEXT NOT NULL,players INTEGER,control_type TEXT,source_version TEXT NOT NULL) WITHOUT ROWID;
     WITH RECURSIVE seq(i) AS (VALUES(1) UNION ALL SELECT i+1 FROM seq WHERE i<50000)
     INSERT INTO mame_machines SELECT 'machine'||i,'','Machine '||i,1+(i%4),'joy','0.288 (mame0288)' FROM seq;
     CREATE TABLE mame_software_items(list_name TEXT NOT NULL,item_name TEXT NOT NULL);
     INSERT INTO mame_software_items VALUES('lynx','one'),('megadriv','one'),('n64','one'),('nes','one'),('saturn','one'),('sms','one'),('snes','one');"
  if "$ROOT/scripts/package-distribution.sh" \
      --binary "$package_tmp/mister-magik-fb" \
      --mame-sqlite "$package_tmp/mame.sqlite3" \
      --name forbidden-raw-database \
      --out-dir "$package_tmp/out" >/dev/null 2>&1; then
    echo "expected raw database package input to fail" >&2
    exit 1
  fi

  sqlite3 "$package_tmp/hbmame.sqlite3" <<'SQL'
CREATE TABLE mame_machines(
  setname TEXT PRIMARY KEY,
  parent_setname TEXT,
  title TEXT NOT NULL,
  players INTEGER,
  control_type TEXT,
  source_version TEXT NOT NULL
) WITHOUT ROWID;
WITH RECURSIVE seq(i) AS (
  VALUES(1)
  UNION ALL
  SELECT i + 1 FROM seq WHERE i < 5000
)
INSERT INTO mame_machines
SELECT 'dummy' || i, '', 'Dummy ' || i, 1+(i%4), 'joy', 'self-test' FROM seq;
INSERT INTO mame_machines VALUES('marpy', 'mappy', 'Marpy', 2, 'joy', 'self-test');
CREATE TABLE package_padding(data BLOB NOT NULL);
INSERT INTO package_padding VALUES(zeroblob(1048576));
SQL
  "$ROOT/scripts/release/databases/game-databases-bundle.py" create \
    --mame-sqlite "$package_tmp/mame.sqlite3" \
    --hbmame-sqlite "$package_tmp/hbmame.sqlite3" \
    --release-version 1 --mame-tag mame0288 \
    --mame-sha 1111111111111111111111111111111111111111 \
    --mame-listxml-asset mame0288lx.zip --mame-listxml-sha256 "$(printf 5%.0s {1..64})" \
    --hbmame-tag tag24532 \
    --hbmame-sha 2222222222222222222222222222222222222222 \
    --mame-builder-sha "$fixture_magik" --hbmame-builder-sha "$fixture_magik" \
    --output "$package_tmp/game-databases" >/dev/null
  "$ROOT/scripts/package-distribution.sh" \
    --binary "$package_tmp/mister-magik-fb" \
    --game-databases-release-dir "$package_tmp/game-databases" \
    "${platform_args[@]}" \
    --name valid-hbmame \
    --release-assets-dir "$package_tmp/release-assets" \
    --out-dir "$package_tmp/out" >/dev/null
  test -f "$package_tmp/release-assets/release-assets.json"
  test -f "$package_tmp/release-assets/SHA256SUMS"
  python3 - "$package_tmp/out/valid-hbmame.zip" "$fixture_magik" "$fixture_menu" <<'PY'
import sys
import zipfile

with zipfile.ZipFile(sys.argv[1]) as archive:
    names = set(archive.namelist())
    required = {
        "mister-magik/THIRD-PARTY-NOTICES.txt",
        "mister-magik/SOURCE-OFFER.txt",
        "Scripts/MiSTer-MagiK.sh",
        "mister-magik/licenses/MiSTer-MagiK-GPL-3.0-or-later.txt",
        "mister-magik/licenses/RUST-LIBRARIES.txt",
        "mister-magik/licenses/FFMPEG-LGPL-2.1-or-later.txt",
        "mister-magik/licenses/PRESS-START-2P-OFL-1.1.txt",
        "MiSTer_MagiK",
        "mister-magik/platform-v2.manifest",
        "mister-magik/platform-bundle-v0.1.json",
        "mister-magik/game-databases-manifest.json",
        "mister-magik/mister_magik_scanout_slots.ko",
        "mister-magik/mister_magik_scanout_slots.metadata.txt",
        "mister-magik/fpga/menu-magik-vblank-latch.rbf",
        "mister-magik/fpga/menu-magik-vblank-latch.metadata.txt",
    }
    missing = sorted(required - names)
    if missing:
        raise SystemExit(f"distribution missing legal files: {', '.join(missing)}")
    legacy_root_paths = {
        "THIRD-PARTY-NOTICES.txt",
        "SOURCE-OFFER.txt",
        "licenses/MiSTer-MagiK-GPL-3.0-or-later.txt",
        "licenses/RUST-LIBRARIES.txt",
        "licenses/FFMPEG-LGPL-2.1-or-later.txt",
        "licenses/PRESS-START-2P-OFL-1.1.txt",
    }
    unexpected = sorted(legacy_root_paths & names)
    if unexpected:
        raise SystemExit(f"distribution leaks legal files outside mister-magik/: {', '.join(unexpected)}")
    notices = archive.read("mister-magik/THIRD-PARTY-NOTICES.txt").decode()
    source_offer = archive.read("mister-magik/SOURCE-OFFER.txt").decode()
    release = archive.read("mister-magik/release-v1.txt").decode()
    if "game_database_version=1" not in release:
        raise SystemExit("distribution release identity is missing game database version")
    for expected in ("mame0288", "2222222222222222222222222222222222222222", "not ROM, BIOS, firmware, or game media"):
        if expected not in notices:
            raise SystemExit(f"distribution notices missing: {expected}")
    if "mister_magik_scanout_slots kernel module is also\nGPL-3.0-or-later" not in notices:
        raise SystemExit("distribution notices omit the kernel-module license")
    module_source = (
        "https://github.com/NigelBreslaw/MiSTer-MagiK/tree/"
        f"{sys.argv[2]}/mister/platform/kernel/scanout-slots"
    )
    if module_source not in source_offer:
        raise SystemExit("distribution source offer omits exact kernel-module source")
    menu_source = (
        "https://github.com/MiSTer-devel/Menu_MiSTer/tree/"
        f"{sys.argv[3]}"
    )
    if menu_source not in source_offer:
        raise SystemExit("distribution source offer omits exact Menu_MiSTer source")
    if "FFmpeg 8.1.2 source" not in source_offer:
        raise SystemExit("distribution source offer is missing FFmpeg source")
PY
fi

if grep -R -E 'scripts/(bench-effects|profile-camera-effects|profile-sprite-effects|profile-text-effects|profile-raster-effects|profile-transition-effects)\.sh|scripts/experiments/(profile-preview-transition-mega|bench-effects|profile-camera-effects|profile-sprite-effects|profile-text-effects|profile-raster-effects|profile-transition-effects)\.sh' \
  "$ROOT/AGENTS.md" "$ROOT/docs/benchmarking.md" "$ROOT/apps/mister/BUILD.md" "$ROOT/apps/mister/ui/bench/README.md" >/dev/null; then
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
if 'run_capture "supervised-reboot-soak-4" "$MISTER" agent boot-profile 4 --timeout 60 --fail-on-timeout' not in text:
    print("launcher lifecycle supervised reboot soak is not fixed at four required samples", file=sys.stderr)
    sys.exit(1)
if re.search(r'if tier_selected "soak"; then\s+run_supervised_reboot_soak', body):
    print("launcher lifecycle supervised reboot soak must not be gated by the optional long-soak tier", file=sys.stderr)
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
    "arming_state_clear",
    "arming_state_assert_clean",
]:
    if needle not in body:
        print(f"fs-fault cleanup_on_exit missing {needle}", file=sys.stderr)
        sys.exit(1)
PY

"$ROOT/scripts/checks/check-no-main-kill.sh"
"$ROOT/scripts/checks/check-no-direct-arcade-scene.sh"
"$ROOT/scripts/checks/check-scanout-slots-contract.sh"
python3 "$ROOT/scripts/checks/check-latch-protocol.py"
python3 "$ROOT/scripts/tests/test-scanout-platform-contract.py"
if rg -n '(^|[^[:alnum:]_])(println!|eprintln!|print!|eprint!)' "$ROOT/apps/mister/src" "$ROOT/crates/catalog/src" \
  -g '*.rs' \
  -g '!**/bin/**' \
  -g '!fallible_log.rs' >/dev/null; then
  echo "standard Rust stdio macros panic on output errors; use ui_log*/ui_errln* instead" >&2
  exit 1
fi
source "$ROOT/scripts/lib/bench-context-lib.sh"
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
fast_context="$(bench_context_binary_fields release launcher ui,bench-tools "$ROOT/does-not-exist" bench-tools verified)"
case "$fast_context" in
  *$'binary_scope=fast-launcher-scope'*$'runtime_type=bench-tools'*$'deployment_state=verified'*$'production_restore_required=1'*) ;;
  *) echo "unexpected fast benchmark context: $fast_context" >&2; exit 1 ;;
esac
printf 'benchmark identity self-test\n' >"$TMP/identity.bin"
printf 'ui\n' >"$TMP/identity.bin.features"
bench_context_write_build_receipt "$TMP/identity.bin" "$ROOT" release-device ui launcher
[[ "$(bench_context_build_receipt_field "$TMP/identity.bin" build_number)" == "$(git -C "$ROOT" rev-list --count HEAD)" ]]
[[ "$(bench_context_build_receipt_field "$TMP/identity.bin" version)" == "0.2.$(git -C "$ROOT" rev-list --count HEAD)" ]]
identity_sha="$(bench_context_sha256_file "$TMP/identity.bin")"
[[ "${#identity_sha}" -eq 64 ]]
bench_context_require_verified_identity verified "$identity_sha" "$identity_sha"
bench_context_require_binary_contract "$TMP/identity.bin" "$identity_sha" ui release-device launcher
if bench_context_require_binary_contract "$TMP/identity.bin" "$identity_sha" ui,bench-tools release-device launcher; then
  echo "benchmark identity accepted the wrong feature receipt" >&2
  exit 1
fi
printf 'tampered\n' >>"$TMP/identity.bin"
tampered_identity_sha="$(bench_context_sha256_file "$TMP/identity.bin")"
if bench_context_require_binary_contract "$TMP/identity.bin" "$tampered_identity_sha" ui release-device launcher; then
  echo "benchmark identity accepted a build receipt bound to a different binary hash" >&2
  exit 1
fi
bad_identity_sha="${identity_sha%?}$([[ "${identity_sha: -1}" == "0" ]] && printf '1' || printf '0')"
if bench_context_require_verified_identity verified "$identity_sha" "$bad_identity_sha"; then
  echo "benchmark identity accepted a hash mismatch" >&2
  exit 1
fi
source_fields="$(bench_context_source_fields "$ROOT")"
case "$source_fields" in
  source_commit=*$'\tsource_commit_short='*$'\tsource_dirty='[01]) ;;
  *) echo "unexpected source provenance: $source_fields" >&2; exit 1 ;;
esac
set +e
retired_library_io_output="$("$ROOT/scripts/profile-library-io.sh" --self-test 2>&1)"
retired_library_io_status=$?
set -e
if [ "$retired_library_io_status" -ne 2 ]; then
  echo "retired library I/O profiler returned $retired_library_io_status, expected 2" >&2
  exit 1
fi
case "$retired_library_io_output" in
  *"retired V2 monolith"*) ;;
  *) echo "retired library I/O profiler did not explain its V2 retirement" >&2; exit 1 ;;
esac
echo "retired library I/O profiler fail-closed self-test ok"
bash "$ROOT/scripts/lib/benchmark-cleanup-lib.sh" --self-test
bash "$ROOT/scripts/lib/arming-state-lib.sh" --self-test
bash "$ROOT/scripts/checks/check-no-direct-main-fifo.sh"
bash "$ROOT/scripts/lib/platform-manifest-lib.sh" --self-test
bash "$ROOT/scripts/lib/catalog-device-test-lib.sh" --self-test
"$ROOT/scripts/bench-toolchain.sh" --self-test
"$ROOT/scripts/lib/library-sql-output-lib.sh"
"$ROOT/scripts/lib/reboot-wait-lib.sh"
"$ROOT/scripts/profile-first-scan.sh" --self-test
"$ROOT/scripts/profile-media-cold-boot.sh" --self-test
"$ROOT/scripts/profile-media-arcade-contention.sh" --self-test
"$ROOT/scripts/profile-arcade-scroll.sh" --self-test
"$ROOT/scripts/device-catalog-acceptance.sh" --self-test
"$ROOT/scripts/profile-preview-pack-decode.sh" --self-test
"$ROOT/scripts/profile-preview-scroll.sh" --self-test
python3 -m py_compile "$ROOT/scripts/device/diagnostics/reboot-shutdown-summary.py"
python3 "$ROOT/scripts/tests/test-generate-downloader-db.py"
python3 "$ROOT/scripts/tests/test-distribution-workflow.py"
env RUSTC_WRAPPER= cargo test --manifest-path "$ROOT/mister/tools/host/Cargo.toml" --quiet

echo "host tool checks ok"
