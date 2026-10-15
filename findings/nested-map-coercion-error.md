# Finding: a non-map argument to a map function is reported as an internal error

**Status:** re-triaged 2026-10-15 — defect 1 is **already fixed in flight upstream (#25996)**,
so nothing is filed for it; defect 2 is verified, still unfixed, and deliberately not filed yet
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

## Proposed fix — small, one PR (now blocked, see the re-triage below)

- `functions-nested/src/utils.rs:341`: `internal_err!` → a user-facing/plan error.
  **Already done upstream by #25996 — do not redo it.**
- `type_coercion/functions.rs:541`: `exec_err!` → `plan_err!`, since this happens during
  coercion. **Still open upstream as of 2026-10-15.**
- One unit test mirroring the map coercion failure. **Grep the whole repo for the
  user-visible text first** (`datafusion/sqllogictest/**` hard-codes error strings); the
  first attempt at this class of fix nearly shipped with ten un-updated `.slt` expectations.

## Why this is not folded into #26329

Different call site, different reasoning, and #21032 died of scope creep. The daily upstream
cap is one PR, and #26329 is still open and awaiting a maintainer.


## Re-triage, 2026-10-15 (09:00 slot): one defect is already in flight, do not duplicate it

**`apache/datafusion#25996` — "fix: report map_extract on a non-map argument as a plan
error"** (jackylee-ch, opened 2026-10-03, still **OPEN**, last touched 2026-10-04) changes
exactly the line this finding blames: `get_map_entry_field` returns a plan error instead of
`internal_err!` for a non-map argument, with `map.slt` cases for `element_at(1, 1)` and
`map_extract(1, 1)`. Found with `gh search prs --repo apache/datafusion "map_extract"`.

So **defect 1 must not be filed** and the second PR sketched below must not be opened: it
would be a duplicate of an open PR by another contributor, which is the one thing the
procedure is most explicit about. The sweep still reports the internal error because the
pinned DataFusion predates that PR; that is not evidence the bug is unfiled.

Two of the three defects are accounted for upstream:

| defect | upstream | status |
|---|---|---|
| 1. `internal_err!` for a non-map argument (+ bug-report boilerplate) | #25996 | open PR, fixes it |
| 3. candidate list names the internal `map_extract(UserDefined)` | #15817 *Improve `candidate functions` suggestions* | open issue |
| 2. a plan-time coercion failure is labelled `Execution error` | — | **verified below, unfiled** |

### Defect 2 is real, is not fixed by #25996, and is not a duplicate

`get_valid_types_with_udf`'s `TypeSignature::UserDefined` arm
(`datafusion/expr/src/type_coercion/functions.rs:542`) wraps *every* error out of
`coerce_types` — including `Plan` errors — in `exec_err!`. Measured on the pinned DataFusion,
with functions whose `coerce_types` already returns a plan error, so the mislabelling is
visible **independently of `map_extract`**:

```console
$ lakewatch sql "SELECT array_add(1, 2)"
Error during planning: Execution error: Function 'array_add' user-defined coercion failed with: Error during planning: array_add does not support type Int64. …

$ lakewatch sql "SELECT cosine_distance([1.0], 'x')"
Error during planning: Execution error: Function 'cosine_distance' user-defined coercion failed with: Error during planning: cosine_distance does not support type Utf8. …
```

A failure that happens while coercing arguments at plan time is reported with the
`Execution error` kind (and the internal phrase "user-defined coercion failed with"). After
#25996 the `map_extract` message becomes the same shape — `Error during planning: Execution
error: Function 'map_extract' user-defined coercion failed with: Error during planning:
Expected a Map type, got Int64.` — so that PR fixes the internal error and the boilerplate but
leaves the wrong error kind. Verified by code path, not by guesswork: the failing arm is the
one `array_add` above demonstrably goes through.

**Decision: do not file today.** It is adjacent to the open #25996 and to the open family
tracking issues #14431 / #14432 / #20501, our own #26329 is open in the same area, and one
upstream artefact at a time is the rule. Revisit once #25996 merges: then either a small
standalone issue (wrong error *kind*, which matters to callers matching on
`DataFusionError::Execution`) or — better — nothing at all, if #20501's cleanup covers it.

### Optional, for the user: a note on #25996

The comment text is in `findings/map-extract-25996-note.md`. It is a review note on an open
PR, so it duplicates nothing; it is optional and the slot run does not depend on it.
