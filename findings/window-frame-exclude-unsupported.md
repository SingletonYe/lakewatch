# SQL:2011 window-frame `EXCLUDE` is not supported, and the gap is in sqlparser

Found 2026-11-02 while looking for uncovered window behaviour. **Not filed upstream** — see the
attribution below.

## What happens

```sql
SELECT v, SUM(v) OVER (ORDER BY v
                       ROWS BETWEEN UNBOUNDED PRECEDING AND UNBOUNDED FOLLOWING
                       EXCLUDE CURRENT ROW) AS s
FROM (VALUES (1),(2),(3)) t(v);
```

DataFusion 55.2.0 rejects it **at parse time**:

```
SQL error: ParserError("Expected: ), found: EXCLUDE at Line: 1, Column: 92")
```

DuckDB computes the standard answer `(1,5), (2,4), (3,3)` — the sum of every row except the current
one — and also handles `EXCLUDE GROUP` (`(1,2), (2,4), (3,2)` for a `1 PRECEDING`/`1 FOLLOWING` frame).

## Why this is not a DataFusion issue

The error is raised by **sqlparser**, and sqlparser 0.62.0 has **no model for frame exclusion at all**:
`grep -rn "Exclusion" sqlparser-0.62.0/src/` returns nothing, and `WindowFrame` (in `src/ast/mod.rs`)
has no exclusion field. So the feature cannot be represented, let alone planned — support has to start
in [sqlparser-rs](https://github.com/apache/datafusion-sqlparser-rs).

The `EXCLUDE` keyword entries that *do* exist in sqlparser are unrelated: they implement Snowflake's
`* EXCLUDE (col, …)` wildcard syntax (and BigQuery's `EXCEPT`), not the SQL:2011 frame clause.

## Not made into a probe

The probe schema pins `expect_rows`, i.e. a **result**; there is no way to pin "this must fail to
parse", so an `EXCLUDE` probe would have to be marked as a documented divergence whose entire content
is "unsupported". That is a weaker signal than this note, and it would sit in the catalogue claiming a
divergence the harness cannot check. Recorded here instead, so a future run does not re-derive it.

If support ever lands, the two DuckDB answers above are the expectations to pin.
