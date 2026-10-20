# Ready-to-post issue text: unsupported-feature errors dump internal structures instead of naming the construct

One issue, two layers (SQL planner and physical planner), each fixable on its own. We cannot open
issues on `apache/*`, so this is handed to the user to post at
<https://github.com/apache/datafusion/issues/new>. Paste from the horizontal rule down; the title
line goes in the title box. **Posting is optional** — the underlying gaps are tracked elsewhere;
this asks only for the message.

Title:

```text
Unsupported-feature errors dump the internal AST / plan instead of naming the construct
```

---

### Describe the bug

When a query uses something DataFusion does not implement, the error the user gets is the internal
representation, Debug-formatted. Two layers do this.

**1. SQL planner** — the whole sqlparser AST (~2 KB for `PIVOT`):

```text
This feature is not implemented: Unsupported ast node MatchRecognize { table: Table { name: ObjectName([Identifier(Ident { value: "t", quote_style: None, span: Span(Location(1,15)..Location(1,27)) })]), alias: None, args: None, … } } in create_relation
```

**2. Physical planner** — the logical expression it could not turn into a physical one:

```text
This feature is not implemented: Physical plan does not support logical expression OuterReferenceColumn(Field { name: "x", data_type: Int64 }, Column { relation: Some(Bare { table: "a" }), name: "x" })
This feature is not implemented: Physical plan does not support logical expression ScalarSubquery(<subquery>)
```

None of these tell the user which *SQL construct* is unsupported, or that it is a known gap.

### To reproduce

Verified on `main` (build of 2026-10-16, `5f21582`; the four source sites re-read on
`main@3ee3e39` on 2026-10-20 and unchanged) with the `sqllogictest` runner (all
self-contained). Note the `MATCH_RECOGNIZE` query must select from a **named table**: one
`PATTERN` clause is required or sqlparser stops earlier, and a derived table is rejected by
the parser (`ParserError("Expected: end of statement, found: MATCH_RECOGNIZE …")`) before
the planner is reached.

```sql
-- 1. SQL planner: MATCH_RECOGNIZE (out of core scope, see #13583) and PIVOT (#12907)
CREATE TABLE t(id int, g int) AS VALUES (1, 10), (2, 20);
SELECT * FROM t MATCH_RECOGNIZE (PARTITION BY g ORDER BY id PATTERN (A+) DEFINE A AS id > 0);

SELECT * FROM (SELECT g FROM (VALUES (10), (20)) t(g)) PIVOT (SUM(g) FOR g IN (10, 20));

-- 2. Physical planner: a correlated LATERAL whose correlation is not an equality
SELECT * FROM (VALUES (2)) a(x),
  LATERAL (SELECT MAX(y) AS m FROM (VALUES (1),(2)) b(y) WHERE b.y < a.x) l;

-- a correlated scalar subquery with a non-equality correlation
SELECT (SELECT MAX(a2.x) FROM (VALUES (1),(2)) a2(x) WHERE a2.x < a1.x) FROM (VALUES (2)) a1(x);
```

The equality-correlated versions of both shapes work, so in layer 2 the limitation is specifically
the non-equality correlation.

### Expected behaviour

Name the construct and, where one exists, the tracking issue:

```text
This feature is not implemented: MATCH_RECOGNIZE is not supported yet
This feature is not implemented: correlated subqueries with a non-equality correlation are not supported yet
```

### Where it comes from

* `datafusion/sql/src/relation/mod.rs:310` —
  `not_impl_err!("Unsupported ast node {relation:?} in create_relation")`
* `datafusion/sql/src/expr/mod.rs:831` (`:842` in 55.2.0) —
  `not_impl_err!("Unsupported ast node in sqltorel: {sql:?}")`
* `datafusion/physical-expr/src/planner.rs:541` and `:703` (540/692 in 55.2.0) —
  `not_impl_err!("Physical plan does not support logical expression {e:?}")`

Each site already knows the node variant (a `match` arm), so naming it is a small change; the four
sites are independent, so they can land as separate commits.

### Additional context

* The underlying gaps are tracked: #12907 (`PIVOT`/`UNPIVOT`), #21201 and #5492
  (LATERAL / decorrelation). `MATCH_RECOGNIZE` is a special case: #13583 was **closed on
  2026-08-11 as completed** because it was decided *not* to live in core (it is meant to be
  built as an extension on the planning-extension APIs), so there is no tracking issue to
  point at — only the construct name. This report is only about what the user sees.

### One more instance of the same family (message names something the user did not write)

`3 * INTERVAL '1 hour'` (or `INTERVAL '1 hour' * 3`, or `/ 3`) is not supported; the support
gap itself is tracked in #13850 and is not part of this report. What the user gets is:

```text
Error during planning: Cannot get result type for temporal operation Interval(MonthDayNano) * Int64:
Invalid argument error: Invalid interval arithmetic operation: Interval(MonthDayNano) * Interval(MonthDayNano)
```

The second operand was an `Int64`, and the message says `Interval * Interval` on both sides —
the coercion step rewrote the pair before the arithmetic failed, so the message describes an
expression that was never written (`datafusion/expr-common/src/type_coercion/binary.rs:322`,
which formats `{coerced} {} {coerced}`, i.e. the same type twice). Reporting the original
left/right types, or "interval multiplication/division is not supported yet; use
`INTERVAL * n` … (see #13850)", would be equally small a change. Same class as above: the
message is about the implementation's internal state, not about the user's query.
* On 55.2.0 the same physical-planner message also appears for `= ANY (…)` / `> ALL (…)` in a
  project list. On `main` that instance is gone: apache/datafusion#24972 (merged 2026-09-15)
  rewrites those subqueries before the physical planner sees them. The rewrite is only correct
  with an explicit projection alias, though — without one the *same* query now fails an
  optimizer invariant check instead (an internal error of its own, `main`-only). That is a
  separate bug with its own ready-to-post text:
  `findings/set-comparison-projection-internal-error.md`. Do not mix the two when posting.
