FROM ubuntu:18.04

ENV DEBIAN_FRONTEND=noninteractive
RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        bash ca-certificates file make perl procps python3 rsync strace tar gzip xz-utils locales \
        libc6-i386 lib32stdc++6 lib32z1 \
        libfontconfig1 libfreetype6 libglib2.0-0 libgoogle-perftools4 libice6 \
        libncurses5 libsm6 libx11-6 libxau6 libxdmcp6 libxext6 libxft2 libxi6 \
        libxrender1 libxt6 libxtst6 \
    && locale-gen en_US.UTF-8 \
    && rm -rf /var/lib/apt/lists/*

ENV LC_ALL=en_US.UTF-8
ENV LANG=en_US.UTF-8
ENV PATH=/opt/intelFPGA_lite/17.0/quartus/bin:${PATH}
