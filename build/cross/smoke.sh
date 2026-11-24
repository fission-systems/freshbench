#!/usr/bin/env bash
# Compile and link a C file for every (arch, compiler) through fbcc, check the
# ELF machine, and print each compiler's exact version line. Nothing is run.
set -euo pipefail
d=$(mktemp -d)
cat > "$d/t.c" <<'C'
#include <stdio.h>
#include <math.h>
int f(int x) { return x > 3 ? x * 2 : x - 1; }
int main(int argc, char **argv) { printf("%d %f\n", f(argc), sqrt((double)argc)); return 0; }
C
for arch in x86_64 i686 aarch64 armv7; do
  for fam in gcc clang; do
    FB_ARCH=$arch FB_FLAGS=-O2 "fbcc-$fam" "$d/t.c" -o "$d/t-$arch-$fam" -lm
    m=$(python3 -c "import sys,struct;h=open(sys.argv[1],'rb').read(20);print(h[4],struct.unpack('<H',h[18:20])[0])" "$d/t-$arch-$fam")
    v=$(FB_ARCH=$arch fbcc-$fam --fb-version)
    echo "$arch $fam elfclass_machine=[$m] $v"
  done
done
rm -rf "$d"
