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

## 2026-10-14 21:00 — slot 4 (last slot: daily digest)

Fresh sandbox: everything was rebuilt from GitHub, which is itself the test.

| check | result |
|---|---|
| health gate, **from a clean clone** | 7/7 PASS — the harness is reproducible from zero, which every later slot depends on |
| Tier-A `df_robust.py` | 391 findings, 20 root causes, 0 panics — baseline reproduced exactly |
| Semantic `df_diff.py` | **63 probes** (2 new), 54 OK, 7 known-difference, 2 both-error, 0 candidates |

**PR #26329 needs nothing from us.** OPEN, MERGEABLE, 0 reviews, 0 comments, every workflow
still `action_required` (the first-time-contributor gate). Did not re-push, as instructed.
The user replaced the auto-generated PR body with the templated one.

### New finding: a second call site, worse than the first

`element_at` / `map_extract` with a non-map argument produce a message that is wrong in three
separate ways at once — an internal error for a user mistake, an *execution* error inside a
*planning* error, and an unhelpful candidate list. Attribution: `utils.rs:341` emits
`internal_err!("Expected a Map type, got ...")` and `type_coercion/functions.rs:541` wraps it
with `exec_err!`. Written up with the exact locations and the fix in
`findings/nested-map-coercion-error.md`, deliberately **not** folded into #26329 (that is how
#21032 died). 16 of the 391 findings come from this path.

Checked and rejected as a bug: `element_at` being map-only. It is documented as an alias of
`map_extract` in this version, so the aliasing is self-consistent; only the error path is
wrong. The catalogue now pins the DuckDB divergence so it is not re-triaged.

### Catalogue growth

Two probes added, both passing: `map_extract_present_and_missing` (oracle-comparable with
DuckDB) and `element_at_is_map_extract_here` (pins the documented alias, recorded as a
`known_difference`). 61 → 63.

### Process notes

* A `pgrep -f "cargo build"` wait loop matched **its own shell command line** and never
  exited, burning 25 minutes of wall clock while the build had finished in 4m29s. Watch for
  self-matching process patterns; read the build log instead of polling.
* This slot initially ran the semantic sweep with `--oracle none` because DuckDB had not been
  installed in the fresh sandbox, which silently downgrades the sweep (7 oracle comparisons
  became plain "OK"). Install the oracle in Phase 0 as written.

### Next

* Add a `--session-option key=value` path
* Add a `--session-option key=value` path
* Add a `--session-option key=value` path so probes can run under
  `enable_ansi_mode=true` and assert the documented overflow error.
* Add a generated-data fuzz mode (random tables, random queries) checked for
  panics and for 1-vs-N partition agreement — the technique that found
  apache/datafusion#24432.
* Extend the catalogue to operators lakewatch does not use yet: `GROUPING SETS`,
  decimal and interval arithmetic, nested types, `MERGE`.

## 2026-10-15 08:55 — PR-response run (review on #26329)

**Trigger:** the user, who forwarded the review notification. Not a probe slot: the scope was
Phase 0b only, and no sweep was run, so the numbers below are deliberately absent rather than
zero.

`gh pr view 26329 --repo apache/datafusion` → `OPEN`, one review from **alamb**
(`CONTRIBUTOR`, `COMMENTED`, on `1fd13a7`): *"this looks great … other than adjusting the
tests, this looks ready to me"*, with two inline comments asking for the added unit test to be
deleted because the `.slt` cases cover the same paths, or moved to `.slt` if we think the
coverage is needed.

**Acted in-slot, as the table in the procedure requires.** Push `268933e` on
`fix/oneof-signature-mismatch-plan-error` deletes the unit test (39 lines, nothing else). The
claim that `.slt` already covers it was checked, not assumed, and it holds: `round`'s `OneOf`
has 1- and 2-argument branches, so `SELECT round('x')` fails one on the argument type and the
other on the arity — the exact shape the unit test built — and `round(1, 2, 3)`, `round(true)`,
`round(timestamp)`, the four `generate_series` cases and `nth_value` cover the rest. Nothing
was moved into `.slt`. Re-verified: `cargo fmt --all --check` clean, `cargo test -p
datafusion-expr --lib` 281 passed / 0 failed, no new warnings.

**What the `.slt` verification actually rests on.** `sqllogictest-0.29.1` matches an expected
error as an **unanchored regex against the full error string** (`parser.rs:465`), and
`verify_function_arguments` (`datafusion/expr/src/expr_schema.rs:790`) renders a `Plan` error
as the bare message while any other variant keeps its `Display` prefix. So on `main` the
actual text is `Error during planning: Internal error: Function 'round' failed to match any
signature, …` and the expectation does not match it; after the change it does. Each of the ten
updated expectations is therefore a real regression guard, and the error-variant change is
observable through it. This is also the answer to the previous slot's open worry ("CI is the
verification for the four `.slt` files") — and it was confirmed end to end rather than left as
an argument. The `sqllogictests` binary builds in **6m14s** here and takes file substrings as
filters, so the four affected files were run directly: **exit 0** on `268933e`. With the
one-line source change reverted and the `.slt` expectations left in place, the run fails on
**exactly the ten updated expectations** (`array_range.slt:247,251,254,257`, `errors.slt:127`,
`math.slt:178,181`, `spark/math/round.slt:596,600,604`), each with the old text
`Error during planning: Internal error: Function 'round' failed to match any signature, …`.
The regression guard is demonstrated, not asserted. Recorded with the exact commands in
`findings/oneof-signature-mismatch-plan-error.md`.

**Harness lesson:** `testing/` is an uninitialised *submodule* in a plain clone, so the first
`errors.slt` run reported two unrelated failures (lines 44 and 159, empty `aggregate_test_100`).
`git submodule update --init --depth 1 testing` (12 MB) fixes it. Future slots that run `.slt`
files must do this first, or they will misread an empty table as a regression.

**Capability boundary, re-confirmed (do not retry):** `gh pr edit 26329 --body-file …` fails
with *"The 'login' field requires one of the following scopes: ['read:org']"* — the same wall
as `gh pr create`. So the PR-body correction that this push makes necessary
(`findings/oneof-signature-mismatch-plan-error-pr-body.md`, which still claims the unit test
exists and says "282 passed") and the reply comment on the review are handed to the user as
pasted-ready text. Neither is posted by us, and neither is claimed to be.

**Next:** when the next slot runs, `268933e` is the head; check whether the user posted the
reply and whether CI needs re-approval for the new commit (first-time-contributor gate). If the
review is satisfied, this PR is one approval away from the merge queue and the next Tier-A
target is the `element_at` / `map_extract` path in `findings/nested-map-coercion-error.md`.

## 2026-10-15 09:00 — slot 1 (first slot of the day)

**结论**：probes 66（+3）· candidates 0 · 新 root cause 0 · 但拦下了一个重复 PR

| 阶段 | 结果 |
|---|---|
| Phase 0 健康闸门 | 7/7 PASS（fresh clone，duckdb 1.5.6 oracle 已装，build 6m33s） |
| Phase 1a Tier-A | 1874 次调用 / 391 findings / **20 个根因**（INTERNAL=369, NOT_IMPL=22）—— 与基线一致，无新增根因 |
| Phase 1b 语义探针 | 66 探针 · OK=56 · KNOWN_DIFF=8 · BOTH_ERROR=2 · **0 个待研判候选** |
| Phase 4 扩目录 | +3 探针，1 处 harness 修正 |
| Phase 6 邮件 | 未发（见下） |

### Phase 0b — PR #26329：无需动作，球在用户那边

`OPEN`，head `268933e`，评审线程现在是 `isOutdated`（因为那行代码已删），**0 条新评论**。
alamb 没有追加要求。但上一轮的交接**还没被执行**：PR 描述里仍写着 "A unit test covering that
failure" 和 "282 passed"，回复评论也还没贴。我们这边无可推送的东西；**不要为了唤醒 CI 再
push**（新 commit 之后首次贡献者门禁要维护者再点一次 Approve，这是正常的）。

### Phase 2 — 今天的实质产出：拦住一个重复的上游 PR

上一轮把 `map_extract` / `element_at` 的内部错误记为"下一个独立 PR 的靶子"。今天按流程
先搜上游，`gh search prs --repo apache/datafusion "map_extract"` 直接命中：

**#25996 "fix: report map_extract on a non-map argument as a plan error"**（jackylee-ch，
2026-10-03 开，**仍 OPEN**）改的正是我们指认的那一行
（`functions-nested/src/utils.rs` 的 `get_map_entry_field`），测试也写好了。

所以那个 PR **不能提**，会撞车。三个缺陷重新归因：

| 缺陷 | 上游 | 状态 |
|---|---|---|
| 1. 非 map 参数报 internal error（+ 让人提 bug report） | #25996 | 在飞，已修 |
| 3. 候选列表列出内部的 `map_extract(UserDefined)` | #15817 | 已开 issue |
| 2. plan 期的 coercion 失败被标成 `Execution error` | — | **已实测、未提** |

缺陷 2 今天用**不依赖 map_extract 的函数**实测证实了（这才是它值得记的理由）：
`array_add(1, 2)` 和 `cosine_distance([1.0], 'x')` 在 main 上就是
`Error during planning: Execution error: Function 'array_add' user-defined coercion failed
with: Error during planning: array_add does not support type Int64. …` —— 内部本来就是 Plan
错误，却被 `get_valid_types_with_udf` 的 `TypeSignature::UserDefined` 分支用 `exec_err!` 包成
了 Execution 错误。**#25996 不会修掉这一条**（它只把最内层换成 plan_err!），修完的
`map_extract(1, 1)` 会是同样的形状。

今天**不提**这条：它与在飞的 #25996、与已开的家族 issue #14431/#14432/#20501、以及我们自己的
#26329 都相邻，一天一个上游产物是规矩。等 #25996 合并后再决定是提一个独立 issue（错误
*kind* 影响按 `DataFusionError::Execution` 匹配的调用方）还是不必提。写到
`findings/nested-map-coercion-error.md`。

### Phase 4 — 目录增长：decimal 与 GROUPING SETS

三个新探针（66）：

* `decimal_mul_add_scale`（arithmetic）—— 乘法必须把 scale 加宽（2+2=4），加法必须精确；
  `2.50 + 0.05` 是最能暴露"悄悄退化成 float"的用例。两引擎一致。
* `decimal_division_scale`（arithmetic）—— **known_difference**：DataFusion 保留 DECIMAL
  并按自己的规则定 scale（此处 6 → `0.333333`），DuckDB 提升为 DOUBLE
  （`0.3333333333333333`）。上游 #7301 就是在讲这个兼容模式，所以不是 bug。
* `grouping_sets_grand_total`（新类别 `grouping_sets`）—— `GROUPING SETS ((g), ())`
  必须给出每组一行并含总计 12 行，且 NULL 组不能与总计合并。带 `expect_rows` 固定答案，
  无 oracle 也能跑。两引擎一致。

**Harness 修正（`harness_noise`，附修法）**：`df_diff.canon` 把 DECIMAL 处理成了两种拼法 ——
DataFusion 的 DECIMAL 列以文本到达（`2.2000`，每位 scale 一个数字），DuckDB 给的是
`Decimal` 对象（走 float 格式化 → `2.2`），于是任何 decimal 探针都会假报 MISMATCH。现在带小数
点的十进制字面量在两条路径上走同一套规范化。只对**含小数点**的字面量生效，所以整数拼法
（`1` 与 `1.0`）仍然区分得开，不会掩盖真实差异。

### 过程教训

`pkill -f "cargo build --release"` 匹配到了**执行它的那个 shell 自身**，把整条命令连同刚起的
两个后台任务一起杀了 —— 与 2026-10-14 记的 `pgrep` 自匹配是同一类错误。用
`pkill -f "[c]argo build"` 这种括号写法，或者别用 pkill。

### Phase 6 — 为什么这个 slot 不发邮件

规则是：09/13/17 点只在**提了上游 issue/PR、准备了待点一下的分支、或运行失败**时发。本 slot
三者都不是（而且我们**故意没有**提东西，免得撞 #25996），所以不发。用户在 21:00 的日报里会
拿到全部内容；需要用户做的仍是上一轮的两件事（贴回复、换 PR 描述）。
