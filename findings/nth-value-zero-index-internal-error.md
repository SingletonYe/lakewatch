# Finding: `nth_value(x, 0)` is an internal error in the aggregate form and a silent NULL in the window form

**Status:** confirmed, fixable in one small change — **not filed** (PR #26329 is open, one
upstream PR at a time); issue text ready in `findings/nth-value-zero-index-issue.md`
**DataFusion:** crates.io `datafusion 55.2.0` (the pinned runtime); sources read at
`apache/datafusion@upstream/main`, where both sites below are still present
**Found by:** the new `--function-type AGGREGATE` sweep (`scripts/df_robust.py`), which had
never been run — the scalar sweep cannot reach `nth_value` at all
**Severity class:** `INTERNAL` (an internal error telling the user to file a bug report for
their own input) — the top class after `PANIC`
**Upstream search:** no issue or PR covers `n == 0`. Searched
`"Nth value indices are 1 based"`, `nth_value "index 0"`, `nth_value 0 internal error`, plus
`gh search prs nth_value`; the near misses are #24595 (zero *arguments*, merged), #12320 (NULL
input, closed) and #13201 (the UDWF conversion, closed).

## Symptom — the same expression, two different answers

Aggregate form (no `OVER`), a *planning/execution* failure with the "file a bug report" text:

```console
$ lakewatch sql "SELECT nth_value(v_int, 0) FROM probe_values"
Internal error: Assertion failed: n != 0: Nth value indices are 1 based. 0 is invalid index.
This issue was likely caused by a bug in DataFusion's code. Please help us to resolve this by filing a bug report in our issue tracker: https://github.com/apache/datafusion/issues
```

Window form (any `OVER`, including the empty one), no error at all — every row is NULL:

```console
$ lakewatch sql "SELECT id, nth_value(v_int, 0) OVER (ORDER BY id) AS nv FROM probe_values"
+----+----+
| id | nv |
+----+----+
| 1  |    |
...
| 12 |    |
+----+----+
```

`n = 1` and `n = -1` both work and are documented; only `0` is undefined, and the module says
so itself: the assertion message is *"Nth value indices are 1 based. 0 is invalid index"*.
So the aggregate path knows `0` is invalid and reports it in the worst possible way (as an
internal error asking for a bug report), while the window path silently answers NULL.

## Attribution

| path | site | what it does with `n == 0` |
|---|---|---|
| aggregate | `datafusion/functions-aggregate/src/nth_value.rs:219` (`TrivialNthValueAccumulator::try_new`) and `:350` (`NthValueAccumulator::try_new`) | `assert_or_internal_err!(n != 0, …)` → internal error |
| window | `datafusion/functions-window/src/nth_value.rs:433-441` (`NthValueEvaluator::memoize`) | `self.n.cmp(&0)` has an explicit `Ordering::Equal => (false, false)` arm, i.e. "never a result", and `evaluate` then returns a NULL scalar — no error anywhere |

The window path is a separate `WindowUDFImpl` (`nth_value_udwf`), which is why the two forms
diverge; nothing validates `n` before either implementation runs. Both docs (`nth_value`
aggregate at `nth_value.rs:90`, window at `:217`) describe 1-based indices with negatives
counting from the last row and say nothing about `0`.

## Why this is its own finding

* It is **not** the `OneOf` signature family of #26329 / #20501: the arguments match the
  signature (`(Any, Int64)`); the *value* of the second argument is what is invalid, and the
  failure happens when the accumulator is built, not during coercion.
* It is **not** a duplicate of anything upstream (search above), and #25996 (map coercion) does
  not touch it.
* It is the single new root cause in an area the catalogue had never swept. Worth stating
  plainly: 184 of the 185 `INTERNAL` findings in the aggregate sweep *are* the #26329 root
  cause (`avg(DATE …)`, `sum(DATE …)`, `median(DATE …)`, `approx_percentile_cont(…)` all say
  "failed to match any signature"), so this assertion is the one genuinely new thing there.

## Proposed fix (small, but two crates — decide the scope before pushing)

Reject `n == 0` where `n` is read, with a user-facing error instead of an assert, and make the
window path agree:

1. `functions-aggregate/src/nth_value.rs`, both `try_new`s: `assert_or_internal_err!` →
   `plan_err!`/`exec_err!`, keeping the existing message text (it is already the right words).
2. `functions-window/src/nth_value.rs`: validate `n != 0` where the evaluator is built (or in
   the `Ordering::Equal` arm, return an error rather than "never a result").

If a reviewer prefers one crate per PR, part 1 is the user-visible bug (internal error +
bug-report text) and part 2 is the silent wrong answer; the issue text asks which they want.
Before pushing: grep the whole repo for "Nth value indices are 1 based" and for the message
that replaces it, because `datafusion/sqllogictest/**` hard-codes error strings; and run both
forms as `.slt` cases (`math.slt` or `window.slt` style) — the aggregate form is already
covered by `errors.slt`-style expectations for other functions.

## Not done, deliberately

No branch and no issue was created: the procedure allows one upstream PR at a time and #26329
is still open. The ready-to-post issue text is the hand-off.
