# DataFusion findings

Append-only log of what each hunting run found and how it was triaged. Written by the
daily maintenance workflow; the procedure and quality bar are in
[`docs/upstream-datafusion.md`](../docs/upstream-datafusion.md).

Statuses: `confirmed_bug` · `known_upstream` · `by_design` · `not_reproducible` ·
`harness_noise`.

## 2026-10-14 — harness brought online (baseline)

Probes: **61** · Oracle: DuckDB 1.5.6 · DataFusion 55.2.0 · Result: **52 OK, 7
known-difference, 2 both-error, 0 candidates**

| candidate | status | evidence | resolution |
|---|---|---|---|
| `9_223_372_036_854_775_807 + 1` returns `-9_223_372_036_854_775_808` | `by_design` | `information_schema.df_settings`: `datafusion.execution.enable_ansi_mode=false`; reproduced with and without constant folding | DataFusion defaults to Spark-compatible wrapping. Upstream tracks the ANSI-mode side in apache/datafusion#24950. Added as `known_difference`; TODO to add an ANSI-mode probe path. |
| `CAST(2.5 AS INT)` → `2`, `CAST(1.999 AS INT)` → `1` | `known_upstream` | truncates toward zero where the standard, PostgreSQL and DuckDB round | apache/datafusion#11886 "Float to int cast behaviour". Recorded as `known_difference`. |
| `ORDER BY x` puts `-0.0` before `0.0`, while `-0.0 = 0.0`, `MIN`, `MAX` and `COUNT(DISTINCT)` treat them as equal | `by_design` | `SELECT x FROM (VALUES (0.0),(-0.0),(1.0)) t(x) ORDER BY x, 1` → `-0.0, 0.0, 1.0`; `COUNT(DISTINCT x)` → `1` | apache/datafusion#24432 chose the float *total* ordering for sorting and MIN/MAX, so this is the intended asymmetry. Pinned with two `expect_rows` probes so a regression is visible. |
| `ROUND(1.005, 2)`: DataFusion `1.0`, DuckDB `1.01` | `harness_noise` | literal typing — DuckDB parses the literal as DECIMAL, DataFusion as DOUBLE (1.005 is really 1.00499…) | Documented, not filed. |
| `UPPER('ß')`: DataFusion `SS`, DuckDB `ẞ` | `harness_noise` | Unicode full folding vs simple case mapping | Both defensible. Documented, not filed. |
| `DATE '2024-02-29' + INTERVAL '1' YEAR` yields DATE, DuckDB yields TIMESTAMP | `harness_noise` | type divergence, same value | Low value. Documented, not filed. |

### Bugs this harness found in lakewatch itself

The point of an oracle is that it also audits the tool.

| bug | impact | fix |
|---|---|---|
| `render.rs` divided by zero rendering a nanosecond timestamp in JSON output | **every** `CAST(... AS TIMESTAMP)` query panicked under `-f json`; DataFusion's default precision is nanoseconds | round-tripped through `i128` nanoseconds; covered by `renders_every_timestamp_precision` and `renders_timestamps_beyond_the_i64_nanosecond_range` |
| `serde_json` sorted object keys, so CSV columns did not match the header | every `--format csv` run | enabled `preserve_order`; covered by `csv_columns_follow_the_schema_order` |

## 2026-10-14 — Tier-A robustness sweep (baseline)

`scripts/df_robust.py`: **364 scalar functions · 1874 invocations · 0 panics · 391
findings across 20 distinct root causes**

The clustering is the result, not the raw count:

| cluster | functions | repro |
|---|---|---|
| `Internal error: Function 'FN' failed to match any signature …` (75) | `date_trunc` | `SELECT date_trunc(DATE '2024-01-01', DATE '2024-01-01')` |
| same, alias (75) | `datetrunc` | `SELECT datetrunc(…)` |
| same (50) | `generate_series`, `range` | `SELECT generate_series(DATE '2024-01-01')` |
| same (36) | `round`, `trunc` | `SELECT round(DATE '2024-01-01')` |
| same (30) | `regexp_like`, `string_to_array` | `SELECT regexp_like(DATE '2024-01-01', …)` |
| … 15 more clusters of the same family | `btrim`, `ltrim`, `rtrim`, `date_part`, `substr`, `element_at`, `log`, `trim`, … | |
| `Unsupported data type Date32 for function FN` (11) | `to_time`, `to_timestamp*` | `SELECT to_time(DATE '2024-01-01')` |
| `Function FN(UserDefined)` (10) | `gcd`, `lcm` | `SELECT gcd('', '')` |

**Verdict: no novel bug today.** All 20 root causes are the same known family —
apache/datafusion#20501 (type-signature errors leak internals, open) and #19004 (coercion
error messages, open). apache/datafusion#21032 attempted exactly this fix and was **closed on
2026-06-24 without being merged**; the behaviour still reproduces on `main`. That PR is now
the workflow's number-one Tier-A target, with `df_robust.py` as the acceptance test.

Note the contrast that justified building this tool: the semantic harness produced **0**
candidates from 61 probes, while the robustness sweep produced **391 findings** in about two
minutes — but clustering collapses them to 20 root causes, all known. Volume is not value;
root-cause clustering is.

## 2026-10-14 17:00 — slot 3 (first scheduled run)

Probes: **364 functions / 1874 invocations** (Tier-A) and **61 probes** (semantic) ·
Oracle: DuckDB 1.5.6 · DataFusion 55.2.0

| sweep | result |
|---|---|
| Tier-A `df_robust.py` | 391 findings, **20 root causes**, 0 panics — identical to the baseline, nothing new |
| Semantic `df_diff.py` | 52 OK, 7 known-difference, 2 both-error, **0 candidates** — identical to the baseline |
| Health gate | 7/7 stages PASS |

Nothing new to triage, so the slot went to the instruction's number-one Tier-A target.

### Acted on: the `OneOf` signature-mismatch error path

`datafusion/expr/src/type_coercion/functions.rs` returned `internal_err!` when every
`OneOf` branch rejected the arguments. That is a user mistake, but the message told the
user *"This issue was likely caused by a bug in DataFusion's code. Please help us to
resolve this by filing a bug report"*. Attribution was measured, not guessed: **353 of the
391 Tier-A findings (13 of 20 clusters) come from this single call site.**

Fix: one line, `internal_err!` → `plan_err!`, plus a unit test that mirrors `substr`'s real
signature and **fails on `main`**, plus the ten `sqllogictest` expectations that asserted the
old user-visible text (`array/array_range.slt`, `spark/math/round.slt`, `errors.slt`,
`math.slt`).

Deliberately out of scope — the reason apache/datafusion#21032 died was scope: the message
is still verbose and still lacks separators (#19004), and the arity phrasing suggested by a
maintainer is a separate improvement.

* Branch pushed: `SingletonYe/datafusion:fix/oneof-signature-mismatch-plan-error` (1fd13a7)
* Verified: `cargo test -p datafusion-expr --lib` → 282 passed; the new test fails when the
  fix is reverted; `cargo fmt --all --check` clean; audited every other assertion on this
  message.

### PR opened: apache/datafusion#26329

The user opened it manually from the compare URL (2026-10-14T21:13Z):

```text
https://github.com/apache/datafusion/pull/26329
```

Auto-labelled `logical-expr`, `sqllogictest` by the repo's Labeler. Every other workflow sits
at `action_required` — GitHub holds CI for a first-time contributor until a maintainer
approves the run. **That is not a failure and nothing on our side can unblock it.**

Capability boundary measured at the same time: we can read the PR and its logs, and we can
push to the branch on the fork, but we cannot comment, edit the PR body, or open issues on
apache/* (org OAuth restrictions plus a token without `read:org`). So fixes are ours and the
words on the thread are the user's — every push must be handed over with a suggested reply.

### Fast response: what is wired up, and what is not

Goal: notice CI failures and review comments in minutes, not up to six hours — a slow reply
is the exact mechanism that killed apache/datafusion#21032. Measured on 2026-10-14:

| channel | status |
|---|---|
| Gmail trigger, `from` contains `github.com` **and** subject contains `26329` | **enabled** |
| `github-workflow-run-completed` for apache/datafusion | **impossible** — silent no-op. It needs the Okou GitHub App installed by the repo owner, and apache will not install a third-party app. |
| 4 scheduled slots (09/13/17/21) | enabled, so the worst case is a 4-hour delay |

Caveat worth remembering: this mailbox holds ~200 GitHub mails, **all of them account and
security notices from `noreply@github.com`** — no PR or CI notification has ever arrived, and
Gmail has nothing matching `subject:datafusion`. So the trigger is unproven until the first
review comment lands. If nothing fires when a comment appears, the fix is on the GitHub side
(Settings → Emails must list this address; Settings → Notifications → "Participating and
@mentions" must stay on), not in the workflow.

Note also that GitHub does not email CI outcomes by default, so a red CI run is caught by the
next scheduled slot rather than by this trigger.

### Blocker: apache/* writes are blocked for this OAuth app

`gh pr create --repo apache/datafusion` fails with:

> the `apache` organization has enabled OAuth App access restrictions, meaning that data
> access to third-parties is limited

So the CLI cannot open the PR, and it cannot open an issue on an apache/* repository either.
The fork and the branch are public and ready; the PR needs one click in a browser:

```text
https://github.com/apache/datafusion/compare/main...SingletonYe:fix/oneof-signature-mismatch-plan-error?expand=1
```

The workflow instruction has been updated so every later slot stops at "push the branch and
hand over the compare URL" instead of retrying the blocked API call.

### Next

* Add a `--session-option key=value` path
* Add a `--session-option key=value` path so probes can run under
  `enable_ansi_mode=true` and assert the documented overflow error.
* Add a generated-data fuzz mode (random tables, random queries) checked for
  panics and for 1-vs-N partition agreement — the technique that found
  apache/datafusion#24432.
* Extend the catalogue to operators lakewatch does not use yet: `GROUPING SETS`,
  decimal and interval arithmetic, nested types, `MERGE`.
