### Describe the bug

A **correlated** subquery in a `JOIN … ON` condition fails, while the identical predicate in `WHERE`
works and the **uncorrelated** form in `ON` also works. It fails with a field-resolution error that reads
like the user misspelled a column, rather than with the clear "not supported here" message that the
scalar equivalent produces.

### To Reproduce

```sql
-- 1. the predicate works in WHERE  ->  1
SELECT count(*) FROM (VALUES (1),(2)) t(v)
 WHERE EXISTS (SELECT 1 FROM (VALUES (1)) s(w) WHERE s.w = t.v);

-- 2. the same predicate in ON fails
SELECT count(*) FROM (VALUES (1),(2)) t(v) JOIN (VALUES (1),(2)) u(k)
    ON EXISTS (SELECT 1 FROM (VALUES (1)) s(w) WHERE s.w = t.v);
-- Schema error: No field named t.v.
-- Valid fields are __correlated_sq_1.column1.

-- 3. the uncorrelated form in ON works  ->  2
SELECT count(*) FROM (VALUES (1),(2)) t(v) JOIN (VALUES (1)) u(k)
    ON EXISTS (SELECT 1 FROM (VALUES (1)) s(w));

-- 4. referencing the right side fails the same way
SELECT count(*) FROM (VALUES (1),(2)) t(v) JOIN (VALUES (1)) u(k)
    ON EXISTS (SELECT 1 FROM (VALUES (1)) s(w) WHERE s.w = u.k);
-- Schema error: No field named u.k.

-- 5. a conjunct does not help
SELECT count(*) FROM (VALUES (1),(2)) t(v) JOIN (VALUES (1)) u(k)
    ON t.v = u.k AND EXISTS (SELECT 1 FROM (VALUES (1)) s(w) WHERE s.w = t.v);
-- Schema error: No field named t.v.
```

DuckDB answers `1`, `1`, `2`, `2` and `1` respectively for the five queries.

### Expected behavior

Either the correlated form should work in a join condition, or it should be refused with the same kind
of message the scalar case already produces:

```sql
SELECT count(*) FROM (VALUES (1),(2)) t(v) JOIN (VALUES (1)) u(k)
    ON u.k = (SELECT max(w) FROM (VALUES (1)) s(w) WHERE s.w = t.v);
-- Error during planning: Correlated scalar subquery can only be used in Projection, Filter,
-- Aggregate and MERGE DML plan nodes
```

That message is honest and actionable. `No field named t.v … Valid fields are
__correlated_sq_1.column1` is not: it points at an internal subquery alias and suggests the user's
column reference is wrong, when the same reference is accepted one clause earlier.

### Additional context

* The Analyzer's own allowed-position list names join nodes — `ORDER BY` is refused with
  *"In/Exist/SetComparison subquery can only be used in Projection, Filter, TableScan, Window functions,
  Aggregate and Join plan nodes, but was used in [Sort: …]"* — so `JOIN … ON` is meant to be in scope.
* apache/datafusion#24972 covered projections; apache/datafusion#26657 the field names in them;
  apache/datafusion#26704 is the still-open gap for aggregate positions (`GROUP BY` key, aggregate
  argument, aggregate `FILTER`). This report is the join-condition position, which none of the three
  covers.
* Verified on `main` (`3d7262d87`) on 2026-11-09. Searched upstream, open and closed, for correlated
  subqueries in join conditions and for this error text before filing.
