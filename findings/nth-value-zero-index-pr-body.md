# Ready-to-open PR: reject `0` as an index in `nth_value` (both the aggregate and the window form)

Branch: `fix/nth-value-zero-index` on `SingletonYe/datafusion`
Compare URL: see `findings/recursive-cte-subquery-internal-error-issue.md`-style handoff in the slot
report; the automation prints it in the email.

Post the issue first (text in `nth-value-zero-index-issue.md`) and put its number in the first line
below, or leave that line as-is if you prefer to send the report inline instead of an issue.

---

## Which issue does this PR close?

Closes #<issue-number> — if no issue has been filed, this body contains the whole report.

## Rationale for this change

`nth_value`'s index is 1-based (negative values count back from the end), so `0` is never valid. The
two ways of calling the same function disagreed about it, and neither behaved acceptably:

* **aggregate form** (`nth_value(x, 0)`) tripped `assert_or_internal_err!`, so an ordinary typo was
  reported as `Internal error: Assertion failed: n != 0: Nth value indices are 1 based. 0 is invalid
  index` and the user was asked to file a bug report against DataFusion;
* **window form** (`nth_value(x, 0) OVER (...)`), which is the form most people write, was not
  validated at all: `NthValueEvaluator` matches `self.n.cmp(&0)` and has an explicit
  `Ordering::Equal` arm, so a zero index simply meant "never a result" and **every row came out NULL**
  with no error at all.

Both are the same kind of defect as `NTILE`, which reports `Execution error: NTILE requires a positive
integer` for the same class of mistake, and that is the behaviour this PR brings `nth_value` to.

### Before / after

`SELECT nth_value(a, 0) FROM t` (aggregate form)

Before:

```text
Internal error: Assertion failed: n != 0: Nth value indices are 1 based. 0 is invalid index
This issue was likely caused by a bug in DataFusion's code. Please help us to resolve this by filing a bug report in our issue tracker: https://github.com/apache/datafusion/issues
```

After:

```text
Execution error: nth_value index must not be 0; indices are 1-based and negative values count from the end
```

`SELECT nth_value(a, 0) OVER (ORDER BY a) FROM t` (window form)

Before: `NULL` for every row, no error.

After: the same `Execution error: nth_value index must not be 0; ...` as above.

`nth_value(x, 1)`, `nth_value(x, -1)`, `nth_value(x, i64::MIN)` (window form) and first/last value are
unchanged; the existing `The second argument of nth_value must not be i64::MIN` guard in the window
path is untouched.

## What changes are included in this PR?

- `datafusion/functions-aggregate/src/nth_value.rs`: reject `n == 0` once in `NthValue::accumulator`,
  where the index is read from the literal, and drop the two `assert_or_internal_err!` guards that
  produced the `Internal error` (they are the only callers of the two `try_new`s).
- `datafusion/functions-window/src/nth_value.rs`: extend the existing `validate_nth_value_n` helper —
  already called from `partition_evaluator` and already guarding `i64::MIN` — with the same `0` check.
- Two `sqllogictest` cases: `window.slt` next to the `NTILE` negative-argument test, and a
  self-contained block at the end of `aggregate.slt`.

The message is identical in both forms on purpose: they are one behaviour, and a single string is what
the tests assert.

## Are these changes tested?

Yes.

- `cargo test -p datafusion-sqllogictest --test sqllogictests -- window` and `-- aggregate` pass with
  the new cases, and the new cases fail on a tree without the fix (they assert the new error text).
- `cargo fmt --all --check` is clean.

## Are there any user-facing changes?

Yes, and they are the point of the PR: `nth_value(x, 0)` now produces a user-facing `Execution error`
in both forms instead of an internal error in one and a silent `NULL` column in the other.

## Notes

* One asymmetry is left alone deliberately: the aggregate path still does `-*value` on the literal
  without the `i64::MIN` guard the window path has. I could not reach it from SQL (`is_reversed` is
  false for every form I tried, and `nth_value(x, i64::MIN)` as an aggregate returns NULL rather than
  panicking), so it is not part of this PR. Happy to add the guard as a follow-up if you want the two
  paths symmetric.
* Not part of this PR, but found by the same harness and reported separately if you want it: a
  recursive CTE that references itself inside a scalar subquery produces
  `Internal error: Unexpected empty work table.` instead of the clean
  `Multiple recursive references to the same CTE are not supported` its sibling shapes produce
  (`datafusion/physical-plan/src/recursive_query.rs:390`).
