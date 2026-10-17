# Ready-to-open PR: report a zero `nth_value` index as a user error in the aggregate form

Branch: `fix/nth-value-zero-index` on `SingletonYe/datafusion`
Commit: `dcaac3e` (2 files: `functions-aggregate/src/nth_value.rs`, `aggregate.slt`)
Compare URL: printed by the slot's email; also
`https://github.com/apache/datafusion/compare/main...SingletonYe:fix/nth-value-zero-index?expand=1`

Post the issue first (text in `nth-value-zero-index-issue.md`) and put its number in the first line
below, or leave the line as it is and let the body carry the report.

---

## Which issue does this PR close?

Closes #<issue-number> — if no issue was filed, this body contains the report.

## Rationale for this change

`nth_value`'s index is 1-based, so `0` is not a valid index. In the aggregate form it is rejected —
but it is rejected as an *internal* error, which tells the user to file a bug report for their own
typo:

```sql
SELECT nth_value(a, 0) FROM t;
```

Before:

```text
Internal error: Assertion failed: n != 0: Nth value indices are 1 based. 0 is invalid index
This issue was likely caused by a bug in DataFusion's code. Please help us to resolve this by filing a bug report in our issue tracker: https://github.com/apache/datafusion/issues
```

After:

```text
Execution error: nth_value index must not be 0; indices are 1-based and negative values count from the end
```

Nothing else changes: `nth_value(x, 1)`, `nth_value(x, -1)` and the ordered form
(`nth_value(x, n ORDER BY ...)`) behave exactly as before, and the message is the only user-visible
difference. This is the same class of fix as `NTILE`, which already answers
`Execution error: NTILE requires a positive integer` for the analogous mistake.

## What changes are included in this PR?

- `datafusion/functions-aggregate/src/nth_value.rs`: check `n == 0` once in `NthValue::accumulator`,
  where the index is read from the literal and the ordered/unordered accumulators are chosen, and
  remove the two `assert_or_internal_err!` guards. `accumulator()` is their only caller, so no other
  path loses its validation.
- `datafusion/sqllogictest/test_files/aggregate.slt`: one case asserting the new message.

## Are these changes tested?

Yes.

- `cargo test -p datafusion-sqllogictest --test sqllogictests -- test_files/aggregate.slt` passes,
  including the new case (it fails on a tree without the fix).
- `cargo fmt --all --check` is clean.
- `window.slt` is untouched and still passes.

## Are there any user-facing changes?

Yes: the error text for `nth_value(x, 0)` in the aggregate form, from an internal error plus
bug-report boilerplate to a plain execution error.

## Notes — deliberately not changed

* **The window form is left alone.** `nth_value(x, 0) OVER (...)` returns `NULL` for a zero index,
  and `window.slt` asserts exactly that in a section titled *"test handle NULL and 0 value of
  nth_value"*. That is deliberate, documented behaviour, so changing it is a separate decision and
  would be a behaviour change rather than an error-kind fix. If you want the two forms to agree,
  either validate in `NthValueEvaluator`/`partition_evaluator` as well, or drop the check here and
  return `NULL` in the aggregate form too — say which and I will send it.
* **`i64::MIN` in the aggregate form.** The window path already rejects `i64::MIN` through
  `validate_nth_value_n`, while the aggregate path evaluates `-*value` unguarded. I could not reach
  that code from SQL (`is_reversed` was false in every form I tried, and
  `nth_value(x, i64::MIN)` returns NULL rather than panicking), so it is not part of this PR.

## Separate report, no PR

A recursive CTE that references itself inside a scalar subquery fails with
`Internal error: Unexpected empty work table.` where its sibling spellings (`JOIN`, `EXISTS`, `IN`)
get the clean `Multiple recursive references to the same CTE are not supported` that `cte.slt:892`
asserts. `assign_work_table` (`physical-plan/src/recursive_query.rs:390`) only walks the recursive
term, so the subquery-embedded reference escapes the guard and the second reader dies in
`WorkTable::take` (`physical-plan/src/work_table.rs:77`). I have not touched it here — one thing at a
time. Happy to send that fix as well if you want it.
