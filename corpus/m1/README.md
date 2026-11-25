# FreshBench M1

C, 20 projects, 4 architectures × 2 compilers × 6 optimization levels.
The files are on Hugging Face. This directory pins the exact revision and
hashes.

| | |
|---|---|
| dataset | [`sjkim1127/freshbench-v2`](https://huggingface.co/datasets/sjkim1127/freshbench-v2), under `m1/` |
| revision | `15b6f2c52c379cf9361f22edaa75886602e5e1ff` |
| manifest | `manifest.jsonl.gz`: 29,402 files, 13,558,883,354 bytes; paths are relative to `m1/` (uncompressed sha256 `d64f8af35369769b0fe0b49a3b15f7165bf800c2f1997d2f1e0eca7671318318`) |
| built by | [`qualify-arch` run 45460924446](https://github.com/fission-systems/freshbench/actions/runs/45460924446) at `7dcfcef` |
| build image | `ghcr.io/fission-systems/freshbench-build-cross@sha256:83f041f7a522f4012b7fe756a22c26000a424791a29ebac9dcf0af3df3ec3407` (`digest.txt`; package lists in `dpkg-versions.txt`, `foreign-packages.txt`) |
| compilers | GCC 16.2.0 and Clang 23.1.6 per target (`toolchains.txt`; each manifest entry repeats its version line and flags) |
| project list | `selection/m1/m1.json` (repo@commit); how it was chosen: `selection/m1/qualification.json` |

## Contents

| kind | files | bytes |
|---|---:|---:|
| unstripped binaries (`binaries/`, grading only) | 8,594 | 7,439,593,892 |
| stripped binaries (`stripped/`, decompiler input) | 8,594 | 2,680,521,136 |
| preprocessed sources (`sources/<arch>/<project>/`) | 11,231 | 3,308,737,724 |
| selected functions (`truth/<build>/<project>/targets.json`) | 960 | 129,784,005 |
| license files (`projects/<project>/`) | 23 | 246,597 |

Build names carry the architecture except on x86-64: `gcc-O0`,
`i686-gcc-O0`, `aarch64-clang-Os`, `armv7-gcc-O2-noinline`. ARMv7 is
Thumb-2, hard float; Thumb functions are requested at `address | 1`.

Up to 300 functions are selected per (project, build), 285,875 in all
(7,885 distinct project/source/name triples). O0 and O2-noinline keep all
6,000 per build; higher levels lose 37–124 per build to inlining and
merging. Selection order is sha256 of `project/source/name`, so the same
functions are chosen on every architecture and compiler wherever they
survive.

| build (per arch) | x86-64 | i686 | AArch64 | ARMv7 |
|---|---:|---:|---:|---:|
| gcc O0 / O2-noinline | 6,000 | 6,000 | 6,000 | 6,000 |
| gcc O1 | 5,948 | 5,952 | 5,942 | 5,943 |
| gcc O2 | 5,903 | 5,911 | 5,898 | 5,902 |
| gcc O3 | 5,876 | 5,878 | 5,884 | 5,885 |
| gcc Os | 5,943 | 5,950 | 5,937 | 5,941 |
| clang O0 / O2-noinline | 6,000 | 6,000 | 6,000 | 6,000 |
| clang O1 | 5,945 | 5,946 | 5,942 | 5,943 |
| clang O2 | 5,947 | 5,948 | 5,943 | 5,944 |
| clang O3 | 5,948 | 5,949 | 5,944 | 5,945 |
| clang Os | 5,962 | 5,963 | 5,956 | 5,957 |

## How many functions this really is

A row is one function in one build. M1 has 285,875 rows, but they are 48
builds of **7,885 distinct source functions** (project, source file, name).
Within a build, no function is selected twice (one binary each).

Distinct is not unique either: projects copy code between their own files.
`freshbench duplicates` compares the selected definitions on their source
tokens, since hashing compiled bytes misses copies (the same source compiles
to different bytes in different binaries) and conflates different sources.
Preprocessed text differs per architecture, so the audit is run on each
(`duplicates-<arch>.json`, every group listed):

| architecture | functions | rows | exact copies | renamed copies | shape copies |
|---|---:|---:|---:|---:|---:|
| x86-64 | 7,810 | 71,472 | 266 (2,415 rows) | 964 (8,922 rows) | 1,259 (11,951 rows) |
| i686 | 7,811 | 71,497 | 267 (2,404) | 967 (8,916) | 1,263 (11,944) |
| AArch64 | 7,617 | 71,446 | 264 (2,397) | 930 (8,781) | 1,218 (11,796) |
| ARMv7 | 7,609 | 71,460 | 263 (2,389) | 929 (8,786) | 1,216 (11,792) |
| all | | 285,875 | 9,605 rows (3.4%) | 35,405 rows (12.4%) | 47,483 rows (16.6%) |

* exact: the same tokens; own name, comments and layout aside.
* renamed: also any consistent renaming of identifiers. About 260 of these
  per architecture are 50 tokens or longer.
* shape: also any literal.

"Redundant" is a group's size minus one. Nearly all groups stay inside one
project: one project copies helpers into each of its programs (one
234-token function eight times), another generates 122 accessors of the
same 13 tokens. After removing renamed copies, M1 holds about 6,680 to 6,850
distinct functions per architecture.

Against DecBench (`--against` its 5,404 preprocessed sources): 7 or 8
functions per architecture are exact copies of a DecBench function and 282
to 312 are renamed copies. All of them are under 50 tokens (`return 0;`-sized
initializers and wrappers). The selection excludes exact copies by token
fingerprint including the name, so the exact ones differ from their DecBench
twin only in the name.

For scale: DecBench's full set has 96,103 rows and 24,269 distinct
(project, name) pairs over 3 configurations, not audited for copies the
same way. M1 has about 3x the rows and
about a third of the distinct functions. Each M1 function appears in up to
48 builds, so M1 measures how a decompiler copes across architectures,
compilers and optimization levels, on fewer distinct functions.

Reproduce (x86-64 sources and truth fetched as above):

```bash
freshbench duplicates data/m1 --arch x86_64 --top 1000000 --against <decbench-dataset>/sources
```

Results that report a single score over M1 must also report it on one
representative per renamed-clone group (`report --dedup` with the four
files; a row of an architecture no file covers is an error).

## Projects

Every project ships its license file(s) under `projects/<project>/`. GPL
and LGPL binaries ship with their preprocessed sources and the upstream
commit below.

| project | SPDX | commit |
|---|---|---|
| `ComposersDesktop__CDP8` | LGPL-2.1 | `28bc42c72c1a` |
| `HANCORE-linux__OmaQ` | GPL-3.0 | `008f7c812d4c` |
| `Matroska-Org__foundation-source` | BSD-3-Clause | `d69761114b79` |
| `Mikachu__openbox` | GPL-2.0 | `ebfa7271302b` |
| `Paxa__fast_excel` | MIT | `1053d5641646` |
| `aliyun__iotkit-embedded` | Apache-2.0 | `0d3beeedb31a` |
| `amazon-ion__ion-c` | Apache-2.0 | `493c12251be6` |
| `bmc0__dsp` | ISC | `2e7eac120f21` |
| `bstansell__conserver` | BSD-3-Clause | `a8ae3db02369` |
| `buserror__simavr` | GPL-3.0 | `13d4ed32433a` |
| `faragon__libsrt` | BSD-3-Clause | `2520dc115610` |
| `i-rinat__apulse` | MIT | `5d654cecd184` |
| `kakahuote1__TinyPKI` | Apache-2.0 | `764a63ad82e1` |
| `lcgamboa__picsim` | GPL-2.0 | `3ee791875e15` |
| `libretro__libretro-fceumm` | GPL-2.0 | `77468e568d9f` |
| `mbuesch__razer` | GPL-2.0 | `877482f8ba2b` |
| `milgra__sov` | GPL-3.0 | `19809ae535f8` |
| `monkey__monkey` | Apache-2.0 | `e257e6383585` |
| `syoyo__tinydng` | MIT | `1f181699511e` |
| `troglobit__mg` | Unlicense | `66bcd7ef2233` |

Full commits and repository URLs are in every manifest entry
(`source_repo`, `source_commit`, `license`).

## Fetch and verify

```bash
hf download sjkim1127/freshbench-v2 --repo-type dataset \
  --revision 15b6f2c52c379cf9361f22edaa75886602e5e1ff \
  --include "m1/**" --local-dir data
freshbench manifest verify data/m1 corpus/m1/manifest.jsonl.gz
```

`verify` reports missing files, size or sha256 mismatches, and files the
manifest does not list. One architecture is about 3.1–3.6 GB; add
`--include` patterns such as `"m1/*/aarch64-*/**" "m1/sources/aarch64/**"
"m1/projects/**"` to fetch less (x86-64 builds have no prefix); `verify`
then lists the rest as missing.

## What M1 is and is not

* It measures how close a decompiler's output for a stripped binary comes to
  the original C function: CFG shape (FreshBench and Joern graphs), types
  against DWARF, and whether the output recompiles. See the top-level
  README for each metric.
* It does not measure correctness. The harness executes no corpus binary,
  and a similar graph says nothing about whether the output computes the
  same thing.
* Changing anything above (revision, image, selection rule) makes a new
  milestone, not a new M1.
