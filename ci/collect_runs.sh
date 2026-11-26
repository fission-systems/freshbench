#!/usr/bin/env bash
# Gather the results of completed evaluate runs and run the M1 publication
# gate over them. Shared by pages.yml (before it builds the leaderboard) and
# sweep-collect.yml (before it proposes a run list), so both apply the same
# checks to the same inputs.
#
#   ci/collect_runs.sh RUNS_JSON FRESHBENCH_BIN
#
# In the current directory it leaves:
#   dl/<run>/...        every eval-* and bound-* artifact of the runs
#   site-src/rows/      every rows/<tool>.jsonl, named uniquely
#   corpus-data/m1/     the pinned M1 truth (no binaries or source files)
#   meta.json           page metadata from the runs' own provenance
# It exits 3 when the runs' provenance or the gate refuses them, and with another non-zero status
# when it could not get that far (a download failed). Needs GH_TOKEN and
# GITHUB_REPOSITORY.
set -euo pipefail
runs="$1"; fb="$2"
jq -e 'type == "array" and length > 0 and all(.[]; tostring | test("^[0-9]+$"))' <<< "$runs" >/dev/null

mkdir -p site-src/rows
for id in $(jq -r '.[]' <<< "$runs"); do
  gh run download "$id" -R "$GITHUB_REPOSITORY" -p 'eval-*' -p 'bound-*' -D "dl/$id"
done
find dl -path '*/rows/*.jsonl' | sort | while read -r f; do
  cp "$f" "site-src/rows/$(echo "$f" | tr '/' '_')"
done
find site-src/rows -name "*.jsonl" | wc -l

python -m pip install -q 'huggingface_hub==0.36.0'
python - <<'PY'
from huggingface_hub import snapshot_download
from ci.leaderboard_gate import REVISION
snapshot_download(repo_id="sjkim1127/freshbench-v2", repo_type="dataset",
                  revision=REVISION, allow_patterns=["m1/truth/*"],
                  local_dir="corpus-data")
PY

REVISION=$(python -c 'from ci.leaderboard_gate import REVISION; print(REVISION)')
# refuses runs that measured one tool at two versions or came from two corpus builds
"$fb" provenance meta dl --corpus "M1 · dataset $REVISION" \
  --note "evaluate runs: $runs" --out meta.json || exit 3
jq '.title = "FreshBench M1 Leaderboard"' meta.json > meta.tmp
mv meta.tmp meta.json

python ci/leaderboard_gate.py --corpus corpus-data/m1 \
  --manifest corpus/m1/manifest.jsonl.gz --rows site-src/rows \
  --meta meta.json --registry ci/tools.json --boundaries dl || exit 3
