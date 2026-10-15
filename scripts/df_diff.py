#!/usr/bin/env python3
"""Hunt for Apache DataFusion bugs by differential testing.

The idea is simple: run the same SQL against two independent implementations and
compare the answers.

* **Oracle differential** — every probe runs in DataFusion (through the
  `lakewatch` binary) and in DuckDB over the *same* CSV files. A disagreement is
  a candidate DataFusion bug.
* **Self-consistency** — every probe also runs in DataFusion twice, with one
  target partition and with several. The same query must not answer differently
  depending on how the plan was parallelised. This needs no oracle at all, so a
  disagreement here is unambiguous evidence of a bug.
* **Pinned expectations** — probes whose correct answer is fixed by the SQL
  standard (three-valued logic, aggregates over empty inputs) carry an
  `expect_rows` value, so they are checked even without DuckDB installed.

Exit status is 0 unless `--strict` is passed, because a finding is the point of
running this at all, not an error in the harness.

    python3 scripts/df_diff.py
    python3 scripts/df_diff.py --only window --json reports/df-diff.json
    python3 scripts/df_diff.py --oracle none        # self-consistency only
"""

from __future__ import annotations

import argparse
import datetime
import decimal
import json
import math
import re
import shutil
import subprocess
import sys
from pathlib import Path

ISO_DATETIME = re.compile(r"^(\d{4}-\d{2}-\d{2})(?:[ T](\d{2}:\d{2}:\d{2}(?:\.\d+)?))?Z?$")
# A decimal literal with a fractional part. DataFusion hands us DECIMAL columns as text
# (`2.2000`, one digit per unit of scale) while DuckDB hands us `Decimal` objects
# (`2.2`); both spell the same value, so both spellings have to normalise the same way.
# The fractional part is required: an integer spelled `1` must stay distinct from `1.0`.
DECIMAL_LITERAL = re.compile(r"^[+-]?\d+\.\d+$")

ROOT = Path(__file__).resolve().parent.parent
DEFAULT_CATALOGUE = ROOT / "probes" / "semantics.json"
DEFAULT_PROBE_ROOT = ROOT / "probes" / "data"


# --------------------------------------------------------------------------- #
# canonical comparison
# --------------------------------------------------------------------------- #

def canon(value) -> str:
    """Normalise a value from either engine into one comparable string."""
    if value is None:
        return "NULL"
    if isinstance(value, bool):
        return "true" if value else "false"
    if isinstance(value, int):
        return str(value)
    if isinstance(value, float):
        if value != value:
            return "NaN"
        if value in (float("inf"), float("-inf")):
            return "inf" if value > 0 else "-inf"
        if value == 0.0:
            return "-0.0" if math.copysign(1.0, value) < 0 else "0.0"
        # 10 significant digits absorbs float formatting differences between
        # engines without hiding a real disagreement.
        return format_number(value)
    if isinstance(value, decimal.Decimal):
        if value != value:
            return "NaN"
        return format_number(float(value))
    if isinstance(value, (datetime.datetime, datetime.date, datetime.time)):
        return normalise_datetime(value.isoformat(sep=" "))
    if isinstance(value, (bytes, bytearray)):
        return value.hex()
    if isinstance(value, (list, tuple)):
        return "[" + ",".join(canon(item) for item in value) + "]"
    if isinstance(value, float) and value == 0.0:
        # keep the sign of zero visible: it is the whole point of some probes
        return "-0.0" if math.copysign(1.0, value) < 0 else "0.0"
    text = str(value)
    if DECIMAL_LITERAL.match(text):
        return format_number(float(text))
    return normalise_datetime(text)


def format_number(value: float) -> str:
    """One spelling per numeric value, whatever type the engine chose."""
    if value != value:
        return "NaN"
    text = f"{value:.10g}"
    if "e" not in text and "." not in text:
        text += ".0"  # keep `1.0` distinguishable from the integer `1`
    return text


def normalise_datetime(text: str) -> str:
    """Compare instants, not spellings: both engines render datetimes differently.

    `2024-03-15T12:34:56.789000Z` (DataFusion) and `2024-03-15 12:34:56.789` (DuckDB)
    are the same value and must compare equal.
    """
    match = ISO_DATETIME.match(text)
    if not match:
        return text
    date, time = match.group(1), match.group(2)
    if not time:
        return date
    if "." in time:
        whole, fraction = time.split(".", 1)
        fraction = fraction.rstrip("0")
        time = f"{whole}.{fraction}" if fraction else whole
    return f"{date} {time}"


def canon_rows(rows) -> list[list[str]]:
    return [[canon(cell) for cell in row] for row in rows]


def normalise(rows: list[list[str]], ordered: bool) -> list[list[str]]:
    return rows if ordered else sorted(rows)


def summarise_error(stderr: str, exit_code: int) -> str:
    """One readable line out of a DataFusion error or panic.

    DataFusion nests its messages ("Arrow error: X: Array error: X"), and a panic
    ends with a backtrace hint, so both are collapsed here.
    """
    lines = [line.strip() for line in stderr.splitlines() if line.strip()]
    lines = [line for line in lines if not line.startswith("note: run with")]
    if not lines:
        return f"exit {exit_code}"
    message = lines[-1]
    if "panicked at" in message or "panicked at" in (lines[0] if lines else ""):
        panic = next((line for line in lines if "panicked at" in line), message)
        reason = lines[-1] if lines[-1] != panic else ""
        return f"PANIC {panic}" + (f" ({reason})" if reason else "")
    # collapse the repeated tail that DataFusion's error chain produces
    parts = [part for part in message.split(": ")]
    unique: list[str] = []
    for part in parts:
        if part not in unique:
            unique.append(part)
    collapsed = ": ".join(unique)
    return collapsed[:300]


def describe(rows: list[list[str]], limit: int = 4) -> str:
    shown = rows[:limit]
    text = " | ".join(",".join(row) for row in shown)
    if len(rows) > limit:
        text += f" … (+{len(rows) - limit} more)"
    return f"[{len(rows)} row(s)] {text}"


# --------------------------------------------------------------------------- #
# engines
# --------------------------------------------------------------------------- #

class Runner:
    """Runs one probe in DataFusion and, when available, in DuckDB."""

    def __init__(self, binary: Path, probe_root: Path, oracle: str):
        self.binary = binary
        self.probe_root = probe_root
        self.oracle = oracle
        self.duck = None
        self.last_stderr = ""
        if oracle == "duckdb":
            import duckdb  # imported lazily so `--oracle none` needs no install

            self.duck = duckdb.connect()
            self._load_tables()

    def _tables(self) -> dict[str, list[Path]]:
        listing = subprocess.run(
            [str(self.binary), "--root", str(self.probe_root), "tables", "-f", "json"],
            capture_output=True,
            text=True,
            check=True,
        )
        entries = json.loads(listing.stdout)
        tables: dict[str, list[Path]] = {}
        for entry in entries:
            # `tables` reports how many files a dataset has, not their names; the
            # discovery rule is "<dir>/<table>.csv" for root-level files and
            # "<dir>/<sub-path>/*.csv" for directories.
            directory = self.probe_root / entry["path"]
            if entry["path"] == ".":
                files = [directory / f"{entry['table']}.{entry['format']}"]
            else:
                files = sorted(directory.glob(f"*.{entry['format']}"))
            tables[entry["table"]] = files
        return tables

    def _load_tables(self) -> None:
        for table, files in self._tables().items():
            file_list = ", ".join(f"'{path}'" for path in files)
            self.duck.execute(
                f'CREATE TABLE "{table}" AS '
                f"SELECT * FROM read_csv_auto([{file_list}], header=true)"
            )

    def datafusion(
        self, sql: str, partitions: int, session_options: tuple[str, ...] = ()
    ) -> tuple[list[list[str]] | None, str | None]:
        options: list[str] = []
        for option in session_options:
            options += ["--session-option", option]
        completed = subprocess.run(
            [
                str(self.binary),
                "--root", str(self.probe_root),
                "--partitions", str(partitions),
                *options,
                "sql", sql,
                "-f", "json",
            ],
            capture_output=True,
            text=True,
        )
        if completed.returncode != 0:
            self.last_stderr = completed.stderr.strip()
            return None, summarise_error(completed.stderr, completed.returncode)
        try:
            rows = json.loads(completed.stdout)
        except json.JSONDecodeError as error:
            return None, f"unparseable output: {error}"
        return canon_rows([list(row.values()) for row in rows]), None

    def duckdb(self, sql: str) -> tuple[list[list[str]] | None, str | None]:
        try:
            cursor = self.duck.execute(sql)
            return canon_rows(cursor.fetchall()), None
        except Exception as error:  # duckdb raises its own error hierarchy
            return None, str(error).strip().splitlines()[0]


# --------------------------------------------------------------------------- #
# main
# --------------------------------------------------------------------------- #

def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--catalogue", type=Path, default=DEFAULT_CATALOGUE)
    parser.add_argument("--root", type=Path, default=DEFAULT_PROBE_ROOT)
    parser.add_argument("--bin", type=Path, default=None, help="lakewatch binary (default: target/debug/lakewatch)")
    parser.add_argument("--oracle", choices=["auto", "duckdb", "none"], default="auto")
    parser.add_argument("--only", default=None, help="substring filter on probe id or category")
    parser.add_argument("--partitions", type=int, default=4, help="second partition count for self-consistency")
    parser.add_argument("--json", type=Path, default=None, help="write the full report here")
    parser.add_argument("--strict", action="store_true", help="exit 1 when anything was found")
    return parser.parse_args()


def resolve_binary(explicit: Path | None) -> Path:
    if explicit:
        return explicit
    candidate = ROOT / "target" / "debug" / "lakewatch"
    if candidate.exists():
        return candidate
    found = shutil.which("lakewatch")
    if found:
        return Path(found)
    sys.exit("lakewatch binary not found: run `cargo build` first or pass --bin")


def pick_oracle(requested: str) -> str:
    if requested != "auto":
        return requested
    try:
        import duckdb  # noqa: F401
    except ImportError:
        return "none"
    return "duckdb"


def main() -> int:
    args = parse_args()
    binary = resolve_binary(args.bin)
    oracle = pick_oracle(args.oracle)

    catalogue = json.loads(args.catalogue.read_text())
    probes = catalogue["probes"]
    if args.only:
        probes = [
            probe
            for probe in probes
            if args.only in probe["id"] or args.only in probe.get("category", "")
        ]
    if not probes:
        sys.exit(f"no probe matches --only {args.only!r}")

    runner = Runner(binary, args.root, oracle)

    findings: list[dict] = []
    results: list[dict] = []

    for probe in probes:
        pid = probe["id"]
        sql = probe["sql"]
        ordered = bool(probe.get("ordered", False))
        session_options = tuple(probe.get("session_options", ()))
        record: dict = {
            "id": pid,
            "category": probe.get("category", ""),
            "sql": sql,
            "why": probe.get("why"),
            "ordered": ordered,
            "session_options": list(session_options),
            "status": "OK",
            "details": [],
        }

        single, single_error = runner.datafusion(sql, 1, session_options)
        multi, multi_error = runner.datafusion(sql, args.partitions, session_options)

        # 1. self-consistency: same engine, different amount of parallelism
        if single_error and multi_error:
            record["status"] = "BOTH_ERROR"
            record["details"].append(f"datafusion errored both ways: {single_error}")
        elif single_error or multi_error:
            record["status"] = "PLAN_DIVERGENCE"
            record["details"].append(
                f"one plan errored and the other did not: "
                f"p1={single_error or 'ok'} p{args.partitions}={multi_error or 'ok'}"
            )
        elif normalise(single, ordered) != normalise(multi, ordered):
            record["status"] = "PLAN_DIVERGENCE"
            record["details"].append(
                f"partitions=1 {describe(single)} vs partitions={args.partitions} {describe(multi)}"
            )

        # 2. pinned expectation, checked only when the engine produced a result
        expected = probe.get("expect_rows")
        actual_for_expectation = single if single is not None else multi
        if expected is not None and actual_for_expectation is not None:
            wanted = normalise(canon_rows(expected), ordered)
            if normalise(actual_for_expectation, ordered) != wanted:
                record["status"] = "EXPECTATION_FAILED"
                record["details"].append(
                    f"expected {describe(wanted)} but got {describe(actual_for_expectation)}"
                )

        # 3. oracle differential
        if oracle == "duckdb":
            duck_rows, duck_error = runner.duckdb(sql)
            documented = probe.get("known_difference")
            if documented and (duck_error or actual_for_expectation is None):
                record["status"] = "KNOWN_DIFF" if record["status"] == "OK" else record["status"]
                record["details"].append(
                    "documented divergence: "
                    + (f"duckdb errored ({duck_error})" if duck_error else f"datafusion errored ({single_error or multi_error})")
                )
            elif duck_error and actual_for_expectation is None:
                record["details"].append(f"both engines errored (duckdb: {duck_error})")
            elif duck_error:
                record["status"] = "ERROR_DIVERGENCE" if record["status"] == "OK" else record["status"]
                record["details"].append(f"duckdb errored: {duck_error}")
            elif actual_for_expectation is None:
                record["status"] = "ERROR_DIVERGENCE" if record["status"] == "OK" else record["status"]
                record["details"].append(f"datafusion errored: {single_error or multi_error}")
            elif probe.get("known_difference"):
                if normalise(duck_rows, ordered) != normalise(actual_for_expectation, ordered):
                    record["status"] = "KNOWN_DIFF" if record["status"] == "OK" else record["status"]
                    record["details"].append(
                        f"documented divergence: datafusion {describe(actual_for_expectation)} "
                        f"vs duckdb {describe(duck_rows)}"
                    )
            elif normalise(duck_rows, ordered) != normalise(actual_for_expectation, ordered):
                record["status"] = "MISMATCH" if record["status"] == "OK" else record["status"]
                record["details"].append(
                    f"datafusion {describe(actual_for_expectation)} vs duckdb {describe(duck_rows)}"
                )
        elif single is not None and not single_error:
            record["details"].append(f"result {describe(single)}")

        if record["status"] != "OK" and runner.last_stderr:
            record["stderr"] = runner.last_stderr

        if record["status"] not in ("OK",):
            findings.append(record)
        results.append(record)

    # ------------------------------------------------------------------ report
    width = max((len(r["id"]) for r in results), default=10)
    interesting = [r for r in results if r["status"] not in ("OK",)]
    for record in results:
        if record["status"] == "OK":
            continue
        print(f"{record['status']:<18} {record['id']:<{width}}  {record['category']}")
        for detail in record["details"]:
            print(f"    {detail}")
        print(f"    sql: {record['sql']}")
        print()

    counts: dict[str, int] = {}
    for record in results:
        counts[record["status"]] = counts.get(record["status"], 0) + 1

    print("-" * 72)
    print(f"oracle: {oracle} | probes: {len(results)} | binary: {binary}")
    print("results: " + ", ".join(f"{key}={value}" for key, value in sorted(counts.items())))
    candidate = counts.get("MISMATCH", 0) + counts.get("PLAN_DIVERGENCE", 0) + counts.get("EXPECTATION_FAILED", 0) + counts.get("ERROR_DIVERGENCE", 0)
    print(f"candidates needing triage: {candidate}")

    if args.json:
        args.json.parent.mkdir(parents=True, exist_ok=True)
        args.json.write_text(
            json.dumps(
                {
                    "oracle": oracle,
                    "binary": str(binary),
                    "counts": counts,
                    "findings": findings,
                    "results": results,
                },
                indent=2,
            )
            + "\n"
        )
        print(f"wrote {args.json}")

    if args.strict and candidate:
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
