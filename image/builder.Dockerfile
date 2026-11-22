# Builder for OpenMinerOS ramdisk images. Pin by digest for release builds.
FROM rust:1-slim

RUN dpkg --add-architecture armhf \
 && apt-get update \
 && apt-get install -y --no-install-recommends \
      cpio gzip u-boot-tools device-tree-compiler qemu-user-static \
      busybox-static:armhf \
 && rm -rf /var/lib/apt/lists/* \
 && rustup target add armv7-unknown-linux-musleabihf
