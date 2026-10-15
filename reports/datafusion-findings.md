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
