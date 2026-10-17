# Ready-to-post issue text: recursive CTE referenced inside a scalar subquery → `Internal error: Unexpected empty work table.`

We cannot open issues on `apache/*` (the organisation blocks OAuth apps for third-party writes), so
this is handed to the user to post at <https://github.com/apache/datafusion/issues/new>. Paste from
the horizontal rule down; the title line goes in the title box.

Title:

```text
Internal error: "Unexpected empty work table" when a recursive CTE references itself inside a scalar subquery
```

---

### Describe the bug

A recursive CTE that references itself inside a scalar subquery in its recursive term fails with an
internal error instead of the clean error DataFusion already produces for every other
"referenced more than once" shape:

```text
Internal error: Unexpected empty work table.
This issue was likely caused by a bug in DataFusion's code. Please help us to resolve this by filing a bug report in our issue tracker: ...
```

The same query written with a self-join, an `EXISTS` or an `IN` gets
`This feature is not implemented: Multiple recursive references to the same CTE are not supported`,
which is the message `cte.slt` asserts. So the restriction is intended; the scalar-subquery spelling
just escapes the check and dies later inside the work table.

### To reproduce

```sql
WITH RECURSIVE t(n) AS (
    SELECT 1
    UNION ALL
    SELECT n + 1 FROM t WHERE n < 3 AND n < (SELECT MAX(n) FROM t)
)
SELECT n FROM t ORDER BY n;
```

```text
Internal error: Unexpected empty work table.
This issue was likely caused by a bug in DataFusion's code. …
```

Same with the second reference inside a `CASE`:

```sql
WITH RECURSIVE t(n) AS (
    SELECT 1
    UNION ALL
    SELECT CASE WHEN (SELECT COUNT(*) FROM t) < 4 THEN n + 1 END FROM t WHERE n IS NOT NULL
)
SELECT n FROM t ORDER BY n;
```

For contrast, these all give the clean `Multiple recursive references …` not-implemented error on the
same version — same mistake, different message:

```sql
WITH RECURSIVE t(n) AS (
    SELECT 1
    UNION ALL
    SELECT a.n + 1 FROM t a JOIN t b ON a.n = b.n WHERE a.n < 3
) SELECT n FROM t;

WITH RECURSIVE t(n) AS (
    SELECT 1
    UNION ALL
    SELECT n + 1 FROM t WHERE n < 3 AND EXISTS (SELECT 1 FROM t x WHERE x.n = t.n)
) SELECT n FROM t;

WITH RECURSIVE t(n) AS (
    SELECT 1
    UNION ALL
    SELECT n + 1 FROM t WHERE n + 1 IN (SELECT n FROM t WHERE n < 3)
) SELECT n FROM t;
```

### Expected behaviour

The same `This feature is not implemented: Multiple recursive references to the same CTE are not supported`
error as the three shapes above (a clean user error is enough — this does not need to become
supported behaviour). Anything except "Internal error … please file a bug report" for a query the
user wrote by accident.

### Where it comes from

`EXPLAIN` shows two work-table readers, the second one inside the subquery's plan, which is a sibling
of the recursive term rather than part of it:

```text
ScalarSubqueryExec: subqueries=1
  RecursiveQueryExec: name=t, is_distinct=false
    ProjectionExec: expr=[CAST(1 AS Int64) as n]
      PlaceholderRowExec
    ProjectionExec: expr=[n@0 + 1 as n]
      FilterExec: n@0 < 3 AND n@0 < scalar_subquery(<pending>)
        WorkTableExec: name=t
  AggregateExec: mode=Single, gby=[], aggr=[max(t.n)]
    WorkTableExec: name=t
```

* `assign_work_table` (`datafusion/physical-plan/src/recursive_query.rs:390`, called from `:98`)
  counts work-table readers with `transform_down` over **only the recursive term**. A reference
  nested in an expression subquery is not in that subtree, so `work_table_refs` stays at 1 and the
  `not_impl_err!("Multiple recursive references to the same CTE are not supported")` is never
  reached. A self-join / `EXISTS` / `IN` keeps the second reader inside the subtree, which is why
  only those shapes are caught.
* Execution then reaches `WorkTable::take`
  (`datafusion/physical-plan/src/work_table.rs:77`), where
  `.ok_or_else(|| internal_datafusion_err!("Unexpected empty work table"))` turns the missing batches
  into an internal error.

Suggested fix: run the reference count over the whole recursive query physical plan (recursive term
plus the subquery plans attached to it), so this shape raises the existing clean error. The test
belongs beside the existing one in `datafusion/sqllogictest/test_files/cte.slt:892`. Happy to send a
PR for that if it is the scope you want.

### Additional context

Tested on `datafusion` 55.2.0 (crates.io); both files above are unchanged on `main` as of 2026-10-16.
DuckDB executes the first query (returning 1, 2, 4), so either behaviour — a clean error or support —
would be defensible; the internal error is the part that is not.
