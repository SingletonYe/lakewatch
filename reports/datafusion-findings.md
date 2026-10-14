# DataFusion findings

Append-only log of what each hunting run found and how it was triaged. Written by the
daily maintenance workflow; the procedure and quality bar are in
[`docs/upstream-datafusion.md`](../docs/upstream-datafusion.md).

Statuses: `confirmed_bug` · `known_upstream` · `by_design` · `not_reproducible` ·
`harness_noise`.

## 2026-10-14 — harness brought online (baseline)

Probes: **61** · Oracle: DuckDB 1.5.6 · DataFusion 55.2.0 · Result: **52 OK, 7
known-difference, 2 both-error, 0 candidates**

| candidate | status | evidence | resolution |
|---|---|---|---|
| `9_223_372_036_854_775_807 + 1` returns `-9_223_372_036_854_775_808` | `by_design` | `information_schema.df_settings`: `datafusion.execution.enable_ansi_mode=false`; reproduced with and without constant folding | DataFusion defaults to Spark-compatible wrapping. Upstream tracks the ANSI-mode side in apache/datafusion#24950. Added as `known_difference`; TODO to add an ANSI-mode probe path. |
| `CAST(2.5 AS INT)` → `2`, `CAST(1.999 AS INT)` → `1` | `known_upstream` | truncates toward zero where the standard, PostgreSQL and DuckDB round | apache/datafusion#11886 "Float to int cast behaviour". Recorded as `known_difference`. |
| `ORDER BY x` puts `-0.0` before `0.0`, while `-0.0 = 0.0`, `MIN`, `MAX` and `COUNT(DISTINCT)` treat them as equal | `by_design` | `SELECT x FROM (VALUES (0.0),(-0.0),(1.0)) t(x) ORDER BY x, 1` → `-0.0, 0.0, 1.0`; `COUNT(DISTINCT x)` → `1` | apache/datafusion#24432 chose the float *total* ordering for sorting and MIN/MAX, so this is the intended asymmetry. Pinned with two `expect_rows` probes so a regression is visible. |
| `ROUND(1.005, 2)`: DataFusion `1.0`, DuckDB `1.01` | `harness_noise` | literal typing — DuckDB parses the literal as DECIMAL, DataFusion as DOUBLE (1.005 is really 1.00499…) | Documented, not filed. |
| `UPPER('ß')`: DataFusion `SS`, DuckDB `ẞ` | `harness_noise` | Unicode full folding vs simple case mapping | Both defensible. Documented, not filed. |
| `DATE '2024-02-29' + INTERVAL '1' YEAR` yields DATE, DuckDB yields TIMESTAMP | `harness_noise` | type divergence, same value | Low value. Documented, not filed. |

### Bugs this harness found in lakewatch itself

The point of an oracle is that it also audits the tool.

| bug | impact | fix |
|---|---|---|
| `render.rs` divided by zero rendering a nanosecond timestamp in JSON output | **every** `CAST(... AS TIMESTAMP)` query panicked under `-f json`; DataFusion's default precision is nanoseconds | round-tripped through `i128` nanoseconds; covered by `renders_every_timestamp_precision` and `renders_timestamps_beyond_the_i64_nanosecond_range` |
| `serde_json` sorted object keys, so CSV columns did not match the header | every `--format csv` run | enabled `preserve_order`; covered by `csv_columns_follow_the_schema_order` |

### Next

* Add a `--session-option key=value` path so probes can run under
  `enable_ansi_mode=true` and assert the documented overflow error.
* Add a generated-data fuzz mode (random tables, random queries) checked for
  panics and for 1-vs-N partition agreement — the technique that found
  apache/datafusion#24432.
* Extend the catalogue to operators lakewatch does not use yet: `GROUPING SETS`,
  decimal and interval arithmetic, nested types, `MERGE`.
