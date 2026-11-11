# DML is a surface no probe reaches, and a subquery in its WHERE is refused

Measured 2026-11-11 on main (`3d7262d87`, `datafusion-cli`), DuckDB as the oracle.

The harness's probes are single `SELECT`s, so **DML has never been covered**. The scalar-subquery error
message mentions "…and MERGE DML plan nodes", which suggested DML positions were at least contemplated.

Method: a script per case through `datafusion-cli -f` (create, insert, the DML, then a `SELECT` of the
final table), replayed statement by statement in DuckDB.

## The boundary

| DML form | subquery in the statement | result |
|---|---|---|
| `INSERT … SELECT … WHERE <subquery>` | yes | **works** |
| `CREATE TABLE u AS SELECT … WHERE <subquery>` | yes | **works** |
| `DELETE` / `UPDATE … WHERE <subquery>` | `IN (SELECT …)`, `EXISTS`, `= ANY (SELECT …)` | **refused, with an explanation** |
| `DELETE` / `UPDATE … WHERE <subquery>` | scalar `= (SELECT …)` | **refused, with a generic message** |
| `DELETE … WHERE v IN (1, 2)` | literal list | **works** |

The explained refusal reads:

```
This feature is not implemented: DELETE on table 't' with an IN or an EXISTS subquery in its WHERE
clause is not supported: the optimizer rewrites the subquery into a LeftSemi join, and the condition
does not reach the table provider
```

`= ANY` gets the same message with `LeftMark join`. That is a **deliberate limitation with its reason
stated**, so it is recorded in `docs/upstream-datafusion.md` rather than filed. Searched upstream on
2026-11-11 for the message text and for the pattern: no matching issue.

The one thing worth flagging is the **inconsistency**: three of the four forms get a tailored,
actionable message, while the scalar form gets

```
This feature is not implemented: Physical plan does not support logical expression ScalarSubquery(<subquery>)
```

which does not mention DML, the provider, or why. A user who hits `DELETE … WHERE v = (SELECT …)` is told
something that reads like an internal limitation of the physical planner, and the next thing they will try
(`v IN (SELECT …)`) fails too, for a different reason. Low value on its own; recorded, not filed.

## The tooling trap that made this look like a data-loss bug first

The first run of the matrix reported **17 of 17 cases as value differences** — the table simply had not
changed after the failed `DELETE`/`UPDATE`. The reason:

**`datafusion-cli -f` prints the error to stdout, continues with the next statement, and exits 0.** The
output was `count 2` (the insert), then the error text, then the unchanged table. A script that checks
only the exit code sees success.

With `-c "SELECT 1/0"` the same binary exits **1**, so the behaviour differs between `-c` and `-f`, and a
failed DML inside a `-f` script is the easy case to miss.

`lakewatch` does not have this problem (errors to stderr, exit 2), which is why the probes have never been
fooled by it. Any future script that drives `datafusion-cli` must scan stdout for `Error:` /
`not implemented`, not trust the exit code.
