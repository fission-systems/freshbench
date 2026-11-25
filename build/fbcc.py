#!/usr/bin/env python3
"""Compiler wrapper that pins the optimization configuration.

Invoked as fbcc-gcc / fbcc-clang (C) or fbcxx-gcc / fbcxx-clang (C++). Whatever
the project's build system passes, the optimization, debug, inlining, LTO and
strip options are removed and replaced by FB_FLAGS (e.g. "-O2 -fno-inline"),
always with "-g -fno-builtin" (DecBench's base flags). A Makefile that hardcodes
CFLAGS=-O2 therefore cannot turn an O0 build into an O2 one.

For every C source compiled, the same command is re-run with -E to write the
preprocessed translation unit (the GED ground-truth input), and one JSON line
{cwd, src, i, args} is appended to FB_LOG.
"""
import hashlib, json, os, re, subprocess, sys

me = os.path.basename(sys.argv[0])
cxx = me.startswith("fbcxx")
fam = "clang" if me.endswith("clang") else "gcc"
# FB_ARCH selects the target (default x86_64 = the base image's native build, unchanged).
# Cross GCC is GCC 16.2.0 built from the same release (build/Dockerfile.cross);
# cross Clang is the same /opt/llvm 23.1.6 with --target, the cross GCC's crt
# files and libgcc, and the target's binutils ld.
ARCH = os.environ.get("FB_ARCH", "x86_64")
TRIPLET = {"x86_64": None, "i686": "i686-linux-gnu", "aarch64": "aarch64-linux-gnu",
           "armv7": "arm-linux-gnueabihf"}[ARCH]
GCC_VER = "16.2.0"
if TRIPLET is None:
    REAL = {("gcc", False): "/usr/local/bin/gcc", ("gcc", True): "/usr/local/bin/g++",
            ("clang", False): "/opt/llvm/bin/clang", ("clang", True): "/opt/llvm/bin/clang++"}[(fam, cxx)]
    TGT = []
elif fam == "gcc":
    REAL = f"/opt/gcc-cross/bin/{TRIPLET}-{'g++' if cxx else 'gcc'}"
    TGT = []
else:
    REAL = "/opt/llvm/bin/clang++" if cxx else "/opt/llvm/bin/clang"
    TGT = {"i686": ["--target=i686-linux-gnu"],
           "aarch64": ["--target=aarch64-linux-gnu"],
           "armv7": ["--target=armv7a-linux-gnueabihf", "-mthumb", "-mfpu=vfpv3-d16",
                     "-mfloat-abi=hard"]}[ARCH]
    TGT += [f"--gcc-install-dir=/opt/gcc-cross/lib/gcc/{TRIPLET}/{GCC_VER}",
            f"--ld-path=/usr/bin/{TRIPLET}-ld"]
if sys.argv[1:] == ["--fb-version"]:
    out = subprocess.run([REAL] + TGT + ["--version"], capture_output=True, text=True).stdout
    print(out.splitlines()[0] + (f" target={TRIPLET}" if TRIPLET else " target=x86_64-linux-gnu"))
    sys.exit(0)
FB_FLAGS = os.environ.get("FB_FLAGS", "-O0").split()
BASE = ["-g", "-fno-builtin"]

DROP_EXACT = {"-s", "-g", "-ggdb", "-march=native", "-mtune=native", "-mcpu=native",
              "-Werror", "-pedantic-errors", "-fno-builtin"}
DROP_RE = re.compile(
    r"^(-O.*|-g\d|-ggdb\d?|-gdwarf.*|-gsplit-dwarf|-gz.*|-gline-tables-only|-flto.*|-fno-lto|"
    r"-ffat-lto-objects|-fwhole-program|-f(no-)?inline.*|-fprofile-.*|-fauto-profile.*|"
    r"-Werror=.*|-Wl,(-s|--strip-all|--strip-debug|-S|-O\d)|--coverage|-fcoverage-mapping|"
    r"-fprofile-instr-.*|-fsanitize.*|-fdebug-prefix-map=.*|-ffile-prefix-map=.*)$")


def filt(args):
    out = []
    for a in args:
        if a in DROP_EXACT or DROP_RE.match(a):
            continue
        out.append(a)
    return out


args = filt(sys.argv[1:])
# GCC 15+ defaults to C23 (bool/true/false keywords), which breaks code that
# typedefs bool; a project that names no -std gets gnu17, the default of every
# GCC 8-14 and of Clang. Dialect only, no codegen effect.
STD = [] if cxx or any(a.startswith(("-std=", "--std")) for a in args) else ["-std=gnu17"]
final = TGT + args + STD + BASE + FB_FLAGS
if fam == "clang":
    final += ["-Wno-error", "-Wno-unused-command-line-argument"]
rc = subprocess.call([REAL] + final)
if rc != 0 or cxx:
    sys.exit(rc)

# Preprocessed TUs for every C source on the command line.
log = os.environ.get("FB_LOG")
outdir = os.environ.get("FB_IDIR")
if log and outdir:
    srcs = [a for a in args if a.endswith(".c") and os.path.isfile(a)]
    if srcs and "-E" not in args and "-M" not in args and "-MM" not in args:
        # Strip output/dep/compile-only flags; keep everything else.
        base, skip = [], False
        for a in args:
            if skip:
                skip = False; continue
            if a in ("-o", "-MF", "-MT", "-MQ"):
                skip = True; continue
            if a in ("-c", "-MD", "-MMD", "-MP", "-S") or a.startswith("-o") and len(a) > 2 \
               or a.startswith(("-MF", "-MT", "-MQ", "-Wl,", "-l", "-L")) or a in srcs:
                continue
            if a.endswith((".o", ".a", ".so", ".lo")):
                continue
            base.append(a)
        for s in srcs:
            src = os.path.realpath(s)
            tag = hashlib.sha256((src + "\0" + " ".join(base)).encode()).hexdigest()[:16]
            ipath = os.path.join(outdir, f"{os.path.splitext(os.path.basename(src))[0]}.{tag}.i")
            r = subprocess.call([REAL] + TGT + base + STD + BASE + FB_FLAGS + ["-E", s, "-o", ipath],
                                stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            if r == 0:
                with open(log, "a") as f:
                    f.write(json.dumps({"cwd": os.getcwd(), "src": src, "i": ipath,
                                        "args": final, "arch": ARCH}) + "\n")
sys.exit(0)
