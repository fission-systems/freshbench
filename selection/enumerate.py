#!/usr/bin/env python3
"""Enumerate every GitHub repository that meets the FreshBench selection rule.

Rule (fixed before any build was attempted):
  language:C (GitHub's primary-language classification), 50..3000 stars,
  pushed on or after CUTOFF (2 years before the snapshot date), not a fork,
  not archived, not a template, OSI-approved license (SPDX id in OSI_SPDX),
  repository size <= 200 MB.
The result is written sorted by the selection hash
  sha256("freshbench-v1/" + full_name.lower())
so the candidate order is fixed by the rule, not by hand.

Uses the `gh` CLI (GitHub search API; 1000 results per query, so the star
range is split until every slice is under the cap).
"""
import hashlib, json, subprocess, sys, time

SNAPSHOT = "2026-11-22"
CUTOFF = "2024-11-22"
OSI_SPDX = {
    "MIT", "Apache-2.0", "BSD-2-Clause", "BSD-3-Clause", "BSD-3-Clause-Clear", "0BSD",
    "GPL-2.0", "GPL-3.0", "LGPL-2.1", "LGPL-3.0", "AGPL-3.0", "MPL-2.0", "ISC", "Zlib",
    "Unlicense", "BSL-1.0", "EPL-2.0", "EPL-1.0", "Artistic-2.0", "MIT-0", "NCSA",
    "UPL-1.0", "OSL-3.0", "ECL-2.0", "MS-PL", "MulanPSL-2.0", "EUPL-1.2", "CDDL-1.0",
    "BSD-4-Clause",  # not OSI; listed here only to be excluded explicitly below
}
OSI_SPDX.discard("BSD-4-Clause")
MAX_SIZE_KB = 200 * 1024


def search(q, page):
    for attempt in range(8):
        p = subprocess.run(
            ["gh", "api", "-X", "GET", "search/repositories", "-f", f"q={q}",
             "-f", "per_page=100", "-f", f"page={page}", "-f", "sort=stars", "-f", "order=desc"],
            capture_output=True, text=True)
        if p.returncode == 0:
            time.sleep(2.1)  # search API: 30 requests/minute
            return json.loads(p.stdout)
        if "rate limit" in p.stderr.lower() or "secondary" in p.stderr.lower() or "403" in p.stderr:
            time.sleep(30 * (attempt + 1)); continue
        if "422" in p.stderr:
            return {"total_count": 0, "items": []}
        time.sleep(5)
    raise SystemExit(f"search failed: {q} p{page}: {p.stderr}")


def base_q(lo, hi):
    return (f"language:C stars:{lo}..{hi} pushed:>={CUTOFF} fork:false archived:false "
            f"template:false")


def slices(lo, hi):
    r = search(base_q(lo, hi), 1)
    n = r["total_count"]
    if n <= 1000 or lo == hi:
        if n > 1000:
            print(f"WARN stars:{lo} has {n} > 1000; splitting by created year", file=sys.stderr)
            for y in range(2008, int(SNAPSHOT[:4]) + 1):
                yield f"{base_q(lo, hi)} created:{y}-01-01..{y}-12-31"
            return
        yield base_q(lo, hi)
        return
    mid = (lo + hi) // 2
    yield from slices(lo, mid)
    yield from slices(mid + 1, hi)


def main(out):
    repos = {}
    for q in slices(50, 3000):
        page = 1
        while True:
            r = search(q, page)
            for it in r["items"]:
                repos[it["full_name"]] = it
            if len(r["items"]) < 100 or page == 10:
                break
            page += 1
        print(f"{q!r}: total so far {len(repos)}", file=sys.stderr)
    rows = []
    for fn, it in repos.items():
        lic = (it.get("license") or {}).get("spdx_id")
        reason = None
        if lic not in OSI_SPDX:
            reason = f"license:{lic}"
        elif it["size"] > MAX_SIZE_KB:
            reason = f"size:{it['size']}KB"
        elif it.get("is_template"):
            reason = "template"
        rows.append({
            "full_name": fn, "stars": it["stargazers_count"], "license": lic,
            "pushed_at": it["pushed_at"], "size_kb": it["size"],
            "default_branch": it["default_branch"], "description": it.get("description"),
            "topics": it.get("topics", []), "html_url": it["html_url"],
            "hash": hashlib.sha256(("freshbench-v1/" + fn.lower()).encode()).hexdigest(),
            "rule_excluded": reason,
        })
    rows.sort(key=lambda r: r["hash"])
    with open(out, "w") as f:
        json.dump({"snapshot": SNAPSHOT, "cutoff": CUTOFF, "count": len(rows), "repos": rows}, f, indent=1)
    print(f"wrote {len(rows)} repos, {sum(1 for r in rows if not r['rule_excluded'])} pass license/size",
          file=sys.stderr)


if __name__ == "__main__":
    main(sys.argv[1])
