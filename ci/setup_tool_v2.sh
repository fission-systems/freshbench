#!/usr/bin/env bash
# v2: install one decompiler on a GitHub runner, pinned by ci/pins.env, and
# print the adapter command for `freshbench decompile` to $GITHUB_ENV as FB_ADAPTER.
set -euo pipefail
here="$(cd "$(dirname "$0")/.." && pwd)"
source "$here/ci/pins.env"
tool="$1"; T="$RUNNER_TEMP/tools"; mkdir -p "$T"
case "$tool" in
  ghidra)
    curl -fsSL -o "$T/ghidra.zip" "$GHIDRA_URL"
    echo "$GHIDRA_SHA256  $T/ghidra.zip" | sha256sum -c -
    unzip -q "$T/ghidra.zip" -d "$T"
    echo "GHIDRA_INSTALL_DIR=$T/$GHIDRA_DIR" >> "$GITHUB_ENV"
    echo "FB_ADAPTER=$here/adapters/ghidra/run" >> "$GITHUB_ENV";;
  kuna)
    curl -fsSL -o "$T/kuna.tgz" "$KUNA_URL"
    echo "$KUNA_SHA256  $T/kuna.tgz" | sha256sum -c -
    mkdir -p "$T/kuna"; tar -xzf "$T/kuna.tgz" -C "$T/kuna"
    k=$(find "$T/kuna" -type f -name kuna -perm -u+x | head -1); "$k" --version
    echo "KUNA_BIN=$k" >> "$GITHUB_ENV"
    echo "FB_ADAPTER=$here/adapters/kuna/run" >> "$GITHUB_ENV";;
  angr)
    python -m pip install -q "angr==$ANGR_VERSION"
    python -c "import angr;print('angr',angr.__version__)"
    echo "FB_ADAPTER=$(command -v python) $here/adapters/angr/run" >> "$GITHUB_ENV";;
  borer)
    curl -fsSL -o "$T/borer.tgz" "$BORER_URL"
    echo "$BORER_SHA256  $T/borer.tgz" | sha256sum -c -
    mkdir -p "$T/borer"; tar -xzf "$T/borer.tgz" -C "$T/borer"
    b=$(find "$T/borer" -type f -name borer -perm -u+x | head -1)
    BORER_NO_UPDATE_CHECK=1 "$b" --version
    echo "BORER_BIN=$b" >> "$GITHUB_ENV"
    echo "FB_ADAPTER=$here/adapters/borer/run" >> "$GITHUB_ENV";;
  fission) :;;  # built in; see evaluate.yml
  *) echo "unknown tool $tool"; exit 1;;
esac
