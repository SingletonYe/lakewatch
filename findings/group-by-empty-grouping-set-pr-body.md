# PR body for `fix/group-by-empty-grouping-set` (not yet opened as an apache/datafusion PR)

**Everything between the BEGIN and END markers is the PR description, ready to paste as-is. Nothing
outside them is part of the body** — that mistake is what put authoring notes into the description of
#26657.

**One click left:**
https://github.com/apache/datafusion/compare/main...SingletonYe:fix/group-by-empty-grouping-set?expand=1

Branch head `7a9ee5f`, one commit on top of today's `main`, two files, +32/−1. Fork CI is already
queued. Searched upstream on 2026-11-05 for an existing issue about the empty grouping set / the
"Empty tuple not supported" error and found none, so the `Closes` line says so rather than carrying a
placeholder.

<!-- BEGIN PASTABLE BODY -->

## Which issue does this PR close?

No issue was filed; this body carries the report.

## Rationale for this change

`GROUP BY ()` is the empty grouping set: it groups on nothing, so the aggregate covers the whole table.
It is the standard way to write a grand total, and it fails here:

```sql
SELECT count(*) FROM t GROUP BY ();
```

```text
Error during planning: This feature is not implemented: Empty tuple not supported yet
```

The empty tuple reaches the expression planner, which has no meaning for it. DuckDB answers `2` for a
two-row table.

## What changes are included in this PR?

* `datafusion/sql/src/select.rs`: drop empty tuples from the grouping list before planning them, and
  remember that the clause contained one. The query is then still treated as an aggregation, so the
  collapse actually happens: `GROUP BY ()` groups on nothing, and `GROUP BY (), k` becomes
  `GROUP BY k`, which is what cross-producting `()` with `(k)` gives.
* `datafusion/sqllogictest/test_files/group_by.slt`: three cases — the grand total, an empty set
  beside a real one, and a grand total over an empty input.

## What is the testing strategy for this PR?

`group_by.slt` gains the three cases above; each one errors on `main` with the message quoted in the
rationale. `group_by.slt`, `grouping.slt`, `aggregate.slt` and `group.slt` pass on the branch,
`cargo test -p datafusion-sql` is 99 + 607 + 12 passed with 1 ignored, and `cargo fmt --all --check` is
clean.

Only queries that currently fail are affected: before this change every one of them stopped at
planning, so no existing result can move.

## Are there any user-facing changes?

Yes, and only in the permissive direction: `GROUP BY ()` and `GROUP BY (), <expr>` now plan and return
results instead of failing with "Empty tuple not supported yet".

## Not covered on purpose

`SELECT 1 FROM t GROUP BY ()` — an empty grouping set with no aggregate function — still fails, now
with DataFusion's existing message *"Aggregate requires at least one grouping or aggregate
expression"*. It needs a plan node for an aggregation with neither grouping nor aggregate expressions,
which DataFusion does not have; the alternative was to silently return one row per input row, which is
the wrong answer. Say if you would rather have that case keep the old "Empty tuple" error, or be
planned as a one-row `VALUES`.

<!-- END PASTABLE BODY -->
