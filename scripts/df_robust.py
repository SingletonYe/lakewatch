#!/usr/bin/env python3
"""Tier-A hunter: shake DataFusion's built-in functions with hostile input.

The differential harness in `df_diff.py` looks for *wrong answers*, which is slow,
because most disagreement turns out to be a documented dialect difference. This
script looks for something much cheaper to judge and much faster to fix:

* a **panic** on user input (the process aborts instead of returning an error),
* an **internal error** leaking through to the user ("Internal error: ... This was
  likely caused by a bug"), where a plain validation error belongs,
* a **not-implemented** error where the function's own documentation promises it works.

None of those need an oracle: correct is obvious, incorrect is obvious, and they are
exactly the class of bug maintainers merge quickly.

The function inventory comes from DataFusion itself —
`information_schema.routines` carries `function_type` and `syntax_example`, which is
enough to call every scalar function with the right number of arguments.

    python3 scripts/df_robust.py                 # whole catalogue
    python3 scripts/df_robust.py --limit 40      # first N functions
    python3 scripts/df_robust.py --only regexp   # name filter
    python3 scripts/df_robust.py --json reports/df-robust.json
"""

from __future__ import annotations

import argparse
import json
import re
import shutil
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
DEFAULT_PROBE_ROOT = ROOT / "probes" / "data"
BATCH_SIZE = 30

# Argument profiles, applied to every position of the call. The point is to reach
# code paths a happy-path test never touches: empty strings, negative and boundary
# integers, NULLs, and values of the wrong-but-parseable type.
PROFILES: list[tuple[str, list[str]]] = [
    ("null", ["NULL"]),
    ("empty", ["''"]),
    ("text", ["'abc'"]),
    ("zero", ["0"]),
    ("negative", ["-1"]),
    ("int_max", ["9223372036854775807"]),
    ("float_max", ["1e308"]),
    ("date", ["DATE '2024-01-01'"]),
    ("mixed", ["''", "-1", "0", "'abc'", "NULL", "9223372036854775807", "1e308"]),
]

CLASSIFIERS: list[tuple[str, re.Pattern[str]]] = [
    ("PANIC", re.compile(r"panicked at|attempt to (?:add|subtract|multiply|divide|negate) with overflow")),
    ("INTERNAL", re.compile(r"Internal error|This was likely caused by a bug|Unexpected internal error|index out of bounds|unreachable", re.I)),
    ("NOT_IMPL", re.compile(r"not implemented|NotImplemented|Unsupported|unsupported", re.I)),
]

# A wrong-arity call is a clean, expected error; it must not be reported as a finding.
ARITY_ERROR = re.compile(r"does not support \d+ arguments|No function matches the given name|signature", re.I)


def classify(stderr: str, exit_code: int) -> tuple[str, str]:
    """Map one invocation's failure to a finding class."""
    text = stderr.strip()
    lines = [line.strip() for line in text.splitlines() if line.strip()]
    message = lines[-1] if lines else f"exit {exit_code}"
    combined = "\n".join(lines)
    if exit_code < 0 or "panicked at" in combined:
        for line in lines:
            if "panicked at" in line:
                return "PANIC", line
        return "PANIC", message
    for name, pattern in CLASSIFIERS:
        if pattern.search(combined):
            # collapse DataFusion's repeated error chain
            parts, unique = message.split(": "), []
            for part in parts:
                if part not in unique:
                    unique.append(part)
            return name, ": ".join(unique)[:300]
    if ARITY_ERROR.search(combined):
        return "ARITY", message[:200]
    return "CLEAN_ERROR", message[:200]


class Shaker:
    def __init__(self, binary: Path, probe_root: Path):
        self.binary = binary
        self.probe_root = probe_root

    def run(self, sql: str) -> tuple[bool, str, str]:
        completed = subprocess.run(
            [str(self.binary), "--root", str(self.probe_root), "sql", sql, "-f", "json"],
            capture_output=True,
            text=True,
        )
        return completed.returncode == 0, completed.stdout, completed.stderr

    def functions(self) -> list[dict]:
        sql = (
            "SELECT routine_name, data_type, syntax_example, description "
            "FROM information_schema.routines WHERE function_type = 'SCALAR' "
            "ORDER BY routine_name"
        )
        ok, stdout, stderr = self.run(sql)
        if not ok:
            sys.exit(f"cannot read information_schema.routines: {stderr.strip()}")
        return json.loads(stdout)


def arities(syntax_example: str | None, name: str) -> list[int]:
    """How many arguments to try, from the documented syntax example."""
    if not syntax_example:
        return [1, 2]
    inside = re.search(r"\((.*)\)", syntax_example)
    if not inside:
        return [1, 2]
    body = inside.group(1)
    # `substr(str, start[, len])`: the bracketed part is optional
    required = re.sub(r"\[[^\]]*\]", "", body).strip()
    if not required:
        return [1, 2]
    count = len([part for part in required.split(",") if part.strip()])
    count = max(1, min(count, 4))
    extra = [count] if count <= 1 else [count, count - 1]
    return sorted(set(extra), reverse=True)


def message_signature(message: str, function: str) -> str:
    """Collapse a finding to its root cause so identical failures group together."""
    text = message.replace(function, "FN")
    text = re.sub(r"'[^']*'", "'X'", text)
    text = re.sub(r"\b\d+\b", "N", text)
    return text[:170]


def build_call(name: str, args: list[str]) -> str:
    return f"{name}({', '.join(args)})"


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--root", type=Path, default=DEFAULT_PROBE_ROOT)
    parser.add_argument("--bin", type=Path, default=None)
    parser.add_argument("--only", default=None, help="substring filter on the function name")
    parser.add_argument("--limit", type=int, default=None)
    parser.add_argument("--json", type=Path, default=None)
    parser.add_argument("--strict", action="store_true", help="exit 1 when a panic or internal error was found")
    args = parser.parse_args()

    binary = args.bin or (ROOT / "target" / "debug" / "lakewatch")
    if not binary.exists():
        binary = shutil.which("lakewatch") or sys.exit("lakewatch binary not found; run cargo build")
    shaker = Shaker(binary, args.root)

    functions = shaker.functions()
    if args.only:
        functions = [f for f in functions if args.only in f["routine_name"]]
    if args.limit:
        functions = functions[: args.limit]
    if not functions:
        sys.exit("no functions selected")

    findings: list[dict] = []
    clean_errors = 0
    calls = 0

    for function in functions:
        name = function["routine_name"]
        for arity in arities(function.get("syntax_example"), name):
            for profile, values in PROFILES:
                args_list = [values[index % len(values)] for index in range(arity)]
                call = build_call(name, args_list)
                calls += 1
                ok, _, stderr = shaker.run(f"SELECT {call}")
                if ok:
                    continue
                verdict, message = classify(stderr, 1)
                if verdict == "ARITY":
                    break  # this arity is not the function's, try the next one
                if verdict == "CLEAN_ERROR":
                    clean_errors += 1
                    continue
                findings.append(
                    {
                        "function": name,
                        "profile": profile,
                        "verdict": verdict,
                        "call": call,
                        "sql": f"SELECT {call}",
                        "message": message,
                        "documented_syntax": function.get("syntax_example"),
                        "documented_type": function.get("data_type"),
                    }
                )
            else:
                continue
            break

    order = {"PANIC": 0, "INTERNAL": 1, "NOT_IMPL": 2}

    # Hundreds of functions fail the same way. What matters to a triager is the
    # number of *distinct root causes*, so cluster by a normalised message first.
    clusters: dict[str, dict] = {}
    for finding in findings:
        key = f"{finding['verdict']}|{message_signature(finding['message'], finding['function'])}"
        cluster = clusters.setdefault(
            key,
            {
                "verdict": finding["verdict"],
                "signature": message_signature(finding["message"], finding["function"]),
                "count": 0,
                "functions": [],
                "example": finding,
            },
        )
        cluster["count"] += 1
        if finding["function"] not in cluster["functions"]:
            cluster["functions"].append(finding["function"])

    ranked = sorted(
        clusters.values(),
        key=lambda c: (order.get(c["verdict"], 9), -c["count"]),
    )

    for cluster in ranked:
        shown = ", ".join(cluster["functions"][:6])
        extra = len(cluster["functions"]) - 6
        if extra > 0:
            shown += f", +{extra} more"
        print(f"{cluster['verdict']:<9} x{cluster['count']:<4} {shown}")
        print(f"    {cluster['signature']}")
        print(f"    repro: {cluster['example']['sql']}")
        print()

    findings.sort(key=lambda f: (order.get(f["verdict"], 9), f["function"], f["profile"]))

    counts: dict[str, int] = {}
    for finding in findings:
        counts[finding["verdict"]] = counts.get(finding["verdict"], 0) + 1
    print("-" * 72)
    print(f"functions: {len(functions)} | invocations: {calls} | clean expected errors: {clean_errors}")
    print(f"findings: {len(findings)} across {len(clusters)} distinct root cause(s)")
    print("by verdict: " + (", ".join(f"{k}={v}" for k, v in sorted(counts.items())) or "none"))

    if args.json:
        args.json.parent.mkdir(parents=True, exist_ok=True)
        args.json.write_text(
            json.dumps(
                {
                    "functions": len(functions),
                    "invocations": calls,
                    "counts": counts,
                    "findings": findings,
                },
                indent=2,
            )
            + "\n"
        )
        print(f"wrote {args.json}")

    if args.strict and (counts.get("PANIC") or counts.get("INTERNAL")):
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
