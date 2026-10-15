# Ready-to-post issue text: `nth_value(x, 0)` — internal error in the aggregate form, silent NULL in the window form

We cannot open issues on `apache/*` (the organisation blocks OAuth apps for third-party
writes), so this is handed to the user to post at
<https://github.com/apache/datafusion/issues/new>. Paste from the horizontal rule down; the
title line goes in the title box.

Title:

```text
nth_value(x, 0): internal error in the aggregate form, silent NULL in the window form
```

---

### Describe the bug

`nth_value` rejects `0` as an index in one execution form and silently returns NULL in the
other. The index is documented as 1-based (negative counts back from the last row), so `0` is
invalid either way; the aggregate path even says so, in the worst possible way.

Aggregate form (no `OVER`):

```sql
SELECT nth_value(1, 0);
-- Internal error: Assertion failed: n != 0: Nth value indices are 1 based. 0 is invalid index.
-- This issue was likely caused by a bug in DataFusion's code. Please help us to resolve this by filing a bug report in our issue tracker: ...
```

So an ordinary wrong index asks the user to file a bug report against DataFusion.

Window form (any `OVER`, including `OVER ()`):

```sql
SELECT id, nth_value(v_int, 0) OVER (ORDER BY id) AS nv FROM t;
-- nv is NULL for every row, no error
```

Same function, same arguments, two different outcomes — and the window one is a silently
wrong-looking answer rather than an error.

### To reproduce

```sql
CREATE TABLE t(id INT, v_int INT) AS VALUES (1, 1), (2, 2), (3, 3);

-- aggregate form: internal error
SELECT nth_value(v_int, 0) FROM t;

-- window form: all NULL, no error
SELECT id, nth_value(v_int, 0) OVER (ORDER BY id) AS nv FROM t ORDER BY id;
```

`n = 1` and `n = -1` both work as documented, so only `0` is affected.

### Expected behaviour

A user-facing error in both forms — e.g. "nth_value index must not be 0; indices start at 1
and negative values count from the end" — since `0` is not a valid index. Failing that, at
minimum the error should not be an internal error telling the user to file a bug report, and
the two forms should agree.

### Where it comes from

* Aggregate: `assert_or_internal_err!(n != 0, "Nth value indices are 1 based. 0 is invalid
  index")` in `TrivialNthValueAccumulator::try_new`
  (`datafusion/functions-aggregate/src/nth_value.rs:219`) and `NthValueAccumulator::try_new`
  (`:350`).
* Window: `NthValueEvaluator::memoize` matches `self.n.cmp(&0)` and has an explicit
  `Ordering::Equal => (false, false)` arm
  (`datafusion/functions-window/src/nth_value.rs:433-441`), so a zero index is simply "never a
  result" and the row comes out NULL. No validation happens before either implementation.

### Additional context

Tested on `datafusion` 55.2.0 (crates.io) and the code is unchanged on `main`. Searched the
tracker for `n == 0` / "Nth value indices are 1 based" and found nothing; #24595 was about
zero *arguments*, #12320 about NULL input. Happy to send a PR for whichever scope you prefer:
both sites together, or the aggregate assertion first (that is the user-visible one).
