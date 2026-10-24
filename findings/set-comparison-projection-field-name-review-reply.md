# Reply text for the review thread on apache/datafusion#26657

We cannot post this (`apache/*` refuses our writes — see `reports/datafusion-findings.md`), so it is
handed to the user to paste. Head at the time of writing: `12a42d9`.

---

Good catch on `Expr::qualified_name()` — you are right, it has exactly those three arms and is
documented for precisely this case, so the helper was a second copy of a rule that already existed.
Dropped it and it now uses `rewritten_expr.qualified_name().1`.

Also took the `.zip()` suggestion: the projection's expressions and its schema's field names are
zipped, so the pairing is structural instead of positional.

On the `as aliased` case: agreed, it is a control rather than a regression test — it passes on `main`
too. I extended the comment in `subquery.slt` to say that, and what it pins (the alias arm), so nobody
reads it as evidence for the fix.

Re-verified after the change rather than reusing your numbers: `subquery.slt`,
`subquery_projection.slt`, `null_aware_anti_join.slt`, `null_aware_mark_join.slt` and `prepare.slt`
all pass with 0 errors, `cargo test -p datafusion-optimizer` is 951 + 29 + 5 passed and 0 failed, and
`cargo fmt --all --check` is clean. Pushed as `12a42d9`; net change is −19/+10 on the source, plus the
comment.

And thanks for flagging the description scaffolding — I have tidied it.
