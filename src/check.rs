//! Declarative data-quality checks.
//!
//! A rules file is JSON, and every rule compiles down to a DataFusion query that
//! returns the *violating* rows. "No violating rows" is the pass condition, which
//! keeps the semantics uniform and makes each rule cheap to explain: `--verbose`
//! prints the SQL that ran.
//!
//! ```json
//! {
//!   "version": 1,
//!   "rules": [
//!     { "name": "order_id_is_unique", "table": "orders", "type": "unique",
//!       "columns": ["order_id"], "severity": "error" },
//!     { "name": "amount_is_positive", "table": "orders", "type": "expression",
//!       "predicate": "amount > 0" }
//!   ]
//! }
//! ```

use std::collections::HashMap;
use std::path::Path;
use std::time::Instant;

use anyhow::{Context, Result};
use datafusion::arrow::array::{ArrayRef, RecordBatch, StringArray, UInt64Array};
use datafusion::arrow::datatypes::{DataType, Field, Schema};
use datafusion::execution::context::SessionContext;
use datafusion::scalar::ScalarValue;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::catalog::TableEntry;
use crate::render::rows_of;
use crate::sqlutil::{ident, literal};

/// How badly a failure should be treated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    /// Fails the run (`lakewatch check` exits 1).
    #[default]
    Error,
    /// Reported, but does not fail the run.
    Warn,
}

impl Severity {
    fn label(&self) -> &'static str {
        match self {
            Severity::Error => "error",
            Severity::Warn => "warn",
        }
    }
}

/// Outcome of a single rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    /// No violations.
    Pass,
    /// Violations found.
    Fail,
    /// The rule could not be evaluated.
    Error,
}

impl Status {
    fn label(&self) -> &'static str {
        match self {
            Status::Pass => "pass",
            Status::Fail => "FAIL",
            Status::Error => "ERROR",
        }
    }
}

/// The supported assertion kinds.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RuleKind {
    /// The column combination must be unique.
    Unique {
        /// Columns forming the key.
        columns: Vec<String>,
    },
    /// The listed columns must never be NULL.
    NotNull {
        /// Columns to inspect.
        columns: Vec<String>,
    },
    /// A column may only contain the listed values.
    AcceptedValues {
        /// Column to inspect.
        column: String,
        /// Allowed values.
        values: Vec<String>,
    },
    /// A boolean expression must hold for every row.
    Expression {
        /// SQL predicate evaluated per row.
        predicate: String,
    },
    /// The table must contain between `min` and `max` rows.
    RowCount {
        /// Inclusive lower bound.
        #[serde(default)]
        min: Option<i64>,
        /// Inclusive upper bound.
        #[serde(default)]
        max: Option<i64>,
    },
    /// Free-form SQL returning a single value that must equal `expect`.
    Query {
        /// SQL to run.
        sql: String,
        /// Expected first-cell value, compared as text.
        expect: String,
    },
}

impl RuleKind {
    /// Stable name used in reports.
    pub fn name(&self) -> &'static str {
        match self {
            RuleKind::Unique { .. } => "unique",
            RuleKind::NotNull { .. } => "not_null",
            RuleKind::AcceptedValues { .. } => "accepted_values",
            RuleKind::Expression { .. } => "expression",
            RuleKind::RowCount { .. } => "row_count",
            RuleKind::Query { .. } => "query",
        }
    }
}

/// One rule as written in the rules file.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Rule {
    /// Stable rule name, shown in reports.
    pub name: String,
    /// Table the rule applies to; omitted for `query` rules.
    #[serde(default)]
    pub table: Option<String>,
    /// Severity; defaults to `error`.
    #[serde(default)]
    pub severity: Severity,
    /// Optional human explanation.
    #[serde(default)]
    pub description: Option<String>,
    /// The assertion itself.
    #[serde(flatten)]
    pub kind: RuleKind,
}

/// The parsed rules file.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct RulesFile {
    /// Rules-file format version.
    #[serde(default = "default_version")]
    pub version: u32,
    /// Optional suite name.
    #[serde(default)]
    pub name: Option<String>,
    /// The rules.
    pub rules: Vec<Rule>,
}

fn default_version() -> u32 {
    1
}

impl RulesFile {
    /// Parse a rules file from disk.
    pub fn load(path: &Path) -> Result<Self> {
        let raw = std::fs::read_to_string(path)
            .with_context(|| format!("cannot read rules file `{}`", path.display()))?;
        let parsed: RulesFile = serde_json::from_str(&raw)
            .with_context(|| format!("`{}` is not a valid rules file", path.display()))?;
        anyhow::ensure!(!parsed.rules.is_empty(), "rules file contains no rules");
        Ok(parsed)
    }

    /// Render the parsed file back to pretty JSON (used by `--print-rules`).
    pub fn to_json(&self) -> Result<String> {
        Ok(serde_json::to_string_pretty(self)?)
    }
}

/// Result of one rule.
#[derive(Debug, Clone, Serialize)]
pub struct RuleOutcome {
    /// Rule name.
    pub rule: String,
    /// Rule type.
    pub rule_type: String,
    /// Declared severity.
    pub severity: Severity,
    /// Evaluation status.
    pub status: Status,
    /// Number of violating rows, when the rule counts them.
    pub violations: Option<u64>,
    /// One-line explanation.
    pub message: String,
    /// Wall-clock duration in milliseconds.
    pub duration_ms: u128,
    /// Up to three violating rows, for failed rules.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sample: Option<Vec<Map<String, Value>>>,
    /// SQL that was executed, included by `--verbose`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sql: Option<String>,
}

/// Whole-suite result.
#[derive(Debug, Clone, Serialize)]
pub struct CheckReport {
    /// Rules-file name, when the file declares one.
    pub suite: Option<String>,
    /// Rules evaluated.
    pub total: usize,
    /// Rules that passed.
    pub passed: usize,
    /// Rules that failed.
    pub failed: usize,
    /// Rules that could not be evaluated.
    pub errored: usize,
    /// Failed rules with `warn` severity.
    pub warnings: usize,
    /// True when nothing failed: the CI gate.
    pub ok: bool,
    /// Per-rule outcomes, in file order.
    pub outcomes: Vec<RuleOutcome>,
}

impl CheckReport {
    /// Flatten to a `RecordBatch` for the shared renderer.
    pub fn to_batch(&self) -> Result<RecordBatch> {
        let schema = Schema::new(vec![
            Field::new("status", DataType::Utf8, false),
            Field::new("severity", DataType::Utf8, false),
            Field::new("rule", DataType::Utf8, false),
            Field::new("type", DataType::Utf8, false),
            Field::new("violations", DataType::UInt64, true),
            Field::new("ms", DataType::UInt64, false),
            Field::new("message", DataType::Utf8, false),
        ]);
        let columns: Vec<ArrayRef> = vec![
            std::sync::Arc::new(StringArray::from_iter_values(
                self.outcomes.iter().map(|o| o.status.label()),
            )),
            std::sync::Arc::new(StringArray::from_iter_values(
                self.outcomes.iter().map(|o| o.severity.label()),
            )),
            std::sync::Arc::new(StringArray::from_iter_values(
                self.outcomes.iter().map(|o| o.rule.clone()),
            )),
            std::sync::Arc::new(StringArray::from_iter_values(
                self.outcomes.iter().map(|o| o.rule_type.clone()),
            )),
            std::sync::Arc::new(UInt64Array::from(
                self.outcomes
                    .iter()
                    .map(|o| o.violations)
                    .collect::<Vec<_>>(),
            )),
            std::sync::Arc::new(UInt64Array::from_iter_values(
                self.outcomes.iter().map(|o| o.duration_ms as u64),
            )),
            std::sync::Arc::new(StringArray::from_iter_values(
                self.outcomes.iter().map(|o| o.message.clone()),
            )),
        ];
        Ok(RecordBatch::try_new(std::sync::Arc::new(schema), columns)?)
    }
}

/// Run every rule in `file` against the registered tables.
pub async fn run(
    ctx: &SessionContext,
    tables: &[TableEntry],
    file: &RulesFile,
    verbose: bool,
) -> Result<CheckReport> {
    let known: HashMap<&str, &TableEntry> = tables.iter().map(|t| (t.name.as_str(), t)).collect();
    let mut outcomes = Vec::with_capacity(file.rules.len());

    for rule in &file.rules {
        let outcome = evaluate(ctx, &known, rule, verbose).await;
        outcomes.push(outcome);
    }

    let passed = outcomes.iter().filter(|o| o.status == Status::Pass).count();
    let failed = outcomes.iter().filter(|o| o.status == Status::Fail).count();
    let errored = outcomes
        .iter()
        .filter(|o| o.status == Status::Error)
        .count();
    let warnings = outcomes
        .iter()
        .filter(|o| o.status == Status::Fail && o.severity == Severity::Warn)
        .count();
    let ok = outcomes
        .iter()
        .all(|o| o.status == Status::Pass || o.severity == Severity::Warn);

    Ok(CheckReport {
        suite: file.name.clone(),
        total: outcomes.len(),
        passed,
        failed,
        errored,
        warnings,
        ok,
        outcomes,
    })
}

async fn evaluate(
    ctx: &SessionContext,
    known: &HashMap<&str, &TableEntry>,
    rule: &Rule,
    verbose: bool,
) -> RuleOutcome {
    let started = Instant::now();
    let (mut result, sql): (RuleEvaluation, Option<String>);

    match &rule.kind {
        RuleKind::Query {
            sql: user_sql,
            expect,
        } => {
            result = evaluate_query(ctx, user_sql, expect).await;
            sql = Some(user_sql.clone());
        }
        _ => {
            let table_name = match rule.table.as_deref() {
                Some(name) => name,
                None => {
                    return RuleOutcome {
                        rule: rule.name.clone(),
                        rule_type: rule.kind.name().to_string(),
                        severity: rule.severity,
                        status: Status::Error,
                        violations: None,
                        message: "rule has no `table`".to_string(),
                        duration_ms: started.elapsed().as_millis(),
                        sample: None,
                        sql: None,
                    }
                }
            };
            if !known.contains_key(table_name) {
                return RuleOutcome {
                    rule: rule.name.clone(),
                    rule_type: rule.kind.name().to_string(),
                    severity: rule.severity,
                    status: Status::Error,
                    violations: None,
                    message: format!("unknown table `{table_name}` (known: {})", {
                        let mut names: Vec<&str> = known.keys().copied().collect();
                        names.sort_unstable();
                        if names.is_empty() {
                            "none".to_string()
                        } else {
                            names.join(", ")
                        }
                    }),
                    duration_ms: started.elapsed().as_millis(),
                    sample: None,
                    sql: None,
                };
            }
            match violations_sql(rule, table_name) {
                Ok((kind, statement)) => {
                    result = match kind {
                        ViolationKind::Rows => evaluate_rows(ctx, &statement, rule).await,
                        ViolationKind::Scalar => evaluate_scalar(ctx, &statement, rule).await,
                    };
                    sql = Some(statement);
                }
                Err(error) => {
                    return RuleOutcome {
                        rule: rule.name.clone(),
                        rule_type: rule.kind.name().to_string(),
                        severity: rule.severity,
                        status: Status::Error,
                        violations: None,
                        message: error.to_string(),
                        duration_ms: started.elapsed().as_millis(),
                        sample: None,
                        sql: None,
                    }
                }
            }
        }
    }

    let duration_ms = started.elapsed().as_millis();
    match result.as_mut() {
        Ok((status, violations, message, sample)) => RuleOutcome {
            rule: rule.name.clone(),
            rule_type: rule.kind.name().to_string(),
            severity: rule.severity,
            status: *status,
            violations: *violations,
            message: message.clone(),
            duration_ms,
            sample: sample.take(),
            sql: if verbose { sql } else { None },
        },
        Err(error) => RuleOutcome {
            rule: rule.name.clone(),
            rule_type: rule.kind.name().to_string(),
            severity: rule.severity,
            status: Status::Error,
            violations: None,
            message: error.to_string(),
            duration_ms,
            sample: None,
            sql: if verbose { sql } else { None },
        },
    }
}

/// What evaluating a rule produces: status, violation count, message, sample rows.
type RuleEvaluation = Result<(Status, Option<u64>, String, Option<Vec<Map<String, Value>>>)>;

/// Whether a rule counts violating rows or compares a scalar.
enum ViolationKind {
    Rows,
    Scalar,
}

/// Compile a rule into the SQL that produces its violating rows.
fn violations_sql(rule: &Rule, table: &str) -> Result<(ViolationKind, String)> {
    let table = ident(table);
    let sql = match &rule.kind {
        RuleKind::Unique { columns } => {
            anyhow::ensure!(!columns.is_empty(), "`unique` needs at least one column");
            let cols = columns
                .iter()
                .map(|c| ident(c))
                .collect::<Vec<_>>()
                .join(", ");
            format!(
                "SELECT {cols}, COUNT(*) AS _occurrences FROM {table} GROUP BY {cols} HAVING COUNT(*) > 1"
            )
        }
        RuleKind::NotNull { columns } => {
            anyhow::ensure!(!columns.is_empty(), "`not_null` needs at least one column");
            let predicate = columns
                .iter()
                .map(|c| format!("{} IS NULL", ident(c)))
                .collect::<Vec<_>>()
                .join(" OR ");
            format!("SELECT * FROM {table} WHERE ({predicate})")
        }
        RuleKind::AcceptedValues { column, values } => {
            anyhow::ensure!(
                !values.is_empty(),
                "`accepted_values` needs at least one value"
            );
            let list = values
                .iter()
                .map(|v| literal(v))
                .collect::<Vec<_>>()
                .join(", ");
            let column_ident = ident(column);
            format!(
                "SELECT {column_ident}, COUNT(*) AS _occurrences FROM {table} \
                 WHERE CAST({column_ident} AS VARCHAR) NOT IN ({list}) GROUP BY {column_ident}"
            )
        }
        RuleKind::Expression { predicate } => {
            anyhow::ensure!(
                !predicate.trim().is_empty(),
                "`expression` needs a predicate"
            );
            format!("SELECT * FROM {table} WHERE COALESCE(({predicate}), false) = false")
        }
        RuleKind::RowCount { .. } => {
            return Ok((
                ViolationKind::Scalar,
                format!("SELECT COUNT(*) FROM {table}"),
            ))
        }
        RuleKind::Query { sql, .. } => return Ok((ViolationKind::Scalar, sql.clone())),
    };
    Ok((ViolationKind::Rows, sql))
}

/// Count violating rows and collect a small sample.
async fn evaluate_rows(ctx: &SessionContext, violations_sql: &str, _rule: &Rule) -> RuleEvaluation {
    let count_sql = format!("SELECT COUNT(*) AS n FROM ({violations_sql}) AS lakewatch_violations");
    let batches = ctx.sql(&count_sql).await?.collect().await?;
    let count = scalar_u64(&batches).unwrap_or(0);

    if count == 0 {
        return Ok((Status::Pass, Some(0), "no violating rows".to_string(), None));
    }

    let sample_sql = format!("{violations_sql} LIMIT 3");
    let sample = ctx
        .sql(&sample_sql)
        .await?
        .collect()
        .await
        .unwrap_or_default();
    let rows: Vec<Map<String, Value>> = sample.iter().flat_map(rows_of).collect();
    Ok((
        Status::Fail,
        Some(count),
        format!("{count} violating row(s)"),
        Some(rows),
    ))
}

/// Evaluate a rule that compares one scalar.
async fn evaluate_scalar(ctx: &SessionContext, scalar_sql: &str, rule: &Rule) -> RuleEvaluation {
    match &rule.kind {
        RuleKind::RowCount { min, max } => {
            let batches = ctx.sql(scalar_sql).await?.collect().await?;
            let count = scalar_u64(&batches).unwrap_or(0) as i64;
            let lower_ok = min.map(|min| count >= min).unwrap_or(true);
            let upper_ok = max.map(|max| count <= max).unwrap_or(true);
            let bounds = format!(
                "min={} max={}",
                min.map(|v| v.to_string())
                    .unwrap_or_else(|| "-".to_string()),
                max.map(|v| v.to_string())
                    .unwrap_or_else(|| "-".to_string())
            );
            if lower_ok && upper_ok {
                Ok((
                    Status::Pass,
                    Some(count.max(0) as u64),
                    format!("{count} rows within {bounds}"),
                    None,
                ))
            } else {
                Ok((
                    Status::Fail,
                    Some(count.max(0) as u64),
                    format!("{count} rows outside {bounds}"),
                    None,
                ))
            }
        }
        RuleKind::Query { expect, .. } => {
            let batches = ctx.sql(scalar_sql).await?.collect().await?;
            let actual = scalar_text(&batches);
            let matched = actual.as_deref() == Some(expect.as_str());
            let shown = actual.clone().unwrap_or_else(|| "NULL".to_string());
            Ok((
                if matched { Status::Pass } else { Status::Fail },
                Some(if matched { 0 } else { 1 }),
                format!("expected `{expect}`, got `{shown}`"),
                None,
            ))
        }
        _ => anyhow::bail!("internal error: rule is not scalar"),
    }
}

async fn evaluate_query(ctx: &SessionContext, sql: &str, expect: &str) -> RuleEvaluation {
    let batches = ctx.sql(sql).await?.collect().await?;
    let actual = scalar_text(&batches);
    let matched = actual.as_deref() == Some(expect);
    let shown = actual.clone().unwrap_or_else(|| "NULL".to_string());
    Ok((
        if matched { Status::Pass } else { Status::Fail },
        Some(if matched { 0 } else { 1 }),
        format!("expected `{expect}`, got `{shown}`"),
        None,
    ))
}

fn scalar_u64(batches: &[RecordBatch]) -> Option<u64> {
    let batch = batches.iter().find(|b| b.num_rows() > 0)?;
    match ScalarValue::try_from_array(batch.column(0), 0).ok()? {
        ScalarValue::UInt64(v) => v,
        ScalarValue::Int64(v) => v.map(|v| v.max(0) as u64),
        ScalarValue::Int32(v) => v.map(|v| v.max(0) as u64),
        ScalarValue::Float64(v) => v.map(|v| v.max(0.0) as u64),
        other => other.to_string().parse().ok(),
    }
}

fn scalar_text(batches: &[RecordBatch]) -> Option<String> {
    let batch = batches.iter().find(|b| b.num_rows() > 0)?;
    let scalar = ScalarValue::try_from_array(batch.column(0), 0).ok()?;
    if scalar.is_null() {
        return None;
    }
    match scalar {
        ScalarValue::Utf8(v) | ScalarValue::LargeUtf8(v) => v,
        ScalarValue::Utf8View(v) => v,
        other => Some(other.to_string()),
    }
}
