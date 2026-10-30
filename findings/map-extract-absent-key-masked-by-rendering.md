# `map_extract` on an absent key: a real divergence our probe could not see

Found 2026-10-30 while looking for uncovered `nested_types` behaviour. Classification:
**`known_upstream`, already fixed in 56.0.0** — no issue to file, and no duplicate.

## The behaviour

On the pinned DataFusion 55.2.0, a lookup for a key that is **not in the map** returns a
**one-element list holding NULL** (`[NULL]`), not an empty list:

```sql
SELECT array_length(map_extract(MAP {'a': 1}, 'zz'));   -- DataFusion 1 | DuckDB 0
SELECT array_length(map_extract(MAP {'a': 1}, NULL));   -- DataFusion 1 | DuckDB 0
SELECT array_length(map_extract(MAP {'a': 1, 'b': NULL}, 'b'));  -- DataFusion 1 | DuckDB 1
```

That contradicts DataFusion's own built-in documentation, which
`information_schema.routines` reports as: *"Returns a list containing the value for the given key
**or an empty list if the key is not present** in the map."*

The consequence is not cosmetic: an absent key and a key that is present holding NULL both come
back as a one-element list, so **the two cases are indistinguishable** — which is exactly what the
function's contract says they should not be.

## Already known and already fixed

`docs/source/library-user-guide/upgrading/56.0.0.md` has a section,
*"`map_extract` / `element_at` return an empty list for absent keys"*, which states the old
behaviour, the new one, and the two related changes (a NULL map input now yields NULL; a NULL
lookup key now yields `[]`). It cites **apache/datafusion#24981** — "`map_extract` returns
`[NULL]` instead of `[]` for missing keys" — which is **closed/completed**.

So the pinned version predates the fix. Nothing to report upstream.

## What this actually cost us: our own probe was blind to it

`probes/semantics.json` already had `map_extract_present_and_missing`, whose expectation was
`[["[1]", "[]"]]` and which was passing as **OK** — i.e. "both engines agree". They do not.

The reason is a rendering coincidence: DataFusion prints a NULL **element** inside a list as an
empty string (`map_values(MAP {'a': 1, 'b': NULL})` renders as `"[1, ]"`), so a one-element list
holding NULL prints as `[]` — **the same text an actually empty list prints**. A comparison over
rendered output therefore sees agreement where the values differ.

**The probe was upgraded rather than replaced**: it now also asserts
`array_length(map_extract(..., 'zz'))` and `array_length(map_extract(..., NULL))` alongside the
existing text, and carries a `known_difference` recording the above. It reports `KNOWN_DIFF`
instead of `OK` now, which is the truth. When the pin moves to 56.0.0 it will flip to `OK`, which
is the signal that the fix has arrived.

## The lesson, for the catalogue

**A list-valued expression can hide a NULL-versus-empty difference behind its rendering.** Any
probe whose output is a list, array or map should assert a **structural** property of it
(`array_length`, an element `IS NULL`, a count) rather than trusting the printed form. This case
had been sitting in the catalogue looking green for weeks.

Two other dead ends found in the same sweep, recorded so nobody retries them:

* **JSON functions are not available in this build** — `json_extract` does not exist
  (the parser suggests `union_extract`), `::json` is "Unsupported SQL type JSON", and
  `information_schema.routines` lists no json routine.
* **`MERGE` is parsed but cannot be probed here** — the CLI accepts only a single statement
  ("The context currently only supports a single SQL statement"), and MERGE is DML: it would
  mutate the shared `probe_values` table and change the answer of every later probe.
