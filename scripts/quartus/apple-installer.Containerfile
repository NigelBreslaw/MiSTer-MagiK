FROM ubuntu:20.04

ENV DEBIAN_FRONTEND=noninteractive
RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        ca-certificates binfmt-support debootstrap qemu-user-static \
    && rm -rf /var/lib/apt/lists/*
