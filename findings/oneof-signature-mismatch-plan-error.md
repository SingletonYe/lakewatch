# Finding: `OneOf` signature mismatch reported as an internal error

**Status:** fix prepared, branch pushed, PR awaiting one click
**DataFusion:** `apache/datafusion@6023600` (2026-10-14)
**Upstream refs:** #20501 (open, this is part of it), #19004 (open, related), #21032 (closed
unmerged — the same fix attempted as a much larger change and asked to be split up)
**Branch:** `SingletonYe/datafusion:fix/oneof-signature-mismatch-plan-error` @ `1fd13a7`
**PR:** https://github.com/apache/datafusion/compare/main...SingletonYe:fix/oneof-signature-mismatch-plan-error?expand=1

## Symptom

Any call whose arguments match no branch of a `OneOf` signature is reported to the user as
an internal DataFusion bug:

```console
$ lakewatch sql "SELECT substr(1, 2)"
Error during planning: Internal error: Function 'substr' failed to match any signature, errors: Error during planning: Function 'substr' requires String, but received Int64 (DataType: Int64).,Error during planning: Function 'substr' expects 3 arguments but received 2.
This issue was likely caused by a bug in DataFusion's code. Please help us to resolve this by filing a bug report in our issue tracker: https://github.com/apache/datafusion/issues
```

Nothing internal went wrong: the caller passed the wrong argument types or arity.

## Root cause

`datafusion/expr/src/type_coercion/functions.rs`, `get_valid_types_with_udf`: when every
`OneOf` branch fails, the collected errors are returned through `internal_err!` rather than
`plan_err!`. `DataFusionError::Internal` renders with the "likely caused by a bug … file a
bug report" boilerplate; `DataFusionError::Plan` renders as a plain planning error.

## Attribution

A robustness sweep over all 364 scalar functions of the pinned DataFusion
(`scripts/df_robust.py`, 1874 invocations) produced 391 findings in 20 root-cause clusters.
Re-running each cluster's repro and inspecting the full stderr shows **353 of 391 findings
(13 of 20 clusters) come from this single call site**:

`btrim`, `date_part`, `datepart`, `date_trunc`, `datetrunc`, `generate_series`, `log`,
`regexp_like`, `round`, `string_to_list`, `substr`, `substring`, `trim`.

The remaining 38 are `element_at`/`map_extract` (8 + 8, a different internal-error path —
still untriaged) and `NOT_IMPL` gaps (`gcd`/`lcm`, `to_time`/`to_timestamp*`, `arrow_cast`).

## Fix

One line, plus a test and the user-visible expectations that come with it. Deliberately
narrow: the message body, candidate signatures and the "No function matches…" follow-up are
unchanged. Message verbosity and the missing separator (`…#19004`) are left for follow-ups.

## Verification performed

* new unit test fails on `main`, passes with the change;
* `cargo test -p datafusion-expr --lib` → 282 passed, 0 failed;
* `cargo fmt --all --check` clean;
* audited every other assertion on this message across `.rs` and `.slt`;
* ten `sqllogictest` expectations updated (`array/array_range.slt`, `spark/math/round.slt`,
  `errors.slt`, `math.slt`).

Full `sqllogictest` was **not** run locally (needs a full `datafusion-core` build); CI is the
verification for those four files.
