# Finding: a non-map argument to a map function is reported as an internal error

**Status:** triaged, fix not started — waiting for #26329 to clear (one PR at a time)
**DataFusion:** `apache/datafusion@6023600` (2026-10-14)
**Family:** the same class as #26329, at a **different call site**
**Attribution:** 16 of the 391 Tier-A findings (`df_robust.py`, 8 × `element_at`, 8 × `map_extract`)

## Symptom

`SELECT element_at([1, 2, 3], 1)` — or `map_extract(DATE '2024-01-01', DATE '2024-01-01')`:

```text
Error during planning: Execution error: Function 'map_extract' user-defined coercion failed with: Internal error: Expected a Map type, got List(Int64).
This issue was likely caused by a bug in DataFusion's code. Please help us to resolve this by filing a bug report in our issue tracker: https://github.com/apache/datafusion/issues
	Candidate functions:
	map_extract(UserDefined)
```

## Three separate defects in one message

1. **A user mistake is reported as an internal error.** `datafusion/functions-nested/src/utils.rs:341`:
   `_ => internal_err!("Expected a Map type, got {data_type}")`.
2. **A planning-time failure is reported as an execution error.** The wrapper at
   `datafusion/expr/src/type_coercion/functions.rs:541` returns `exec_err!` for the
   `TypeSignature::UserDefined` coercion failure, hence
   `Error during planning: Execution error: ...`.
3. **The candidate list is unhelpful.** It names the internal `map_extract(UserDefined)`
   instead of the function the user called.

## What is *not* a bug

`element_at` really is documented as an alias of `map_extract` in this version
(`information_schema.routines` gives it `syntax_example = "map_extract(map, key)"`); array
access is `array_element` / `array_extract` / `list_element`. So the aliasing is
self-consistent and must not be filed as a regression. It is still surprising to a DuckDB
user, where `element_at` is array access — the catalogue now pins that divergence instead of
re-triaging it every run.

## Proposed fix — small, one PR

- `functions-nested/src/utils.rs:341`: `internal_err!` → a user-facing/plan error.
- `type_coercion/functions.rs:541`: `exec_err!` → `plan_err!`, since this happens during
  coercion.
- One unit test mirroring the map coercion failure. **Grep the whole repo for the
  user-visible text first** (`datafusion/sqllogictest/**` hard-codes error strings); the
  first attempt at this class of fix nearly shipped with ten un-updated `.slt` expectations.

## Why this is not folded into #26329

Different call site, different reasoning, and #21032 died of scope creep. The daily upstream
cap is one PR, and #26329 is still open and awaiting a maintainer.
