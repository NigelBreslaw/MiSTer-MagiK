# syntax=docker/dockerfile:1.7
FROM ubuntu:18.04

LABEL org.opencontainers.image.source="https://github.com/NigelBreslaw/MiSTer-MagiK"
LABEL org.opencontainers.image.description="MiSTer MagiK Quartus Prime Lite 17.0 Cyclone V build image"

ENV DEBIAN_FRONTEND=noninteractive
RUN apt-get update \
  && apt-get install -y --no-install-recommends \
    bash ca-certificates curl file make perl procps python3 strace tar gzip xz-utils locales \
    libc6-i386 lib32stdc++6 lib32z1 \
    libfontconfig1 libfreetype6 libglib2.0-0 libice6 libncurses5 libsm6 \
    libx11-6 libxau6 libxdmcp6 libxext6 libxft2 libxi6 libxrender1 libxt6 \
    libxtst6 \
  && locale-gen en_US.UTF-8 \
  && rm -rf /var/lib/apt/lists/*

ENV LC_ALL=en_US.UTF-8
ENV LANG=en_US.UTF-8
ENV PATH=/opt/intelFPGA_lite/17.0/quartus/bin:$PATH

RUN --mount=type=secret,id=quartus_run_url,required=true \
    --mount=type=secret,id=cyclonev_qdz_url,required=true \
    bash <<'EOF'
set -euo pipefail

install_dir=/opt/intelFPGA_lite/17.0
payload_dir=/tmp/quartus-installer
quartus_run="$payload_dir/QuartusLiteSetup-17.0.0.595-linux.run"
cyclonev_qdz="$payload_dir/cyclonev-17.0.0.595.qdz"
quartus_url="$(cat /run/secrets/quartus_run_url)"
cyclonev_url="$(cat /run/secrets/cyclonev_qdz_url)"

download_payload() {
  local url="$1"
  local output="$2"
  curl \
    --fail \
    --location \
    --show-error \
    --retry 5 \
    --retry-all-errors \
    --retry-delay 10 \
    --retry-max-time 900 \
    --connect-timeout 30 \
    --max-time 900 \
    --speed-time 120 \
    --speed-limit 1048576 \
    --output "$output" \
    "$url"
}

verify_sha1() {
  local path="$1"
  local expected="$2"
  local actual
  actual="$(sha1sum "$path" | awk '{print $1}')"
  if [[ "$actual" != "$expected" ]]; then
    echo "sha1 mismatch for $path" >&2
    echo "expected $expected" >&2
    echo "actual   $actual" >&2
    exit 1
  fi
}

mkdir -p "$payload_dir" /opt/intelFPGA_lite
download_payload "$quartus_url" "$quartus_run"
download_payload "$cyclonev_url" "$cyclonev_qdz"
verify_sha1 "$quartus_run" "99ccfb15962febceba64de2dc9b28c47e5a3b8df"
verify_sha1 "$cyclonev_qdz" "2198dedb99866f38d43ff6c029d4bd668e2bbb59"

chmod +x "$quartus_run"
set +e
timeout 20m bash -lc '
  set +e
  /tmp/quartus-installer/QuartusLiteSetup-17.0.0.595-linux.run --mode unattended --unattendedmodeui minimal --installdir /opt/intelFPGA_lite/17.0 &
  installer_pid=$!
  while kill -0 "$installer_pid" 2>/dev/null; do
    echo "quartus installer still running $(date -u +%Y-%m-%dT%H:%M:%SZ)"
    du -sh /opt/intelFPGA_lite/17.0 2>/dev/null || true
    sleep 60
  done &
  heartbeat_pid=$!
  wait "$installer_pid"
  installer_status=$?
  kill "$heartbeat_pid" 2>/dev/null || true
  wait "$heartbeat_pid" 2>/dev/null || true
  exit "$installer_status"
'
install_status=$?
set -e

if [[ "$install_status" -ne 0 && "$install_status" -ne 124 ]]; then
  echo "Quartus installer failed with status $install_status" >&2
  exit "$install_status"
fi
if [[ "$install_status" -eq 124 ]]; then
  log="$install_dir/logs/quartus-17.0.0.595-linux-install.log"
  if [[ ! -f "$log" ]] || ! tail -50 "$log" | grep -q 'Installation completed'; then
    echo "Quartus installer timed out and completion was not found in $log" >&2
    exit 124
  fi
  echo "Quartus installer timed out after completion; continuing with completed install" >&2
fi

test -x "$install_dir/quartus/bin/quartus_sh"
rm -rf "$payload_dir" /tmp/*
quartus_sh --version
quartus_map --version
EOF
