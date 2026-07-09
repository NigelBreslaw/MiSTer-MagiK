FROM ubuntu:18.04

SHELL ["/bin/bash", "-o", "pipefail", "-c"]

ARG QUARTUS_RUN=QuartusLiteSetup-17.0.0.595-linux.run
ARG CYCLONEV_QDZ=cyclonev-17.0.0.595.qdz
ARG QUARTUS_SHA1=99ccfb15962febceba64de2dc9b28c47e5a3b8df
ARG CYCLONEV_SHA1=2198dedb99866f38d43ff6c029d4bd668e2bbb59
ARG QUARTUS_INSTALL_TIMEOUT=20m
ARG QUARTUS_INSTALL_HEARTBEAT_SECS=60

ENV DEBIAN_FRONTEND=noninteractive
RUN apt-get update \
  && apt-get install -y --no-install-recommends \
    bash ca-certificates coreutils file make perl procps python3 tar gzip xz-utils locales \
    libc6-i386 lib32stdc++6 lib32z1 \
    libfontconfig1 libfreetype6 libglib2.0-0 libice6 libncurses5 libsm6 \
    libx11-6 libxau6 libxdmcp6 libxext6 libxft2 libxi6 libxrender1 libxt6 \
    libxtst6 \
  && locale-gen en_US.UTF-8 \
  && rm -rf /var/lib/apt/lists/*

WORKDIR /tmp/quartus-installer
COPY ${QUARTUS_RUN} ./
COPY ${CYCLONEV_QDZ} ./
COPY install-quartus-in-image.sh ./

RUN QUARTUS_RUN="${QUARTUS_RUN}" \
  CYCLONEV_QDZ="${CYCLONEV_QDZ}" \
  QUARTUS_SHA1="${QUARTUS_SHA1}" \
  CYCLONEV_SHA1="${CYCLONEV_SHA1}" \
  QUARTUS_INSTALL_TIMEOUT="${QUARTUS_INSTALL_TIMEOUT}" \
  QUARTUS_INSTALL_HEARTBEAT_SECS="${QUARTUS_INSTALL_HEARTBEAT_SECS}" \
  bash ./install-quartus-in-image.sh

ENV LC_ALL=en_US.UTF-8 \
    LANG=en_US.UTF-8 \
    PATH=/opt/intelFPGA_lite/17.0/quartus/bin:$PATH

RUN quartus_sh --version
