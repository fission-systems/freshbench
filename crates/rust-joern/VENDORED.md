# Vendored rust-joern

Source: https://github.com/Noelo-Lab/rust-joern (Apache-2.0, see LICENSE).

This copy is upstream main `6b85a04213f1bd747dddde4b23f5c984b0601305`
(2026-11-09) with the open pull requests #21 through #68 merged in number
order, plus 11 follow-up commits that resolve semantic conflicts between them.
The merged tree is the local integration commit
`2dcd3c02ace20f6194061845fc3556b0704efd30`, which is not published upstream.
PR #69 (a draft marked "do not merge") is not included.

`INTEGRATION.md` records how each PR was merged, every follow-up commit, the
five test expectations a later PR changed on purpose, and the test results:
`cargo test --release` 364 passed, 0 failed (main alone: 281 passed).

Local changes beyond the integration: none. CLAUDE.md and AGENTS.md, agent
instructions for the upstream repository, are not copied.

When the upstream PRs merge, replace this copy with upstream main and delete
INTEGRATION.md.
