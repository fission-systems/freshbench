#!/usr/bin/env python3
"""Compare a local tree with the Hub repo: every local file present with the same content."""
import hashlib, os, sys
from huggingface_hub import HfApi
repo, local = sys.argv[1], sys.argv[2]
api = HfApi()
remote = {}
for e in api.list_repo_tree(repo, repo_type="dataset", recursive=True, expand=False):
    if hasattr(e, "blob_id"):
        remote[e.path] = e
bad, missing, ok = [], [], 0
for d, dirs, files in os.walk(local):
    dirs[:] = [x for x in dirs if x != ".cache"]
    for f in files:
        p = os.path.join(d, f); rel = os.path.relpath(p, local)
        data = open(p, "rb").read()
        e = remote.get(rel)
        if e is None:
            missing.append(rel); continue
        if e.lfs:
            same = hashlib.sha256(data).hexdigest() == e.lfs.sha256
        else:
            same = hashlib.sha1(b"blob %d\0" % len(data) + data).hexdigest() == e.blob_id
        (ok := ok + 1) if same else bad.append(rel)
extra = sorted(set(remote) - {os.path.relpath(os.path.join(d, f), local) for d, _, fs in os.walk(local) for f in fs} - {".gitattributes"})
print(f"ok {ok}, differ {len(bad)}, missing {len(missing)}, remote-only {len(extra)}")
for x in (bad[:10], missing[:10], [e for e in extra if not e.startswith(".cache")][:10]): print(x)
