# Reply text for the review thread on apache/datafusion#26329

We cannot post this (see `reports/datafusion-findings.md`: `apache/*` refuses our writes), so
it is handed to the user to paste. Post after pushing nothing further — `268933e` is the head.

---

Removed the unit test in 268933e, and I did not move it to `.slt`.

I checked the coverage instead of assuming it: `round`'s signature is a `OneOf` with 1- and
2-argument branches, so `SELECT round('x')` fails one branch on the argument type and the other
on the arity — the exact shape the unit test constructed — and `round(1, 2, 3)`, `round(true)`,
`round(timestamp)`, the four `generate_series` cases and `nth_value` cover the remaining
combinations. Nothing was lost, so nothing needed to move into `.slt`.

The behaviour change is unchanged: one `internal_err!` → `plan_err!` in
`get_valid_types_with_udf`, plus the ten expectations. Those expectations are a real guard and
not just a text update — I ran the four affected files locally (`errors.slt`, `math.slt`,
`array/array_range.slt`, `spark/math/round.slt`): with the source change reverted they fail on
exactly the ten updated expectations, with `Internal error: Function 'round' failed to match
any signature, …`; with it, all four files pass.

`cargo fmt --all --check` is clean and `cargo test -p datafusion-expr --lib` is 281 passed,
0 failed (282 before, i.e. exactly the removed test). Thanks for the review — and sorry for the
CI approval this push costs.
