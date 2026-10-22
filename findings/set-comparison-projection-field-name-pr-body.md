# Ready-to-post PR body: keep a projection's field names when decorrelating a predicate subquery

Branch: `fix/set-comparison-projection-field-name` on `SingletonYe/datafusion` (based on
`apache/datafusion@main` `5f21582`).

Re-checked on 2026-10-22 against `main@2717e25`: the projection-name code this patch changes is
byte-identical there, `rewrite_set_comparison.rs` is untouched (so the bug is still live), and the
commit cherry-picks onto today's main with **no conflicts**. The only change upstream made to this
rule since then (#25391) rewrites the mark-join construction, which is orthogonal to this hunk.
A full end-to-end suite run on today's main was not possible in the build sandbox (disk), so the
first complete run against current main will be this PR's CI. Against `5f21582` the patch was
verified directly: all six query shapes return the right answer, `subquery.slt`,
`null_aware_mark_join.slt` and `prepare.slt` are green, and `cargo test -p datafusion-optimizer`
passes (941 + 29 + 5 tests).

We can push the branch but **cannot open the PR** (`apache` organisation OAuth App restrictions),
so the remaining action is the compare link, and this text is pasted into the PR description:

```text
https://github.com/apache/datafusion/compare/main...SingletonYe:fix/set-comparison-projection-field-name?expand=1
```

If the user also posts `findings/set-comparison-projection-internal-error.md` as an issue, put its
number in the first line below; otherwise delete that line.

---

### Which issue does this PR close?

Fixes #<issue-number> (the internal error reported there; the reproduction is repeated below).

### Rationale for this change

A set comparison in a projection that has no alias fails with an internal error:

```console
> SELECT 2 > ALL (SELECT x FROM (VALUES (1),(0)) t(x));
Internal error: Assertion failed: compatible: Failed due to a difference in schemas:
original schema: DFSchema { … [Field { name: "Int64(2) > ALL", data_type: Boolean, nullable: true }] … },
new schema: DFSchema { … [Field { name: "CASE WHEN EXISTS THEN Boolean(false) WHEN EXISTS THEN Boolean(NULL) ELSE Boolean(true) END", … }] … }
This issue was likely caused by a bug in DataFusion's code. …
```

`DecorrelatePredicateSubquery` rebuilds a projection from its expressions, and it tried to
preserve the output field names by comparing against `Expr::schema_name()` of the expression it
was handed. That is not the name the rebuilt projection uses, in two ways:

1. `Expr::schema_name()` keeps a column's qualifier (`t.k`), while the rebuilt field is named after
   the bare column (`k`) and keeps the qualifier separately.
2. Earlier rules rewrite expressions with `LogicalPlan::map_expressions`, which deliberately keeps
   the schema but can leave an expression whose display name no longer matches its field name.
   `RewriteSetComparison` does exactly that — it replaces `x > ALL (…)` with a `CASE WHEN EXISTS …`
   — so this rule sees an expression named `CASE WHEN EXISTS …` for a field named
   `Int64(2) > ALL`, rebuilds the projection under that stale name, and the optimizer's
   "the schema is invariant under optimization" check (`assert_valid_optimization` in
   `optimizer.rs`) fails. `datafusion.optimizer.skip_failed_rules` defaults to `false`, so the user
   gets the internal error rather than a skipped rule.

Regression note: the release line does not have this, because it does not contain
apache/datafusion#24972, which introduced the projection rewrite; on 55.2.0 the same query still
reports `Physical plan does not support logical expression Exists(…)` instead.

### What changes are included in this PR?

* `datafusion/optimizer/src/decorrelate_predicate_subquery.rs`: take the field names from the
  projection's own schema (the authority for its output) instead of from the expression, and
  compare the name the rebuilt projection *would* use — the bare column name for a column, the
  alias name for an alias, the display name otherwise — so an alias is only added when the rebuilt
  name would really differ. Without the second half, a qualified column (`t.k`) in the same
  projection would be aliased and lose its qualifier, which breaks the same check from the other
  direction (that is what the `gs_*`/`oj_*` cases in `subquery.slt` cover).

### Are these changes tested?

Yes. Three cases are appended to `datafusion/sqllogictest/test_files/subquery.slt`: the unaliased
set comparison (failed before this change), the same with a second projected column, and the
aliased form (worked before, kept as a guard).

Also run locally, all green: `test_files/subquery.slt` (which contains the qualified-column and
mark-join cases that a naive "take the name from the schema" fix breaks),
`test_files/null_aware_mark_join.slt`, `test_files/prepare.slt`,
`cargo test -p datafusion-optimizer` (941 + 29 + 5 tests, 0 failures) and
`cargo fmt --all --check`.

### Are there any user-facing changes?

No.
