# lakewatch dropped a column when two output columns shared a name

Found 2026-11-04 by the new query-shape sweep. **A bug in this harness, not in DataFusion** — and the
second one of its kind, after the `serde_json` key-order bug of 2026-10-14.

## Symptoms

A valid query that returns two columns with the same name:

```sql
WITH c AS (SELECT k, sum(v) AS s FROM (VALUES (1,1),(1,2),(2,3)) t(k,v) GROUP BY k)
SELECT a.k, a.s, b.s FROM c a JOIN c b ON a.k = b.k ORDER BY a.k;
```

Before the fix, lakewatch printed a header with **three** names and rows with **two** cells:

```
k,s,s
1,3
2,3
```

and `--format json` printed `{"k":1,"s":3}` — one `s`. A shorter repro needs no CTE:

```sql
SELECT a.s, b.s FROM (SELECT 1 AS s) a, (SELECT 2 AS s) b;
-- lakewatch: header "s,s", row "2"       (one cell for two names)
```

## How it was attributed

The first reading was that DataFusion had lost a column — a silent wrong answer, which is the highest
value thing this workflow can find. **It is not.** The `sqllogictest` oracle settled it: a scratch
`.slt` file in the local `df-fork` clone asks DataFusion directly, bypassing lakewatch's writer
entirely, and DataFusion returns the correct answer:

```
SELECT a.s, b.s FROM (SELECT 1 AS s) a, (SELECT 2 AS s) b          -- actual: 1 2
SELECT a.k, a.s, b.s FROM (VALUES (1,3)) a(k,s) JOIN (VALUES (1,4)) b(k,s) ON a.k = b.k
                                                                   -- actual: 1 3 4
```

So the plan and the values were right all along; only the rendering lost a cell.

## The defect

`render.rs` built each row as a `Map<String, Value>` keyed by **column name**:

```rust
for (col, field) in batch.schema().fields().iter().enumerate() {
    object.insert(field.name().clone(), value);   // the second `s` overwrites the first
}
```

The CSV header, by contrast, came straight from `schema().fields()`, so it kept every name. Header and
rows were therefore counting differently, and a consumer that trusts the header (every CSV consumer
does) sees a row that is one field short.

## The fix

Rows are now positional, and names are made unique only where a name-keyed format demands it:

* `cells_of(batch)` returns each row as a `Vec<Value>` **in column order**. The CSV writer takes the
  header from `unique_names` and the cells from here, so the two are the same width by construction.
* `rows_of(batch)` keeps a JSON object — JSON cannot hold two identical keys — but renames the second
  and later occurrence of a name to `s_2`, `s_3`, … through `unique_names`, which keeps trying until
  the name is actually free. No value is dropped, and the CSV header gains the same suffix
  (`k,s,s_2`), so header and data agree and both values are addressable.

Two unit tests pin it: `keeps_columns_that_share_a_name` (both cells and both JSON keys survive) and
`csv_header_matches_row_width_with_duplicate_names` (header width equals row width).

## Impact on the catalogue: none, but only by luck

Every one of the 119 probes was checked, and **none of them returns a duplicate column name** — probes
alias their projections. So no past comparison was corrupted, and re-running `df_diff.py` after the fix
reproduces the same counts (`OK=99, KNOWN_DIFF=18, BOTH_ERROR=2, 0 candidates`) with no change to
`reports/df-diff.json`.

It was one `SELECT a.s, b.s` away from mattering, and it silently produced a malformed result for any
user of the CLI, not just for the harness.
