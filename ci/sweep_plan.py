#!/usr/bin/env python3
"""The evaluate dispatches of one full M1 sweep.

A sweep runs every registered tool over every M1 target in both tracks. It is
split the way the manual reruns were: per architecture and track, one
dispatch for the ordinary projects (every build in one job each) and one for
each large project, whose builds get a job each. A tool whose registry entry
says `"decompile": false` prints no evidence a decompile row could be scored
by, so it runs in the boundary track only. One with `"sweep": false` is left
out of sweeps (it cannot run on the standard runners); it stays registered and
can be dispatched by hand.

The evaluate runs of a sweep share one cap on concurrent jobs
(`TOTAL_PARALLEL`, sweep.yml's `total_parallel` input): each run gets its own
`max_parallel`, an equal share of it, the remainder going to the largest runs.
GitHub has no limit across runs, so a run that ends early frees its share
only for other repositories.

The population comes from the pinned M1 manifest (its truth files are
`truth/<build>/<project>/targets.json`), never from a hand-kept list.
"""
import argparse
import gzip
import json
from pathlib import Path

ARCHES = ("x86_64", "i686", "aarch64", "armv7")
TRACKS = ("decompile", "boundaries")
# concurrent evaluate jobs of a whole sweep: the organization runs 60 at once,
# and the rest stay free for other repositories' CI
TOTAL_PARALLEL = 40
# builds whose name has no architecture prefix are x86_64 (gcc-O0, clang-Os)
PREFIXED = tuple(a for a in ARCHES if a != "x86_64")


def arch_of(build):
    for arch in PREFIXED:
        if build.startswith(arch + "-"):
            return arch
    return "x86_64"


def population(manifest):
    """{arch: {project: [builds]}} from the manifest's truth files."""
    out = {}
    with gzip.open(manifest, "rt") as source:
        for line in source:
            if not line.strip():
                continue
            path = json.loads(line)["path"]
            parts = path.split("/")
            if len(parts) != 4 or parts[0] != "truth" or parts[3] != "targets.json":
                continue
            build, project = parts[1], parts[2]
            out.setdefault(arch_of(build), {}).setdefault(project, []).append(build)
    return {a: {p: sorted(b) for p, b in sorted(ps.items())} for a, ps in sorted(out.items())}


def tools_for(registry, track):
    """Registered non-LLM tools, in registry order, that run in `track`."""
    out = []
    for tool, info in registry.items():
        if info.get("llm", False) or info.get("sweep", True) is False:
            continue
        if track == "decompile" and info.get("decompile", True) is False:
            continue
        out.append(tool)
    return out


def share(slices, total):
    """Set each slice's `max_parallel` so that they sum to `total`."""
    if total < len(slices):
        raise ValueError(f"total_parallel {total} is below one job for each of {len(slices)} runs")
    each, rest = divmod(total, len(slices))
    jobs = lambda s: len(s["tools"]) * len(s["projects"]) * len(s["builds"])
    for s in slices:
        s["max_parallel"] = each
    for s in sorted(slices, key=jobs, reverse=True)[:rest]:
        s["max_parallel"] += 1
    return slices


def plan(pop, registry, split, total=TOTAL_PARALLEL):
    """The dispatches: one dict of evaluate inputs each."""
    unknown = sorted(set(split) - {p for ps in pop.values() for p in ps})
    if unknown:
        raise ValueError(f"split projects not in the corpus: {unknown}")
    slices = []
    for arch in ARCHES:
        projects = pop.get(arch)
        if not projects:
            raise ValueError(f"no M1 targets for {arch}")
        for track in TRACKS:
            tools = tools_for(registry, track)
            if not tools:
                raise ValueError(f"no tools for the {track} track")
            ordinary = [p for p in projects if p not in split]
            if ordinary:
                slices.append({"arch": arch, "track": track, "tools": tools,
                               "projects": ordinary, "builds": [""]})
            for project in split:
                if project in projects:
                    slices.append({"arch": arch, "track": track, "tools": tools,
                                   "projects": [project], "builds": projects[project]})
    for s in slices:
        jobs = len(s["tools"]) * len(s["projects"]) * len(s["builds"])
        # GitHub runs at most 256 jobs from one matrix
        if jobs > 256:
            raise ValueError(f"{s['arch']} {s['track']} {s['projects'][:3]}: {jobs} jobs > 256; split further")
    return share(slices, total)


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--manifest", type=Path, default=Path("corpus/m1/manifest.jsonl.gz"))
    ap.add_argument("--registry", type=Path, default=Path("ci/tools.json"))
    ap.add_argument("--split", default='["ComposersDesktop__CDP8"]',
                    help="JSON list of projects that get one job per build")
    ap.add_argument("--total-parallel", type=int, default=TOTAL_PARALLEL,
                    help="concurrent evaluate jobs of the whole sweep")
    args = ap.parse_args()
    registry = json.loads(args.registry.read_text())
    split = json.loads(args.split)
    if not isinstance(split, list) or not all(isinstance(p, str) for p in split):
        raise SystemExit("--split must be a JSON list of project names")
    print(json.dumps(plan(population(args.manifest), registry, split, args.total_parallel)))


if __name__ == "__main__":
    main()
