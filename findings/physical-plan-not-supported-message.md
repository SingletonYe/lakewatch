# Ready-to-post issue text: physical planner leaks internal Debug output in "does not support logical expression …"

We cannot open issues on `apache/*` (the organisation blocks third-party OAuth writes), so this is
handed to the user to post at <https://github.com/apache/datafusion/issues/new>. Paste from the
horizontal rule down; the title line goes in the title box. **Posting it is optional** — the
underlying limitations are already tracked (#21201, #5492, #25789); this only asks for a readable
message.

Title:

```text
Physical planner errors show internal Debug output: "does not support logical expression OuterReferenceColumn(Field { … })"
```

---

### Describe the bug

When a query hits a shape the decorrelation framework cannot rewrite, the physical planner reports it
with the *Debug* rendering of the internal expression type. The user cannot act on that:

```text
This feature is not implemented: Physical plan does not support logical expression OuterReferenceColumn(Field { name: "x", data_type: Int64 }, Column { relation: Some(Bare { table: "a" }), name: "x" })
```

Same message, other expressions, all Debug-formatted:

```text
This feature is not implemented: Physical plan does not support logical expression ScalarSubquery(<subquery>)
This feature is not implemented: Physical plan does not support logical expression Exists(Exists { subquery: <subquery>, negated: false })
```

### To reproduce

Verified on `main` on 2026-10-17 with the `sqllogictest` runner (queries below are self-contained):

```sql
-- a correlated LATERAL whose correlation is not an equality
SELECT * FROM (VALUES (2)) a(x),
  LATERAL (SELECT MAX(y) AS m FROM (VALUES (1),(2)) b(y) WHERE b.y < a.x) l;
-- This feature is not implemented: Physical plan does not support logical expression
--   OuterReferenceColumn(Field { name: "x", data_type: Int64 }, Column { relation: Some(Bare { table: "a" }), name: "x" })

-- a correlated scalar subquery with a non-equality correlation
SELECT (SELECT MAX(a2.x) FROM (VALUES (1),(2)) a2(x) WHERE a2.x < a1.x) FROM (VALUES (2)) a1(x);
-- This feature is not implemented: Physical plan does not support logical expression ScalarSubquery(<subquery>)
```

Both queries work if the correlation is an equality instead (`... WHERE b.y = a.x`), so the limitation
is specifically about non-equality correlations, not about `LATERAL` or subqueries as such.

### Expected behaviour

A message in SQL terms, for example:

```text
This feature is not implemented: correlated subqueries with a non-equality correlation are not supported yet
```

possibly with a pointer at the tracking issue. The behavioural gap itself is already tracked
(#21201 for `LATERAL` with outer references outside `WHERE`, #5492 for the general decorrelation
framework, #25789 and neighbours for correlated filters) — this report is only about what the user
sees.

### Where it comes from

`datafusion/physical-expr/src/planner.rs` (line 541 and line 703 on `main`, 540/692 in 55.2.0):

```rust
not_impl_err!("Physical plan does not support logical expression {e:?}")
```

The `{e:?}` is a `Debug` format of a logical expression. The comment above it already explains the
situation ("either a correlated subquery that wasn't rewritten to a join, or an uncorrelated one that
wasn't registered by the physical planner"), so the message could describe the query shape instead of
the internal type.

### Additional context

* This is a diagnostics issue, not a wrong-results issue — the query is correctly rejected.
* On 55.2.0 the same message also appears for `= ANY (…)` / `> ALL (…)` in a project list; that case
  is fixed on `main` by #24972 ("Support predicate subqueries in projections"), where those queries
  now return results (verified: `2 > ALL (VALUES (1),(0))` → `true`, and
  `1 = ANY (VALUES (NULL),(1))` → `true`, i.e. correct three-valued logic). So the message remains
  reachable on `main` for the non-equality correlation shapes only.
