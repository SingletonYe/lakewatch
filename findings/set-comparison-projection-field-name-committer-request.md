# Committer request for apache/datafusion#26657

## Why the sidebar route did not work

On 2026-10-28 the PR's `requested_reviewers` became `["raghavyadav01", "namanjain24-sudo"]` — the two
people who had **already** reviewed it, and neither has write access. The likely cause: GitHub's
reviewer picker only offers people who have already participated on the PR, so a committer who has
never touched it cannot be selected from that list. `jayzhan211` was never offered.

So the sidebar is a dead end here. The contributor guide's own instruction for reaching a committer is
to **mention them in the PR** ("Please mention the members from committers list in the PR"), and that
is the only channel left that we can hand over.

## Who to mention

`jayzhan211` — measured on 2026-10-28 over merged PRs from the previous week:

| area | merges |
|---|---|
| `datafusion/sqllogictest/` | 16 (next is alamb with 6) |
| `datafusion/optimizer/` | 3 (tied for first) |
| any area, last 3 days | 8 (still active) |

Mention **one** person. Do not mention several — review bandwidth is the project's stated bottleneck
and a broadcast reads as pressure.

## Comment to post

Short version:

---

@jayzhan211 would you take a look? Review points are in `12a42d9`, the branch is green, and the
description is tidied.

---

If a little more context is wanted:

---

@jayzhan211 would you have a moment to take a look? The review points from @namanjain24-sudo and
@raghavyadav01 are in `12a42d9`, the branch is green and merges cleanly with `main`, and the
description scaffolding has been replaced.

---

Both are requests for a review, not for a merge — `raghavyadav01` has already approved, so what is
missing is an approval from someone with write access, and a committer will know that from the PR page.
