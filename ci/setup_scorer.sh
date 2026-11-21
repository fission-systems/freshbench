#!/usr/bin/env bash
# Python env with DecBench (pinned) + rust-joern (pinned) on a GitHub runner.
set -euo pipefail
source "$(dirname "$0")/pins.env"
python -m pip install -q --upgrade pip
# DECBENCH_SHA is the head of Noelo-Lab/decbench PR #129; fetch the commit itself.
git init -q "$RUNNER_TEMP/decbench"
git -C "$RUNNER_TEMP/decbench" fetch -q --depth 1 "$DECBENCH_REPO" "$DECBENCH_SHA"
git -C "$RUNNER_TEMP/decbench" checkout -q FETCH_HEAD
test "$(git -C "$RUNNER_TEMP/decbench" rev-parse HEAD)" = "$DECBENCH_SHA"
python -m pip install -q -e "$RUNNER_TEMP/decbench" "angr==$ANGR_VERSION"
if ls "$RUNNER_TEMP"/wheels/rust_joern-*.whl >/dev/null 2>&1; then
  python -m pip install -q "$RUNNER_TEMP"/wheels/rust_joern-*.whl
else
  python -m pip install -q "git+$RUST_JOERN_REPO@$RUST_JOERN_SHA"
fi
python -c "import rust_joern, decbench, angr; print('scorer ok angr', angr.__version__)"
echo "DECBENCH_ROOT=$RUNNER_TEMP/decbench" >> "$GITHUB_ENV"
