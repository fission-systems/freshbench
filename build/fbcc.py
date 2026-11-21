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
REAL = {("gcc", False): "/usr/local/bin/gcc", ("gcc", True): "/usr/local/bin/g++",
        ("clang", False): "/opt/llvm/bin/clang", ("clang", True): "/opt/llvm/bin/clang++"}[(fam, cxx)]
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
final = args + STD + BASE + FB_FLAGS
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
            r = subprocess.call([REAL] + base + STD + BASE + FB_FLAGS + ["-E", s, "-o", ipath],
                                stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            if r == 0:
                with open(log, "a") as f:
                    f.write(json.dumps({"cwd": os.getcwd(), "src": src, "i": ipath,
                                        "args": final}) + "\n")
sys.exit(0)
