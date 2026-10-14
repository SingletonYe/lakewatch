#!/usr/bin/env bash
#
# Deterministic health check for lakewatch.
#
# This is the single definition of "the project is healthy". It is used by the
# nightly GitHub Actions workflow and by the daily maintenance workflow, so the
# two can never drift apart.
#
# It prints one `STAGE <name> <PASS|FAIL> <seconds>` line per stage, dumps the
# tail of any failing stage's output, and exits non-zero when anything failed.
#
#   bash scripts/daily_check.sh
#
set -uo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

BIN="${LAKEWATCH_BIN:-$ROOT/target/debug/lakewatch}"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

declare -a NAMES=() STATUSES=() DURATIONS=()
FAILED=0

run_stage() {
  local name="$1"
  shift
  local log="$WORK/$name.log"
  local start end status
  start=$(date +%s)
  if "$@" >"$log" 2>&1; then
    status=PASS
  else
    status=FAIL
    FAILED=1
  fi
  end=$(date +%s)
  NAMES+=("$name")
  STATUSES+=("$status")
  DURATIONS+=("$((end - start))")
  printf 'STAGE %-14s %-4s %ss\n' "$name" "$status" "$((end - start))"
  if [[ "$status" == FAIL ]]; then
    echo "  --- last 40 lines of the $name stage ---"
    tail -40 "$log" | sed 's/^/  /'
  fi
}

# --- stages ----------------------------------------------------------------

check_format() {
  cargo fmt --all --check
}

check_lints() {
  cargo clippy --all-targets -- -D warnings
}

check_tests() {
  cargo test --all-targets
}

# The committed sample lake must be exactly what the generator produces, so a
# demo can never silently rot into something the rules no longer describe.
check_data_drift() {
  python3 scripts/gen_sample_data.py >/dev/null || return 1
  if git diff --quiet -- data; then
    return 0
  fi
  echo "data/ is not what scripts/gen_sample_data.py produces:"
  git --no-pager diff --stat -- data
  git checkout -- data
  return 1
}

# The gate must FAIL on the deliberately dirty sample lake, and it must fail for
# exactly the documented reasons. Proving the failures is how we prove the
# checks are wired up.
check_gate() {
  "$BIN" --root data check --rules rules/quality.json --format json >"$WORK/gate.json" 2>"$WORK/gate.err"
  local code=$?
  if [[ $code -ne 1 ]]; then
    echo "expected exit code 1 from a dirty lake, got $code"
    cat "$WORK/gate.err"
    return 1
  fi
  python3 - "$WORK/gate.json" <<'PY'
import json
import sys

expected = {
    "orders.order_id_is_unique": 1,
    "orders.customer_id_is_not_null": 2,
    "orders.amount_is_positive": 4,
    "orders.status_is_known": 1,
    "orders.channel_is_known": 0,
    "orders.customer_email_is_valid": 11,
    "orders.row_count_is_plausible": 240,
    "customers.email_is_well_formed": 2,
    "products.unit_price_is_positive": 0,
}

report = json.load(open(sys.argv[1]))
problems = []
if report["ok"] is not False:
    problems.append("report.ok should be false on the sample lake")
if report["total"] != len(expected):
    problems.append(f"expected {len(expected)} rules, got {report['total']}")
if report["failed"] != 6:
    problems.append(f"expected 6 failing rules, got {report['failed']}")
if report["errored"] != 0:
    problems.append(f"expected 0 unevaluated rules, got {report['errored']}")

seen = {outcome["rule"]: outcome for outcome in report["outcomes"]}
for rule, violations in expected.items():
    outcome = seen.get(rule)
    if outcome is None:
        problems.append(f"rule {rule} is missing from the report")
    elif outcome["violations"] != violations:
        problems.append(f"{rule}: expected {violations} violations, got {outcome['violations']}")

if problems:
    print("\n".join(problems))
    sys.exit(1)
print(f"gate behaves as documented: {report['passed']} passed, {report['failed']} failed")
PY
}

# Discovery, scanning, profiling and the custom functions all have to keep
# working against the sample lake.
check_sample_lake() {
  "$BIN" --root data tables --counts --format json >"$WORK/tables.json" || return 1
  python3 - "$WORK/tables.json" <<'PY'
import json
import sys

rows = {row["table"]: row for row in json.load(open(sys.argv[1]))}
expected = {"customers": (1, 61), "orders": (2, 240), "products": (1, 30)}
if set(rows) != set(expected):
    print(f"expected tables {sorted(expected)}, got {sorted(rows)}")
    sys.exit(1)
for table, (files, count) in expected.items():
    if rows[table]["files"] != files or rows[table]["rows"] != count:
        print(f"{table}: expected {files} file(s) and {count} rows, got "
              f"{rows[table]['files']} and {rows[table]['rows']}")
        sys.exit(1)
print("discovery and row counts match the sample lake")
PY
  for table in customers orders products; do
    "$BIN" --root data profile "$table" --quantiles --format json >"$WORK/profile-$table.json" || return 1
    python3 -c "
import json,sys
rows=json.load(open('$WORK/profile-$table.json'))
if not rows: sys.exit('profile of $table is empty')
" || return 1
  done
  # the custom UDAFs must agree across partition layouts
  local one four
  one=$("$BIN" --root data --partitions 1 sql "SELECT channel, mode(status) m, null_ratio(customer_email) nr FROM orders GROUP BY channel ORDER BY channel" -f json) || return 1
  four=$("$BIN" --root data --partitions 4 sql "SELECT channel, mode(status) m, null_ratio(customer_email) nr FROM orders GROUP BY channel ORDER BY channel" -f json) || return 1
  if [[ "$one" != "$four" ]]; then
    echo "mode/null_ratio disagree between 1 and 4 partitions"
    diff <(echo "$one") <(echo "$four")
    return 1
  fi
  echo "sample lake, profiling and custom functions all behave"
}

# --- run -------------------------------------------------------------------

run_stage build cargo build
run_stage format check_format
run_stage lints check_lints
run_stage tests check_tests
run_stage data_drift check_data_drift
run_stage gate check_gate
run_stage sample_lake check_sample_lake

echo
printf 'SUMMARY stages=%d failed=%d\n' "${#NAMES[@]}" "$(printf '%s\n' "${STATUSES[@]}" | grep -c FAIL)"
for index in "${!NAMES[@]}"; do
  printf 'RESULT %s %s %ss\n' "${NAMES[$index]}" "${STATUSES[$index]}" "${DURATIONS[$index]}"
done

exit "$FAILED"
