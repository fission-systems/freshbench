#!/usr/bin/env bash
# Install the v1 image's project library packages for each foreign
# architecture, one package at a time, never removing anything (--no-remove).
# A package Debian cannot co-install with the native one is skipped and
# recorded; the result file is part of the image identity.
set -u
PKGS="libncurses-dev libreadline-dev zlib1g-dev libbz2-dev liblzma-dev libssl-dev
libcurl4-openssl-dev libpcre2-dev libexpat1-dev libsqlite3-dev libevent-dev libjson-c-dev
libyaml-dev libxml2-dev libpcap-dev libffi-dev libuv1-dev libjansson-dev libmagic-dev
libedit-dev libcap-dev libseccomp-dev libpopt-dev libargon2-dev libsodium-dev libzstd-dev
liblz4-dev libgmp-dev libmpfr-dev libsdl2-dev libx11-dev libgtk-3-dev libglib2.0-dev
libusb-1.0-0-dev libmnl-dev libnl-3-dev libnl-genl-3-dev libudev-dev libsystemd-dev
libcmocka-dev check"
mkdir -p /etc/freshbench
out=/etc/freshbench/foreign-packages.txt
: > "$out"
for arch in i386 arm64 armhf; do
  for p in $PKGS; do
    if apt-get install -y -q --no-install-recommends --no-remove "$p:$arch" > /tmp/apt.log 2>&1; then
      echo "$arch $p installed" >> "$out"
    else
      echo "$arch $p skipped: $(grep -m1 -E '^E:' /tmp/apt.log | cut -c1-160)" >> "$out"
    fi
  done
done
dpkg-query -W -f '${Package}:${Architecture} ${Version}\n' | sort > /etc/freshbench/dpkg-versions.txt
grep -c installed "$out"; grep -c skipped "$out" || true
