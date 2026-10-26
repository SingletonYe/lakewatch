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

@namanjain24-sudo — sorry, the change went in on 10-24 but I never said so here. `projected_field_name`
is gone in favour of `Expr::qualified_name()`; I also took the `.zip()` and marked the `as aliased` case
as a control in the comment. Re-verified after the change: `subquery.slt` plus the four related files
pass, `cargo test -p datafusion-optimizer` is 951 + 29 + 5 green, `fmt` clean — that is `12a42d9`.

@raghavyadav01 — thanks for the LGTM, and for calling out that the comment is the part worth keeping.

You are both right about the description scaffolding; replacing it with the template body now.

Anything else you would like to see before this can go in?
