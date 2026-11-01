# PR body for `fix/nth-value-zero-index` (not yet opened as an apache/datafusion PR)

**Everything between the BEGIN and END markers is the PR description, ready to paste as-is. Nothing
outside them is part of the body.** The previous revision of this file put the authoring notes inside
the pastable region, and that is exactly how scaffolding reached the description of #26657 — do not
reintroduce it.

**One click left:**
https://github.com/apache/datafusion/compare/main...SingletonYe:fix/nth-value-zero-index?expand=1

Branch head `5943a34` (the fix `dcaac3e` merged with `main` at `6a9a1298`), two files, +22/−13.
Re-checked 2026-11-01: upstream `nth_value.rs` still carries both `assert_or_internal_err!` guards, so
this branch is still needed. No issue was filed, so the `Closes` line says so instead of a placeholder.

<!-- BEGIN PASTABLE BODY -->

## Which issue does this PR close?

No issue was filed; this body carries the report.

## Rationale for this change

`nth_value`'s index is 1-based, so `0` is not a valid index. The aggregate form rejects it — but as an
*internal* error, which tells the user to file a bug report for their own typo:

```sql
SELECT nth_value(a, 0) FROM t;
```

```text
before: Internal error: Assertion failed: n != 0: Nth value indices are 1 based. 0 is invalid index
        … This issue was likely caused by a bug in DataFusion's code. Please help us to resolve this
        by filing a bug report …
after:  Execution error: nth_value index must not be 0; indices are 1-based and negative values count from the end
```

Nothing else changes: `nth_value(x, 1)`, `nth_value(x, -1)` and the ordered form
`nth_value(x, n ORDER BY …)` behave exactly as before. `NTILE` already answers a plain execution error
for the analogous mistake.

## What changes are included in this PR?

* `datafusion/functions-aggregate/src/nth_value.rs`: check `n == 0` once in `NthValueAgg::accumulator`,
  where the index is read and the ordered or unordered accumulator is chosen, and remove the two
  `assert_or_internal_err!` guards — `accumulator()` is their only caller, so the check cannot be
  bypassed.
* `datafusion/sqllogictest/test_files/aggregate.slt`: one `query error` case asserting the new message.

## What is the testing strategy for this PR?

`aggregate.slt` gains one `query error` case for the new message. `main` still trips
`assert_or_internal_err!` for a zero index, so that case cannot pass without the source change. On the
branch merged with `main`, `aggregate.slt` and `window.slt` both pass with 0 errors,
`cargo test -p datafusion-functions-aggregate` is 267 + 1 + 2 passed and 0 failed, and
`cargo fmt --all --check` is clean.

## Are there any user-facing changes?

Yes: the error text for `nth_value(x, 0)` in the aggregate form. An internal error plus bug-report
boilerplate becomes a plain execution error, so the user is no longer asked to file a bug for their own
typo.

## The window form is deliberately unchanged

`nth_value(x, 0)` as a window function returns `NULL`, and `window.slt` asserts that on purpose, so this
PR leaves it alone: making the two forms agree would be a behaviour change and a maintainer decision.
Say which way you want it and I will send it. (`i64::MIN` is only reachable through the window path, so
it is out of scope here.)

<!-- END PASTABLE BODY -->
