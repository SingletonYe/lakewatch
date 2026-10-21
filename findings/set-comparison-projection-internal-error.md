# Ready-to-post issue text: `> ALL` subquery in an unaliased projection trips an optimizer invariant

Found 2026-10-20 by the lakewatch differential harness (new probe family around set
comparisons / predicate subqueries), verified on **`main`** and **not** present in the
released 55.2.0 — so it is a regression that came in with apache/datafusion#24972
*"Support predicate subqueries in projections"* (merged 2026-09-15).

We cannot open issues on `apache/*` (`apache` organisation blocks third-party OAuth apps and
this token lacks `read:org`), so this is handed to the user to post at
<https://github.com/apache/datafusion/issues/new>. Paste from the horizontal rule down; the
title line goes in the title box.

**A fix is already prepared** (2026-10-21): branch `fix/set-comparison-projection-field-name` on
the fork is pushed and verified, so the two can go out together — post this issue, then open the
PR from
<https://github.com/apache/datafusion/compare/main...SingletonYe:fix/set-comparison-projection-field-name?expand=1>
with `findings/set-comparison-projection-field-name-pr-body.md` pasted in (put this issue's number
in its first line). Posting only the PR is also fine.

**Upstream search before writing this** (open *and* closed, by message text and by feature):
`decorrelate_predicate_subquery`, `Check optimizer-specific invariants`, `difference in
schemas`, `SetComparison`, `predicate subquery projection`, `> ALL`, `CASE WHEN EXISTS`,
`24972` — no issue or PR matches this failure. The nearest items are #16073 (merge the
decorrelation rules) and #25341 (a perf problem in the same feature), neither of which is
this.

Title:

```text
Internal error: a set-comparison subquery in an unaliased projection fails the optimizer schema invariant
```

---

### Describe the bug

`SELECT <expr> > ALL (SELECT …)` (and the other set comparisons) works when the projection is
given an alias, and fails with an internal error when it is not:

```console
$ # fails
> SELECT 2 > ALL (SELECT x FROM (VALUES (1),(0)) t(x));
Error during planning: Optimizer rule 'decorrelate_predicate_subquery' failed
caused by
Check optimizer-specific invariants after optimizer rule: decorrelate_predicate_subquery
caused by
Internal error: Assertion failed: compatible: Failed due to a difference in schemas:
original schema: DFSchema { inner: Schema { fields: [Field { name: "Int64(2) > ALL", … }] }, … },
new schema: DFSchema { inner: Schema { fields: [Field { name: "CASE WHEN EXISTS THEN Boolean(false) WHEN EXISTS THEN Boolean(NULL) ELSE Boolean(true) END", … }] }, … }.
This issue was likely caused by a bug in DataFusion's code. Please help us to resolve this by filing a bug report in our issue tracker: https://github.com/apache/datafusion/issues

$ # same query with an alias: fine
> SELECT 2 > ALL (SELECT x FROM (VALUES (1),(0)) t(x)) AS d;
true
```

An ordinary query that should simply return a Boolean fails, and the user is told to file a bug
report. Nothing about it is an unusual input — omitting a projection alias is the common case.

### To reproduce

Verified on `main` built from `5f21582` (2026-10-16) with the `sqllogictest` runner; the
relevant sources were re-read on `main@3ee3e39` (2026-10-20) and are unchanged.

```sql
-- fails: unaliased set comparison in a projection
SELECT 2 > ALL (SELECT x FROM (VALUES (1),(0)) t(x));

-- fails too: parenthesised, or alongside other columns
SELECT (2 > ALL (SELECT x FROM (VALUES (1),(0)) t(x)));
SELECT 2 > ALL (SELECT x FROM (VALUES (1),(0)) t(x)), 9 AS k;

-- all of these are fine
SELECT 2 > ALL (SELECT x FROM (VALUES (1),(0)) t(x)) AS d;
SELECT EXISTS (SELECT 1 FROM (VALUES (1)) t(x));
SELECT 1 IN (SELECT x FROM (VALUES (1)) t(x));
SELECT * FROM (VALUES (1)) a(x) WHERE 2 > ALL (SELECT x FROM (VALUES (1),(0)) t(x));
```

So the trigger is narrow and mechanical: a **set-comparison predicate subquery
(`Expr::SetComparison`, i.e. `> ALL` / `< ANY` / …) sitting in a projection that has no
explicit alias**. `EXISTS` and `IN` in the same position are unaffected, and the same
expression in a `WHERE` clause is unaffected.

### Expected behaviour

The query returns `true`, exactly as it does with an alias.

### Where it comes from

The failure is not in the planner proper but in an optimisation invariant check, and it is
reached only because the rule reports success with a plan whose field name changed:

* `datafusion/optimizer/src/optimizer.rs:686` runs `assert_valid_optimization` after *every*
  rule (this is not a `debug_assertions`-only check).
* `datafusion/optimizer/src/optimizer.rs:775` → `assert_expected_schema`
  (`datafusion/expr/src/logical_plan/invariants.rs:114`), which compares
  `logically_equivalent_names_and_types`. Here only the **name** differs
  (`Int64(2) > ALL` versus the rewritten `CASE WHEN EXISTS …`); the type is `Boolean` on both
  sides.
* `OptimizerConfig::skip_failed_rules` defaults to `false`, so the failure is surfaced to the
  user as an internal error instead of being skipped.
* The rule is `DecorrelatePredicateSubquery`
  (`datafusion/optimizer/src/decorrelate_predicate_subquery.rs`). Its projection branch
  (`:71–93`) names the rewritten expression deliberately —
  `if rewritten_expr.schema_name().to_string() != original_name { rewritten_expr = rewritten_expr.alias(original_name) }` —
  which is why the aliased form works. The unaliased `SetComparison` rewrite evidently leaves
  the mark-join `CASE` name in place, so either that path does not go through the preserving
  branch or its `schema_name()` compares equal before the plan is rebuilt.

The same behaviour is what makes the aliased form work, which should make the fix small: keep
the original projection field name in the `SetComparison` path as well (or alias
unconditionally when the name changes *after* the plan is built, then assert).

### Additional context

* Not reproducible on the released **55.2.0** (the pinned engine of the harness): there the
  same unaliased query reports `This feature is not implemented: Physical plan does not support
  logical expression Exists(…)` instead, because 55.2.0 does not contain apache/datafusion#24972.
  So this is `main`-only today, and it lands in the next release unless the name is preserved.
* Introduced by apache/datafusion#24972 *"Support predicate subqueries in projections"*
  (merged 2026-09-15).
* A sqllogictest case is just the two statements above (with and without the alias); the unaliased
  one currently fails, so it would have caught this. The prepared fix adds exactly those cases.
