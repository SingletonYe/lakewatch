# Optional note for the user to leave on apache/datafusion#25996

Nothing is filed or posted by us (`apache/*` refuses our writes). This is a review note on an
open PR — it duplicates no issue — so it is optional and carries no dependency.

---

Tested this on `main` with functions that already return a plan error from `coerce_types`
(`array_add(1, 2)`, `cosine_distance([1.0], 'x')`), and the message shape is:

```text
Error during planning: Execution error: Function 'array_add' user-defined coercion failed with: Error during planning: array_add does not support type Int64. …
```

The reason is the `TypeSignature::UserDefined` arm of `get_valid_types_with_udf`
(`datafusion/expr/src/type_coercion/functions.rs:542`), which wraps *any* error from
`coerce_types` — plan errors included — in `exec_err!`. So with this PR applied I think
`map_extract(1, 1)` will read:

```text
Error during planning: Execution error: Function 'map_extract' user-defined coercion failed with: Error during planning: Expected a Map type, got Int64. …
```

The internal error and the bug-report boilerplate are gone, which is the important part, but
the kind is still `Execution` for something that fails during planning. If you want to close
that too, `exec_err!` → `plan_err!` on that arm should do it; otherwise it may be worth a
sentence in the PR description so it is not mistaken for "completely fixed". Not a blocker for
this PR either way — thanks for fixing the internal error.

---

(Not sent. If it is ever sent, do not claim it was sent by the automation.)
