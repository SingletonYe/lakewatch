# Reply for the CHANGES_REQUESTED review on apache/datafusion#26657

`jayzhan211` reviewed on 2026-10-31T03:46:19Z and asked for the fix to move out of
`DecorrelatePredicateSubquery` and into `RewriteSetComparison`, using `NamePreserver` the way
`simplify_exprs` and `type_coercion` wrap `map_expressions`. He supplied a tested diff and an optional
extra sqllogictest case. Both are done in `ed26e85`.

We cannot post this (`apache/*` refuses our writes), so the user pastes it.

---

Thanks — moved it to the rule that leaves the name.

`rewrite_set_comparison` now wraps `map_expressions` in a `NamePreserver`, the same way `simplify_exprs`
and `type_coercion` do, so the `CASE WHEN EXISTS …` never carries a stale name out of the rule. I removed
the `decorrelate_predicate_subquery.rs` change entirely: that file is back to `main`'s version and is no
longer part of the diff. Two files now instead of three.

Added your qualified-column case as a fourth `subquery.slt` test.

Verified on the branch: `subquery.slt` (all four cases), `subquery_projection.slt`, `subquery_sort.slt`,
`merge_into.slt` and `null_aware_mark_join.slt` pass, and `cargo test -p datafusion-optimizer` is
951 + 29 + 5 passed, 0 failed. That is `ed26e85`.
