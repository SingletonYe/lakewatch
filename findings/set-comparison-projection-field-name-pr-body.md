# Ready-to-open PR: keep a projection's field names when decorrelating a predicate subquery

Branch `fix/set-comparison-projection-field-name` (commit `0973cfa`, 2 files) on `SingletonYe/datafusion`.
Open at:
`https://github.com/apache/datafusion/compare/main...SingletonYe:fix/set-comparison-projection-field-name?expand=1`

Post the issue first (text in `set-comparison-projection-internal-error.md`) and put its number in the
first line below, or leave the line as it is and let the body carry the report.
**Paste from the horizontal rule down.**

---

## Which issue does this PR close?

Closes #<issue-number> — if no issue was filed, this body contains the report.

## Rationale for this change

A set comparison in a projection that has no alias fails with an internal error:

```console
> SELECT 2 > ALL (SELECT x FROM (VALUES (1),(0)) t(x));
Internal error: Assertion failed: compatible: Failed due to a difference in schemas:
original schema: … Field { name: "Int64(2) > ALL", data_type: Boolean } …
new schema:      … Field { name: "CASE WHEN EXISTS THEN Boolean(false) …" } …
```

`DecorrelatePredicateSubquery` rebuilds a projection from its expressions, and it preserved the output
field names from `Expr::schema_name()`. That is not the name the rebuilt projection uses, for two
reasons: `schema_name()` keeps a column's qualifier (`t.k`) while the field is named after the bare
column, and earlier rules rewrite expressions with `map_expressions`, which keeps the schema but can
leave an expression whose display name no longer matches its field name. `RewriteSetComparison` does
exactly that — it replaces `x > ALL (…)` with a `CASE WHEN EXISTS …` — so this rule rebuilt the
projection under the stale name and the optimizer's schema-invariance check failed.
`datafusion.optimizer.skip_failed_rules` defaults to false, so the user sees the internal error.

Not on the release line: 55.2.0 does not contain #24972, so the same query still reports
`Physical plan does not support logical expression Exists(…)` there.

## What changes are included in this PR?

* `datafusion/optimizer/src/decorrelate_predicate_subquery.rs`: take the field names from the
  projection's own schema, and compare them against the name the rebuilt projection *would* use (the
  bare column name for a column, the alias name for an alias, the display name otherwise), so an
  alias is added only when it would really differ. Comparing against the schema name alone would
  alias a qualified column and lose its qualifier — the same check failing from the other side.
* `datafusion/sqllogictest/test_files/subquery.slt`: three cases — unaliased, unaliased with a second
  projected column, and aliased.

## Are these changes tested?

Yes. Cherry-picked onto `main@2717e25` and rebuilt: all six query shapes return the right answer, and
`subquery.slt` (by then also containing #25391's new cases), `null_aware_mark_join.slt` and
`prepare.slt` pass with 0 errors. On the base `5f21582` the same checks passed, plus
`cargo test -p datafusion-optimizer` (941 + 29 + 5 tests) and `cargo fmt --all --check`; the optimizer
unit tests were not re-run on `2717e25`.

## Are there any user-facing changes?

No.
