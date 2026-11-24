# rust-joern PR integration (#21 through #68)

Repository: `rust-joern/`, a clone of Noelo-Lab/rust-joern at main `6b85a04`.

- **Final branch:** `integration2`
- **Final commit:** `2dcd3c02ace20f6194061845fc3556b0704efd30`
- **Result:** 48 of 48 PRs merged. 20 merged without conflict, 28 needed conflict resolution, 0 skipped.
- **Follow-up commits:** 11 separate commits fix semantic conflicts between PRs (listed below).
- **Final `cargo test --release`:** 364 passed, 0 failed. Python suite: 217 run, OK, 15 skipped by the environment.
- PR #69 (draft "do not merge") is excluded. It is not an ancestor of `integration2`.
- Nothing was pushed. No GitHub PR or issue was touched. The `prN` branches are unchanged. The old `integration` branch was left as it was.

Each `prN` head matched the PR's head SHA on GitHub (checked read-only with `gh pr list`). Merge commits use the PR's GitHub title: `Merge PR #N: <title>`.

## Method

- `integration2` starts at `main`. PRs were merged in increasing number order with `git merge --no-ff prN` and `core.hooksPath=/dev/null`.
- After every merge, and again after each follow-up commit, these ran:
  - `cargo build --release --all-targets`
  - `cargo test --release --no-fail-fast`
  - the Python suite: `PYTHONPATH=python RUST_JOERN_LIBRARY=target/release/librust_joern.dylib python3 -m unittest discover -s tests -p 'test_*.py'`
  - `cargo fmt --all -- --check`
- The per-merge runs overrode the release profile to `lto=off, codegen-units=16` to save time. The final run used the repository's real release profile (thin LTO, codegen-units=1) with no override.
- `cargo clippy --all-targets -- -D warnings` is clean on both `main` and `integration2`.
- Conflicts in tests and docs were resolved as a union at the level of whole items: both sides' test functions and `docs/divergences.md` sections were kept. Each PR's new divergence section stays at the top, which is the file's existing convention.
- Every code conflict was resolved by hand, as described below.

## Baseline on `main` (`6b85a04`)

`cargo test --release`: **281 passed, 0 failed** (no pre-existing failures).

| Test binary | Passed | Failed |
|---|---:|---:|
| src/lib.rs (unit) | 198 | 0 |
| src/main.rs | 0 | 0 |
| tests/builtin_dataflow.rs | 1 | 0 |
| tests/cpp_call_ast.rs | 10 | 0 |
| tests/cpp_class_alias_scope.rs | 3 | 0 |
| tests/cpp_constructor_initializers.rs | 5 | 0 |
| tests/cpp_destructor_calls.rs | 9 | 0 |
| tests/cpp_isolated_special_members.rs | 4 | 0 |
| tests/cpp_noexcept_cfg.rs | 3 | 0 |
| tests/cpp_qualified_expressions.rs | 16 | 0 |
| tests/cpp_range_for_cfg.rs | 3 | 0 |
| tests/cpp_range_for_init.rs | 3 | 0 |
| tests/cpp_range_for_ternary.rs | 1 | 0 |
| tests/cpp_scope_qualifier.rs | 3 | 0 |
| tests/cpp_scoped_enum.rs | 2 | 0 |
| tests/cpp_sizeof_pack.rs | 2 | 0 |
| tests/cpp_source_context.rs | 4 | 0 |
| tests/cpp_static_assert_cfg.rs | 3 | 0 |
| tests/ddg_stored_order.rs | 3 | 0 |
| tests/directory_dataflow.rs | 7 | 0 |
| tests/scope_shadowing.rs | 1 | 0 |
| doc-tests | 0 | 0 |

Python suite on `main`: 194 run, OK, 9 skipped. The 9 skips need a local DecBench checkout, or are DDG audit tests that report "Native library unavailable".

## Per-PR outcome

"Clean" means `git merge` had no textual conflict. "Resolved" means a textual conflict was resolved inside the merge commit. Follow-up commits are listed in the next section.

| PR | Outcome | How |
|---|---|---|
| #21 | Clean | |
| #22 | Clean | Its catch-binding scope was later adapted to #39's API (follow-up F2). |
| #23 | Resolved | `parser.rs` `looks_declaration`: kept #21's braced-temporary check, which uses the `start + 1` fallback when no expression name is found. #23's shift guard runs only when an expression name exists, as #23 wrote it. |
| #24 | Clean | |
| #25 | Clean | |
| #26 | Clean | |
| #27 | Clean | |
| #28 | Resolved | `divergences.md` and `cpp_qualified_expressions.rs`: kept both sides' sections and tests. |
| #29 | Resolved | `cpp_name`: type contexts (#24) keep `angle_close`; expression lookahead uses #29's bounded `expression_template_close`. Kept both helpers and the union of tests. |
| #30 | Resolved | #21 and #30 both added a `braced_type_name` with different signatures. #30's two-argument version is renamed `braced_expression_type_name`. Its explicit-type branch delegates to #21's `braced_type_name`, so #21's `variable_types` check is kept. #30's qualified-name trivia match is added on top. |
| #31 | Clean | |
| #32 | Resolved | Constructor initializers: kept #25's pointer-type map (`Option<HashMap>`, pointer members and parameters) and #32's nested scope argument, initializer-body skip and braced-literal plan. Parenthesized values take #25's extended `scalar_initialization_value`. Braced values take #32's literal-only rule. Kept both divergence sections. |
| #33 | Resolved | Kept #33's `parse_with(&Options)` API, combined with #25's `HashMap` scalar classes (`constructor_initializers: options.cpp_constructor_initializers`). The `Options::joern_cpp_control_flow` doc now names both `if constexpr` (#26) and consteval if (#33). Kept both sections. Follow-ups F1, F7, F9 and F10 adapt later code to this API. |
| #34 | Clean | |
| #35 | Clean | Its 6 new Python tests are skipped here because `tree_sitter` / `tree_sitter_cpp` are not installed. |
| #36 | Clean | |
| #37 | Clean | Its unit test used the old `parse_with` arguments (follow-up F1). |
| #38 | Resolved | `function_header_in` keeps #31's `declaration_start` and #38's `after_template_heads` skip. The function body keeps #33's consteval scope and #38's template-body limit. Follow-ups F3 and F5 update #38's test expectations; F4 updates #43's code for this skip. |
| #39 | Resolved | Function-scope restore: #33's consteval alias restore, then #39's `variables.restore`. |
| #40 | Resolved | `cpp_name` condition: kept #24 type context, #29's bounded close, and #40's `template_subscript_follows` disjunct. Kept both helpers. |
| #41 | Resolved | `cpp_qualified_expressions.rs`: kept both sides' tests. |
| #42 | Resolved | Added #42's `terminal_template_arguments` as another disjunct beside #40's. Kept both helpers and the union of tests. |
| #43 | Resolved | Kept both helper functions and the union of tests. See follow-up F4. |
| #44 | Resolved | Class scope: kept #31's `primary_class_templates`, #32's scope-aware `collect_class_fields`, and #25's `HashMap` scalar classes. #44's specialization save/restore now restores the pointer-type map (`Option<HashMap>`) instead of a bool. Restore order: #44's specialization state, then #39's variable restore. `record_definition` uses #38's `after_template_heads`, #36's qualified record names, and #44's specialization close and `(introducer, specialized)` result. |
| #45 | Clean | |
| #46 | Clean | |
| #47 | Resolved | Kept #39's and #47's unit tests in `mod tests`. Checked that no merged PR replaces `self.tokens` without updating `delimiter_ends`. The only replacement is in macro-expansion statement parsing, which #47 already handles. |
| #48 | Resolved | `cpp_qualified_expressions.rs`: kept both sides' tests. |
| #49 | Clean | Changes method start lines (see follow-up F5). |
| #50 | Resolved | Added #50's `logical_template_operand` as a third disjunct in `cpp_name`. Kept both helpers. |
| #51 | Resolved | `divergences.md`: kept both sections. |
| #52 | Clean | Changes the default constructor-initializer behavior (see follow-up F6). #52 itself replaced main's `recovery_does_not_qualify_unknown_initialization_effects` with `recovery_keeps_the_body_without_initialization_effects`. |
| #53 | Resolved | `record_definition`: the qualified-record loop runs when `self.cpp || !self.cdt_failures`, which covers #36's C++ case and #53's decompiled-C case. Kept #44's specialization check. Kept #31's and #53's helpers and both sections. See follow-up F7. |
| #54 | Resolved | Type-name insertion: #54's `ns::` scope guard first, then #33's consteval alias remembering. |
| #55 | Resolved | Braced temporary at `{`: #30/#21's `braced_expression_type_name`, OR #55's "unknown name inside an expression or qualified", AND #55's balanced-brace check. See follow-up F8. |
| #56 | Clean | |
| #57 | Resolved | #57's later-declared fields are inserted with #39's `VariableNames::insert`. They are rolled back by the function's `restore`. |
| #58 | Clean | |
| #59 | Resolved | Kept #24's `declaration_specifier_end` and #59's `parameter_specifier_end`; they cover different contexts. Kept both sides' tests. |
| #60 | Resolved | Parser fields: both. `template_classes` is inserted beside #44's `new_scope` and #31's primary template. The duplicate-binding key combines #43's explicit-specialization exemption, #60's `identity_signature` (computed with `distinct_dependent_types`), and #44's `(class_specialization, key)` tuple. |
| #61 | Clean | |
| #62 | Resolved | `builder.rs`: kept #33's `ConstevalIf` arms and #62's declaration-condition `while`/`switch` arm. Kept both sections. |
| #63 | Clean | |
| #64 | Resolved | `cpp_call_ast.rs`: kept both sides' tests. |
| #65 | Resolved | `cpp_call_ast.rs`: kept both sides' tests. |
| #66 | Clean | Its unit tests used the old `parse_with` arguments (follow-up F9). |
| #67 | Resolved | `function_header_in`: kept #43's specialization branch first, then #67's `operator_ids` gate on the operator-name branches. Kept #53's `c_operand_ends_at` and #67's `unclosed_conditional`. See follow-ups F10 and F11. |
| #68 | Resolved | `divergences.md` and `cpp_qualified_expressions.rs`: kept both sides' sections and tests. |

**Skipped:** none.

## Follow-up commits (semantic conflicts, separate from merges)

| # | Commit | PRs | What and why |
|---|---|---|---|
| F1 | `bc0ae83` | #33, #37 | #37's unit test called the old six-argument `parse_with`. It now passes `Options { joern_cdt_failures, .. }`. |
| F2 | `b2aed68` | #22, #39 | #22's catch-parameter scope cloned and reassigned the name set. It now uses #39's `checkpoint()`/`restore()`. This was the `VariableNames::clone` / E0308 build failure on the old `integration` branch. |
| F3 | `292db3a` | #38, #40 | #38's recovery test expected a diagnostic for `detail::values<T, 0>[0]`, which it documented as unsupported. #40 parses that subscript, so the test now expects no diagnostic. #38's function-extent assertions are unchanged. |
| F4 | `119bcf4` | #31, #38, #43 | #38 makes `function_header_in` skip template heads, so its local `start` no longer points at `template`. #43's `specialized_function_name` now receives #31's pre-skip `declaration_start`. Without this, #43's 3 tests fail. |
| F5 | `3193f0a` | #31, #38, #49 | #49 anchors template definitions after their template heads to match Joern 4.0.150. Its test checks Joern's locations. Three tests from #31 and #38 expected the template-head line. Only their start lines changed (+1); body, owner and end-line assertions are unchanged. |
| F6 | `57f0183` | #32, #52 | #52 removes the profile-off "member-initializer CFG is unsupported" error, as Joern does. #32's two new tests and its divergence sentence expected that error. They now assert #52's behavior: accepted, no field-initialization nodes. #32's profile-on braced-plan assertions are unchanged. |
| F7 | `dffe73c` | #33, #53 | #53's C operator-name fusion read the `cdt_failures` local that #33 removed. It now reads `options.joern_cdt_failures`. |
| F8 | `7e96b75` | #30, #55 | #55 reads any qualified `T{...}` in an expression as a braced temporary, as Joern does (backed by its frozen PyJoern fixture). #30's negative cases for qualified names outside the supplied type context now expect success. #30's positive trivia-matching cases are unchanged. |
| F9 | `7f81d5f` | #33, #66 | #66's unit tests: old `parse_with` arguments changed to `Options`. |
| F10 | `fb5e713` | #33, #67 | #67's unit tests: old `parse_with` arguments changed to `Options`. |
| F11 | `8165364` | #53, #67 | #67's test expected default (not CDT-failure) C parsing to diagnose `operator new(...)` statements. #53 fuses `operator new` before `(` into one name by default, so these are now ordinary calls with no diagnostic. #67's `joern_cdt_failures` assertions are unchanged and pass. |

## Final `cargo test --release` on `integration2` (`2dcd3c0`, real release profile)

**364 passed, 0 failed.** Build warnings: 0. `cargo fmt --check`: clean.

| Test binary | Passed | Failed |
|---|---:|---:|
| src/lib.rs (unit) | 205 | 0 |
| src/main.rs | 0 | 0 |
| tests/builtin_dataflow.rs | 1 | 0 |
| tests/c_decompiled_cpp_recovery.rs | 5 | 0 |
| tests/cpp_call_ast.rs | 15 | 0 |
| tests/cpp_catch_bindings.rs | 1 | 0 |
| tests/cpp_class_alias_scope.rs | 4 | 0 |
| tests/cpp_class_specializations.rs | 2 | 0 |
| tests/cpp_consteval_if.rs | 5 | 0 |
| tests/cpp_constexpr_if_cfg.rs | 5 | 0 |
| tests/cpp_constructor_initializers.rs | 10 | 0 |
| tests/cpp_declaration_conditions.rs | 2 | 0 |
| tests/cpp_destructor_calls.rs | 9 | 0 |
| tests/cpp_global_qualified_types.rs | 1 | 0 |
| tests/cpp_isolated_special_members.rs | 5 | 0 |
| tests/cpp_logical_template_operands.rs | 2 | 0 |
| tests/cpp_noexcept_cfg.rs | 3 | 0 |
| tests/cpp_out_of_line_template_members.rs | 2 | 0 |
| tests/cpp_pack_expansions.rs | 2 | 0 |
| tests/cpp_qualified_expressions.rs | 39 | 0 |
| tests/cpp_qualified_object_shift.rs | 2 | 0 |
| tests/cpp_qualified_records.rs | 4 | 0 |
| tests/cpp_range_for_cfg.rs | 3 | 0 |
| tests/cpp_range_for_init.rs | 3 | 0 |
| tests/cpp_range_for_ternary.rs | 1 | 0 |
| tests/cpp_scope_qualifier.rs | 3 | 0 |
| tests/cpp_scoped_enum.rs | 2 | 0 |
| tests/cpp_shift_statements.rs | 1 | 0 |
| tests/cpp_sizeof_pack.rs | 2 | 0 |
| tests/cpp_source_context.rs | 5 | 0 |
| tests/cpp_static_assert_cfg.rs | 3 | 0 |
| tests/cpp_template_definition_locations.rs | 1 | 0 |
| tests/cpp_unknown_braced_temporaries.rs | 2 | 0 |
| tests/ddg_stored_order.rs | 3 | 0 |
| tests/directory_dataflow.rs | 7 | 0 |
| tests/msvc_pragmas.rs | 2 | 0 |
| tests/scope_shadowing.rs | 2 | 0 |
| doc-tests | 0 | 0 |

Python suite: 217 run, OK, 15 skipped. 9 skips are the same as on `main`. The other 6 are #35's alias-evidence tests, which need the optional `tree_sitter` / `tree_sitter_cpp` packages. They were not installed because that would mean downloading packages.

### Checks against the requirements

- **Every test that passed on `main` still passes,** with one exception: `recovery_does_not_qualify_unknown_initialization_effects`. #52 itself replaced it with `recovery_keeps_the_body_without_initialization_effects`, which passes. That is #52's own intended behavior change, not an integration loss.
- **Every Rust test function added by a merged PR passes** (86 checked by name). The 11 follow-up commits changed only these tests' expected values: F3, F5, F6, F8 and F11.
- **Every Python test added by a merged PR passes,** except #35's 6 environment skips.

## Notes for vendoring

- Each cross-PR interaction above is covered by the PRs' own tests and the frozen PyJoern fixtures. Nothing was compared against Joern beyond the checked-in references.
- **#30 is mostly redundant after #55.** With #55, any qualified non-variable `T{...}` in an expression is accepted anyway, so #30's qualified-context match only matters for the `!variables.contains(canonical)` distinction.
- **#60 also records explicit and partial class specializations.** It records a class as a template whenever the record starts with `template`. After #44, that includes these specializations. This is harmless when the primary template is also in the source. With only a full specialization in the source (for example after header stripping), its injected name would be typed `ANY`. No test covers this case.
