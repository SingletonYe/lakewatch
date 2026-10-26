# Review request for apache/datafusion#26657

The PR is green, both reviewers are satisfied, and `raghavyadav01` has approved it — but his approval
does not count, because the branch protection requires "at least 1 approving review ... by reviewers
with write access" and he does not have it. He said so himself: *"Needs a committer from here."*

So the remaining step is a committer. Two ways to ask, in order of preference:

1. **Request a review from the PR sidebar** (Reviewers → search the handle). This is the native
   mechanism and puts it in that person's review queue without a public ask. `jayzhan211` is the best
   target: he merged 22 of the recent PRs touching `optimizer/` or `sqllogictest/`, so this is his
   area. `comphead` (who fixed #24432, the float MIN/MAX ordering) and `alamb` are reasonable
   fallbacks — `alamb` is the project lead and the busiest.
2. **If the sidebar does not offer them**, mention one person in a comment. Do not mention several at
   once.

Do the description tidy-up **first**: both reviewers say it is the first thing a committer reads.

Draft comment if a mention is needed (one committer, not a broadcast):

---

@jayzhan211 would you have a moment to take a look? The three review points from @namanjain24-sudo and
@raghavyadav01 are in `12a42d9`, the branch is green and merges cleanly with `main`, and I have replaced
the description scaffolding that was still in the body.
