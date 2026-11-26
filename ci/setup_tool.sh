#!/usr/bin/env bash
# Install one decompiler on a GitHub runner, pinned by ci/pins.env, and
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
  unlace)
    # built once by evaluate.yml's build job at UNLACE_SHA (no binary releases)
    u="$RUNNER_TEMP/bins/unlace-bin"; chmod +x "$u/unlace"
    "$u/unlace" --version || true
    echo "UNLACE_BIN=$u/unlace" >> "$GITHUB_ENV"
    echo "UNLACE_SPECS=$u/specs" >> "$GITHUB_ENV"
    echo "FB_ADAPTER=$(command -v python) $here/adapters/unlace/run" >> "$GITHUB_ENV";;
  ghidra-rs)
    # built once by evaluate.yml's build job from adapters/ghidra-rs (crate pinned in its Cargo.lock)
    g="$RUNNER_TEMP/bins/ghidra-rs-bin/freshbench-ghidra-rs"; chmod +x "$g"
    echo "GHIDRA_RS_BIN=$g" >> "$GITHUB_ENV"
    echo "FB_ADAPTER=$here/adapters/ghidra-rs/run" >> "$GITHUB_ENV";;
  revng)
    # fetched once by evaluate.yml's build job (artifact revng-dist); checked again here
    a="$RUNNER_TEMP/revng-dist/revng.tar.xz"
    echo "$REVNG_SHA256  $a" | sha256sum -c -
    mkdir -p "$T/revng"; tar -xJf "$a" -C "$T/revng" --strip-components=1
    rm -f "$a"   # 4 GB extracted; keep the runner's disk
    "$T/revng/revng" --version
    python -m pip install -q "pyyaml==$PYYAML_VERSION"
    echo "REVNG_BIN=$T/revng/revng" >> "$GITHUB_ENV"
    echo "FB_ADAPTER=$(command -v python) $here/adapters/revng/run" >> "$GITHUB_ENV";;
  r2sleigh)
    # built once by evaluate.yml's build job at R2SLEIGH_SHA (no binary releases)
    r="$RUNNER_TEMP/bins/r2sleigh-bin"; chmod +x "$r/r2s"
    "$r/r2s" -q -c i "$r/r2s" >/dev/null  # r2s has no --version; this opens an ELF
    echo "R2S_BIN=$r/r2s" >> "$GITHUB_ENV"
    echo "FB_ADAPTER=$(command -v python) $here/adapters/r2sleigh/run" >> "$GITHUB_ENV";;
  reko)
    curl -fsSL -o "$T/reko.zip" "$REKO_URL"
    echo "$REKO_SHA256  $T/reko.zip" | sha256sum -c -
    curl -fsSL -o "$T/dotnet.tgz" "$DOTNET_RUNTIME_URL"
    echo "$DOTNET_RUNTIME_SHA512  $T/dotnet.tgz" | sha512sum -c -
    mkdir -p "$T/reko" "$T/dotnet"
    unzip -q "$T/reko.zip" -d "$T/reko"; tar -xzf "$T/dotnet.tgz" -C "$T/dotnet"
    "$T/dotnet/dotnet" "$T/reko/decompile.dll" --version
    echo "REKO_DIR=$T/reko" >> "$GITHUB_ENV"
    echo "REKO_DOTNET=$T/dotnet/dotnet" >> "$GITHUB_ENV"
    echo "FB_ADAPTER=$(command -v python) $here/adapters/reko/run" >> "$GITHUB_ENV";;
  r2dec)
    # radare2 from its pinned release .deb; the r2dec plugin was built against it once by
    # evaluate.yml's build job (r2dec's release .so targets another radare2 ABI)
    curl -fsSL -o "$T/radare2.deb" "$RADARE2_DEB_URL"
    echo "$RADARE2_DEB_SHA256  $T/radare2.deb" | sha256sum -c -
    sudo apt-get install -y -q "$T/radare2.deb" >/dev/null
    p="$RUNNER_TEMP/bins/r2dec-bin/libcore_pdd.so"
    r2 -v | head -1
    r2 -NN -q -c "L $p; Lc" -- | grep -q '^r2dec '  # loads: same radare2 ABI
    echo "R2_BIN=$(command -v r2)" >> "$GITHUB_ENV"
    echo "R2DEC_PLUGIN=$p" >> "$GITHUB_ENV"
    echo "FB_ADAPTER=$(command -v python) $here/adapters/r2dec/run" >> "$GITHUB_ENV";;
  manifold)
    # built once by evaluate.yml's build job at MANIFOLD_SHA (no releases); libz3.so beside it (rpath $ORIGIN)
    m="$RUNNER_TEMP/bins/manifold-bin"; chmod +x "$m/manifold"
    ldd "$m/manifold" | grep -q "libz3.so => $m/libz3.so"   # manifold has no --version
    echo "MANIFOLD_BIN=$m/manifold" >> "$GITHUB_ENV"
    echo "FB_ADAPTER=$(command -v python) $here/adapters/manifold/run" >> "$GITHUB_ENV";;
  fission) :;;  # built in; see evaluate.yml
  *) echo "unknown tool $tool"; exit 1;;
esac
