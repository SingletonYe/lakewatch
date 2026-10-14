# lakewatch

**SQL analytics and data-quality checks for a local data lake — powered by [Apache DataFusion](https://github.com/apache/datafusion).**

`lakewatch` points at a directory, treats every CSV/Parquet file below it as a table, and gives you
a query engine plus two opinionated workflows on top of it: **column profiling** and **declarative
data-quality gates** that fail a CI build.

```console
$ lakewatch --root data tables --counts
+-----------+--------+--------+-------+-------+------+
| table     | format | path   | files | bytes | rows |
+-----------+--------+--------+-------+-------+------+
| customers | csv    | .      | 1     | 3987  | 61   |
| orders    | csv    | orders | 2     | 19342 | 240  |
| products  | csv    | .      | 1     | 1689  | 30   |
+-----------+--------+--------+-------+-------+------+
```

No database, no server, no ingest step. `orders` is two files; `customers` is one; a directory of
daily Parquet files is one table too. Schema inference, predicate/projection pushdown, parallel scans,
joins, aggregates and window functions all come from DataFusion.

---

## Why this exists

Most data-quality tools assume a warehouse you have to deploy first. Most "just query my CSV" scripts
fall over the moment you need a join. `lakewatch` sits in the gap: **a single binary that turns a
folder into a queryable, testable dataset**, and that you can drop into any repository to keep its
committed sample data honest.

It is built to exercise DataFusion properly rather than to hide it:

| DataFusion capability | Where lakewatch uses it |
|---|---|
| `ListingTable` + `ListingOptions` | `catalog.rs` registers every file group as a table, one schema over many objects |
| `SessionContext`, `SessionConfig` | `engine.rs` builds the session, sets `target_partitions`, enables `information_schema` |
| Scalar UDFs (`create_udf`) | `is_email`, `mask_email`, `normalize_text` |
| Aggregate UDFs (`AggregateUDFImpl` + `Accumulator`) | `mode`, `null_ratio` — including `state()` / `merge_batch()` for the partial→final protocol |
| SQL planning and execution | every command: CTEs, joins, `GROUP BY`, window functions |
| Aggregate pushdown of one wide `SELECT` | `profile` profiles *N* columns in a *single* scan instead of *N* scans |
| `write_parquet` | `sql --format parquet` writes a real Parquet dataset you can read back |
| `information_schema` | queryable like any other table |
| `DataFrame::explain` | `sql --explain` prints the logical and physical plan |

---

## Install

```bash
git clone https://github.com/SingletonYe/lakewatch.git
cd lakewatch
cargo build --release
./target/release/lakewatch --help
```

Requires a recent stable Rust toolchain (tested on 1.99) and nothing else.

---

## Quickstart

```bash
# 1. What is in the lake?
lakewatch --root data tables --counts

# 2. Ad-hoc SQL: joins, aggregates, window functions
lakewatch --root data sql "
  SELECT p.category,
         COUNT(DISTINCT o.customer_id) AS buyers,
         ROUND(SUM(o.amount), 2)       AS revenue
  FROM orders o
  JOIN products p ON p.product_id = o.product_id
  WHERE o.status = 'paid'
  GROUP BY p.category
  ORDER BY revenue DESC" --timing

# 3. What does the data actually look like?
lakewatch --root data profile orders --quantiles

# 4. Is it safe to use? (exits 1 when a rule fails)
lakewatch --root data check --rules rules/quality.json; echo "exit=$?"
```

```console
+--------------+--------+----------+
| category     | buyers | revenue  |
+--------------+--------+----------+
| hardware     | 17     | 10354.26 |
| training     | 18     | 8013.21  |
| subscription | 14     | 4140.91  |
| services     | 12     | 3492.4   |
| software     | 1      | 270.8    |
+--------------+--------+----------+
elapsed: 0.021s
```

---

## Commands

| Command | Purpose |
|---|---|
| `tables` | list discovered datasets (optionally with row counts) |
| `sql` | run SQL; `--format table\|json\|ndjson\|csv\|parquet`, `--out`, `--limit`, `--explain`, `--timing`, `--var`, `--file`, `--stdin` |
| `profile` | column statistics for a table (`<TABLE>`) or a query (`--query <SQL>`) |
| `check` | evaluate a data-quality rules file; exit 1 on failure |
| `functions` | list the UDFs/UDAFs lakewatch adds to SQL |

Global: `--root <DIR>` (or `-C`) selects the lake; it defaults to the current directory.

Exit codes: `0` success · `1` a data-quality rule failed · `2` usage or runtime error.

---

## How a directory becomes tables

```text
data/
├── customers.csv                  -> table customers   (1 object)
├── products.csv                   -> table products    (1 object)
└── orders/
    ├── orders_2024_01.csv         -> table orders      (2 objects, one schema)
    └── orders_2024_02.csv
```

* A **sub-directory** is one table: every file in it with the same extension joins the same
  `ListingTable`. Partitioned exports just work.
* A file **directly in the root** is its own table, named after the file stem.
* A directory holding both `.csv` and `.parquet` produces one table per format
  (`orders` and `orders_parquet`).
* Names are lowercased and sanitised to valid unquoted SQL identifiers; hidden files and symlinked
  directories are skipped.

---

## Profiling

`profile` builds **one** wide aggregate query, runs it, and transposes the single result row into one
line per column — so a 200-column table costs one scan, not 200.

```console
$ lakewatch --root data profile orders --quantiles
profiled 9 column(s) of orders (240 row(s))
+----------------+---------+-------+------------+----------+--------------------------+------------------------+------------+---------+------------+------------+--------------------------+
| column         | type    | nulls | null_ratio | distinct | min                      | max                    | mean       | p25     | p50        | p75        | top                      |
+----------------+---------+-------+------------+----------+--------------------------+------------------------+------------+---------+------------+------------+--------------------------+
| order_id       | utf8    | 0     | 0.0        | 239      | O10001                   | O10240                 |            |         |            |            | O10129                   |
| customer_id    | utf8    | 2     | 0.008333   | 60       | C0001                    | C0060                  |            |         |            |            | C0005                    |
| product_id     | utf8    | 0     | 0.0        | 30       | P0001                    | P0030                  |            |         |            |            | P0002                    |
| quantity       | int64   | 0     | 0.0        | 5        | 1                        | 5                      | 2.8375     | 1.25    | 3.0        | 4.0        | 1                        |
| amount         | float64 | 0     | 0.0        | 194      | -149.0                   | 4946.8                 | 462.277125 | 94.2525 | 202.829292 | 472.369167 | 421.04                   |
| status         | utf8    | 0     | 0.0        | 5        | COMPLETED                | refunded               |            |         |            |            | paid                     |
| created_at     | date32  | 0     | 0.0        | 59       | 2024-01-01               | 2024-02-29             |            |         |            |            | 2024-02-10               |
| channel        | utf8    | 0     | 0.0        | 4        | android                  | web                    |            |         |            |            | ios                      |
| customer_email | utf8    | 3     | 0.0125     | 60       | ana.haddad37@example.com | xiu.weber9@example.com |            |         |            |            | chen.moreau5@example.com |
+----------------+---------+-------+------------+----------+--------------------------+------------------------+------------+---------+------------+------------+--------------------------+
```

Even in that short table the defects are visible: `order_id` has 239 distinct values over 240 rows,
`amount` goes negative, `status` contains an outlier (`COMPLETED`), `customer_email` has NULLs.

`--query` profiles any SELECT, so you can profile a transform before materialising it:

```bash
lakewatch --root data profile --query "
  SELECT o.*, p.category FROM orders o JOIN products p USING (product_id)" --format json
```

---

## Data-quality gates

A rules file is JSON. Each rule compiles to a DataFusion query that returns **violating rows**;
"zero violating rows" is the pass condition.

```json
{
  "version": 1,
  "name": "ecommerce-core",
  "rules": [
    { "name": "orders.order_id_is_unique", "table": "orders", "type": "unique",
      "columns": ["order_id"], "severity": "error" },

    { "name": "orders.amount_is_positive", "table": "orders", "type": "expression",
      "predicate": "amount > 0" },

    { "name": "orders.status_is_known", "table": "orders", "type": "accepted_values",
      "column": "status", "values": ["paid", "pending", "refunded", "cancelled"] },

    { "name": "orders.customer_email_is_valid", "table": "orders", "type": "expression",
      "predicate": "is_email(customer_email)", "severity": "warn" },

    { "name": "orders.row_count_is_plausible", "table": "orders", "type": "row_count",
      "min": 200 }
  ]
}
```

| `type` | Fields | Passes when |
|---|---|---|
| `unique` | `columns` | the column combination has no duplicates |
| `not_null` | `columns` | none of the columns is NULL |
| `accepted_values` | `column`, `values` | every non-NULL value is in the list |
| `expression` | `predicate` | the SQL predicate is TRUE for every row (NULL counts as a violation) |
| `row_count` | `min`, `max` | the table's row count is inside the bounds |
| `query` | `sql`, `expect` | the query's first cell equals `expect`, compared as text |

`severity: "error"` (the default) fails the run; `severity: "warn"` is reported but does not.

```console
$ lakewatch --root data check --rules rules/quality.json; echo "exit=$?"
+--------+----------+---------------------------------+-----------------+------------+----+-------------------------------+
| status | severity | rule                            | type            | violations | ms | message                       |
+--------+----------+---------------------------------+-----------------+------------+----+-------------------------------+
| FAIL   | error    | orders.order_id_is_unique       | unique          | 1          | 34 | 1 violating row(s)            |
| FAIL   | error    | orders.customer_id_is_not_null  | not_null        | 2          | 23 | 2 violating row(s)            |
| FAIL   | error    | orders.amount_is_positive       | expression      | 4          | 26 | 4 violating row(s)            |
| FAIL   | error    | orders.status_is_known          | accepted_values | 1          | 29 | 1 violating row(s)            |
| pass   | warn     | orders.channel_is_known         | accepted_values | 0          | 14 | no violating rows             |
| FAIL   | warn     | orders.customer_email_is_valid  | expression      | 11         | 31 | 11 violating row(s)           |
| pass   | error    | orders.row_count_is_plausible   | row_count       | 240        | 9  | 240 rows within min=200 max=- |
| FAIL   | warn     | customers.email_is_well_formed  | expression      | 2          | 20 | 2 violating row(s)            |
| pass   | error    | products.unit_price_is_positive | expression      | 0          | 9  | no violating rows             |
+--------+----------+---------------------------------+-----------------+------------+----+-------------------------------+
9 rule(s): 3 passed, 6 failed (2 of them warnings), 0 could not be evaluated
  [FAIL] orders.order_id_is_unique — 1 violating row(s)
  [FAIL] orders.customer_id_is_not_null — 2 violating row(s)
  [FAIL] orders.amount_is_positive — 4 violating row(s)
  [FAIL] orders.status_is_known — 1 violating row(s)
  [FAIL] orders.customer_email_is_valid — 11 violating row(s)
  [FAIL] customers.email_is_well_formed — 2 violating row(s)
exit=1
```

`--format json` switches to a machine-readable report that includes a **sample of the offending rows**:

```json
{
  "suite": "ecommerce-core",
  "total": 9, "passed": 3, "failed": 6, "errored": 0, "warnings": 2,
  "ok": false,
  "outcomes": [
    {
      "rule": "orders.order_id_is_unique",
      "rule_type": "unique", "severity": "error", "status": "fail",
      "violations": 1, "message": "1 violating row(s)", "duration_ms": 37,
      "sample": [{ "_occurrences": 2, "order_id": "O10129" }]
    }
  ]
}
```

Add `--verbose` and each failed rule also carries the SQL that produced it.

---

## The functions lakewatch adds to SQL

```console
$ lakewatch functions
Functions provided by lakewatch
------------------------------------------------------------------------------------------------
is_email         scalar    (utf8) -> boolean  True when the text is a structurally valid email address
mask_email       scalar    (utf8) -> utf8     Keep the first character and the domain: `a***@example.com`
normalize_text   scalar    (utf8) -> utf8     Trim, lowercase and collapse internal whitespace
mode             aggregate (any) -> utf8      Most frequent non-null value in the group, rendered as text
null_ratio       aggregate (any) -> float64   Fraction of NULL values in the group
```

They are ordinary SQL from any query or rule:

```console
$ lakewatch --root data sql "
  SELECT mask_email('ana.silva@example.com') AS masked,
         is_email('nope@@example.com')       AS bad,
         normalize_text('  Ana   SILVA ')    AS normalized,
         mode(channel)                       AS top_channel,
         null_ratio(customer_email)          AS email_null_ratio
  FROM orders"
+-----------------------+-------+------------+-------------+------------------+
| masked                | bad   | normalized | top_channel | email_null_ratio |
+-----------------------+-------+------------+-------------+------------------+
| a********@example.com | false | ana silva  | ios         | 0.0125           |
+-----------------------+-------+------------+-------------+------------------+
```

`mode` is a full `AggregateUDFImpl`: it keeps a per-group histogram and implements `state()` /
`merge_batch()`, so it is correct under the partial→final aggregation protocol too — not just in
single-partition plans. Tie-breaks are resolved on the smallest rendered value, so results are
deterministic across runs and across partition layouts.

---

## Output formats

`table` (default, pretty ASCII) · `json` · `ndjson` · `csv` · `parquet`.

```bash
# Pipe into the rest of your tooling
lakewatch --root data sql "SELECT * FROM orders WHERE status='paid'" -f ndjson | jq -r .order_id

# Materialise a curated Parquet dataset and query it back
lakewatch --root data sql "SELECT * FROM orders WHERE status='paid'" -f parquet -o /tmp/paid
lakewatch --root /tmp sql "SELECT COUNT(*) AS paid_orders FROM paid_orders"
```

Parquet output is a DataFusion sink, so the target is a **directory** with one file per partition.

---

## Repository layout

```text
src/
├── catalog.rs      file discovery -> ListingTable registration, name sanitising
├── engine.rs       SessionContext wiring (partitions, information_schema, functions)
├── functions/
│   ├── scalar.rs   is_email, mask_email, normalize_text
│   └── aggregate.rs mode, null_ratio (AggregateUDFImpl + Accumulator)
├── profile.rs      one-scan column profiling
├── check.rs        rules model, SQL compilation, reporting
├── render.rs       RecordBatch -> table/json/ndjson/csv
├── sqlutil.rs      identifier and literal quoting
└── main.rs         clap CLI
data/               sample lake (synthetic, deliberately dirty)
rules/quality.json  the gate that runs in CI
scripts/gen_sample_data.py  regenerates data/ deterministically
tests/cli.rs        end-to-end tests driving the real binary
```

## Development

```bash
cargo test           # 7 unit + 9 end-to-end tests
cargo clippy --all-targets -- -D warnings
cargo fmt --check
cargo run -- --root data check --rules rules/quality.json
```

CI (`.github/workflows/ci.yml`) runs fmt, clippy, tests and a live `check` against the sample lake.

The sample data is generated, not real, and is *supposed* to be dirty: the rules in
`rules/quality.json` assert on the exact defects the generator plants. Regenerate it with

```bash
python3 scripts/gen_sample_data.py
```

## License

Apache-2.0 — the same license as Apache DataFusion, which this project is built on.
