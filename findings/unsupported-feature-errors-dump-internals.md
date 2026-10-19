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

Verified on `main` on 2026-10-17/19 with the `sqllogictest` runner (all self-contained):

```sql
-- 1. SQL planner: MATCH_RECOGNIZE (feature request #13583) and PIVOT (#12907)
SELECT * FROM (VALUES (1, 10), (2, 20)) t(id, g)
  MATCH_RECOGNIZE (PARTITION BY g ORDER BY id PATTERN (A+) DEFINE A AS id > 0);

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
This feature is not implemented: MATCH_RECOGNIZE is not supported yet (see #13583)
This feature is not implemented: correlated subqueries with a non-equality correlation are not supported yet
```

### Where it comes from

* `datafusion/sql/src/relation/mod.rs:310` —
  `not_impl_err!("Unsupported ast node {relation:?} in create_relation")`
* `datafusion/sql/src/expr/mod.rs:842` —
  `not_impl_err!("Unsupported ast node in sqltorel: {sql:?}")`
* `datafusion/physical-expr/src/planner.rs:541` and `:703` (540/692 in 55.2.0) —
  `not_impl_err!("Physical plan does not support logical expression {e:?}")`

Each site already knows the node variant (a `match` arm), so naming it is a small change; the four
sites are independent, so they can land as separate commits.

### Additional context

* The underlying gaps are tracked: #13583 (`MATCH_RECOGNIZE`), #12907 (`PIVOT`/`UNPIVOT`),
  #21201 and #5492 (LATERAL / decorrelation). This report is only about what the user sees.
* On 55.2.0 the same physical-planner message also appears for `= ANY (…)` / `> ALL (…)` in a
  project list; that case is fixed on `main` by #24972, where those queries now return results
  (checked: `2 > ALL (VALUES (1),(0))` → `true`, `1 = ANY (VALUES (NULL),(1))` → `true`).
