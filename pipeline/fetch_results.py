#!/usr/bin/env python3
"""Download res-<slug>-<tool>-<compiler> artifacts of decompile runs into results/<slug>/.
usage: fetch_results.py <dest> <run_id>..."""
import json, os, subprocess, sys, shutil
dest = sys.argv[1]
for run in sys.argv[2:]:
    arts = json.loads(subprocess.run(["gh", "api", f"repos/sjkim1127/freshbench/actions/runs/{run}/artifacts?per_page=100",
                                      "--paginate", "--jq", ".artifacts[]|{name,expired}"], capture_output=True,
                                     text=True).stdout.replace("}\n{", "},{").join(["[", "]"]) or "[]")
    for a in arts:
        n = a["name"]
        if not n.startswith("res-"):
            continue
        mark = os.path.join(dest, ".done", f"{run}-{n}")
        if os.path.exists(mark):
            continue
        tmp = os.path.join(dest, ".tmp", n)
        shutil.rmtree(tmp, ignore_errors=True)
        r = subprocess.run(["gh", "run", "download", run, "-R", "sjkim1127/freshbench", "-n", n, "-D", tmp],
                           capture_output=True, text=True)
        if r.returncode:
            print("fail", n, r.stderr[:200]); continue
        body = n[4:]
        tool_comp = body.rsplit("-", 2)
        slug = tool_comp[0]
        for build in os.listdir(tmp):
            for tool in os.listdir(os.path.join(tmp, build)):
                d = os.path.join(dest, slug, build, tool)
                shutil.rmtree(d, ignore_errors=True)
                os.makedirs(os.path.dirname(d), exist_ok=True)
                shutil.move(os.path.join(tmp, build, tool), d)
        shutil.rmtree(tmp, ignore_errors=True)
        os.makedirs(os.path.dirname(mark), exist_ok=True)
        open(mark, "w").close()
        print("ok", n)
