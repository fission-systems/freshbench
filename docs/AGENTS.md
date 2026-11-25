# AGENTS.md — `docs/`

Root `AGENTS.md` applies. `docs/decbench-compatibility-audit.md` is the public record of which
known benchmark defects are handled, and by which test.

## Audit rules

- Keep the status vocabulary exact: **fixed**, **by design**, **partial**, **open**. "Measured"
  requires that the defect was reproduced here before the fix.
- Do not mark a defect **fixed** without a regression test that fails without the fix. Name
  DecBench-parity tests `decbench_NNN_*`.
- Do not delete an unresolved, partial or by-design row to make the audit look complete. Update
  its explanation and test reference instead.
- Add a row when a new DecBench issue or an equivalent benchmark defect is found here. Record the
  affected metric and what is still missing.
- Update the audit in the same change as the code and test. A stale audit is a defect.
- Keep `README.md` normative for what is measured and what is not. Do not describe structural GED,
  compilation success or a perfect signature as proof of correctness.

## Review

Check that every claim names a real module, command or test, that paths still exist, and that
the stated limitation matches current behaviour. There is no documentation build; run the tests
for any behaviour referenced by a changed row and report what ran.
