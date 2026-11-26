#!/usr/bin/env bash
# ci/capped.sh COMMAND... : run COMMAND (freshbench and the tool processes it
# starts) in one memory cgroup of $FB_MEMORY_MAX without swap. When the tools
# exceed it, the kernel kills the largest of them and freshbench records that
# call's failure. Without the cap the runner itself ran out of memory and was
# lost, and the job uploaded no row at all (every armv7 revng job of sweep
# 2acaabc6f). Needs ci/setup_systemd_user.sh to have run.
#
# A kill by the cap is counted into $RUNNER_TEMP/oom-kills: which process dies
# depends on what else ran at that moment, so such a job's outputs are not
# offered for reuse (evaluate.yml).
set -euo pipefail
if [ "${1:-}" != --inside ]; then
  : "${FB_MEMORY_MAX:?set FB_MEMORY_MAX}"
  # OOMPolicy=continue: systemd would otherwise stop the whole scope, harness
  # included, after the first kill
  exec systemd-run --user --scope --quiet -p MemoryMax="$FB_MEMORY_MAX" -p MemorySwapMax=0 \
    -p OOMPolicy=continue -- "$0" --inside "$@"
fi
shift
status=0
"$@" || status=$?
events="/sys/fs/cgroup$(sed -n 's/^0:://p' /proc/self/cgroup)/memory.events"
kills=$(awk '$1 == "oom_kill" { print $2 }' "$events")
if [ "${kills:?no oom_kill count in $events}" -gt 0 ]; then
  echo "::warning::$kills process(es) killed at the $FB_MEMORY_MAX memory cap"
  echo "$kills" >> "$RUNNER_TEMP/oom-kills"
fi
exit "$status"
