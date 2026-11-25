#!/usr/bin/env bash
# v2: build one corpus project for ONE architecture at all 12 (compiler, opt)
# configurations with the cross image, and prepare its rows.
# usage: qualify_arch.sh <owner/repo@sha> <out_dir> <arch>
#   arch: x86_64 | i686 | aarch64 | armv7
# Same gates as v1 (ci/qualify.sh): both O0 builds need >= 300 eligible
# functions, then `freshbench prepare` (Rust, no DecBench; FRESHBENCH_BIN = a
# Linux build of it) selects 300 per build in the same sha256 order,
# which is architecture-independent - the same source functions are chosen on
# every architecture where they are eligible.
set -uo pipefail
# FRESHBENCH_BIN: a Linux build of freshbench, run in the image; on the runner
# itself the same binary records the status file
FRESHBENCH_BIN_HOST="${FRESHBENCH_BIN_HOST:-$FRESHBENCH_BIN}"
here="$(cd "$(dirname "$0")/.." && pwd)"
source "$here/ci/pins.env"
spec="$1"; out="$2"; arch="$3"
repo="${spec%@*}"; pin="${spec#*@}"
[ "$pin" = "$spec" ] && { echo "pinned commit required: owner/repo@sha" >&2; exit 2; }
slug="${repo//\//__}"
img="$BUILD_IMAGE_CROSS"
mkdir -p "$out/builds" "$out/prep"
status() { "$FRESHBENCH_BIN_HOST" status "$out/status.json" "$@"; }
status repo="$repo" arch="$arch" image="$img" stage=clone
src="$RUNNER_TEMP/src/$slug"
git init -q "$src" && git -C "$src" fetch -q --depth 1 "https://github.com/$repo" "$pin" \
  && git -C "$src" checkout -q FETCH_HEAD \
  && git -C "$src" submodule update -q --init --recursive --depth 1 \
  || { status stage=clone result=clone_failed; exit 0; }
sha=$(git -C "$src" rev-parse HEAD)
subs=$(git -C "$src" config --file .gitmodules --get-regexp path 2>/dev/null | awk '{print $2}' | paste -sd, -)
status sha="$sha" submodules="$subs"
mkdir -p "$out/license"; cp "$src"/LICENSE* "$src"/COPYING* "$src"/LICENCE* "$src"/NOTICE* "$out/license/" 2>/dev/null || true
build() {  # compiler config
  docker run --rm --network none -v "$src:/src:ro" -v "$out/builds:/out" "$img" \
    timeout 1800 build_project /src "$1" "$2" "/out/$1-$2" "$arch" > "$out/builds/$1-$2.log" 2>&1
}
# freshbench (Rust) runs inside the image: its llvm-strip is the pinned 23.1.6
fb() {
  docker run --rm --network none -v "$out:/w" -v "$FRESHBENCH_BIN:/usr/local/bin/freshbench:ro" \
    -v "$here/overlap:/overlap:ro" "$img" freshbench "$@"
}
quick() {
  fb quickcount "/w/builds/$1" 2>/dev/null | tail -1
}
for c in gcc clang; do
  status stage="build_$c-O0"; build "$c" O0
  n=$(quick "$c-O0"); status "quick_$c-O0=${n:-0}"
  if [ "${n:-0}" -lt 300 ]; then status stage=quick result="fails_quick_$c-O0"; exit 0; fi
done
status stage=precheck
fb prepare --project "$slug" --builds /w/builds --out /w/precheck --submodules "$subs" \
  --exclude /overlap/decbench_token_fingerprints.json --only gcc-O0,clang-O0 --count-only \
  > "$out/precheck.txt" 2>&1
if ! grep -q "^QUALIFIES" "$out/precheck.txt"; then
  status stage=precheck result=fails_O0_eligible; exit 0
fi
for c in gcc clang; do for o in O1 O2 O3 Os O2-noinline; do
  status stage="build_$c-$o"; build "$c" "$o"
done; done
status stage=prepare
fb prepare --project "$slug" --builds /w/builds --out /w/prep --submodules "$subs" \
  --exclude /overlap/decbench_token_fingerprints.json > "$out/prepare.txt" 2>&1
if grep -q "^QUALIFIES" "$out/prepare.txt"; then status stage=done result=qualifies
else status stage=done result=fails_prepare; fi
