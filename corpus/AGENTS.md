# AGENTS.md — `corpus/`

Root `AGENTS.md` applies. This directory pins corpus identity; it does not hold corpus bytes.

## What is committed

`corpus/m1/` contains the M1 manifest (`manifest.jsonl.gz`), its uncompressed digest, duplicate
audits, toolchain/package lists and `README.md`. Binaries, stripped inputs, sources, truth files
and licenses live on Hugging Face at the revision pinned in `corpus/m1/README.md`.

- Do not commit corpus data, result trees, caches or downloads here.
- Do not use Git LFS. Storage is a pinned Hugging Face revision.
- Every manifest entry carries path, size, sha256, kind and provenance (repo, commit and license
  where applicable). Binary entries also record compiler version, target and flags.

## Immutability

- A milestone is immutable once published. Changing the revision, build image, selection rule,
  project list, compiler set or function count creates a new milestone; it does not update M1.
- Never edit a manifest, digest, hash table or README table by hand to make a check pass.
- To change a manifest, rebuild it from the verified tree with
  `freshbench manifest build` and its provenance input, then update the digest, README figures
  and any dependent duplicate audits in the same change.
- Never add a project by hand. Corpus membership comes from `selection/` and qualification.

## Duplicate audits

The `duplicates-<arch>.json` files are generated with
`freshbench duplicates <corpus> --arch <arch> --top 1000000` for all four architectures. Do not
hand-edit them.

Any M1-wide score must also be reported with duplicates removed: `report --dedup` with all four
files and `--dedup-level renamed`. A row whose architecture no file covers is an error, not a
silent omission.

## Verification

Download the pinned revision as described in `corpus/m1/README.md`, then:

```bash
cargo build --release --locked -p freshbench
target/release/freshbench manifest verify data/m1 corpus/m1/manifest.jsonl.gz
```

`verify` reports missing files, size or sha256 mismatches, and unlisted files. If the data is not
present, report the check as **Not executed** or **Blocked**; never infer success from the
manifest alone.
