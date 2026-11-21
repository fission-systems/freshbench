#!/usr/bin/env bash
# Build one candidate at all 12 configurations and prepare its rows.
# usage: qualify.sh <owner/repo> <out_dir>
set -uo pipefail
here="$(cd "$(dirname "$0")/.." && pwd)"
source "$here/ci/pins.env"
spec="$1"; out="$2"
repo="${spec%@*}"; pin=""; [ "$spec" != "$repo" ] && pin="${spec#*@}"
slug="${repo//\//__}"
mkdir -p "$out/builds" "$out/prep"
status() { python3 - "$out/status.json" "$@" <<'PY'
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
if [ -n "$pin" ]; then
  # re-run of a corpus project at the commit it was selected at
  git init -q "$src" && git -C "$src" fetch -q --depth 1 "https://github.com/$repo" "$pin" \
    && git -C "$src" checkout -q FETCH_HEAD \
    && git -C "$src" submodule update -q --init --recursive --depth 1 \
    || { status stage=clone result=clone_failed; exit 0; }
elif ! timeout 900 git clone -q --depth 1 --recurse-submodules --shallow-submodules "https://github.com/$repo" "$src"; then
  status stage=clone result=clone_failed; exit 0
fi
sha=$(git -C "$src" rev-parse HEAD)
subs=$(git -C "$src" config --file .gitmodules --get-regexp path 2>/dev/null | awk '{print $2}' | paste -sd, -)
status sha="$sha" submodules="$subs"
mkdir -p "$out/license"; cp "$src"/LICENSE* "$src"/COPYING* "$src"/LICENCE* "$src"/NOTICE* "$out/license/" 2>/dev/null || true
build() {  # compiler config
  docker run --rm --network none -v "$src:/src:ro" -v "$out/builds:/out" "$BUILD_IMAGE" \
    timeout 1800 build_project /src "$1" "$2" "/out/$1-$2" > "$out/builds/$1-$2.log" 2>&1
}
quick() {
  docker run --rm --network none -v "$out/builds:/out:ro" -v "$here/pipeline:/p:ro" "$BUILD_IMAGE" \
    python3 /p/quickcount.py "/out/$1" 2>/dev/null | tail -1
}
for c in gcc clang; do
  status stage="build_$c-O0"; build "$c" O0
  n=$(quick "$c-O0"); status "quick_$c-O0=${n:-0}"
  if [ "${n:-0}" -lt 300 ]; then status stage=quick result="fails_quick_$c-O0"; exit 0; fi
done
status stage=setup_scorer
"$here/ci/setup_scorer.sh" > "$out/setup_scorer.log" 2>&1 || { status result=scorer_setup_failed; exit 1; }
python -I "$here/pipeline/prepare.py" "$slug" "$out/builds" "$out/prep" --no-strip \
  --only gcc-O0,clang-O0 --submodules "$subs" --overlap "$here/overlap/decbench_fingerprints.json" \
  > "$out/precheck.txt" 2>&1
cp "$out/prep/prepare_summary.json" "$out/precheck_summary.json" 2>/dev/null
if ! grep -q "^QUALIFIES" "$out/precheck.txt"; then
  status stage=precheck result=fails_O0_eligible; exit 0
fi
for c in gcc clang; do for o in O1 O2 O3 Os O2-noinline; do
  status stage="build_$c-$o"; build "$c" "$o"
done; done
status stage=prepare
python -I "$here/pipeline/prepare.py" "$slug" "$out/builds" "$out/prep" \
  --submodules "$subs" --overlap "$here/overlap/decbench_fingerprints.json" > "$out/prepare.txt" 2>&1
if grep -q "^QUALIFIES" "$out/prepare.txt"; then status stage=done result=qualifies
else status stage=done result=fails_prepare; fi
