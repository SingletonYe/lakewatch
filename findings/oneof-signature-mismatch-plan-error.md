# Finding: `OneOf` signature mismatch reported as an internal error

**Status:** OPEN, **APPROVED by alamb 2026-10-15 13:17:18Z** on `268933e`; the first-time
contributor CI gate is now open and the checks are queued — next slot must read the CI result
and fix it in-slot if it is red. Do not nudge; it was approved minutes ago
**DataFusion:** `apache/datafusion@6023600` (2026-10-14)
**Upstream refs:** #20501 (open, this is part of it), #19004 (open, related), #21032 (closed
unmerged — the same fix attempted as a much larger change and asked to be split up)
**Branch:** `SingletonYe/datafusion:fix/oneof-signature-mismatch-plan-error` @ `268933e`
**PR:** https://github.com/apache/datafusion/pull/26329

## Symptom

Any call whose arguments match no branch of a `OneOf` signature is reported to the user as
an internal DataFusion bug:

```console
$ lakewatch sql "SELECT substr(1, 2)"
Error during planning: Internal error: Function 'substr' failed to match any signature, errors: Error during planning: Function 'substr' requires String, but received Int64 (DataType: Int64).,Error during planning: Function 'substr' expects 3 arguments but received 2.
This issue was likely caused by a bug in DataFusion's code. Please help us to resolve this by filing a bug report in our issue tracker: https://github.com/apache/datafusion/issues
```

Nothing internal went wrong: the caller passed the wrong argument types or arity.

## Root cause

`datafusion/expr/src/type_coercion/functions.rs`, `get_valid_types_with_udf`: when every
`OneOf` branch fails, the collected errors are returned through `internal_err!` rather than
`plan_err!`. `DataFusionError::Internal` renders with the "likely caused by a bug … file a
bug report" boilerplate; `DataFusionError::Plan` renders as a plain planning error.

## Attribution

A robustness sweep over all 364 scalar functions of the pinned DataFusion
(`scripts/df_robust.py`, 1874 invocations) produced 391 findings in 20 root-cause clusters.
Re-running each cluster's repro and inspecting the full stderr shows **353 of 391 findings
(13 of 20 clusters) come from this single call site**:

`btrim`, `date_part`, `datepart`, `date_trunc`, `datetrunc`, `generate_series`, `log`,
`regexp_like`, `round`, `string_to_list`, `substr`, `substring`, `trim`.

The remaining 38 are `element_at`/`map_extract` (8 + 8, a different internal-error path —
still untriaged) and `NOT_IMPL` gaps (`gcd`/`lcm`, `to_time`/`to_timestamp*`, `arrow_cast`).

## Fix

One line, plus a test and the user-visible expectations that come with it. Deliberately
narrow: the message body, candidate signatures and the "No function matches…" follow-up are
unchanged. Message verbosity and the missing separator (`…#19004`) are left for follow-ups.

## Verification performed

At the first push (`1fd13a7`):

* new unit test fails on `main`, passes with the change;
* `cargo test -p datafusion-expr --lib` → 282 passed, 0 failed;
* `cargo fmt --all --check` clean;
* audited every other assertion on this message across `.rs` and `.slt`;
* ten `sqllogictest` expectations updated (`array/array_range.slt`, `spark/math/round.slt`,
  `errors.slt`, `math.slt`).

Full `sqllogictest` was **not** run locally (needs a full `datafusion-core` build); CI is the
verification for those four files.

## Review round 1 — 2026-10-15, alamb (CONTRIBUTOR), `COMMENTED`

> Thanks @SingletonYe -- this looks great -- thank you for the focused PR -- other than
> adjusting the tests, this looks ready to me

Two inline comments on the unit test this branch added, at
`type_coercion/functions.rs:1498`:

> I think we can remove this unit test as the .slt tests cover all the same paths, right? It
> seems like we don't need double coverage
> If you think we need more coverage, please adapt this test to .slt

**Action taken (commit `268933e`, a normal commit, not a force-push):** the unit test is
deleted, 39 lines. Checked before agreeing to drop it, rather than taking the reviewer's word:
`round`'s signature is a `OneOf` with 1-argument and 2-argument branches, so `SELECT
round('x')` in `math.slt` fails one branch on the argument type and the other on the arity —
the exact shape the unit test constructed. `round(1, 2, 3)`, `round(true)`,
`round(timestamp)`, the four `generate_series` cases and `nth_value` cover the rest. So
nothing moved to `.slt`: the coverage was already there.

Why the `.slt` cases really do guard this change, in the runner's semantics: expectations are
unanchored regexes matched against the full error string (`sqllogictest-0.29.1`,
`parser.rs:465`), and `verify_function_arguments` (`expr_schema.rs:790`) renders a `Plan`
error as the bare message but any other variant with its `Display` prefix. So on `main` the
actual text is `Error during planning: Internal error: Function 'round' failed to match any
signature, …` and the expectation `Error during planning: Function 'round' failed to match
any signature` does **not** match it; after the change it does. Each of the ten updated
expectations therefore fails on `main` — that is the regression guard, and it is why the
variant change is observable at all.

Re-verified after the deletion: `cargo fmt --all --check` clean, `cargo test -p
datafusion-expr --lib` 281 passed / 0 failed (282 before, i.e. exactly the deleted test), no
new warnings — none of the test module's helpers became unused, because `MockUdf`,
`assert_contains`, `logical_string` and `logical_int64` are all still used by the remaining
tests.

Not done, and it is not ours to do: the reply comment and the PR-body correction (`findings/oneof-signature-mismatch-plan-error-pr-body.md`) both need the user, because `apache/*` refuses our writes (see `reports/datafusion-findings.md`).

## Local `sqllogictest` verification (added 2026-10-15)

The first push could only say "CI is the verification" for the four `.slt` files; that gap is
now closed. `cargo test -p datafusion-sqllogictest --test sqllogictests --no-run` builds in
**6m14s** on this sandbox (2 vCPU), and the runner takes file substrings as filters, so the
affected files can be executed directly:

```bash
cd datafusion/sqllogictest   # the runner resolves test_files/ from the cwd
../../target/debug/deps/sqllogictests-<hash> math.slt errors.slt array_range.slt spark/math/round.slt
```

* On `268933e`: **exit 0**, all five files pass.
* With the one-line source change reverted (`.slt` expectations untouched): **ten failures, at
  exactly the ten expectations this PR updates** — `array_range.slt:247,251,254,257`,
  `errors.slt:127`, `math.slt:178,181`, `spark/math/round.slt:596,600,604` — each reporting
  `but got error: DataFusion error: Error during planning: Internal error: Function 'round'
  failed to match any signature, …`.

That is the regression guard demonstrated, not argued: the expectations fail without the fix
and pass with it.

**Sandbox gotcha, worth remembering:** the first run of `errors.slt` reported two failures at
lines 44 and 159 that have nothing to do with this change. `testing/` is an empty *submodule*
in a plain `git clone` (arrow-testing), so `aggregate_test_100` comes out empty and those two
cases "succeed". After `git submodule update --init --depth 1 testing` (12 MB) the file is
clean. Do not diagnose a change from an uninitialised-submodule run.
