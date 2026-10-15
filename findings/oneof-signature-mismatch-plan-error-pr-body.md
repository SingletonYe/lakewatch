## Which issue does this PR close?

Part of #20501.

This is a deliberately narrow slice of that issue, and it is the slice a maintainer previously asked for: #21032 attempted this fix as a large change (it also reworked `sum`/`avg` diagnostics and the public `first_value`/`last_value`/`nth_value` arity contracts), was asked to be split up, and then went stale. This PR is **one behaviour change and nothing else**.

## Rationale for this change

`get_valid_types_with_udf` reports "no `OneOf` branch matched" with `internal_err!`. Every alternative signature failing means the *caller* passed the wrong number or types of arguments — a user mistake — yet the user is told:

> This issue was likely caused by a bug in DataFusion's code. Please help us to resolve this by filing a bug report in our issue tracker: ...

So an ordinary SQL typo invites a bug report, and the failure surfaces as `Internal error: ...` even though nothing internal went wrong. The maintainer comment on #20070 named exactly this ("Can try to fix the display + internal error reference").

### Before / after

`SELECT substr(1, 2)`

Before:

```text
Error during planning: Internal error: Function 'substr' failed to match any signature, errors: Error during planning: Function 'substr' requires String, but received Int64 (DataType: Int64).,Error during planning: Function 'substr' expects 3 arguments but received 2.
This issue was likely caused by a bug in DataFusion's code. Please help us to resolve this by filing a bug report in our issue tracker: https://github.com/apache/datafusion/issues
```

After:

```text
Error during planning: Function 'substr' failed to match any signature, errors: Error during planning: Function 'substr' requires String, but received Int64 (DataType: Int64).,Error during planning: Function 'substr' expects 3 arguments but received 2.
```

The message body, the candidate signatures, and the trailing "No function matches the given name and argument types ... You might need to add explicit type casts." line are all unchanged. Only the `Internal error` kind and the bug-report boilerplate are gone.

## What changes are included in this PR?

- `datafusion/expr/src/type_coercion/functions.rs`: return `plan_err!` instead of `internal_err!` when every `OneOf` branch rejects the arguments. One line of behaviour change.
- Ten `sqllogictest` expectations that asserted the old user-visible text, in `array/array_range.slt`, `spark/math/round.slt`, `errors.slt` and `math.slt`. These are the tests for this change: `SELECT round('x')` fails one `OneOf` branch on the argument type and the other on the arity, and `round(1, 2, 3)`, the four `generate_series` cases and `nth_value` cover the remaining branch combinations. (An earlier unit test for the same path was removed at review: it was duplicate coverage.)

## Are these changes tested?

Yes.

- `cargo test -p datafusion-expr --lib` is 281 passed, 0 failed.
- I ran the four affected `sqllogictest` files locally — `errors.slt`, `math.slt`, `array/array_range.slt`, `spark/math/round.slt` — and all pass. With the source change reverted they fail on exactly the ten updated expectations, with the old text `Error during planning: Internal error: Function 'round' failed to match any signature, …`, so those expectations are a real regression guard and not just a text update. The rest of the `.slt` suite was not run locally; CI covers that.
- `cargo fmt --all --check` is clean.
- I audited every other expectation of this message. The remaining ones — `functions.slt`, `aggregate.slt`, `string_literal.slt`, `named_arguments.slt`, and one assertion in `datafusion/optimizer/src/analyzer/type_coercion.rs` — match on a substring that does not include the removed prefix, so they are unaffected. No `sqllogictest` file asserted the "This issue was likely caused by a bug..." boilerplate.

## Scope

Deliberately left out, to keep this reviewable:

- the message is still verbose and still prefixes every branch error with `Error during planning:` — that is #19004;
- `errors.join(",")` still runs the branch errors together without a separator, which is why the after-message reads `...(DataType: Int64).,Error during planning: ...`;
- the arity phrasing suggested on #20070 (`log expects 1 to 2 arguments, got 3`) is a separate improvement.

Happy to follow up on any of those in separate PRs.
