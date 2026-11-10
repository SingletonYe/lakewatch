# Where a subquery may appear: a 5 x 10 matrix against main

Measured 2026-11-09 on main (`3d7262d87`, via `datafusion-cli`) with DuckDB as the oracle.
Method: five subquery constructs crossed with ten positions, 50 queries, comparing values and errors.

## The map

| construct \ position | SELECT | WHERE | HAVING | GROUP BY | agg arg | agg FILTER | ORDER BY | win ORDER | win PART | JOIN ON |
|---|---|---|---|---|---|---|---|---|---|---|
| `2 > ALL (subquery)` | ok | ok | ok | **fail** | **fail** | **fail** | **fail¹** | **fail** | **fail** | ok |
| `EXISTS (subquery)` | ok | ok | ok | **fail** | **fail** | **fail** | **fail¹** | **fail** | **fail** | ok (uncorrelated) |
| `1 IN (subquery)` | ok | ok | ok | **fail** | **fail** | **fail** | **fail¹** | **fail** | **fail** | ok |
| `(scalar subquery) = 1` | ok | ok | ok | ok | ok | ok | ok | ok | ok | ok |
| `EXISTS` **correlated** | ok | ok | ok | **fail** | **fail** | **fail** | **fail¹** | **fail** | **fail** | **fail²** |

ok = agrees with DuckDB · fail = main errors where DuckDB answers · ¹ different message, see below · ² unreported, see below

Totals: **24 agree, 25 fail, 1 both error, 0 value differences.**

## The failures split into three groups

**1. The physical planner does not support it — `#26704`, known and open.**
`GROUP BY` key, aggregate argument, aggregate `FILTER`, window `ORDER BY`, window `PARTITION BY`:
`This feature is not implemented: Physical plan does not support logical expression Exists(…)` (or
`InSubquery`). apache/datafusion#26704 covers the first three by name; the two window positions fail the
same way and are recorded with it rather than filed separately. Already in
`docs/upstream-datafusion.md`.

**2. `ORDER BY` is rejected by the Analyzer — a clear, deliberate restriction.**
`Invalid (non-executable) plan after Analyzer … In/Exist/SetComparison subquery can only be used in
Projection, Filter, TableScan, Window functions, Aggregate and Join plan nodes, but was used in [Sort: …]`.
The message names the allowed positions, so this reads as a documented scope decision, not a defect.
Worth knowing: `ORDER BY <subquery expression>` is valid SQL and DuckDB accepts it.

**3. A correlated subquery in a `JOIN ON` fails with a schema error — unreported.**
This is the one worth attention. The same predicate works in `WHERE` and fails in `ON`:

```sql
-- works, returns 1
SELECT count(*) FROM (VALUES (1),(2)) t(v)
 WHERE EXISTS (SELECT 1 FROM (VALUES (1)) s(w) WHERE s.w = t.v);

-- fails: Schema error: No field named t.v.  Valid fields are __correlated_sq_1.column1.
SELECT count(*) FROM (VALUES (1),(2)) t(v) JOIN (VALUES (1),(2)) u(k)
    ON EXISTS (SELECT 1 FROM (VALUES (1)) s(w) WHERE s.w = t.v);
```

DuckDB answers `1`. The **uncorrelated** form of the same `ON` works, so it is correlation in a join
condition specifically. It fails for a subquery that references the left side (`No field named t.v`) and
for one that references the right side (`No field named u.k`), and a conjunct does not help.

**The tell is an inconsistency**: the *scalar* equivalent is refused with a clear, honest message —

```
Error during planning: Correlated scalar subquery can only be used in Projection, Filter,
Aggregate and MERGE DML plan nodes
```

— while the `EXISTS`/`IN` equivalent falls over with `No field named t.v`, which reads like the user
misspelled a column. Either the position should work (the Analyzer's own list includes join nodes, and
`#24972` decorrelated the projection/where cases), or it should be refused with the same clear message.
Searched upstream on 2026-11-09, open and closed, for correlated subqueries in join conditions and for
this error text: **no matching issue**.

## What this buys

The catalogue can only pin *results*; it cannot express "this position must fail" or "this position must
work". A position matrix like this one is the only way to see the shape of the gaps — which is why
`#24972` was believed to cover "set comparisons" when it actually covered "set comparisons **in
projections**". Re-run it against main whenever the pin looks stale.

## Second round (2026-11-10): four more constructs, same positions

Re-ran with `NOT EXISTS`, `NOT IN (subquery)`, `= ANY (subquery)` and a correlated `IN` added, 45 more
combinations, again against main with DuckDB as the oracle.

**The gaps are a property of the position, not of the construct.** Every one of the nine constructs
fails in exactly the same six places — `GROUP BY` key, aggregate argument, aggregate `FILTER`, window
`ORDER BY`, window `PARTITION BY` (all `#26704`) and `ORDER BY` (the Analyzer restriction) — and works in
`SELECT`, `WHERE`, `HAVING` and `JOIN … ON`.

Two refinements:

* **`JOIN … ON` fails for correlated subqueries of every construct**, and the error message is not even
  stable: `EXISTS` gives `Schema error: No field named t.v`, a correlated `IN` gives
  `Schema error: Ambiguous reference to unqualified field column1`. Two different field-resolution
  failures for the same underlying position strengthens the case that this path is broken rather than
  deliberately restricted (see `correlated-subquery-in-join-condition-issue.md`).
* **`QUALIFY` with a subquery predicate** is refused with *"QUALIFY clause requires window functions in
  the SELECT list or QUALIFY"* — misleading, because the SELECT list does contain one. The canonical
  idiom `QUALIFY rn = 1` works, and so does repeating the window function inside `QUALIFY`, so this is
  specific to a non-window predicate there. Low value on its own (QUALIFY is not standard SQL), recorded
  only so it is not re-investigated.
