# Excluded corpora

`decbench_token_fingerprints.json`: token fingerprints (`freshbench fingerprints`,
sha256 of each function definition's tree-sitter token stream) of every function
defined in the preprocessed sources of the DecBench dataset
(`noelo-lab/decbench-dataset` revision `815eb96f130b6d2ef7fa600e04669bf20139bf27`,
5,404 files, 33,391 distinct fingerprints). A FreshBench function whose fingerprint
is in this list is not selected, so no function a decompiler may have been tuned on
through DecBench enters FreshBench. Only hashes are stored, no code.

Regenerate: `freshbench fingerprints <decbench-dataset>/sources --out decbench_token_fingerprints.json`.

