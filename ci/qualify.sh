#!/usr/bin/env bash
# Build one candidate at all 12 configurations and prepare its rows.
# usage: qualify.sh <owner/repo> <out_dir>
set -uo pipefail
source "$(dirname "$0")/pins.env"
repo="$1"; out="$2"; here="$(cd "$(dirname "$0")/.." && pwd)"
slug="${repo//\//__}"
mkdir -p "$out/builds" "$out/prep"
status() { python - "$out/status.json" "$@" <<'PY'
import json, sys
p = sys.argv[1]; d = {}
try: d = json.load(open(p))
except Exception: pass
for kv in sys.argv[2:]:
    k, v = kv.split("=", 1); d[k] = v
json.dump(d, open(p, "w"), indent=1)
PY
}
status repo="$repo" stage=clone
src="$RUNNER_TEMP/src/$slug"
if ! git clone -q --depth 1 --recurse-submodules --shallow-submodules "https://github.com/$repo" "$src"; then
  status stage=clone result=clone_failed; exit 0
fi
sha=$(git -C "$src" rev-parse HEAD)
subs=$(git -C "$src" config --file .gitmodules --get-regexp path 2>/dev/null | awk '{print $2}' | paste -sd, -)
status sha="$sha" submodules="$subs"
cp "$src"/LICENSE* "$src"/COPYING* "$src"/LICENCE* "$out/" 2>/dev/null || true
build() {  # compiler config
  docker run --rm --network none -v "$src:/src:ro" -v "$out/builds:/out" "$BUILD_IMAGE" \
    timeout 1800 build_project /src "$1" "$2" "/out/$1-$2" > "$out/builds/$1-$2.log" 2>&1
}
status stage=build_gcc-O0
build gcc O0
python -I "$here/pipeline/prepare.py" "$slug" "$out/builds" "$out/prep" --no-strip \
  --only gcc-O0 --submodules "$subs" --overlap "$here/overlap/decbench_fingerprints.json" \
  > "$out/precheck.txt" 2>&1
if ! grep -q QUALIFIES "$out/precheck.txt"; then
  status stage=precheck result=fails_gcc_O0; cp "$out/prep/prepare_summary.json" "$out/precheck_summary.json" 2>/dev/null; exit 0
fi
for c in gcc clang; do for o in O0 O1 O2 O3 Os O2-noinline; do
  [ "$c-$o" = gcc-O0 ] && continue
  status stage="build_$c-$o"; build "$c" "$o"
done; done
status stage=prepare
python -I "$here/pipeline/prepare.py" "$slug" "$out/builds" "$out/prep" \
  --submodules "$subs" --overlap "$here/overlap/decbench_fingerprints.json" > "$out/prepare.txt" 2>&1
if grep -q "^QUALIFIES" "$out/prepare.txt"; then status stage=done result=qualifies
else status stage=done result=fails_all_configs; fi
