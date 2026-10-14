# Hunting DataFusion bugs from lakewatch

`lakewatch` exists to put DataFusion under load in ways its own test suite does not: a
catalog over real files, custom UDFs/UDAFs, profiling queries with hundreds of
aggregates, and a rules engine that compiles user JSON into SQL. When something in
that surface misbehaves, it is usually a DataFusion problem rather than a lakewatch
problem — and it is worth reporting upstream.

This document is the procedure and the quality bar for doing that.

## The pipeline

```
probe  ->  triage  ->  minimise  ->  report  ->  grow the catalogue
```

```bash
python3 scripts/df_diff.py                 # oracle + self-consistency + pinned expectations
python3 scripts/df_diff.py --oracle none   # no DuckDB needed
python3 scripts/df_diff.py --only window   # one category
python3 scripts/df_diff.py --json reports/df-diff.json --strict
```

Three independent signals, in decreasing order of certainty:

| signal | meaning |
|---|---|
| `PLAN_DIVERGENCE` | the same query answered differently with 1 and 4 target partitions. No oracle involved: this is a bug, full stop. |
| `EXPECTATION_FAILED` | a probe whose answer is fixed by the SQL standard (three-valued logic, aggregates over empty inputs) disagreed. |
| `MISMATCH` | DataFusion and DuckDB disagreed. Needs triage — many are legitimate dialect differences. |

`BOTH_ERROR` (both engines reject the input) and `KNOWN_DIFF` (a divergence already
recorded in the catalogue) are informational and are not candidates.

## Triage: what is *not* a bug

Most mismatches are not bugs. These have all been seen and resolved already; check
them before reporting anything.

| pattern | verdict | example |
|---|---|---|
| Integer overflow wraps silently | **by design** | `enable_ansi_mode=false` is the default and matches Spark. Tracked in apache/datafusion#24950. |
| Float → integer casts truncate | **known upstream** | apache/datafusion#11886. `CAST(2.5 AS INT)` is 2, not 3. |
| `-0.0` sorts before `0.0` but `-0.0 = 0.0` is true | **by design** | DataFusion deliberately uses the float *total* ordering for sorting and MIN/MAX (apache/datafusion#24432); equality and DISTINCT stay IEEE. |
| Bare `1.005` rounds differently | **literal typing** | DuckDB reads it as DECIMAL, DataFusion as DOUBLE. Cast explicitly in the probe. |
| `UPPER('ß')` differs | **defensible either way** | Unicode full folding (`SS`) versus simple mapping (`ẞ`). |
| Datetime text differs (`T…Z` vs `…`) | **formatting** | The harness normalises this; if it still fires, it is a real type difference. |

Everything else is a candidate until proven otherwise. Also check whether the
behaviour is simply *undocumented but intended*: search the issue tracker, the PRs,
and the test suite for the exact function name before assuming a bug.

## Quality bar for anything sent upstream

An upstream report is only worth sending when all of these hold:

1. **Minimal repro.** Smallest SQL that shows it, with the exact lakewatch command
   and the exact observed output. Two or three lines, not a transcript.
2. **Not a duplicate.** Search open *and* closed issues and PRs. Link what you found
   and explain what is different. If the answer is "nothing is different", do not file.
3. **Verified against `main`,** not only against the version pinned in `Cargo.toml`.
   Behaviour that was already fixed must be recorded as such, not filed.
4. **Traced to a layer.** Say whether it looks like the logical planner, the
   optimizer, a kernel, or an error message, and paste the `--explain` output.
5. **A fix is small, or there is no fix.** For a PR: the change must be minimal,
   have a test, and be verified with `cargo test -p <crate>`. A speculative rewrite
   of an operator is not a daily-task PR. When no safe fix exists in the time
   available, an issue with a good repro is the correct deliverable.
6. **Nothing private.** No local paths, tokens, or customer data in the report.

Guardrails that hold for every run:

* never force-push, never rewrite history, never merge, never delete branches or tags;
* one upstream PR per day at most, and never while a previous daily PR is still open;
* nothing is pushed to `apache/datafusion` other than a reviewable branch on a fork;
* if the repro cannot be reduced to something a maintainer can run in under a minute,
  it is not ready.

## Recording the outcome

Every candidate ends up in `reports/datafusion-findings.md` with one of:
`confirmed_bug`, `known_upstream` (with a link), `by_design` (with the reason),
`not_reproducible`, or `harness_noise` (with the normalisation that fixed it).

`by_design` and `known_upstream` verdicts are then written back into
`probes/semantics.json` as `known_difference` notes, so tomorrow's run starts from a
cleaner baseline instead of re-litigating today's answers. That is the whole loop:
**the catalogue only ever gets sharper.**

## Growing the catalogue

A fixed catalogue stops finding things. Each run should add a small number of probes
that exercise something not yet covered, and the most valuable additions tend to be:

* a **generated-data fuzz** mode: random tables plus random queries, checked for
  self-consistency across partition counts and for panics. Upstream found
  apache/datafusion#24432 exactly this way.
* operators and functions lakewatch does not use yet: `MERGE`, `GROUPING SETS`,
  `json`/`array` functions, decimal and interval arithmetic, nested types, `EXPLAIN`
  plan-shape assertions.
* the seams lakewatch already sits on: UDF/UDAF state merging, catalog and
  `ListingTable` edge cases (empty files, mixed schemas, zero-column selects).
