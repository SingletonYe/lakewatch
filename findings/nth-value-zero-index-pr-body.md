# Ready-to-open PR: report a zero `nth_value` index as a user error in the aggregate form

Branch `fix/nth-value-zero-index` (commit `dcaac3e`, 2 files) on `SingletonYe/datafusion`.
Open at:
`https://github.com/apache/datafusion/compare/main...SingletonYe:fix/nth-value-zero-index?expand=1`

Post the issue first (text in `nth-value-zero-index-issue.md`) and put its number in the first line
below, or leave the line as it is and let the body carry the report. Re-checked 2026-10-22: upstream
`nth_value.rs` still carries both assertions, so this branch is current.
**Paste from the horizontal rule down.**

---

## Which issue does this PR close?

Closes #<issue-number> — if no issue was filed, this body contains the report.

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
(`nth_value(x, n ORDER BY …)`) behave exactly as before. `NTILE` already answers a plain execution
error for the analogous mistake (`NTILE requires a positive integer`).

## What changes are included in this PR?

* `datafusion/functions-aggregate/src/nth_value.rs`: check `n == 0` once in `NthValue::accumulator`,
  where the index is read from the literal and the ordered/unordered accumulator is chosen, and remove
  the two `assert_or_internal_err!` guards — `accumulator()` is their only caller.
* `datafusion/sqllogictest/test_files/aggregate.slt`: one case asserting the new message.

## Are these changes tested?

Yes. `cargo test -p datafusion-sqllogictest --test sqllogictests -- test_files/aggregate.slt` passes
including the new case (it fails on a tree without the fix), `window.slt` still passes untouched, and
`cargo fmt --all --check` is clean.

## Are there any user-facing changes?

Yes: the error text for `nth_value(x, 0)` in the aggregate form — an internal error plus bug-report
boilerplate becomes a plain execution error.

## Deliberately not changed

The window form returns `NULL` for a zero index and `window.slt` asserts that on purpose, so it is left
alone; making the two forms agree is a behaviour change and a maintainer decision — say which way and I
will send it. (`i64::MIN` is only reachable through the window path, so it is out of scope here.)
