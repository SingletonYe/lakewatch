# Comment for apache/datafusion#26657 — answer the change request and close the loop

We cannot post this (`apache/*` refuses our writes — see `reports/datafusion-findings.md`), so it is
handed to the user to paste. Head at the time of writing: `12a42d9`, all checks green including
`Rust (fork)`.

One comment covers both reviews: `namanjain24-sudo` asked for a change on 2026-10-24 and never got an
answer, and `raghavyadav01` said LGTM the same evening. Nothing else is outstanding, so this is
**not** a nudge — it is the reply we owe. If nobody picks it up in another two or three days, the
next run may add one short reminder, and not before.

**This text is order-independent: post it as it is, with or without the description tidy-up.** The
sentence about the description is phrased as in-progress on purpose, so it is true either way.

Why it matters more than politeness: the contributor guide's PR lifecycle step 3 says *"Please respond
to all feedback on the PR ... you should acknowledge the feedback. PRs waiting for the feedback for
more than a few days will be marked as draft."* The code change went in on 2026-10-24, but the thread
has never been acknowledged, so the PR currently looks unanswered.

---

Thanks both.

@namanjain24-sudo — sorry for the silence, I made the change but never said so on the thread. Dropped
`projected_field_name` in favour of `Expr::qualified_name()`, exactly as you suggested; the helper was
a second copy of a rule that already existed. Also took the `.zip()` suggestion, so the projection's
expressions and its schema's field names are paired structurally instead of positionally.

On the `as aliased` case: you are right that it is a control rather than a regression test — it passes
on `main` too. I extended the comment in `subquery.slt` to say that, and what it pins (the alias arm),
so it does not read as evidence for the fix.

Re-verified after the change rather than reusing your numbers: `subquery.slt`, `subquery_projection.slt`,
`null_aware_anti_join.slt`, `null_aware_mark_join.slt` and `prepare.slt` all pass with 0 errors,
`cargo test -p datafusion-optimizer` is 951 + 29 + 5 passed and 0 failed, and `cargo fmt --all --check`
is clean. That is `12a42d9`.

@raghavyadav01 — thanks for the LGTM, and for calling out that the comment is the part worth keeping.
That is the bit I did not want to lose, so it stayed when the helper went.

You are both right about the description scaffolding — thanks for flagging it; I am replacing it with the
template body.

The branch merges cleanly with `main` and the fork run is green. Is there anything else you would like
to see before this can go in?
