# `Internal error: Unexpected empty work table.` for a recursive CTE that references itself inside a scalar subquery

Found 2026-10-16 (09:00 slot) by hand while opening the "recursive CTE" direction in the catalogue.
Verdict: **confirmed bug, not a duplicate, INTERNAL class** (a user mistake is reported as
DataFusion's own bug and the user is asked to file a bug report). Not filed upstream yet: we cannot
open issues on `apache/*`, and the one-PR-at-a-time rule keeps this behind #26329. Ready-to-post
issue text is in `recursive-cte-subquery-internal-error-issue.md`.

## Repro (DataFusion 55.2.0, crates.io)

```sql
WITH RECURSIVE t(n) AS (
    SELECT 1
    UNION ALL
    SELECT n + 1 FROM t WHERE n < 3 AND n < (SELECT MAX(n) FROM t)
)
SELECT n FROM t ORDER BY n;
```

```console
$ lakewatch sql "…above…"
lakewatch: Internal error: Unexpected empty work table.
This issue was likely caused by a bug in DataFusion's code. Please help us to resolve this by
filing a bug report in our issue tracker: https://github.com/apache/datafusion/issues
```

Same outcome when the second reference hides in a `CASE`:

```sql
WITH RECURSIVE t(n) AS (
    SELECT 1
    UNION ALL
    SELECT CASE WHEN (SELECT COUNT(*) FROM t) < 4 THEN n + 1 END FROM t WHERE n IS NOT NULL
)
SELECT n FROM t ORDER BY n;
-- Internal error: Unexpected empty work table.
```

## The same mistake *is* reported cleanly in neighbouring shapes

Every other way of referencing the recursive CTE twice gives the intended, user-facing error:

| recursive term | result |
|---|---|
| `FROM t a JOIN t b ON a.n = b.n` | `This feature is not implemented: Multiple recursive references to the same CTE are not supported` |
| `WHERE EXISTS (SELECT 1 FROM t x WHERE x.n = t.n)` | same clean message |
| `WHERE n + 1 IN (SELECT n FROM t …)` | same clean message |
| `WHERE n < (SELECT MAX(n) FROM t)` (**this report**) | **internal error** |
| `CASE WHEN (SELECT COUNT(*) FROM t) …` (**this report**) | **internal error** |

That clean message is the intended behaviour and is locked in by the project's own test file:
`datafusion/sqllogictest/test_files/cte.slt:892`

```text
# expect error when recursive CTE is referenced multiple times in the recursive term
query error DataFusion error: This feature is not implemented: Multiple recursive references to the same CTE are not supported
```

So the defect is not "the shape is unsupported" — it is that one family of shapes escapes the guard
and blows up later as an internal invariant violation.

## Where it goes wrong

`EXPLAIN` shows the second reader plainly, as a sibling of the recursive term:

```text
physical_plan
  ScalarSubqueryExec: subqueries=1
    RecursiveQueryExec: name=t, is_distinct=false
      ProjectionExec: expr=[CAST(1 AS Int64) as n]
        PlaceholderRowExec
      ProjectionExec: expr=[n@0 + 1 as n]
        FilterExec: n@0 < 3 AND n@0 < scalar_subquery(<pending>)
          WorkTableExec: name=t          <-- the recursive term's own read
    AggregateExec: mode=Single, gby=[], aggr=[max(t.n)]
      WorkTableExec: name=t              <-- the second read, inside the subquery
```

* The guard counts work-table readers with a `transform_down` over the **recursive term only**:
  `assign_work_table` in `datafusion/physical-plan/src/recursive_query.rs:390-412` (called at `:98`),
  which raises `not_impl_err!("Multiple recursive references to the same CTE are not supported")`
  when it meets more than one node whose `with_new_state` accepts the work table. The subquery's
  plan is not inside the recursive term's subtree, so the counter stays at 1 and the guard never
  fires — a JOIN, an `EXISTS` or an `IN` puts the second reader *inside* the subtree, which is why
  only those shapes get the clean error.
* The error itself is raised by `WorkTable::take`
  (`datafusion/physical-plan/src/work_table.rs:77-83`), whose `.ok_or_else(|| internal_datafusion_err!("Unexpected empty work table"))`
  treats "no batches parked in the work table" as a broken internal invariant — which it is, but the
  user cannot act on it.

Both files are unchanged on `main` as of 2026-10-16 (fetched through the contents API), so this is
not an artifact of the pinned 55.2.0.

## Fix directions

1. **Preferred — make the existing guard see the whole picture.** Run the reference count over the
   recursive query's full physical plan (recursive term *plus* the subquery plans attached to it,
   i.e. the ones that end up as siblings under `ScalarSubqueryExec`), so this shape raises the same
   `not_impl_err!` as a self-join. One crate (`datafusion-physical-plan`), no behaviour change for
   the shapes that already work.
2. **Fallback — a user-facing error instead of an internal one.** `WorkTable::take` returning a
   planning/execution error ("recursive CTE referenced more than once") would stop the "file a bug
   report" text, but it reports the symptom deep in execution rather than the actual mistake, so it
   is strictly worse than (1) as a first patch.

Either way the *test* belongs next to the existing one in `cte.slt:892`, which already has the
self-join form.

## Not a duplicate

`gh search issues` / `gh search prs` in `apache/datafusion` for `"Unexpected empty work table"`,
`"multiple recursive"`, `"recursive references"`, `"recursive subquery"`, `"Unexpected empty"` and
`"work table"` return no report of this. The nearest open work is adjacent but different:
#25893 (recursive term silently cast to the static term's type), #26054/#26056 (MIN/MAX dynamic
filter not reset between iterations), #19427 (join of two recursive CTEs runs indefinitely),
#26238 (materialised CTE as a first-class plan node), #16935 (recursive CTE docs). None of them
concern a reference inside an expression subquery escaping the multiple-reference guard.

## Other data points from the same exploration (no bug)

* `range(1,10,0)` and `generate_series(1,10,0)` → `Error during planning: Step cannot be zero`, a clean
  error (DuckDB also rejects zero steps). A regression here would not fail cleanly but *loop*, so the
  sweep deliberately does not call it with a zero step.
* Steps near the integer limit are overflow-safe: `generate_series(1, 9223372036854775806,
  9223372036854775805)` returns exactly two rows and `range(0, -10, -9223372036854775808)` returns one,
  with no panic and no wrap. Pinned as the probe `table_function_step_near_i64_max`.
