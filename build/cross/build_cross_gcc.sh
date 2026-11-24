#!/usr/bin/env bash
# Build one cross GCC (C and C++) from an unpacked GCC source tree.
# usage: build_cross_gcc.sh <gcc_src> <target triplet> [extra configure args]
# Installs to /opt/gcc-cross. The target's glibc comes from Debian multiarch
# (sysroot /), binutils from Debian's binutils-<triplet>.
set -euo pipefail
src="$1"; t="$2"; shift 2
b="/tmp/gccbuild-$t"
rm -rf "$b"; mkdir -p "$b"; cd "$b"
"$src/configure" --target="$t" --prefix=/opt/gcc-cross \
  --with-sysroot=/ --enable-multiarch --disable-multilib \
  --with-as="/usr/bin/$t-as" --with-ld="/usr/bin/$t-ld" \
  --enable-languages=c,c++ --disable-bootstrap --disable-nls \
  --disable-libsanitizer --disable-libvtv --disable-libssp \
  --enable-shared --enable-threads=posix --enable-__cxa_atexit \
  --enable-linker-build-id --with-system-zlib "$@" > configure.log 2>&1 \
  || { tail -50 configure.log; exit 1; }
make -j"$(nproc)" > make.log 2>&1 || { tail -80 make.log; exit 1; }
make install > install.log 2>&1 || { tail -40 install.log; exit 1; }
"/opt/gcc-cross/bin/$t-gcc" --version | head -1
