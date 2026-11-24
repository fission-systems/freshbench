#!/usr/bin/env bash
# DecBench's run driver puts every decompile in a `systemd-run --user --scope`
# with MemoryMax/RuntimeMaxSec; GitHub runners need a user manager for that.
set -euo pipefail
uid=$(id -u)
sudo loginctl enable-linger "$(whoami)"
for i in $(seq 1 30); do [ -S "/run/user/$uid/bus" ] && break; sleep 1; done
echo "XDG_RUNTIME_DIR=/run/user/$uid" >> "$GITHUB_ENV"
echo "DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/$uid/bus" >> "$GITHUB_ENV"
export XDG_RUNTIME_DIR=/run/user/$uid DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/$uid/bus
systemd-run --user --scope --quiet -p MemoryMax=1G true && echo "systemd user scope ok"
cat "/sys/fs/cgroup/user.slice/user-$uid.slice/user@$uid.service/cgroup.controllers" || true
