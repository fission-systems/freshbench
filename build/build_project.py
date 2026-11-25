#!/usr/bin/env python3
"""Build one project at one (compiler, opt) configuration inside the image.

usage: build_project <src_dir> <compiler gcc|clang> <config> <out_dir> [arch]
config: O0 O1 O2 O3 Os O2-noinline
arch:   x86_64 (default, the native build) | i686 | aarch64 | armv7

A non-native arch is a cross build: configure gets --host=<triplet>, CMake
gets CMAKE_SYSTEM_NAME/PROCESSOR, meson a cross file, and every build system
gets the target's binutils (AR, RANLIB, ...) and its multiarch pkg-config
directory. Build steps that must run a target program fail, and the build
fails with them; nothing built is ever run.

The source tree is copied to a scratch dir (in-tree builds must not leak
between configs). The build system is detected in a fixed order:
  CMakeLists.txt -> meson.build -> configure -> configure.ac/.in (autoreconf)
  -> Makefile/makefile/GNUmakefile.
CC/CXX are the fbcc wrappers, which pin the flags (see fbcc.py).
Every ELF executable / shared object of the target machine left in the tree
is collected.
"""
import json, os, shutil, struct, subprocess, sys, time

src, fam, cfg, out = sys.argv[1:5]
arch = sys.argv[5] if len(sys.argv) > 5 else "x86_64"
# arch -> (GNU triplet, Debian multiarch dir, ELF class, e_machine, CMake/meson cpu)
ARCHES = {"x86_64": ("x86_64-linux-gnu", "x86_64-linux-gnu", 2, 0x3E, "x86_64"),
          "i686": ("i686-linux-gnu", "i386-linux-gnu", 1, 0x03, "x86"),
          "aarch64": ("aarch64-linux-gnu", "aarch64-linux-gnu", 2, 0xB7, "aarch64"),
          "armv7": ("arm-linux-gnueabihf", "arm-linux-gnueabihf", 1, 0x28, "arm")}
triplet, multiarch, ELF_CLASS, ELF_MACHINE, cpu = ARCHES[arch]
cross = arch != "x86_64"
FLAGS = {"O0": "-O0", "O1": "-O1", "O2": "-O2", "O3": "-O3", "Os": "-Os",
         "O2-noinline": "-O2 -fno-inline"}[cfg]
if fam == "clang":
    FLAGS += " -grecord-command-line"
work = f"/tmp/fbwork/{fam}-{cfg}"
shutil.rmtree(work, ignore_errors=True)
shutil.copytree(src, work, symlinks=True)
os.makedirs(out, exist_ok=True)
idir = os.path.join(out, "i"); os.makedirs(idir, exist_ok=True)
log = os.path.join(out, "tu_log.jsonl")
open(log, "w").close()
env = dict(os.environ, FB_FLAGS=FLAGS, FB_LOG=log, FB_IDIR=idir,
           CC=f"fbcc-{fam}", CXX=f"fbcxx-{fam}", CFLAGS="", CXXFLAGS="", LDFLAGS="",
           MAKEFLAGS="-j4", CMAKE_BUILD_PARALLEL_LEVEL="4", LC_ALL="C", FB_ARCH=arch)
if cross:
    env.update({k: f"/usr/bin/{triplet}-{v}" for k, v in
                {"AR": "ar", "RANLIB": "ranlib", "NM": "nm", "STRIP": "strip", "OBJCOPY": "objcopy",
                 "OBJDUMP": "objdump", "LD": "ld", "READELF": "readelf"}.items()})
    env["PKG_CONFIG_LIBDIR"] = f"/usr/lib/{multiarch}/pkgconfig:/usr/share/pkgconfig"
    env["PKG_CONFIG_PATH"] = ""
    # Hand-written configure scripts look for <triplet>-gcc / -cc on PATH: give them
    # this build's wrapper under those names (C and C++), first on PATH.
    tdir = f"/tmp/fbwork/bin-{arch}-{fam}"
    os.makedirs(tdir, exist_ok=True)
    for name, target in [("gcc", f"fbcc-{fam}"), ("cc", f"fbcc-{fam}"), ("clang", f"fbcc-{fam}"),
                         ("g++", f"fbcxx-{fam}"), ("c++", f"fbcxx-{fam}"), ("clang++", f"fbcxx-{fam}")]:
        link = os.path.join(tdir, f"{triplet}-{name}")
        if not os.path.lexists(link):
            os.symlink(f"/usr/local/bin/{target}", link)
    env["PATH"] = tdir + ":" + env.get("PATH", "")
steps, ok, system = [], True, None


def run(cmd, timeout=1500):
    global ok
    t = time.time()
    try:
        p = subprocess.run(cmd, shell=True, cwd=work, env=env, capture_output=True, text=True,
                           timeout=timeout)
        rc, tail = p.returncode, (p.stdout[-3000:] + "\n--stderr--\n" + p.stderr[-6000:])
    except subprocess.TimeoutExpired:
        rc, tail = -999, "timeout"
    steps.append({"cmd": cmd, "rc": rc, "secs": round(time.time() - t, 1), "tail": tail})
    if rc != 0:
        ok = False
    return rc == 0


def has(*names):
    return any(os.path.exists(os.path.join(work, n)) for n in names)


CC, CXX = f"fbcc-{fam}", f"fbcxx-{fam}"
HOST, CMAKE_CROSS, MESON_CROSS, MESON_ENV = "", "", "", ""
if cross:
    HOST = f" --host={triplet}"
    CMAKE_CROSS = (f" -DCMAKE_SYSTEM_NAME=Linux -DCMAKE_SYSTEM_PROCESSOR={cpu}"
                   f" -DCMAKE_AR=/usr/bin/{triplet}-ar -DCMAKE_RANLIB=/usr/bin/{triplet}-ranlib"
                   f" -DCMAKE_LIBRARY_ARCHITECTURE={multiarch}")
    cf = os.path.join(out, "meson-cross.ini")
    with open(cf, "w") as f:
        f.write(f"""[binaries]
c = '{CC}'
cpp = '{CXX}'
ar = '/usr/bin/{triplet}-ar'
strip = '/usr/bin/{triplet}-strip'
pkg-config = 'pkg-config'

[properties]
pkg_config_libdir = ['/usr/lib/{multiarch}/pkgconfig', '/usr/share/pkgconfig']

[host_machine]
system = 'linux'
cpu_family = '{cpu}'
cpu = '{cpu}'
endian = 'little'
""")
    MESON_CROSS = f" --cross-file {cf}"
MESON_ENV = "env -u CC -u CXX -u AR -u RANLIB -u LD -u STRIP " if cross else ""  # meson: env CC is the build machine's
if has("CMakeLists.txt"):
    system = "cmake"
    run(f"cmake -S . -B _fbbuild -G Ninja -DCMAKE_BUILD_TYPE=None -DCMAKE_C_COMPILER={CC} "
        f"-DCMAKE_CXX_COMPILER={CXX} -DCMAKE_INTERPROCEDURAL_OPTIMIZATION=OFF{CMAKE_CROSS}") and \
        run("cmake --build _fbbuild -j4")
elif has("meson.build"):
    system = "meson"
    run(f"{MESON_ENV}meson setup _fbbuild --buildtype=plain -Db_lto=false{MESON_CROSS}") and run("ninja -C _fbbuild -j4")
elif has("configure") and os.access(os.path.join(work, "configure"), os.X_OK):
    system = "configure"
    configured = run(f"./configure{HOST}")
    if not configured and HOST and any(m in steps[-1]["tail"].lower() for m in
                                       ("invalid option", "unrecognized option", "unknown option")):
        # a hand-written configure without --host: cross through CC/CXX (env) alone
        steps[-1]["superseded"] = True
        ok = True
        configured = run("./configure")
    configured and run(f"make -j4 CC={CC} CXX={CXX}")
elif has("configure.ac", "configure.in"):
    system = "autoreconf"
    (run("./autogen.sh") if has("autogen.sh") else run("autoreconf -fi")) and \
        run(f"./configure{HOST}") and run(f"make -j4 CC={CC} CXX={CXX}")
elif has("Makefile", "makefile", "GNUmakefile"):
    system = "make"
    run(f"make -j4 CC={CC} CXX={CXX}")
else:
    ok = False
    steps.append({"cmd": None, "rc": None, "tail": "no recognised build system"})


def elf_kind(p):
    try:
        with open(p, "rb") as f:
            h = f.read(20)
        if h[:4] != b"\x7fELF" or h[4] != ELF_CLASS or h[5] != 1:
            return None
        et, em = struct.unpack("<HH", h[16:20])
        return et if em == ELF_MACHINE and et in (2, 3) else None
    except OSError:
        return None


bins = []
bindir = os.path.join(out, "bin"); os.makedirs(bindir, exist_ok=True)
for root, dirs, files in os.walk(work):
    dirs[:] = [d for d in dirs if d not in (".git", "CMakeFiles", "meson-private", "meson-logs")]
    for fn in files:
        p = os.path.join(root, fn)
        if os.path.islink(p) or fn.endswith((".o", ".a", ".lo")) or fn.startswith("conftest"):
            continue
        k = elf_kind(p)
        if k is None:
            continue
        rel = os.path.relpath(p, work)
        dest = rel.replace("/", "__")
        shutil.copy2(p, os.path.join(bindir, dest))
        bins.append({"rel": rel, "file": dest, "e_type": k, "size": os.path.getsize(p)})

ver = lambda c: subprocess.run(c, shell=True, capture_output=True, text=True).stdout.split("\n")[0]
json.dump({"compiler": fam, "config": cfg, "flags": FLAGS, "build_system": system,
           "arch": arch, "triplet": triplet,
           "cc_fb_version": ver(f"FB_ARCH={arch} fbcc-{fam} --fb-version"),
           "success": ok, "steps": steps, "binaries": bins,
           "cc_version": ver("/usr/local/bin/gcc --version" if fam == "gcc" else "/opt/llvm/bin/clang --version"),
           "work_dir": work},
          open(os.path.join(out, "build.json"), "w"), indent=1)
print(json.dumps({"config": f"{fam}-{cfg}", "ok": ok, "system": system, "bins": len(bins)}))
