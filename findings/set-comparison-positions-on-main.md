# Which subquery positions work on main, and which still do not

Investigated 2026-11-08. The harness pins the released `55.2.0`, so it cannot tell "fixed on main" from
"still broken" — every failure looks the same. This run built `datafusion-cli` on **main** and ran the
query shapes there instead, because a bug in main is one nobody has fixed yet.

Method: `cargo build -p datafusion-cli` on `main` (`57.0.0-dev`, **19m13s**), then 33 shapes run through
the CLI and through DuckDB, comparing values.

## Fixed on main: set comparisons in projections

Five shapes that fail on `55.2.0` now agree with DuckDB, confirming **#24972 + #26657** in main:

```sql
SELECT 2 > ALL (SELECT v FROM (VALUES (1),(0)) s(v));                    -- true
SELECT k, sum(v) > ALL (SELECT v FROM (VALUES (1),(2)) s(v)) FROM …;     -- per-group
SELECT * FROM (SELECT 2 > ALL (…) AS r) x;                               -- nested
SELECT 2 > ALL (SELECT v FROM (VALUES (1),(NULL)) s(v));                 -- NULLs
```

## Still broken on main: a subquery *inside an aggregate*

Three positions fail at physical planning with
`This feature is not implemented: Physical plan does not support logical expression Exists(…)`, where
DuckDB answers all three:

```sql
SELECT sum(CASE WHEN v > ALL (SELECT w FROM (VALUES (0)) s(w)) THEN 1 ELSE 0 END) FROM (VALUES (1),(2)) t(v);   -- aggregate argument
SELECT v > ALL (…) AS flag, count(*) FROM (VALUES (1),(2)) t(v) GROUP BY v > ALL (…);                            -- GROUP BY key
SELECT row_number() OVER (ORDER BY v > ALL (…)) FROM (VALUES (1),(2)) t(v);                                      -- window ORDER BY
```

This is **apache/datafusion#26704**, open and labelled `bug` since 2026-10-24: *"Subqueries in an
aggregate (GROUP BY key, aggregate argument or FILTER) fail with 'Physical plan does not support logical
expression'"*. So it is `known_upstream` — **no new issue**, and the three positions are now recorded in
`docs/upstream-datafusion.md` so no future run re-reports them.

The window-`ORDER BY` position is not named in #26704's title; it fails the same way, so it is recorded
as part of the same defect rather than filed separately.

`GROUP BY ()` also still fails on main, which is expected: the fix is the branch prepared on 2026-11-05
and is not merged yet.

## The structural lesson

A `known_difference` that cites an issue can be **stale in one position and live in another**. Here the
same issue number (#24972) explained failures in *both* projections and aggregates; the projections half
was fixed months ago, the aggregate half is #26704. Testing only the pin would have kept both filed under
the wrong note forever. **When a pinned-version failure cites an upstream issue, check the issue against
main in each position before trusting the grouping.**

## Tooling facts worth keeping

* **`datafusion-cli` prints query errors to stdout and exits 1**; `lakewatch` prints them to stderr and
  exits 2. A comparison script that parses stdout as CSV must check the exit code *first*, or it will
  read an error message as an empty result set. (Mine did exactly that for 4 of 33 shapes before the
  exit-code check flagged them.)
* Building `datafusion-cli` on this sandbox takes **~19 minutes**, matching the skill's "20+ minutes, not
  worth it" note. It is worth it only for a systematic hunt against main, never for a single query.
* main is now **`57.0.0-dev`** while crates.io still publishes **55.2.0**, so the "fixed in 56.0.0" notes
  in the catalogue are two releases behind main and will keep drifting until a release lands.
