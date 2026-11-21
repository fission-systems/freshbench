#!/usr/bin/env bash
# Install one decompiler on a GitHub runner (pinned by ci/pins.env).
set -euo pipefail
source "$(dirname "$0")/pins.env"
tool="$1"; T="$RUNNER_TEMP/tools"; mkdir -p "$T"
case "$tool" in
  ghidra)
    if [ ! -d "$T/$GHIDRA_DIR" ]; then
      curl -fsSL -o "$T/ghidra.zip" "$GHIDRA_URL"
      echo "$GHIDRA_SHA256  $T/ghidra.zip" | sha256sum -c -
      unzip -q "$T/ghidra.zip" -d "$T"
    fi
    python -m pip install -q pyghidra
    echo "GHIDRA_INSTALL_DIR=$T/$GHIDRA_DIR" >> "$GITHUB_ENV"
    python -c "import pyghidra,importlib.metadata as m;print('pyghidra',m.version('pyghidra'))";;
  angr) python -c "import angr;print('angr',angr.__version__)";;
  kuna)
    curl -fsSL -o "$T/kuna.tgz" "$KUNA_URL"; mkdir -p "$T/kuna"; tar -xzf "$T/kuna.tgz" -C "$T/kuna"
    k=$(find "$T/kuna" -type f -name kuna -perm -u+x | head -1); "$k" --version
    echo "KUNA_BIN=$k" >> "$GITHUB_ENV";;
  fission)
    [ -x "$RUNNER_TEMP/fission/target/release/fission_cli" ] || { echo "fission binary missing"; exit 1; }
    echo "FISSION_BIN=$RUNNER_TEMP/fission/target/release/fission_cli" >> "$GITHUB_ENV"
    echo "FISSION_RESOURCE_ROOT=$RUNNER_TEMP/fission/utils" >> "$GITHUB_ENV"
    echo "FISSION_WORKING_DIR=$RUNNER_TEMP/fission" >> "$GITHUB_ENV"
    echo "FISSION_BENCHMARK_NO_CACHE=1" >> "$GITHUB_ENV"
    "$RUNNER_TEMP/fission/target/release/fission_cli" --version || true;;
  *) echo "unknown tool $tool"; exit 1;;
esac
