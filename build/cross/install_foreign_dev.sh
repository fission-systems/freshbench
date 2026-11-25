#!/usr/bin/env bash
# Install the base image's project library packages for each foreign
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
      why=$(grep -m1 -E 'trying to overwrite|^E:' /tmp/apt.log | cut -c1-200)
      echo "$arch $p skipped: $why" >> "$out"
      # a failed unpack leaves foreign packages half-installed and apt broken for every
      # later install: purge what is not fully installed for this arch (never native)
      broken=$(dpkg-query -W -f '${db:Status-Abbrev} ${Package}:${Architecture}\n' | awk -v a=":$arch" 'substr($1,2,1) ~ /[UFHWt]/ && $2 ~ a"$" {print $2}')
      [ -n "$broken" ] && dpkg --purge --force-depends $broken > /dev/null 2>&1
      apt-get install -y -q -f --no-remove > /dev/null 2>&1 || true
    fi
  done
done
# the image must leave apt consistent, or nothing built in it can be trusted
apt-get check || exit 1
# broken = unpacked/half-configured/half-installed/triggers pending (2nd char) or reinstall-required (3rd)
broken_all() { dpkg-query -W -f '${db:Status-Abbrev} ${Package}:${Architecture}\n' | awk 'substr($1,2,1) ~ /[UFHWt]/ || substr($1,3,1) == "R"'; }
if broken_all | grep -q .; then
  echo "packages left in a non-installed state:" >&2
  broken_all >&2
  exit 1
fi
dpkg-query -W -f '${Package}:${Architecture} ${Version}\n' | sort > /etc/freshbench/dpkg-versions.txt
grep -c installed "$out"; grep -c skipped "$out" || true
