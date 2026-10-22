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

**Update 2026-10-15 (slot 2) — a fourth blocked write, and a bigger one than expected:**
re-running a failed CI job is blocked too:

```console
$ gh run rerun 38738221541 --repo apache/datafusion --failed
run 38738221541 cannot be rerun; Must have admin rights to Repository.
```

So the boundary is not "we cannot write content to apache/*", it is "we cannot change
anything at all on an apache/* repository": no PR, no issue, no PR body edit, no re-run.
Every one of those is a one-click hand-off to the user. Worth stating once here so no future
slot spends a check on a re-run, and so a red CI run is reported as an action item instead of
being retried from the sandbox. (Reading is unaffected: PR state, checks, job logs, review
comments and issue search all work — the wasm-pack log above was read this way.)

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

**结论**：probes 66（+3）· candidates 0 · 拦下一个重复 PR · **并在从未扫过的聚合函数里挖到 1 个新根因**

| 阶段 | 结果 |
|---|---|
| Phase 0 健康闸门 | 7/7 PASS（fresh clone，duckdb 1.5.6 oracle 已装，build 6m33s） |
| Phase 1a Tier-A | 1874 次调用 / 391 findings / **20 个根因**（INTERNAL=369, NOT_IMPL=22）—— 与基线一致，无新增根因 |
| Phase 1a+ 聚合函数 | **新**：72 个聚合函数 / 458 次调用 / 191 findings / 13 个根因，其中 **1 个是全新的**（见下） |
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

### 391 findings 全部分归因完毕（本 slot 的新进展）

之前几轮把 Tier-A 的 20 个 cluster 归成了"13 个 = #20501 那条路径 / 16 条 = map_extract /
其余 NOT_IMPL 未细究"。今天把最后一块补上了：**22 条 NOT_IMPL 逐个对文档核实，全部是文档写明的
限制，不是 bug**。于是 391 条发现现在没有任何未解释的残留：

| cluster（消息签名） | 条数 | 归因 |
|---|---|---|
| `date_trunc`, `date_part`, `string_to_array`, `generate_series`, `range`, `round`, `trunc`, `regexp_like`, `substr`, `btrim`, `log`, `ltrim`, `rtrim`（13 个 cluster） | 353 | `OneOf` 全分支失败走 `internal_err!` → **PR #26329**（在审），属于 #20501 |
| `map_extract(UserDefined)` | 16 | 见上：缺陷 1 = #25996 在飞；缺陷 3 = #15817；缺陷 2 已实测待定 |
| `to_timestamp(Date32)` | 5 | 文档写明"Supports strings, integer, unsigned integer, and double types as input"，Date32 不在其中 → **限制，非 bug** |
| `gcd(Date32)` / `lcm(Date32)` | 5 + 5 | 文档写明参数是 "First/Second **numeric** expression" → 日期不是数值 → **限制，非 bug** |
| `to_time(Int64)` / `to_time(Float64)` / `to_time(Date32)` | 3 + 1 + 1 | 文档写明 "Supports strings and timestamps as input" → **限制，非 bug** |
| `arrow_cast('abc', 'abc')` | 2 | 第二个参数必须是 Arrow 类型名；报错 "Unsupported type 'abc'. Must be a supported arrow type" 正确且面向用户 → **非 bug** |

这组结论的价值不在"发现了什么"，而在**不再有人花时间重看它们**：这些错误信息都是面向用户的、
说明白了哪里不对、也没有让人去提 bug report。它们此前一直被当作"NOT_IMPL 待研判"，现在每条都带了
文档依据。（判据是仓库内 `docs/source/user-guide/sql/scalar_functions.md`，逐条比对了签名说明。）

### 版本口径与两处负结果（记录以免下次混淆）

* 探针实际跑在 **crates.io 的 `datafusion 55.2.0`** 上（`Cargo.lock` 锁死；`Cargo.toml` 写的是
  `"55"`）。`findings/*.md` 里写的 `apache/datafusion@<sha>` 是**读源码、定位调用点**时用的上游
  commit，不是探针的运行时版本 —— 两者不是一回事，别再混着写。本 slot 核对：上游 main 已走到
  `6bf43f29`（2026-10-15T12:47Z），**#25996 仍未合并**，所以上面对 map_extract 的归因成立。
* 语义探针改用 `--partitions 8` 重跑一遍（默认是 4）：66 探针结果完全一致
  （OK=56、KNOWN_DIFF=8、BOTH_ERROR=2、0 候选），没有新的 plan divergence。并行度数不是这里的问题源。

### 追加（同一 slot 稍后）：扫到聚合函数里去了，挖到 1 个新根因

`df_robust.py` 此前写死 `function_type = 'SCALAR'`，所以 **72 个聚合函数、11 个窗口函数从来没被
摇过**。给脚本加了 `--function-type` 选项（默认 SCALAR，行为不变），先跑聚合：

`python3 scripts/df_robust.py --function-type AGGREGATE` → 72 函数 / 458 次调用 / **191 findings
/ 13 个根因**（INTERNAL=185, NOT_IMPL=6）。

分两半，结论完全不同：

* **184 / 185 的 INTERNAL 就是 #26329 那个根因**（`avg(DATE …)`、`sum(DATE …)`、`median(DATE …)`、
  `approx_percentile_cont(…)` 全是 "Function 'X' failed to match any signature"）。这是给
  #26329 的一个**新论据**：这个修改不只救标量函数，它把整个函数目录（标量 + 聚合）的
  "Internal error … 请提 bug report" 文案一起去掉了。已加进待粘贴的 PR 描述。
* **1 条是全新的根因**，值得单独立案：

**`nth_value(x, 0)`：聚合形式报 internal error，窗口形式静默返回 NULL。**

```console
$ lakewatch sql "SELECT nth_value(v_int, 0) FROM probe_values"
Internal error: Assertion failed: n != 0: Nth value indices are 1 based. 0 is invalid index.
This issue was likely caused by a bug in DataFusion's code. Please help us to resolve this by filing a bug report in our issue tracker: ...

$ lakewatch sql "SELECT id, nth_value(v_int, 0) OVER (ORDER BY id) FROM probe_values"
-- 12 行全是 NULL，一点错都不报
```

同一个表达式、同一组参数，两种形式给出两种结果：一边把用户自己的输入当 DataFusion 的 bug 让人去
提 issue，另一边悄悄给 NULL。索引 1 起算、负数从末尾数，两条文档都没写 `0`，而聚合路径的断言信息
自己就写着 "0 is invalid index"。

* 聚合：`functions-aggregate/src/nth_value.rs:219`、`:350` 的 `assert_or_internal_err!(n != 0, …)`。
* 窗口：`functions-window/src/nth_value.rs:433-441`，`self.n.cmp(&0)` 里有一个
  `Ordering::Equal => (false, false)` 分支 —— 显式把 0 当成"永远没有结果"，没有任何校验。
  两条路径是两个不同的实现（UDWF vs UDAF），谁都没在入口校验 `n`。

**不是 #26329 那一类**：参数类型完全匹配签名，坏的是第二个参数的**取值**，失败点在建累加器时，
不在类型强制阶段。**不是重复**：`"Nth value indices are 1 based"`、`nth_value "index 0"`、
`nth_value 0 internal error` 三组搜索都没有对应 issue/PR（#24595 是零*参数*，#12320 是 NULL 输入）。

按规矩**没有开分支、没有提 issue**（#26329 还在开，一天一个上游 PR），但把现成的 issue 文本写好了：
`findings/nth-value-zero-index-issue.md`，用户复制即可提。修法很小，但横跨两个 crate
（`functions-aggregate` + `functions-window`），所以 issue 里问了维护者想要哪种范围：两处一起，
还是先修聚合那条（那是对用户可见的错误信息）。等 #26329 清掉后这就是下一个 PR 的靶子。

`--function-type WINDOW` 也跑了：11 个窗口函数 / 79 次调用（自动加 `OVER ()`，否则窗口函数根本不可达）→ **0 findings**，30 条干净的用户级错误，其余正常返回。窗口这一族干净。

（一个值得记住的盲区：`nth_value(x, 0)` 在窗口形式下**不报错**，所以摇法永远不会把它标成 finding —— 
上面那条新根因是靠*聚合*那一侧的错误才暴露出来的。只摇窗口的话会漏掉它。）

**记录口径的小改动**：从本 slot 起，`reports/df-robust.json` 和 `reports/df-diff.json`
（30 KB + 133 KB）也一并提交 —— 之前只提交 markdown，机器可读的原始结果留在沙箱里随沙箱消失，
下一 slot 无法复核"391/20"和"0 候选"这些数字是怎么来的。这两个文件就是那两句话的凭据。

### 更正：本 slot 运行到一半，PR 被批准了（13:17Z），CI 终于跑起来了

上面 Phase 0b 那段写"球在用户那边、0 条新评论"——那是 09:05–09:15 的事实。**09:17:18 EDT
（13:17:18Z）alamb 又交了一次 review，这次是 `APPROVED`**，正文只有一句 "Thank you
@SingletonYe 🙏"。我 09:21 那次核对用的是不带 `reviews` 的字段，所以没看见，直到收尾时才查出来。
（邮件已经在本 slot 发出，只有一个 slot 一封邮件的规矩，所以不在邮件里改口 —— 而且 GitHub 自己的
approval 通知邮件也到用户邮箱了，信息没有丢。这一行是为了让下一个 slot 不被前面的描述误导。）

**批准的直接后果：首次贡献者的 CI 门禁解除了。** `statusCheckRollup` 现在有 23 项，
`linux build test`、`cargo fmt`、`Check License Header`、`Check semver`、CodeQL、Large files 等
一大票处于 `QUEUED`，`reviewDecision` 已经是 `APPROVED`，`mergeStateStatus` 仍是 `BLOCKED`
（等必需检查跑完）。

所以下一个 slot 的 Phase 0b 变成一件明确的事：

1. 看 `268933e` 的 CI 结果。**红了就当场修**（首次贡献者的 PR 挂红是最坏结果）；
2. 绿了就等合并队列，**不要催**——刚批准 14 分钟，按规矩三天内不 nudge；
3. 用户那两件事（贴回复、换 PR 描述）现在都不阻塞了，但 PR 描述里仍写着"A unit test covering
   that failure"和"282 passed"，属于应改未改；回复则是礼貌性回答 reviewer 的问题。

## 2026-10-15 13:00 — slot 2

**结论**：68 探针（+2）· 0 待研判候选 · Tier-A 三族全部复现基线 · PR #26329 已批准、只剩 1 项 CI 挂（**flaky，需用户点一次 Re-run**）

| 阶段 | 结果 |
|---|---|
| Phase 0 健康闸门 | 7/7 PASS（fresh clone，含本 slot 新写的 Rust 代码） |
| Phase 1a 标量 | 1874 调用 / 391 findings / 20 根因 —— 与基线完全一致 |
| Phase 1a 聚合 | 458 调用 / 191 findings / 13 根因 —— 与 slot 1 一致（184 = #26329，1 = nth_value） |
| Phase 1a 窗口 | 79 调用 / 0 findings |
| Phase 1b 语义 | 68 探针 · OK=57 · KNOWN_DIFF=9 · BOTH_ERROR=2 · **0 候选** |
| Phase 4 | 补上目录里自己写的 TODO：`--session-option` 路径 + 2 个探针 |

### Phase 0b — PR 已批准；唯一的红项是环境故障，不是我们的代码

`APPROVED`（alamb，13:17Z），head 仍 `268933e`，无新评论，**不需要 nudge**（才批了 3.7 小时）。
检查 42 项：**38 pass / 3 skip / 1 fail**。红的是 `build and run with wasm-pack`：

```text
driver status: signal: 9 (SIGKILL)
driver stdout: Starting ChromeDriver 155.0.8059.39 ...
Error: driver failed to bind port during startup
```

拆开看，这跟改动无关也跟测试断言无关：浏览器驱动**被 SIGKILL**（runner 资源压力），死在**驱动启动阶段**，
连一个浏览器用例都没跑；同一 job 里 Rust 侧的 9 个用例是 `9 passed; 0 failed`。仓库当前的 `Rust`
workflow 有 10 个 run 在排队，main 分支自己也在排队 —— 这台 runner 明显被打爆了。

**我们能做的和不能做的**（`gh run rerun 38738221541 --failed` → `cannot be rerun; Must have admin
rights to Repository`）：**连重跑都无权**。这是第 4 个被 apache/* 挡住的写操作，补进能力边界表。
所以只能请用户点一下 Re-run：
<https://github.com/apache/datafusion/actions/runs/38738221541/job/116443866487>

不点也行：PR 已经 APPROVED，merge queue 本身会重跑一遍全套。**绝对不要为了"刷新"这个红叉去 push
一个空提交** —— 那在 reviewer 眼里就是刷提交。

### Phase 4 — 把目录自己留的 TODO 做掉了：`--session-option`

上一轮目录里 `integer_overflow` 的备注写着 `TODO(daily): add an --ansi/session-option path to
lakewatch`。本 slot 做完：

* **lakewatch**：新增全局 `--session-option KEY=VALUE`（可重复），在默认配置之后应用，所以显式选项
  永远赢。实现上走 `ConfigOptions::set`（返回 `Result`）而不是 `SessionConfig::set_str` —— 后者内部
  是 `.unwrap()`，手滑写错 key 会 panic 而不是报错；现在 `--session-option datafusion.nope=1` 得到
  一句干净的 `invalid --session-option \`datafusion.nope=1\`: ... not found on ConfigOptions`。
* **df_diff**：探针可以带 `session_options`，并且会把实际用的选项写进报告（可复现）。
* **新探针 `session_option_parse_float_as_decimal`**：`0.1 + 0.2` 在 `parse_float_as_decimal=true`
  下正好是 `0.3`（DuckDB 的 DECIMAL 字面量同样），用来证明这个开关**真的生效**而不是被静默忽略 ——
  这正是 session-option 这类开关最典型的失效方式。结果 OK。
* **新探针 `integer_overflow_ansi`**：同一个表达式在 ANSI 模式下打开。`enable_ansi_mode` 自己的文档
  写着"operations such as division by zero, **numeric overflow**, or invalid casts raise runtime
  errors rather than returning NULL or **adjusted values**"，实测（55.2.0）：

  | 表达式 | ANSI 关 | ANSI 开 |
  |---|---|---|
  | `9223372036854775807 + 1` | -9223372036854775808 | **-9223372036854775808** |
  | `-9223372036854775807 - 2` | 9223372036854775807 | **9223372036854775807** |
  | `9223372036854775807 * 2` | -2 | **-2** |
  | `abs(-9223372036854775808)` | Arrow overflow 报错 | Arrow overflow 报错 |

  即：**ANSI 模式对整数溢出不生效，文档跑在实现前面。** 结案方式：这是 ANSI epic **#23929** 明确
  圈定的范围（原文列了 "arithmetic overflow"），Spark 侧的 cast overflow 另有 #24950 —— **所以不
  提新 issue**，写进探针的 `known_difference` 并指向 #23929，等 epic 落地再回看。

顺手修正上一轮的两处猜测：`1/0` 和常量坏 cast 在 **两种模式都报错**（ANSI 开关不影响它们；
`errors.slt` 里那两个用例的差异来自别处）。现在这两点有实测结论，不再靠推测。

### 同一 slot 追加：把 ANSI 模式接进 Tier-A 摇法，结果是干净的负结果

`df_robust.py` 也加上了 `--session-option`（可重复），于是"ANSI 模式下会不会多出/少掉内部错误"这个
问题第一次可以被直接问出来，而不是靠猜：

| 运行 | 结果 |
|---|---|
| 标量 364 函数 / 1874 调用，**ANSI 开** | 391 findings / 20 根因 / INTERNAL=369 / NOT_IMPL=22 —— 与非 ANSI **逐项相同** |
| 聚合 72 函数 / 458 调用，**ANSI 开** | 191 findings / 13 根因 / INTERNAL=185 / NOT_IMPL=6 —— 与 slot 1 非 ANSI **逐项相同** |
| 窗口 11 函数 / 79 调用 | 0 findings（两轮都是） |

结论：对这些"参数本身就坏"的用例，`enable_ansi_mode` 既不会掩盖也不会制造内部错误 —— 它影响的是
**算术语义**（见 `integer_overflow_ansi`），不是类型/签名校验。这条负结果的价值在于把"要不要在 ANSI
下再摇一遍"这个问题彻底关掉：摇过了，两族都一致。

现在两个 harness 都能接收会话选项，后续想验 ANSI 专属行为（比如某个函数在 ANSI 下应当报
ARITHMETIC_OVERFLOW 而不是溢出值）可以直接写探针，不必再改代码。

## 2026-10-15 13:30 — how merging actually works here (read from the repo's own config)

Question asked, answer recorded so no slot has to rediscover it. Sources: `.asf.yaml`
(`ref=main`), the repository ruleset `Merge Queue` (id 25115946), and the PR's own fields.

**Nothing merges automatically.** The chain is: 1 approving review by someone with write access
+ every *required* status check green → **then a committer presses the button**. Measured on
#26329: `reviewDecision=APPROVED`, `mergeable=MERGEABLE`, `mergeStateStatus=BLOCKED`,
`autoMergeRequest=null`.

* Review requirement: `required_approving_review_count: 1` — satisfied (alamb).
* Required checks are the 36 contexts listed in `.asf.yaml`, including
  `build and run with wasm-pack` — the one that is currently red. So the merge is blocked by
  that flake, not by anything about the change.
* Merge buttons: **squash only** (`enabled_merge_buttons: squash: true, merge: false,
  rebase: false`).
* A merge queue is active on `main` (ruleset): `merge_method: SQUASH`,
  `grouping_strategy: ALLGREEN`, `max_entries_to_build: 5`, `min_entries_to_merge: 1`,
  `min_entries_to_merge_wait_minutes: 5`, `check_response_timeout_minutes: 360`. Once a
  committer queues the PR, GitHub builds it on top of the latest `main` and re-runs the required
  checks there; ALLGREEN means the entry must be green on its own.
* `allow_auto_merge: true` at repo level, but auto-merge has to be *armed* by someone with
  write access; the fork author cannot, and it is not armed on this PR. So "CI green" alone
  never lands it.
* Also from `.asf.yaml`: `creation_cap: max_open_pull_requests: 3` for users without write
  access. That is a hard ceiling above our self-imposed one-PR-per-day rule.
* `del_branch_on_merge: true` — the fork branch disappears on merge, so any post-merge work
  starts from a fresh branch.

### Correction to the slot-2 email: the user cannot re-run the check either

The slot-2 email said "you click Re-run". That is wrong, and the API says why:
`gh api repos/apache/datafusion --jq .permissions` → `{"admin":false,"maintain":false,
"pull":true,"push":false,"triage":false}`. Read-only. GitHub only offers "Re-run failed jobs" to
people with write access, and our own attempt got `Must have admin rights to Repository`. So the
re-run is a *committer* action: either they re-run the flake, or they merge and let the merge
queue re-run everything. Either way **nobody on our side has a click to make**: the honest
hand-off is the comment text in `findings/ci-rerun-request-comment.md`, or nothing at all.

### Do not click "Update branch"

The PR page offers it (`allow_update_branch: true`) and the branch is 32 commits behind. Don't:
a new head SHA resets the first-time-contributor workflow gate to `action_required`, which needs
another maintainer approval before any CI runs again — trading "one flaky check" for "no CI at
all, waiting on a human". The merge queue merges onto the latest `main` itself; being behind is
normal and is not a blocker here.

## 2026-10-15 17:00 — slot 3

**结论**：70 探针（+2）· 0 待研判候选 · Tier-A 三族再次逐项复现 · PR 仍只卡在那一个 flaky 必需检查上

| 阶段 | 结果 |
|---|---|
| Phase 0 健康闸门 | 7/7 PASS（fresh clone，build 6m57s，duckdb 1.5.6） |
| Phase 1a 标量 | 1874 调用 / 391 findings / 20 根因（INTERNAL=369, NOT_IMPL=22）—— 与基线一致 |
| Phase 1a 聚合 | 458 调用 / 191 findings / 13 根因 —— 与 slot 1、slot 2 一致 |
| Phase 1a 窗口 | 79 调用 / 0 findings |
| Phase 1b 语义 | 70 探针 · OK=59 · KNOWN_DIFF=9 · BOTH_ERROR=2 · **0 候选** |
| Phase 4 | +2 探针（ROLLUP + `grouping()`、RANGE 窗口 peer 语义） |

### Phase 0b — 没有可做的事，而且这次连"换个检查"都没得换

`OPEN`，head 仍 `268933e`，`APPROVED`，无新人类评论（只有 Codecov 机器人），
`build and run with wasm-pack` **仍然红**，其余 38 pass / 3 skip。按 Phase 0b 的表，CI 真红要当场修——
读完日志的结论是**没有代码可修**：ChromeDriver 在启动阶段被 SIGKILL（`driver status: signal: 9`），
一个浏览器用例都没跑，同 job 的 Rust 用例 9/9 通过；而且我们既不能重跑（只读账号，
`Must have admin rights to Repository`），也不该 push（新 SHA 会让首次贡献者门禁回到
`action_required`，等于把"一个 flaky 检查"换成"完全没 CI"）。上一轮已经把这些写进
`findings/ci-rerun-request-comment.md`，交给人处理。

**新证据（本 slot 补的）**：这不是仓库范围的宕机。查了最近完成的、同样跑 `rust.yml` 的 PR run，
它的 wasm-pack job 是 `success`：

```console
$ gh api repos/apache/datafusion/actions/workflows/rust.yml/runs?event=pull_request
38796147752  branch=rich-T-kid/bench-ndv-cap-arrow-rle-reader-v2  wasm=[success]
```

（main 分支上这个 job 一律 `skipped`，只在 PR 上跑，所以 main 不能用来对照。）也就是说更可能是我们那次
runner 的**一次性故障**（驱动进程被杀），而不是"这个 job 一直坏"。对合并路径没影响：committer 要么
重跑一次，要么把 PR 丢进 merge queue —— 队列会在最新 main 上重建并**重新跑一遍必需检查**，那时这个
检查是新的一次运行。

### Phase 4 — 两个新探针，都盯"标准语义 vs 实现"

1. `rollup_with_grouping_flag`：`GROUP BY ROLLUP(g)` 配上 `grouping(g)`。`probe_values` 里既有
   `g IS NULL` 那一组（2 行）又有总计（12 行），COALESCE 之后两者都长成 `-1`，**只有 `grouping()`
   能分辨**——这正是这个标志存在的意义，也是"ROLLUP 等价于哪个 GROUPING SETS 列表"最容易写错的地方。
   固定 `expect_rows`，无 oracle 也能跑。两引擎一致。
2. `window_range_unbounded_peers`：`RANGE BETWEEN UNBOUNDED PRECEDING AND CURRENT ROW` 的
   累计和。RANGE 是**按 peer 分组**而不是按行，`probe_values` 特意有并列值（4,4 和 5,5）和 NULL，
   按 ROWS 语义算会得到完全不同的数字，所以这条探针能真正区分两种实现；NULL 那一组按默认
   `NULLS LAST` 落在最后，两行同值同和。两引擎逐行一致。窗口这一族今天第二次扫（79 次调用）仍是
   0 个 Tier-A 发现，这条探针是把它钉住而不是找它麻烦。

### 邮件

按规矩 17:00 这个 slot **不发**：没有提上游 issue/PR、没有准备待点开的分支、运行没有失败。
（21:00 那个 slot 的日报会把今天四段合起来发。）需要人做的仍然是上一轮那几件：
`nth_value(x, 0)` 的 issue 文本、请 committer 重跑 CI 的一句评论、以及 PR 描述里过时的那两行 ——
都不阻塞合并。

## 2026-10-15 21:00 — slot 4 (last slot: daily digest)

**结论**：71 探针（+1）· 0 个待研判候选 · 0 个需要新提的上游产物 · 全天四段基线**逐条**复现 · PR 只差 committer 合并

| 阶段 | 结果 |
|---|---|
| Phase 0 健康闸门 | 7/7 PASS（fresh clone，build 7m21s，duckdb 1.5.6 oracle） |
| Phase 1a 标量 | 1874 调用 / 391 findings / 20 根因（INTERNAL=369, NOT_IMPL=22）—— 与已提交基线**逐条**相同 |
| Phase 1a 聚合 | 458 调用 / 191 findings / 13 根因（INTERNAL=185, NOT_IMPL=6）—— 同上 |
| Phase 1a 窗口 | 79 调用 / 0 findings |
| Phase 1b 语义 | 71 探针 · OK=59 · KNOWN_DIFF=10 · BOTH_ERROR=2 · **0 候选**（`--partitions 8` 结果相同） |
| Phase 4 | +1 探针（集合操作符 `ALL` 变体的多重集语义） |
| Phase 5 | `cargo fmt` / `clippy` / `test` / `daily_check.sh` 全绿 |
| Phase 6 | 日报已发（21:00 无条件是当日汇总） |

### Phase 0b — 早上批准之后毫无动静，本 slot 确认无事可做

`OPEN` · head `268933e` · `APPROVED` · `MERGEABLE` · 42 项检查 = **38 SUCCESS / 3 SKIPPED / 1 FAILURE**。
唯一红的仍是 `build and run with wasm-pack`，而且仍是**同一个 run**（38738221541，没有被重跑过）；
`comments` 里唯一的"新"条目是 Codecov 机器人（15:17Z）。alamb 批准于 13:17Z，距本 slot 约 8 小时，
而 nudge 的规矩是**至少间隔 3 天** —— 所以不催、不重推、不碰。球在 committer 那边：
要么重跑一次失败 job，要么把 PR 丢进 merge queue（队列会在最新 main 上重跑必需检查）。

### Phase 2/3 — 探到两个真缺陷，但两个都是上游 2024 年就开着的

为了给目录开一块新地（集合操作符的 `ALL` 变体从未被任何扫描或探针覆盖），
用 T1={1,1,1,2}（m 份）、T2={1,1,3}（n 份）做最小对照：

```console
$ lakewatch sql "… INTERSECT ALL …"   # DataFusion: 1,1,1     标准 / DuckDB: 1,1
$ lakewatch sql "… EXCEPT ALL …"      # DataFusion: 2         标准 / DuckDB: 1,2
```

SQL-92 §7.10 规定 `INTERSECT ALL` 输出 min(m,n) 份、`EXCEPT ALL` 输出 max(m−n,0) 份。
DataFusion 的行为等于把"该行在 RHS 出现过"当成"删光/全留"：`INTERSECT ALL` 把 LHS 的三份 1
全留着（应留两份），`EXCEPT ALL` 把 LHS 的两份 1 全删掉（应留一份），1 与 4 分区结果相同，
所以是语义缺陷而不是规划假象。

**但它不是新 bug。** `gh search issues` 直接命中两个 OPEN 两年多的 issue，引用的是同一段标准原文：

* apache/datafusion#12955 — *INTERSECT ALL returns wrong number of records from RHS*（2024-10-15 开）
* apache/datafusion#12956 — *EXCEPT ALL returns the wrong number of records*（2024-10-15 开）

按"绝不提重复"的规矩**不提 issue**。本轮的产出因此改成：一个探针 + `known_difference` 记录，
把这个已知偏差钉住，让后续 slot 不再重复研判；等任一 issue 修复时，再把这条探针翻成
pinned `expect_rows`。

**一次有价值的人工对照（负结果）**：另外手写了约 40 条语料里没有的 SQL，同时跑 DataFusion 与
DuckDB，专门找没人扫过的方向 —— interval 的月/年末尾钳制（`DATE '2024-01-31' + INTERVAL '1' MONTH`、
`DATE '2024-02-29' + INTERVAL '1' YEAR`、跨月减）、混合单位 interval、interval 的乘/除/比较/
`EXTRACT`/`SUM`、DECIMAL 溢出与混合精度算术、`CUBE`、多列 `GROUPING SETS`、无 `GROUP BY` 的
`HAVING`、聚合 `FILTER`、多键 `DISTINCT`、窗口帧、`LEAD`/`LAG` 的负偏移、数组与字符串边界 ——
**逐条一致，没有新偏差**；真正"两边都报错"的只有 `nth_value(x,0)`（已知）与 `NTILE(0)`（两边都干净报错）。
记录这个负结果是为了让下一个 slot 不必再走一遍同样的路。

### Phase 4 + harness — 一个探针，外加一处让报告真正可复现的修正

探针（71 个，当日 8 个的预算至此用满）：`set_ops_intersect_except_all_multiset`，
一条 SQL 同时覆盖两个操作符（同一组输入驱动两者，用 `op` 列区分），带 `known_difference` 记录
上述两个 issue。

harness 修正：`df_robust.py` 写出的 JSON **每次运行都会重排**，根因是
`information_schema.routines` 只按 `routine_name` 排序，同名重载行的先后由引擎决定，
而 findings 的排序键 `(verdict, function, profile)` 不足以区分重载行。后果是"与基线逐项一致"
这句话此前只能用**计数**证明，不能逐条比对 —— 而这正是这些报告存在的意义。现在：

1. 清单查询改为 `ORDER BY routine_name, data_type, syntax_example`（全序）；
2. findings 排序键补全为 `(verdict, function, profile, documented_type, sql)`；
3. 根因簇的排序加了 `signature` 兜底。

验证：同一族连跑两次，输出**逐字节相同**（标量、聚合两族均已验证）。用这个新能力复核了本 slot
的结论：391 条与 191 条 findings 与已提交基线是**逐条**相同（多重集相等），不只是计数相同。
`reports/df-robust*.json` 因此在本轮重新生成并提交，此后再运行不会产生无意义 diff。

### 今天四段合起来

| slot | 健康闸门 | 标量 / 聚合 / 窗口 | 探针 | 实质产出 |
|---|---|---|---|---|
| 08:55 PR-response | — | — | — | reviewer 要求调整测试 → 已改并推送（`268933e`） |
| 09:00 slot 1 | 7/7 | 391/20 · **191/13（首次扫描聚合族）** · — | 63→66 | **alamb 批准 #26329**；新根因 `nth_value(x,0)`；拦下会与 #25996 撞车的 PR |
| 13:00 slot 2 | 7/7 | 391/20 · 191/13 · 0 | 66→68 | `--session-option` 落地；wasm-pack 红是环境故障（ChromeDriver SIGKILL）；ANSI 横扫与非 ANSI 逐项相同 |
| 17:00 slot 3 | 7/7 | 391/20 · 191/13 · 0 | 68→70 | 无事可修；查同 workflow 的其他 PR run 证明是偶发 |
| 21:00 slot 4 | 7/7 | 391/20 · 191/13 · 0 | 70→71 | 集合操作符缺陷确认为上游 #12955/#12956；报告确定性修正 |

全天：**1 个新根因**（`nth_value(x,0)`，09:00 段），**1 个新方向的缺陷**（集合操作符，判定为
`known_upstream`），**自提上游产物 0 个** —— PR #26329 在审，按"一天一个、绝不与在审 PR 并行"的
规矩不新增；我们无法在 `apache/*` 开 issue，`nth_value` 的文本已备好交给用户。

### 需要人做的三件（都不阻塞合并）

1. 请 committer 重跑那个 flaky job 的一句评论：`findings/ci-rerun-request-comment.md`
2. `nth_value(x, 0)` 的 issue 文本（`findings/nth-value-zero-index-issue.md`）
3. PR 描述里过时的 "A unit test covering that failure" / "282 passed" 两行

### Next

* **明天 09:00 第一件事**：Phase 0b 看 #26329 是否被重跑或合并。合并之后，本 workflow 的下一个
  PR 靶子是 `nth_value(x, 0)`（聚合侧的 `assert_or_internal_err!` + 窗口侧缺失的校验，两处一个 PR，
  正是"同一根因一个 PR"的合适粒度）；
* `map_extract` 家族等 **#25996** 合并后再定：它只把最内层换成 `plan_err!`，
  而 `TypeSignature::UserDefined` 分支把 plan 期失败包成 `Execution error` 那一条它修不掉；
* **目录下一块真正没扫过的地**：`information_schema.routines` 里还有 **TABLE 函数族**
  （`generate_series`、`range`，本 slot 已确认可用），而 `df_robust.py` 的 `--function-type`
  目前只认 SCALAR/AGGREGATE/WINDOW —— TABLE 型的调用形状不同（`SELECT * FROM f(...)`），
  需要一小段新代码；
* interval / DECIMAL / 嵌套类型今天人工对照过一遍，一致；再往前走应换"标准语义优先"的角度
  （集合操作符其余变体、`MERGE` 是否已可用、`GROUPING SETS` 的更多形状）。

## 2026-10-16 09:00 — slot 1 (first slot of the day)

**结论**：74 探针（+3，TABLE 函数族首次覆盖）· 0 个待研判候选 · **1 个新的 INTERNAL 类 bug（递归 CTE）**
· 1 处 harness 修正

> **记录说明**：这个 slot 在写记录之前被中断，所以本节由下一个时间片（10-17 01:30 手动运行）
> 根据工作区里留下的、已验证的产物补写；该运行同时完成了 Phase 5 的提交与推送。

| 阶段 | 结果 |
|---|---|
| Phase 0 健康闸门 | 7/7 PASS（fresh clone，build 6m05s，duckdb 1.5.6） |
| Phase 1a 标量 / 聚合 / 窗口 | 391/20 · 191/13 · 0 —— 与基线逐条一致，且重跑后 `reports/df-robust*.json` **零 diff**（上一轮刚修好的确定性在此生效） |
| Phase 1b 语义 | 74 探针 · OK=62 · KNOWN_DIFF=10 · BOTH_ERROR=2 · **0 候选** |
| Phase 2/3 | 新 bug：递归 CTE 在标量子查询里自引用 → internal error（见下） |
| Phase 4 | +3 探针（`generate_series` / `range`，即 `function_type='TABLE'` 的整个函数族） |

### Phase 0b — #26329 依旧无事可做

`OPEN` · head `268933e` · `APPROVED` · `MERGEABLE` · 42 项检查 38 SUCCESS / 3 SKIPPED / 1 FAILURE，
唯一红的还是那个 `wasm-pack`（同一个 run 38738221541），无新人类评论。`updatedAt` 在凌晨被
GitHub 刷新了一次（`main` 前进导致的重算），不是有人动过。按规矩不催、不重推。

### Phase 3 — 新发现：递归 CTE 在标量子查询中自引用 → `Internal error: Unexpected empty work table.`

给目录开"递归 CTE"这块新地时，一次人工对照（DataFusion vs DuckDB）撞出真问题：

```sql
WITH RECURSIVE t(n) AS (
    SELECT 1
    UNION ALL
    SELECT n + 1 FROM t WHERE n < 3 AND n < (SELECT MAX(n) FROM t)
)
SELECT n FROM t ORDER BY n;
```

```console
$ lakewatch sql "…"
lakewatch: Internal error: Unexpected empty work table.
This issue was likely caused by a bug in DataFusion's code. Please help us to resolve this by
filing a bug report in our issue tracker: https://github.com/apache/datafusion/issues
```

关键在于**同一个错误的其它写法都得到了干净的报错**：把第二次自引用放进 `JOIN`、`EXISTS` 或
`IN` 时，DataFusion 会说 `This feature is not implemented: Multiple recursive references to the
same CTE are not supported` —— 而且这正是它自己的测试文件所断言的行为
（`datafusion/sqllogictest/test_files/cte.slt:892`）。所以缺陷不是"这个形状不支持"，而是
**一类形状绕过了那道检查**，最后在 work table 里以"内部不变量被破坏"的形式炸掉。

定位（读的是 `main` 上今天的源码，不是只读 55.2.0 的现象）：

* `assign_work_table`（`datafusion/physical-plan/src/recursive_query.rs:390-412`，由 `:98` 调用）
  用 `transform_down` **只遍历递归项子树**来数 work-table 引用，而标量子查询的计划是递归项的
  *兄弟*（`EXPLAIN` 里看得很清楚：`RecursiveQueryExec` 一个孩子是递归项，另一个
  `AggregateExec → WorkTableExec` 是子查询），所以计数器停在 1，那句 `not_impl_err!` 永远不触发。
* 真正抛错的是 `WorkTable::take`（`datafusion/physical-plan/src/work_table.rs:77-83`），
  它把"work table 里没有批次"当成内部不变量被破坏。

**不重复**：`gh search issues/prs` 试了 `"Unexpected empty work table"`、`"multiple recursive"`、
`"recursive references"`、`"recursive subquery"`、`"work table"`，没有任何对应报告；最近的开着
的相邻工作是 #25893（递归项类型被静默转换）、#26054/#26056（MIN/MAX 动态过滤未重置）、#19427
（两个递归 CTE 的 join 不终止）、#26238（物化 CTE 计划节点），都不是这一条。详见
`findings/recursive-cte-subquery-internal-error.md`（含修复方向）与可一键粘贴的 issue 文本
`findings/recursive-cte-subquery-internal-error-issue.md`。**这是我们无法在 `apache/*` 开 issue
的，需要用户代发。**

### Phase 4 — 目录增长：TABLE 函数族（3 个探针）

`information_schema.routines` 里有 4 类函数，`df_robust.py --function-type` 只认 SCALAR/AGGREGATE/
WINDOW，而 `function_type='TABLE'` 的 `generate_series` 与 `range` **从未被任何扫描或探针碰过**：

* `table_function_int_series` —— 两个函数、升序与降序步长一次覆盖（`range` 是右开区间，
  `range(5,-1,-2)` 必须是 5,3,1）；
* `table_function_step_near_i64_max` —— 步长贴着 i64::MAX：朴素的 `current + step` 会溢出，
  而这里的溢出是 PANIC 级回归；正确结果是恰好两行 `[1, 9223372036854775806]`，输出有界所以每轮都能跑；
* `table_function_date_series` —— DATE 边界 + INTERVAL 步长（与整数形式是两条代码路径），端点用
  DATE 断言，避免时间戳渲染差异。

顺手确认了两个"不是 bug"：`range(1,10,0)` / `generate_series(1,10,0)` 报
`Error during planning: Step cannot be zero`（干净报错，DuckDB 同样拒绝）；步长取 `i64::MIN`
也不 panic。注意前者**不能**进扫描目录 —— 一旦这个检查被删掉，不会干净失败而是死循环，所以
本轮刻意没有为它写探针。

### harness 修正：`canon()` 在裸 DATE 上抛异常

新探针 `table_function_date_series` 第一次跑出 `ERROR_DIVERGENCE`，读细节发现是 harness 自己的
bug：`canon()` 对所有日期类值都调用 `value.isoformat(sep=" ")`，但只有 `datetime.isoformat`
接受 `sep`，**裸 `date` 会抛 `date.isoformat() takes no keyword arguments`**，于是任何返回 DATE 的
探针在 DuckDB 一侧都会变成假候选。现在按类型分别处理。这类"oracle 同时审计工具自己"的记录，
与之前的 `render.rs` 时间戳 panic、`canon()` DECIMAL 拼写、`df_robust.py` 报告重排是同一类问题。

### 邮件

这个 slot **没有发邮件**（09:00 的规则是只在提了上游产物 / 准备了待点开的分支 / 运行失败时发，
本轮三者都不是；发现的 bug 已进入仓库记录，交给 21:00 的日报汇总）。

## 2026-10-17 01:30 — manual run (full slot)

**结论**：**PR #26329 已合并 —— 本 workflow 的第一个上游合并** · 昨天被中断的那一段已补推 ·
`nth_value(x, 0)` 的修复已在本地实现并提交（验证状态见下，诚实记录）· 基线逐条复现 · 0 候选

| 阶段 | 结果 |
|---|---|
| Phase 0b | **#26329 `MERGED`**（2026-10-16 14:29:53Z，40 项检查全绿）→ "在审 PR 期间不开新 PR"的约束解除 |
| Phase 1a 标量 / 聚合 / 窗口 | 391/20 · 191/13 · 0 —— 与基线逐条一致，重跑后 `reports/df-robust*.json` **零 diff** |
| Phase 1b 语义 | 74 探针 · OK=62 · KNOWN_DIFF=10 · BOTH_ERROR=2 · **0 候选** |
| Phase 5 健康闸门 | **未完成**（原因见"环境"一节；不做成绿色就不声称是绿色） |
| 上游产物 | 修复分支已提交（`ee2c708`），本 run 未推送 —— 见下 |

### Phase 0b — #26329 合并了

`state=MERGED`，`mergedAt=2026-10-16T14:29:53Z`（10-16 10:29 EDT），合并后检查为
**40 SUCCESS / 3 SKIPPED、0 FAILURE**：那个一直红的 `wasm-pack` 不再是失败项（committer 重跑或
merge queue 重建后通过）。批准之后没有新的维护者评论，我们的分支头也没有被改动。

这是这个 workflow 提交的第一个被上游合并的修复（`fix: report OneOf signature mismatches as
planning errors`），从 10-14 22:19 手动开 PR 到 10-16 10:29 合并，约 36 小时。三件"死于没人回应"
的风险都没有发生：reviewer 唯一的要求（把单测改成 slt）当天就改了，CI 那一个红项我们正确地
判定为环境故障而没有乱推。

**后续影响**：一天一个 PR、且不得与在审 PR 并行的约束不再阻塞；本 run 因此着手准备下一个产物。

### Phase 3 — 第二个上游产物：`nth_value(x, 0)`（已实现、已提交，未推送）

上一轮备好的 issue 文本仍在 `findings/nth-value-zero-index-issue.md`。这一轮不再只准备文本，
而是把修复做出来了，落在 `SingletonYe/datafusion` 的分支 `fix/nth-value-zero-index`（提交
`ee2c708`），改动四文件 36 行：

* `datafusion/functions-aggregate/src/nth_value.rs`：在 `NthValue::accumulator()` 里——
即索引从字面量读出来、两个累加器分叉之前——统一拒绝 `n == 0`，并删掉两处
`assert_or_internal_err!`（它们是这两个 `try_new` 的唯一调用方）；
* `datafusion/functions-window/src/nth_value.rs`：扩展已有的 `validate_nth_value_n`
（它本来就在 `partition_evaluator` 里被调用，并且已经在挡 `i64::MIN`），补上同一个 0 检查；
* `window.slt` / `aggregate.slt`：各一条断言新错误文本的用例（`window.slt` 那条紧挨着 NTILE 的
负参数测试——那是同一类错误的既有先例）。

两个调用点用**同一句**错误文本，因为它们本来是同一件事：`nth_value index must not be 0;
indices are 1-based and negative values count from the end`。按本仓库 reviewer 在上一个 PR 里的
明确偏好（"please adapt this test to .slt"），覆盖写成 slt 而不是单测。

顺带查实了一件不再算 bug 的事：窗口侧**已经**有 `validate_nth_value_n` 在挡 `i64::MIN`（55.2.0
就有），而**聚合侧**的 `-*value` 没有这道保护；但我从 SQL 无法触达 `is_reversed == true` 的路径
（三种写法都返回 NULL 而不是 panic），所以没有把它塞进这个 PR，只在 PR 正文里作为已知不对称
写清楚。这也正是"只在 main 上验证过的现象"与"读 main 源码"必须同时做的例子：一开始以为是
窗口侧完全没校验，读了 main 才发现它只是漏了 0 这一种。

**验证状态**：`cargo fmt --all --check` 干净；补丁已存为 `findings/nth-value-zero-index-fix.patch`。
`cargo test -p datafusion-sqllogictest --test sqllogictests` 的**运行没能在本 run 内完成**
（原因见下），所以**这一轮没有推送分支**——按规矩，未经验证的改动不作为待点开的 PR 交出去。
补丁、PR 正文（`findings/nth-value-zero-index-pr-body.md`）都已在仓库里，下一个 slot 只要
跑一次 slt 就能决定推还是弃。

### 环境 —— 2 核 + 交换：两个 Rust 构建互相饿死

这个沙箱 `nproc=2`、内存 3.9 GB。本 run 同时跑了两件重活：DataFusion workspace 的
`slt` 运行器构建（依赖树巨大）和 lakewatch 的 `cargo build`/`clippy`。结果两边都卡住
（`kswapd` 忙、`rustc` 只占 0.5% CPU），健康闸门两次被牺牲掉，slt 的最终链接在 13 分钟里几乎
没有推进。**教训**：重活必须串行——先跑闸门与扫描（几分钟），再动 DataFusion 的构建；
未来要为"验证一个上游修复"单独留出时间的 slot，或者干脆让 CI 去跑 slt，本地只保证
`cargo fmt` + `cargo check -p <crate>`。

另外两次踩到同一个坑：`pkill -f "cargo build"` / `pgrep -f "bash scripts/daily_check.sh"`
**匹配到了我自己所在的 shell**（自己的命令行里就含这串字符），把自己 kill 掉。10-14 的
`pgrep -f "cargo build"` 等待循环已经记过这一课，这次补一条操作规则：一律用 PID 或
`[c]argo` 这样的括号技巧，不要用会匹配自身命令行的模式。

### 邮件

手动触发按"整段时间片"处理，但 21:00 才是日报；本 run 的邮件按下表判断：只有"提了上游产物 /
准备了待点开的分支 / 运行失败"才发。本 run 三者都不是（PR #26329 的合并是上游动作，不是我们
这一轮提交的产物；分支未推送），所以**不发邮件**。

## 2026-10-17 03:15 — 更正与验证：`nth_value` 修复收窄到聚合侧，分支已推送

**更正上一节**：上一节写"窗口侧也补上了 0 检查" —— **那是错的，已回退**。把 slt 跑起来之后，
`window.slt` 立刻说明了原因：文件里有一段标题就是 `## test handle NULL and 0 value of nth_value`
的测试，明确断言 `nth_value(v2, NULL) OVER (...)`、`nth_value(v2, v2*null) OVER (...)` 和
`nth_value(v2, 0) OVER (...)` **都返回 NULL**。也就是说窗口侧"0 → NULL"是**有意为之、并且已经
写进测试的既有行为**，不是漏掉的校验。原来的改法会直接打破那 4 条断言 —— 而这正是"宁可少做，
也不能让 PR 变红"要避免的；何况改它属于**行为变更**，不是错误信息种类的修正，轮不到我们自作主张。

**收窄后的分支**（`SingletonYe/datafusion` 的 `fix/nth-value-zero-index`，提交 `dcaac3e1`，
2 文件 22+/13-）：

* `functions-aggregate/src/nth_value.rs`：只在 `NthValue::accumulator()` 里校验一次
  （索引从字面量读出、两个累加器分叉之前），删掉两处 `assert_or_internal_err!` —— 它们是这两个
  `try_new` 的唯一调用方，所以没有别的路径失去校验；
* `sqllogictest/test_files/aggregate.slt`：一条断言新错误信息的用例；
* 窗口侧**一行未动**；"两种形式是否应当一致"作为公开问题写进 PR 正文，由维护者决定。

**验证（这次真的跑完了）**：

```console
$ cargo test -p datafusion-sqllogictest --test sqllogictests -- test_files/aggregate.slt   # exit 0
$ cargo test -p datafusion-sqllogictest --test sqllogictests -- test_files/window.slt      # exit 0
$ cargo fmt --all --check                                                                  # clean
```

待点开的 PR（**一步之遥**）：

```text
https://github.com/apache/datafusion/compare/main...SingletonYe:fix/nth-value-zero-index?expand=1
```

### 两条操作教训

1. **`slt` 的期望文件是"行为的第二份文档"。** 这一轮如果顺手把那 4 条断言改成"期望报错"，测试
   会全绿，但那是在**削弱测试**掩盖一个真实结论（窗口侧是有意设计）。规则里"Never weaken a
   test to make something green"这一条，今天真的救了这个 PR —— 而且它救的方式是让结论从
   "两种形式不一致的 bug"变成"一个错误信息种类的 bug + 一个待维护者裁决的行为问题"。
2. **跑 slt 之前必须初始化数据子模块。** `testing/`（datafusion-testing）没初始化时，
   `window.slt` 会报 **104 条假失败**（`aggregate_test_100` 的 CSV 不存在，计划里退化成
   `EmptyExec`）。`git submodule update --init --depth 1 testing` 之后失败清零。之前若不做这一步，
   根本分辨不出"哪些红是我们造成的"。

### 邮件

本 run 属于"准备了一个待点开的分支"，按规矩发一封 intraday 提醒，compare URL 放在正文最显眼处。

## 2026-10-17 09:00 — slot 1 (first slot of the day)

**结论**：77 探针（+3）· 0 个待研判候选 · Tier-A 四族逐条复现（报告文件 **git 零 diff**）·
健康闸门 **7/7 PASS** · 本 slot 无新上游产物（PR 预算被待开的 `nth_value` 分支占着，见下）

| 阶段 | 结果 |
|---|---|
| Phase 0 健康闸门 | **7/7 PASS**（fresh clone，build 6m10s，duckdb 1.5.6） |
| Phase 1a 标量 / 聚合 / 窗口 | 391/20 · 191/13 · 0 —— 与基线逐条一致 |
| Phase 1b 语义 | **77 探针** · OK=65 · KNOWN_DIFF=10 · BOTH_ERROR=2 · **0 候选** |
| Phase 4 | +3 探针（窗口 `nth_value` 的 0 索引、`split_part` 索引语义、ISO-8601 周编号） |
| Phase 0b | 无在审 PR；上一轮的分支仍待用户点开 |

### Phase 0b — 没有在审的 PR，但有一件事在等用户

`gh pr list --author SingletonYe` 只有 #26329（MERGED）。上一轮准备好的
`fix/nth-value-zero-index`（`dcaac3e1`）**还没开成 PR**，只差用户点一下（compare URL 已在上轮邮件里）。
因此本 slot **不动用今天唯一的 PR 预算**：一天一个，而且不与自己待开的分支抢跑道 —— 若今天再准备
第二个分支，用户点两下就是两个我们自己的 PR 在飞，而"无人维护的 PR 比没有 PR 更糟"正是 #21032 的死法。
递归 CTE 那个修复继续排队。

### 本 slot 最有价值的产出：把 `nth_value` 的"不对称"钉进目录

三个新探针里最重要的是 `nth_value_zero_index_window_returns_null`：窗口形式对 0 索引**两个引擎都返回
NULL**，而聚合形式会报错。这个不对称是**上游有意为之**（`window.slt` 那段
"test handle NULL and 0 value of nth_value"），现在它连同解释一起进了目录，免得未来某个 slot 把它
当 bug"修掉"—— 那正是今天凌晨差点发生的事。另两个补的是真空白：字符串切分的索引语义
（1/2/-1/越界，0 被刻意排除，因为那是 DataFusion 干净报错而 DuckDB 返回 `''` 的方言差异），
以及 ISO-8601 周编号（`date_part('week', DATE '2023-01-01') = 52` —— 2023-01-01 是周日，
属于 2022 年第 52 周；美国式编号或"年内天数/7"会给出 1）。

### Tier-A：我们的修复在扫描报告里是可测量的

聚合扫描的 191 条 findings 里有 6 条属于 `nth_value`，其中一条正是：

```text
INTERNAL | SELECT nth_value(0, 0) | This issue was likely caused by a bug in DataFusion's code …
```

`zero` profile 恰好把这个根因摇出来了。等 pinned 版本包含我们的修复，这条会变成干净错误
（CLEAN_ERROR 不上报）→ 聚合族的 INTERNAL 计数下降，这就是**可测量的验收标准**
（与 #26329 用"391 → 39"验收同一套路）。

### 研判：`= ANY` / `> ALL` 在投影里报错 —— 版本落后，不是可提的 bug

给"集合比较子查询"开新地时撞到：在 **SELECT 列表**里写 `= ANY (SELECT …)` / `> ALL (SELECT …)`，
甚至相关的 `EXISTS (…)`，在 pinned 的 55.2.0 上都是

```text
This feature is not implemented: Physical plan does not support logical expression
Exists(Exists { subquery: <subquery>, negated: false })
```

但**先查上游再下结论**救了这一条：`gh search issues "does not support logical expression"` 直接命中
**#23022**（同一条错误信息），它已于 **2026-09-15 被以 `completed` 关闭**，由
**#24972 "Support predicate subqueries in projections"**（2026-09-15 合并）与 #23039 完成；main 的
`subquery_projection.slt:47` 现在正断言 `EXISTS (…)` 出现在投影里能正常工作。也就是说
**我们的 pinned 55.2.0 缺少这个修复**（它是从 branch-55 切出来的补丁版本），这是 pin 落后的
`known_upstream`，**不该提 issue**。

顺带记两条给以后用：

* 同一个形状在 **WHERE** 里则走 mark join 路线能跑（main 的 `null_aware_mark_join.slt:290` 明确写着
  "In a filter the paper's `= ANY` form works today"），所以"能不能用"取决于子查询出现在哪一层 ——
  写探针时必须固定层级，否则结论会随位置漂移。
* 这轮也确认了 **pin 的代价**：`Cargo.toml` 固定 55.2.0 意味着凡是 2026-09-15 之后进入 main 的
  subquery/projection 能力，我们的目录一概测不到。要跟进的话得单独排一次"升级 pin + 重建基线"
  的 slot（会改变 391/20 的基线，不能顺手做）。

### harness 保真度：±inf / NaN 在输出里被压成 NULL

顺带发现（**未修**，只记录）：`src/render.rs:179/182` 用
`serde_json::Number::from_f64(v).map_or(Value::Null, Value::Number)`，于是
`SELECT ln(0.0)` 在 `-f json` 下是 `null`、在 `-f csv` 下是空字段（arrow 的 CSV 写入器对非有限浮点数
就是空），而 `-f table` 才显示 `-inf`。影响：任何产生 ±inf/NaN 的探针在 DataFusion 一侧会变成
"NULL/空"，而 DuckDB 一侧是 `inf` → **凭空造出一个假候选**（或把真实差异藏起来）。目前目录里没有
产生 inf 的探针，所以不着急；修的话要么让 render 输出 `"inf"/"-inf"/"NaN"` 字符串，要么让 `canon()`
对浮点列的 `null` 与 `inf` 做双向归一化。

### 关于"如何分辨红是我们造成的" —— 一条环境规则

`slt` 运行需要 `testing/` 数据子模块（datafusion-testing）。没有它时 `window.slt` 会报 **104 条假失败**。
另外确认：`nproc=2`，两个 Rust 构建同时跑会互相饿死（本 slot 让闸门与扫描先跑完，DataFusion CLI 的
构建放后面，闸门因此拿到了干净的 168s clippy）。

### 邮件

**未发**：09:00 的规则是只在提了上游产物 / 准备了待点开的分支 / 运行失败时发，本 slot 三者都不是
（发现的都是研判结论与目录增长）。21:00 的日报会汇总。

### 补记（本 slot 结束前）

* **`main` 版 CLI 没能编完**：为了把"是不是 55.2.0 落后于 main"这类问题一次问清楚，本 slot 在
  `/home/user/workspace/df-fork`（upstream main 的浅克隆）里起了 `cargo build -p datafusion-cli`。
  2 核机器上它走到 `datafusion-cli` 这个 crate 就没能在 slot 内结束，所以本 slot 关于 #23022 的结论
  依据的是 **issue 状态 + main 的 slt 期望**（#24972 于 2026-09-15 合并、`subquery_projection.slt:47`
  断言投影里的 `EXISTS` 可用），不是运行 main 得到的结果 —— 这一点必须说清楚。
  **下一轮可直接复用**：沙箱在 run 之间保留，`df-fork/target/debug/datafusion-cli` 一旦编完就是一个
  "main 版 oracle"，以后遇到"这个现象是不是已经在上游修好了"可以几秒钟验证，不必再靠读源码和 issue。
  （注意 `RUSTUP_TOOLCHAIN=stable`：上游 `rust-toolchain.toml` 钉的 1.99.0 在本环境装不下来。）
* **`= ANY`/`> ALL` 在投影里的 main 行为仍然待确认**：本 slot 只确认了"55.2.0 会报
  `does not support logical expression Exists(...)`"以及"该报错对应的 issue 已关闭、修复已进 main"。
  ANY/ALL（而不是 EXISTS）在 main 的投影里是否也已支持，留给下一轮用上面那个 CLI 一句话验证；
  在那之前不要把这条写成"已修"。

## 2026-10-17 13:00 — slot 2

**结论**：79 探针（+2）· 0 个待研判候选 · Tier-A 四族逐条复现（零 diff）· 健康闸门 7/7 ·
**用 main 版 oracle 把上一轮两个悬案问清楚了** · 新增一份待发 issue 文本

| 阶段 | 结果 |
|---|---|
| Phase 0 健康闸门 | **7/7 PASS**（fresh clone，build 6m52s，duckdb 1.5.6） |
| Phase 1a 标量 / 聚合 / 窗口 | 391/20 · 191/13 · 0 —— 与基线逐条一致 |
| Phase 1b 语义 | **79 探针** · OK=67 · KNOWN_DIFF=10 · BOTH_ERROR=2 · **0 候选** |
| Phase 4 | +2 探针（`SELECT * EXCLUDE`、`GROUP BY ALL` 的 NULL 组） |
| 上游产物 | 0 个 PR（预算仍留给待点开的 `nth-value-zero-index`）；1 份 issue 文本已备好 |

### Phase 0b — 仍然没有在审的 PR

`gh pr list --author SingletonYe` 只有 #26329（MERGED）；`fix/nth-value-zero-index`（`dcaac3e1`）
仍在等用户点开。今天唯一的 PR 预算继续留给它，本 slot 不另起分支。

### 关键工具突破：**已建好的 slt runner 本身就是 main 版 oracle**

上一轮为了判断"某个现象是不是已经被上游修好了"，去编 `datafusion-cli`，两次都因为 2 核不够而
在 slot 内没编完。这一轮发现：**根本不需要 CLI** —— `df-fork` 里已经编好的 `sqllogictests`
运行器就是一个 main 版 oracle：写一个临时 `.slt` 文件（期望值故意写错），运行它，
diff 里就会打印 **main 的真实结果或真实报错**。

```console
$ ../../target/debug/deps/sqllogictests-<hash> test_files/_tmp_oracle_probe.slt
1. query result mismatch:
[SQL] SELECT CAST(2 > ALL (SELECT x FROM (VALUES (1),(0)) t(x)) AS VARCHAR) AS d;
-   PLACEHOLDER_INTENTIONALLY_WRONG
+   true
```

这比读源码或翻 issue 都更硬。**建议写进流程**：以后遇到"这个现象上游修了吗"，先跑这个，几秒钟出答案；
临时 `.slt` 用完即删（本轮已删，`df-fork` 工作区干净）。

### 上一轮两个悬案，现在都有 main 上的实证

1. **`= ANY` / `> ALL` 出现在投影里**：main 上**已经能用**，而且三值逻辑正确 ——
   `2 > ALL (VALUES (1),(0))` → `true`；`1 = ANY (VALUES (NULL),(1))` → `true`（TRUE OR NULL）。
   所以上一轮"pin 落后于 main"的结论成立，现在从"issue 状态推断"升级为"实跑确认"。
   本仓库的 pin 是 55.2.0（2026-10-07 从 branch-55 切出的补丁版本），因此缺少 2026-09-15 进 main 的
   #24972。**结论不变：不提 issue。**
2. **相关性子查询 / `LATERAL` 的非等值关联**：main 上**仍然不支持**，而且报错依旧是内部 Debug 输出：
   ```text
   Physical plan does not support logical expression OuterReferenceColumn(Field { name: "x", data_type: Int64 }, Column { relation: Some(Bare { table: "a" }), name: "x" })
   Physical plan does not support logical expression ScalarSubquery(<subquery>)
   ```
   等值关联的同一形状在 main 上是**能用**的，所以限制点是"非等值关联"而不是 `LATERAL` 本身。
   行为缺口已由 #21201 / #5492 / #25789 追踪，**但"用户看到的是内部类型名"这件事没人提过**
   （搜 `"Physical plan does not support logical expression"`、`"unsupported logical expression"` 均无
   对应 issue）。源码在 `datafusion/physical-expr/src/planner.rs:541,703`（55.2.0 是 `:540,692`），
   就是 `not_impl_err!("… {e:?}")` 里的 `{:?}` —— 上游注释自己都写明了是"没被改写成 join 的
   相关子查询，或没被注册的非相关子查询"，完全可以换成一句 SQL 语言能懂的话。

   因此备好一份 **issue 文本**：`findings/physical-plan-not-supported-message.md`（只请改善信息，
   并明确链接既有的追踪 issue，避免被当成重复）。**是否发由用户决定。**

### 另外两条被"先查上游"拦下来的

* **`GROUP BY ALL` / `DISTINCT ON` 与 DuckDB 不一致**：DataFusion 把 NULL 组算作一个组（正确），
  DuckDB 在 `SELECT g, COUNT(*) … GROUP BY ALL` 里把 NULL 组丢了。**是 DuckDB 的行为**，不是我们的问题。
  用 `COALESCE(g,-1)` 写成探针后两个引擎一致（新探针 `group_by_all_keeps_null_group`）；
  `why` 里特意写了这个坑，免得以后有人照搬裸 `g` 的形式并得到假候选。
* **`SELECT * EXCLUDE (col)`** 两边一致，已作为新探针 `select_exclude_column` 固定下来。

### 手工补的 Tier-A 盲区（负结果，值得记）

`df_robust.py` 的 profile 是"同一个值套到所有参数位"，所以**类型正确但极端的组合**
（第 1 位是合法字符串、第 2 位是 i64::MAX）从来不会被生成 —— 而"申请巨量内存"的那一类 bug
恰好长这个样子。手动补测了最高危的三个，**全部干净报错，无 panic、无 OOM**：

```console
$ lakewatch sql "SELECT repeat('a', 9223372036854775807)"
Execution error: string size overflow on repeat, max size is 2147483647, but got 9223372036854775807
$ lakewatch sql "SELECT lpad('a', 9223372036854775807, 'b')"
Execution error: lpad requested length 9223372036854775807 too large, maximum allowed length is 2147483647
$ lakewatch sql "SELECT CAST(1e308 * 10 AS BIGINT)"     -- inf -> int
Arrow error: Cast error: Can't cast value inf to type Int64
```

（`inf`/`NaN` 转整数是扫描器根本摇不到的一类：`float_max` profile 是 1e308，有限。
两条都是干净的 Cast error，不是 panic。）

**下一步建议（留给 09:00 那个 slot，不要中途改）**：给 `df_robust.py` 增加 2–3 个
"异质极端" profile（如 `["'abc'", "9223372036854775807"]`、`["9223372036854775807", "'abc'"]`），
让这类组合对整个多参函数族生效。它会让扫描变慢约三分之一，并且**会改变 391/20 的基线**，
所以应当和新基线一起在同一轮里落定，而不是在 13:00 这个 slot 悄悄改掉。

### 邮件

**未发**：13:00 的规则是只在提了上游产物 / 准备了待点开的分支 / 运行失败时发。本 slot 准备的是一份
issue 文本（不在触发条件里），也没有新分支，所以不发；21:00 的日报会带上它。

## 2026-10-17 17:00 — slot 3

**结论**：80 探针（+1）· 0 个待研判候选 · Tier-A 四族逐条复现（零 diff）· 健康闸门 7/7 ·
新把 **一个仍在 main 上复现的已知上游 bug** 钉进目录（附一条很强的旁证）

| 阶段 | 结果 |
|---|---|
| Phase 0 健康闸门 | **7/7 PASS**（fresh clone，build 6m24s，duckdb 1.5.6） |
| Phase 1a 标量 / 聚合 / 窗口 | 391/20 · 191/13 · 0 —— 与基线逐条一致（`reports/df-robust*.json` 零 diff） |
| Phase 1b 语义 | **80 探针** · OK=67 · KNOWN_DIFF=11 · BOTH_ERROR=2 · **0 候选** |
| Phase 4 | +1 探针（`PARTITION BY` 的有符号零） |
| 上游产物 | 0（PR 预算仍留给待点开的 `nth-value-zero-index`） |

### Phase 0b — 仍然没有在审的 PR

`gh pr list --author SingletonYe` 只有 #26329（MERGED），`fix/nth-value-zero-index`（`dcaac3e1`）
继续等用户点开，已经等了约 14 小时。今天的时间片不再动这个分支，也不新起分支。

### 本 slot 的产出：把"同一个引擎自己不一致"这件事钉死

给有符号零找新角度时撞到一个**同引擎内部自相矛盾**的形状：

```sql
SELECT x, COUNT(*) OVER (PARTITION BY x) AS n FROM (VALUES (0.0),(-0.0),(1.0)) t(x) ORDER BY n, x;
-- DataFusion: -0.0,1 | 0.0,1 | 1.0,1     ← 0.0 与 -0.0 被分到两个分区
SELECT x, COUNT(*) AS n FROM (VALUES (0.0),(-0.0),(1.0)) t(x) GROUP BY x;
-- DataFusion: 0.0,2 | 1.0,1              ← 同一个引擎的 GROUP BY 又把它们合在一起
```

DuckDB 两种写法都合并（`n = 2`）。也就是说：**DataFusion 自己的 `GROUP BY` 与
`PARTITION BY` 对"0.0 = -0.0"给出了相反的回答**，而窗口那侧与 DuckDB 不一致。
这不是新 bug —— 对应上游 **apache/datafusion#25766**（"Window PARTITION BY puts 0.0 and -0.0 in
separate partitions"，2026-09-26 开，至今 OPEN）。本轮用 slt oracle 在 **main 上实跑确认它仍未修**
（同样返回三个 n=1），因此按 `known_difference` 收进目录：既避免以后重复研判，也在上游修好时
第一时间可见。`why` 里写明了那条"与自己的 GROUP BY 不对称"的旁证 —— 这是判断"哪一侧是错的"
最有力的证据。

### 顺手排掉的几个方向（都不构成 bug）

* **`MERGE` / `DELETE` / `UPDATE` 在 lakewatch 的表上报 NOT_IMPL**：这些是只读的 listing table
  （CSV），不是引擎缺功能；`INSERT` 的报错还带了"用 StreamTable / CREATE UNBOUNDED EXTERNAL TABLE"
  的提示，信息很清楚。`{e:?}` 那种内部 Debug 泄漏问题在上一轮已经单独记录。
* **`GROUP BY ()`（空分组集）报 `This feature is not implemented: Empty tuple not supported yet`**：
  SQL 标准里有这个写法，DuckDB 也支持，属于一个**很小的缺口**，但报错干净、没有用户可误解之处，
  而且 `GROUPING SETS ((g),())` 里的空集是能用的 —— 按 NOT_IMPL 的研判口径（"真缺口 or 文档化限制"）
  记为已知限制，**不提 issue**（避免为这种边缘写法占用维护者时间）。
* **`GROUPING SETS` 里重复集合**按标准保留重复行 ✓；`regexp_replace` 的 `'g'` 标志与 `$2$1`
  反向引用都正常 ✓。

### 邮件

**未发**：17:00 的规则是只在提了上游产物 / 准备了待点开的分支 / 运行失败时发。本 slot 是
"把已知上游 bug 钉进目录"，三者都不是。21:00 的日报会把今天四段合起来发。

### 补记（同一 slot 的后半段）：又补两个空白，当日探针预算用满（3+2+1+2 = 8）

* `is_distinct_from_three_valued`：`IS DISTINCT FROM` / `IS NOT DISTINCT FROM` 的四值组合
  （NULL/NULL、1/NULL、1/1）。这是目录里**唯一没有覆盖的比较运算符**，而且它恰好是"结果不依赖三值逻辑"
  的那个 —— 实现常常复用 `=`/`<>` 的代码路径，把 NULL/NULL 判成 NULL。DuckDB 逐列一致。
* `date_bin_epoch_anchored_bins`：五列一次把 `date_bin` 的分桶规则钉全 —— 桶内向下取整、
  **正好落在边界上不动**（防 off-by-one）、显式 origin 会把网格整体平移（`12:29:59` 归到 `11:30`
  而不是 `12:30`）、不给 origin 时按 **1970-01-01 纪元**对齐而不是按年/按行
  （7 天桶下 `2024-01-05` 归到 `2024-01-04`，即纪元之后第 2818 个整周）。这四条里任何一条变了都会
  让期望值失败。DuckDB 没有 `date_bin`（它的 `time_bucket` 连 origin 语义都不同），所以这条用
  pinned rows 当规范，并加 `known_difference` 说明"DuckDB 报函数不存在"不该被当成 DataFusion 的 bug。

两条都通过；当日语义目录 **82 探针 · OK=68 · KNOWN_DIFF=12 · BOTH_ERROR=2 · 0 候选**。
当日探针新增数达到上限 8 个，本日其余时间片不再新增探针。

## 2026-10-17 21:00 — slot 4 (last slot: daily digest)

**结论**：82 探针 · **0 个待研判候选** · 当日四段基线逐条复现（零 diff）· 健康闸门 7/7 ·
上游：1 个已合并 PR + 1 个已验证待点开的分支 + 2 份备好的 issue 文本

| 阶段 | 结果 |
|---|---|
| Phase 0 健康闸门 | **7/7 PASS**（fresh clone，build 6m23s，duckdb 1.5.6） |
| Phase 1a 标量 / 聚合 / 窗口 | 391/20 · 191/13 · 0 —— 与基线逐条一致 |
| Phase 1b 语义 | **82 探针** · OK=68 · KNOWN_DIFF=12 · BOTH_ERROR=2 · **0 候选** |

### 今天四段合起来

| 时间片 | 健康闸门 | 标量 / 聚合 / 窗口 | 探针 | 实质产出 |
|---|---|---|---|---|
| 01:30（手动） | 未完成（见下） | — | — | 抢救并推送 10-16 被中断的记录；**记录 #26329 已合并**（本 workflow 第一个上游合并）；做出 `nth_value(x,0)` 修复、slt 全绿后推送分支 |
| 09:00 slot 1 | 7/7 | 391/20 · 191/13 · 0 | 74→77 | 实测确认 pinned 55.2.0 落后于 main；记录 harness 把 ±inf/NaN 压成 null |
| 13:00 slot 2 | 7/7 | 同左 | 77→79 | **slt runner 当 main oracle**；实测 `= ANY`/`> ALL` 在 main 已修、`LATERAL` 非等值仍不支持；备好报错信息 issue 文本 |
| 17:00 slot 3 | 7/7 | 同左 | 79→82 | 钉住 #25766（有符号零的 `PARTITION BY`）并附"与自身 GROUP BY 矛盾"的证据 |
| 21:00 slot 4 | 7/7 | 同左 | 82 | 日报 |

全天：探针 **74 → 82**（新增 8 个 = 每日上限），**0 个待研判候选**；上游产出 =
**1 个被合并的 PR**（#26329，36 小时闭环）、**1 个已验证待点开的分支**（`nth-value-zero-index`，
`dcaac3e1`）、**2 份备好的 issue 文本**（递归 CTE 的内部错误；物理计划报错泄漏内部类型名）。

### Next

* **明天 09:00 第一件事**：看 `nth-value-zero-index` 分支有没有变成 PR。点开了就进维护模式
  （读 CI、按 review 改、绝不 force-push）；还没点就继续把当天唯一的 PR 预算留给它。
* 09:00 那一段可以做 `df_robust.py` 的"异质极端" profile（理由已记在 13:00 段：会让扫描变慢、
  且**会改变 391/20 的基线**，必须与新基线在同一轮里落定）。
* 两份 issue 文本仍在 `findings/` 等用户代发；`= ANY`/`> ALL` 那个 pin 落后问题已确认不必提。
* 02 上仍开着的相关项：#25996（map_extract 内部错误，等它合并后再定我们那条 plan-error 种类的问题）。

### 邮件

21:00 **无条件是当日汇总日报**，已发（key `lakewatch-digest:2026-10-17`）。

## 2026-10-18 09:00 — slot 1 (first slot of the day)

**结论**：85 探针（+3）· 0 个待研判候选 · 基线逐条复现（零 diff）· 健康闸门 7/7 · 本 slot 无上游产物

| 阶段 | 结果 |
|---|---|
| Phase 0 健康闸门 | **7/7 PASS**（fresh clone，build 6m01s，duckdb 1.5.6） |
| Phase 1a 标量 / 聚合 / 窗口 | 391/20 · 191/13 · 0 —— 与基线逐条一致 |
| Phase 1b 语义 | **85 探针** · OK=71 · KNOWN_DIFF=12 · BOTH_ERROR=2 · **0 候选** |

### Phase 0b — 分支仍未点开，预算继续留着

`gh pr list --author SingletonYe --state open` 为空；`fix/nth-value-zero-index`（`dcaac3e1`）已就绪约
26 小时。今天唯一的 PR 预算继续留给它 —— 不新起分支、不催、不重推。

### Phase 4 — 补窗口函数的三个空白（三条都带 pinned 期望，无 oracle 也能跑）

1. `percent_rank_cume_dist_with_ties`：两个公式在**有并列值**时的细节最容易写错 ——
   `PERCENT_RANK` 用 *rank* 而不是 row_number（并列的两行共享 1/3，不是 0/1），
   `CUME_DIST` 把并列一起算进 `<= 当前`（共享 3/4，不是 2/4）；同时钉住 rank 不压缩（v=4 是 1.0）。
2. `window_groups_frame_peers`：`GROUPS` 是三种帧模式里唯一没覆盖的，它按 **peer 组**计数而不是按行 ——
   用 ROWS 语义会把中段算成 3、末段算成 8 的前半，而正确答案是 5 与 8。
3. `window_aggregate_with_filter`：窗口聚合上的 `FILTER`，第一行帧为空时 `SUM` 必须是 **NULL**（三值逻辑）
   而不是 0 —— 一次同时抓住"忽略 filter"和"空帧给 0"两种实现错误。

三条都与 DuckDB 逐行一致。当日探针预算已用 3 个。

### 邮件

**未发**：09:00 的规则是只在提了上游产物 / 准备了待点开的分支 / 运行失败时发；本 slot 只增长目录。

## 2026-10-18 13:00 — slot 2

**结论**：85 探针 · 0 个待研判候选 · 基线逐条复现 · 健康闸门 7/7 · 目录卫生：清掉一处过时的上游引用

| 阶段 | 结果 |
|---|---|
| Phase 0 健康闸门 | **7/7 PASS**（fresh clone，build 6m24s，duckdb 1.5.6） |
| Phase 1a 标量 / 聚合 / 窗口 | 391/20 · 191/13 · 0 —— 与基线逐条一致 |
| Phase 1b 语义 | 85 探针 · OK=71 · KNOWN_DIFF=12 · BOTH_ERROR=2 · **0 候选** |

### Phase 0b — 分支仍未点开（约 32 小时）

`--state open` 为空。今天唯一的 PR 预算继续留给 `nth-value-zero-index`，不新起分支、不催、不重推。

### 目录卫生：把"revisit when X lands"真的查了一遍

`known_difference` 里一共引用了 **8 个上游 issue**，逐个查状态：7 个仍 OPEN
（#11886、#24950、#23929、#7301、#12955、#12956、#25766），只有 **#24432 已 CLOSED（completed，
2026-08-20）**。

而 `signed_zero_ordering` 的备注把它写成"#24432 主动选择了 total ordering" —— 读起来像一个仍在追踪的
设计依据。事实相反：#24432 是**缺陷修复**（分组的 MIN/MAX 累加器用 `partial_cmp`，导致结果依赖行序、
且对 ±0 保留先到者），修法正是让分组累加器也改用 total ordering。备注已改为准确表述：total ordering
是有意设计，#24432 把分组累加器统一到它；DuckDB 全程用 IEEE。

探针本身行为未变（仍 OK），改的只是记录 —— 但这条很关键：如果备注继续把"已修的 bug"写成"设计选择"，
下一个 slot 可能会拿它当理由放过一次真实回归。

### 邮件

**未发**：13:00 的规则是只在提了上游产物 / 准备了待点开的分支 / 运行失败时发；本 slot 是基线复现 + 记录修正。

## 2026-10-19 09:00 — 首次按"每天 09:00"运行

**结论**：88 探针（+3）· 0 个待研判候选 · 基线逐条复现 · 健康闸门 7/7 · 排程自检通过

| 阶段 | 结果 |
|---|---|
| Phase 0 健康闸门 | **7/7 PASS**（fresh clone，build 6m20s，duckdb 1.5.6） |
| Phase 1a 标量 / 聚合 / 窗口 | 391/20 · 191/13 · 0 —— 与基线逐条一致（报告文件零 diff） |
| Phase 1b 语义 | **88 探针** · OK=74 · KNOWN_DIFF=12 · BOTH_ERROR=2 · **0 候选** |

### 两条新的自检项（本日流程第一次执行）

* **连续性**：上一节是 `2026-10-18 13:00`，即昨天，无断档。需要说明的是 10-18 只跑了 09:00 与
  13:00 两段 —— 当天 13:30 排程先被改成"每 3 天"、随后改成"每天 09:00"，17:00 与 21:00 两段因此
  没有触发。从今天起固定每天 09:00 一轮。
* **排程自检**：自动化 `enabled`，表达式 `0 9 * * *`（America/New_York）✓。运行中 `Next run` 显示
  `-` 属于已知显示行为（10-14 首次观察到），不作为故障；自检规则已按这一点写清楚，避免以后误重建。

### Phase 4 — 三个新探针（当日累计 3 个）

1. `extract_epoch_units`：`EXTRACT(EPOCH …)` 对 TIMESTAMP 与 DATE 必须给同值 1704067200.0 ——
   毫秒实现、本地时区实现、或 DATE 路径差一天，都会偏离。
2. `substring_standard_syntax`：`SUBSTRING(x FROM n FOR m)` 是独立于 `substring(x,n,m)` 的解析路径，
   且无 FOR 时必须取到串尾。
3. `grouping_sets_with_filter`：`FILTER` 与 `GROUPING SETS` 组合 —— 过滤须在聚合前逐行生效，
   总计只数被过滤后的行（8/12），不是逐组各算。

三条都与 DuckDB 逐行一致。

### 研判：不支持语法的报错把内部结构直接打印给用户

`MATCH_RECOGNIZE`、`PIVOT` 在 DataFusion 里是**已知功能缺口**（#13583、#12907 追踪），但用户收到的
报错是整个 sqlparser AST（`PIVOT` 那次约 2KB）：

```text
This feature is not implemented: Unsupported ast node MatchRecognize { table: Table { name: ObjectName([Identifier(Ident { value: "probe_values", … })]) … } } in create_relation
```

源码是 `datafusion/sql/src/relation/mod.rs:310` 与 `datafusion/sql/src/expr/mod.rs:842` 的
`not_impl_err!("… {relation:?}")`，与上一轮记录的物理计划层 `physical-expr/src/planner.rs:541,703`
属同一类问题。因此把上一轮备好的 issue 文本升级为**"一类问题、两层来源"**的版本
（`findings/unsupported-feature-errors-dump-internals.md`，含四处独立可修的源码位置），
并删除被取代的旧文本 —— 一份 issue 比两份更容易被接受。

### 邮件

已按新规则发送当日报告（每次运行一封，key `lakewatch-report:2026-10-19`）。

## 2026-10-20 09:00 — 首次撞到 `main` 上的 INTERNAL 回归

**结论**：91 探针（+3）· 0 个待研判候选 · 健康闸门 7/7 · **1 个新的 INTERNAL 级 bug（`main` 专属）**

| 阶段 | 结果 |
|---|---|
| Phase 0 健康闸门 | **7/7 PASS**（沙箱沿用昨日的 clone，与 `origin/main` 零差异、工作区干净；warm build 后全流程 4s） |
| Phase 1a 标量 / 聚合 / 窗口 | 391/20（与基线**逐字节相同**，`reports/df-robust.json` 零 diff） |
| Phase 1b 语义 | **91 探针** · OK=77 · KNOWN_DIFF=12 · BOTH_ERROR=2 · **0 候选** |

### 两条自检

* **连续性**：上一节是 `2026-10-19 09:00`，即昨天，无断档。
* **排程自检**：`okou workflow automation list 3740d7fe-…` 显示 `enabled`，表达式 `0 9 * * *`（America/New_York）✓。
  运行中 `Next run` 显示 `-` 属已知显示行为。**注意**：技能里写的 `okou workflow trigger show <name>` 现在报
  `400: id: Invalid UUID` —— 该命令只收 automation UUID，正确用法是
  `okou workflow automation list <workflow-id|workflow-name>`。这也是技能文本需要修的地方（下次改）。
* 顺带发现第二条 automation（`595ffcb7-…`，Gmail 通知触发器）的事件过滤写死了已合并的 PR 号 `26329`，
  所以对**新** PR 的通知不会触发 PR-response run。本条未改动（属工作流配置变更，且当前无 open PR），
  留给用户决定是否放宽过滤。

### Phase 2 — 逐条复核目录引用的上游 issue（技能第 7 步）

14 个被引用的 issue/PR 全部复核：`#11886 #12955 #12956 #13850 #7301 #23929 #24950 #25766 #20501 #19004`
仍是 open，`#24432`（closed/completed）与 `#12907`（open）与备注一致，`#21032`/`#26329` 状态不变。
**只有一处过期**：`#13583`（MATCH_RECOGNIZE）已于 2026-08-11 以 *completed* 关闭，但关闭理由是
"不进 core，走 planning-extension 用扩展实现"，不是功能落地。已把
`findings/unsupported-feature-errors-dump-internals.md` 里的引用改成准确表述。

### 新发现：`> ALL` 在**无别名投影**里触发优化器不变量断言（INTERNAL，`main` 专属）

```sql
SELECT 2 > ALL (SELECT x FROM (VALUES (1),(0)) t(x));        -- Internal error: Assertion failed: compatible …
SELECT 2 > ALL (SELECT x FROM (VALUES (1),(0)) t(x)) AS d;   -- true（正常）
```

* 触发面很窄也很机械：**`Expr::SetComparison`（`> ALL`/`< ANY` 等）出现在没有别名的投影里**。
  加别名就好；`EXISTS`/`IN` 同位置无事；同样的表达式放 `WHERE` 里也正常；加一列 `9 AS k` 后仍复现。
* 不是 debug-only：`optimizer.rs:686` 的 `assert_valid_optimization` 未被 `debug_assertions` 门控，
  且 `skip_failed_rules` 默认为 `false`，所以**用户会直接吃到"请提 bug report"的内部错误**。
* 版本归属：pin 的 55.2.0 复现不出来（那里是同一条 `Physical plan does not support … Exists` 消息），
  差异来自 apache/datafusion#24972（2026-09-15 合并，*Support predicate subqueries in projections*）——
  即这是**尚未发布的 main 回归**。
* 失败点是 `assert_expected_schema`（`invariants.rs:114`）比较"名字 + 类型"，这里只有名字不同
  （`Int64(2) > ALL` vs 改写后的 `CASE WHEN EXISTS …`）；规则的投影分支本会保留原名
  （`decorrelate_predicate_subquery.rs:71–93`），所以线索就在这条 SetComparison 路径上。
* 上游先搜后写（`decorrelate_predicate_subquery`、`Check optimizer-specific invariants`、
  `difference in schemas`、`SetComparison`、`> ALL`、`24972`，open+closed）**无重复**。
  已备好可直接粘贴的 issue 文本：`findings/set-comparison-projection-internal-error.md`。

**另有两条按"记录而非提 issue"处理的（避免重复）：**

* `INTERVAL * n` / `INTERVAL / n` 不支持（DuckDB、PostgreSQL 支持；缺口由 #13850 追踪，故不提 issue），
  但报错把用户没写过的操作数打了出来：`Interval(MonthDayNano) * Int64` → 消息却是
  `Interval(MonthDayNano) * Interval(MonthDayNano)`（`expr-common/src/type_coercion/binary.rs:322`
  格式化的是 coercion 之后的 `{coerced}`）。已作为"同类"附录并入上文那份报错 issue 文本。
* `LIKE … ESCAPE '!'`（显式 ESCAPE 子句）被拒绝：`LIKE does not support escape_char other than the
  backslash (\)`；`SIMILAR TO … ESCAPE` 是 `does not support escape_char yet`
  （`physical-expr/src/planner.rs:293,324`）。标准允许任意单字符 ESCAPE，DuckDB 支持。与 #13291
  （"没有隐式转义符的 LIKE"）同一族但不同点，未搜到已提 issue；**若用户愿意，可作为独立小 issue**，
  本轮不铺开：报错本身清晰、不误导，优先级低于上面那条。

### Phase 4 — 三个新探针（当日累计 3 个）

1. `interval_month_end_clamp`：`DATE '2024-01-31' + INTERVAL '1' MONTH` 必须夹到月末 2024-02-29
   （不是 3 月 2 日，也不是报错），1-30 加一月同理，3-31 减一月同值。`CAST(… AS DATE)` 让
   `date_add_year_leap` 里记录的 DATE/TIMESTAMP 类型分歧不会掩盖规则本身。
2. `position_in_standard_syntax`：标准 `POSITION(x IN y)` 是独立解析路径，且必须是 1 起算、
   未找到返回 **0**（不是 strpos 形状的 -1），空 needle 命中 1。
3. `window_range_offset_frame`：`RANGE BETWEEN 2 PRECEDING AND CURRENT ROW` 的边界是**值**语义且含
   peers —— x=2 的两行都得 5，x=3 回看到 1 得 8；按 ROWS 实现会给出 1,2,4,6。

三条都与 DuckDB 逐行一致、均带 `expect_rows`。**88 → 91。**

### Next

1. **等用户一步**：`fix/nth-value-zero-index` 分支已推、无 open PR，只差点开 compare 链接（邮件里已置顶）。
   上游 `main` 的 `nth_value.rs` 至今仍带那两处断言（2026-10-15 后无人改动），所以这个修复仍然有效。
2. **两份待发 issue 文本**：`findings/set-comparison-projection-internal-error.md`（新，INTERNAL，建议优先）、
   `findings/unsupported-feature-errors-dump-internals.md`（报错信息类，今日已修正 MATCH_RECOGNIZE 复现与 #13583 状态）。
3. 下次复核 `#13583` 一类"关闭理由 ≠ 功能落地"的引用，别再拿 closed 当"已支持"。

## 2026-10-21 09:00 — 修好了昨天那个 INTERNAL 回归，分支已推

**结论**：94 探针（+3）· 0 个待研判候选 · 健康闸门 7/7 · **1 个修复分支已推（待点开 PR）**

| 阶段 | 结果 |
|---|---|
| Phase 0 健康闸门 | **7/7 PASS**（clone 与 `origin/main` 零差异，工作区干净） |
| Phase 1a 标量 / 聚合 / 窗口 | 391/20，与基线**逐字节相同** |
| Phase 1b 语义 | **94 探针** · OK=80 · KNOWN_DIFF=12 · BOTH_ERROR=2 · **0 候选** |

### 两条自检

* **连续性**：上一节 `2026-10-20 09:00`，即昨天，无断档。
* **排程自检**：`okou workflow automation list lakewatch-datafusion-hunt` → `enabled`、`0 9 * * *`
  （America/New_York）✓。（昨天写进技能的修正命令今天直接可用，不再报 `Invalid UUID`。）

### Phase 0b — 无 open PR，把昨天的新发现做成了修复

`gh pr list --author SingletonYe --state open` 为空；`fix/nth-value-zero-index` 仍在等用户点开。
所以今天的 PR 名额用在昨天新发现的 INTERNAL 回归上。

**根因（读源码 + 实跑确认）**：`RewriteSetComparison` 用 `LogicalPlan::map_expressions` 把
`x > ALL (…)` 换成 `CASE WHEN EXISTS …`；`map_expressions` 对 Projection **保留旧 schema**（这正是
不变量所要求的），于是留下一个"表达式名 ≠ 字段名"的计划。随后
`DecorrelatePredicateSubquery` 从表达式重建投影，用 `Expr::schema_name()` 当字段名 —— 但那是
*过期* 的名字（`CASE WHEN EXISTS …`），而计划 schema 里写的是 `Int64(2) > ALL`，
`assert_valid_optimization` 于是判 schema 变了 → INTERNAL。`skip_failed_rules` 默认 false，用户直接看到。

**修复**：字段名以投影自己的 schema 为准；比较时用"重建后该表达式实际会得到的名字"
（column 取裸名、alias 取别名、其余取 display name），只在真的会变时才补 alias。

**一次踩坑值得记下来**：第一版只把 `original_name` 换成 schema 字段名，结果
`subquery.slt` 里 10 个既有用例挂了 —— 因为 `Expr::schema_name()` 对限定列返回 `t.k`，而字段名是
`k`，于是给限定列也补了 alias、**丢了 qualifier**，从另一个方向违反同一个不变量。
（`gs_*`/`oj_*` 那批用例正是这么发现的。）改成上面的比较方式后两侧都过。

**验证**（在 `apache/datafusion@main 5f21582` 上重编译，19m04s）：
`SELECT 2 > ALL (…)` 无别名 / 加括号 / 带第二列 / 带别名 / `1 = ANY (…)` 六种形态全部返回正确值；
`subquery.slt`、`null_aware_mark_join.slt`、`prepare.slt` 全绿；`cargo test -p datafusion-optimizer`
941+29+5 全过；`cargo fmt --all --check` 干净。

**分支**：`SingletonYe/datafusion@fix/set-comparison-projection-field-name`（`0973cfa`，基于 main 一个提交）。
PR 文本：`findings/set-comparison-projection-field-name-pr-body.md`。
沙箱事实：这轮 2 核 + 磁盘吃紧 —— `target/debug/incremental` 涨到 9.1G，`cargo test -p datafusion-optimizer`
直接 ENOSPC；删掉 incremental 后正常。下次做上游构建前先看这一项。

### Phase 4 — 三个新探针（当日累计 3 个）

1. `aggregate_order_by_clause`：聚合内部的 `ORDER BY`（`string_agg(v, ',' ORDER BY v DESC)`）必须
   c,b,a；忽略内层排序或聚合后再排序会给 a,b,c。
2. `window_null_order`：窗口 `ORDER BY` 的默认空值位置（ASC 下 NULLS LAST）是独立路径，
   NULL 必须拿到 rn=3。
3. `having_aggregate_not_in_select`：`HAVING` 可以过滤投影里没选的聚合（`SUM(v)`），只留一组、n=2。

三条都与 DuckDB 逐行一致、均带 `expect_rows`。**91 → 94。**

### Next

1. **等用户一步（两个链接，都已在邮件里）**：
   - 新：`fix/set-comparison-projection-field-name` → 点开即 PR（附 PR 文本）。
   - 旧：`fix/nth-value-zero-index` → 同上（上游 `nth_value.rs` 至今未改，修复仍有效）。
2. 待发 issue 文本两份：`set-comparison-projection-internal-error.md`（与新 PR 配套）、
   `unsupported-feature-errors-dump-internals.md`。
3. 上游 issue 复核：14 条引用全部与备注一致，本轮无过期项（`#24972` 已 merged、`#13850`/`#13291` 仍 open）。

## 2026-10-22 09:00 — 主线前移，两个待点分支都复核过了

**结论**：97 探针（+3）· 0 个待研判候选 · 健康闸门 7/7 · 修复分支与今日 main **可干净合并**（端到端验证未能在本沙箱完成，见下）

| 阶段 | 结果 |
|---|---|
| Phase 0 健康闸门 | **7/7 PASS** |
| Phase 1a 标量 / 聚合 / 窗口 | 391/20，与基线**逐字节相同** |
| Phase 1b 语义 | **97 探针** · OK=83 · KNOWN_DIFF=12 · BOTH_ERROR=2 · **0 候选** |

**自检**：连续性（上一节 `2026-10-21`，无断档）；排程 `enabled`、`0 9 * * *` ✓。

### Phase 0b — 仍无 open PR，改为复核两个待点分支是否还有效

用户还没点开任何 compare 链接（`gh pr list --state open` 为空），所以本轮不新起分支（第三个待点链接只会变成噪音），
而是回答一个问题：**这两个分支今天点开还有意义吗？**

| 分支 | 上游对应文件自 2026-10-16 起 | 结论 |
|---|---|---|
| `fix/nth-value-zero-index` | `functions-aggregate/src/nth_value.rs` **零改动** | 两处 `assert_or_internal_err!` 仍在，修复仍有效 |
| `fix/set-comparison-projection-field-name` | `optimizer/src/decorrelate_predicate_subquery.rs` 被 #25391 改过（+240/−14）；`rewrite_set_comparison.rs` **零改动** | 见下 |

`set-comparison` 那个分支的细节：

* 我改的那段（投影重建、名字判定）在今日 main 上**逐字节相同**（`Expr::schema_name()` 仍在用），
  `RewriteSetComparison` 也没动 —— 所以**这个 INTERNAL 仍然能复现**，修复没有过时。
* 把修复 commit `0973cfa` **cherry-pick 到今日 main（`2717e25`）上自动合并成功**（两个文件都 auto-merge、无冲突），
  即用户点开 compare 链接会得到一个可合并的 PR。
* #25391 改的是 `build_join` / mark-join 构造与相关子查询重写，**与我这段是正交的**；两者不冲突。

**必须如实说明的一点**：我**没有**在今日 main 上跑完整验证。原因是磁盘：main 这轮把 arrow 升到 60，
重编译把所有 arrow / datafusion crate 都拖下水，`cargo test -p datafusion-sqllogictest --no-run` 在
`datafusion-functions-nested` 处 **ENOSPC**（磁盘 24G，df-fork target 已 20G；删掉 4.0G 的 `incremental` 后
仍需再腾 1.4G 以上才敢重链那个 1.4G 的 slt 二进制，而那会毁掉下一个 run 的 oracle）。
所以今日的结论边界是：**代码路径未变 + cherry-pick 干净**，端到端（含 #25391 交互）的第一次全量验证
会是 PR 上的 CI。上一轮该修复在 `5f21582` 上是逐条实跑验证过的（六种形态 + 三个 slt 文件 + optimizer 单测 975 项）。

### Phase 2 第 7 步 — 上游 issue 复核

12 条被引用的 issue 全部与备注一致（`#11886 #23929 #7301 #24950 #12955 #12956 #25766 #20501 #19004 #12907 #13291 #13850` 均 open）。
另外按错误文本搜了一遍**有没有别人先报了 `set-comparison` 那个不变量失败**（`difference in schemas decorrelate`、
`predicate subquery projection`、`SetComparison`）：**无**。

### Phase 4 — 三个新探针（当日累计 3 个）

1. `date_trunc_week_start`：`DATE_TRUNC('week', …)` 必须锚在**周一**（2024-01-03 → 2024-01-01，周一自身不变）。
2. `lpad_rpad_truncation`：长度不足要截断（abc→ab）、长度更长要**循环**填充（abc+xy→abcxy）、lpad 从左侧补。
3. `case_without_else_is_null`：无 ELSE 的 `CASE` 对不匹配行是 **NULL**：`SUM` 跳过得 1、`COUNT(*)` 仍 3、
   `COUNT(expr)` 只 1。把它改写成 `ELSE 0` 会保住 SUM 却把 `COUNT(expr)` 变成 3。

三条都与 DuckDB 逐行一致、均带 `expect_rows`。**94 → 97。**

### 沙箱事实（写给下一个 run）

* 磁盘 24G 是这活儿的硬约束。只做 sweep 没问题（lakewatch target 2G + 现有 df-fork target）。
  一旦要**在移动过的 main 上重编译**（依赖升级时尤甚），先确认 ≥5G 空闲，否则会在中途 ENOSPC。
* `target/debug/incremental` 是本沙箱最容易回收的一块（昨日 9.1G、今日 4.0G），`rm -rf` 后 cargo 只会变慢。
* `df-fork` 工作区现在停在 `verify-on-main`（今日 main + 修复的 cherry-pick），target 里是**混合指纹**；
  下次要么接着编完，要么 `git checkout fix/set-comparison-projection-field-name` 后按需重编。

### Next

1. **等用户一步（两个链接）**：`fix/set-comparison-projection-field-name`（今日复核：可合并、修复仍对症）、
   `fix/nth-value-zero-index`（上游文件零改动）。PR 文本都在 `findings/*-pr-body.md`。
2. 待发 issue：`set-comparison-projection-internal-error.md`、`unsupported-feature-errors-dump-internals.md`。
3. 若用户点开 PR：**先跑 CI 结果**，再谈别的 —— 那是今日 main 上的第一次端到端验证。
