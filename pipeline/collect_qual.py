#!/usr/bin/env python3
"""Download qual-<slug> summaries of a qualify run and list results in hash order.
usage: collect_qual.py <run_id> <batch.json> <dest>"""
import json, subprocess, sys, tarfile, os
run, batch, dest = sys.argv[1], sys.argv[2], sys.argv[3]
os.makedirs(dest, exist_ok=True)
jobs = json.loads(subprocess.run(["gh", "run", "view", run, "-R", "sjkim1127/freshbench", "--json", "jobs"],
                                 capture_output=True, text=True).stdout)["jobs"]
done = {j["name"][7:-1]: j["conclusion"] for j in jobs if j["name"].startswith("build (") and j["status"] == "completed"}
for repo in json.load(open(batch)):
    slug = repo.replace("/", "__"); d = os.path.join(dest, slug)
    st = {}
    if repo in done and not os.path.exists(os.path.join(d, "status.json")):
        subprocess.run(["gh", "run", "download", run, "-R", "sjkim1127/freshbench", "-n", f"qual-{slug}", "-D", d],
                       capture_output=True)
        if os.path.exists(os.path.join(d, "summary.tgz")):
            tarfile.open(os.path.join(d, "summary.tgz")).extractall(d, filter="data")
    if os.path.exists(os.path.join(d, "status.json")):
        st = json.load(open(os.path.join(d, "status.json")))
    elig = ""
    for f in ("prep/prepare_summary.json", "precheck_summary.json"):
        p = os.path.join(d, f)
        if os.path.exists(p):
            b = json.load(open(p))["builds"]
            elig = " ".join(f"{k}:{v.get('eligible')}" for k, v in b.items()) if f.startswith("prep") and len(b) > 1 \
                else f"gcc-O0:{b.get('gcc-O0', {}).get('eligible')} bins:{b.get('gcc-O0', {}).get('binaries')} sys:{b.get('gcc-O0', {}).get('build_system')}"
            break
    print(f"{repo:50s} {done.get(repo, 'pending'):9s} {st.get('result', st.get('stage', '-')):18s} {elig}")
