//! Column profiling: one aggregate query per dataset, transposed into a report.
//!
//! Everything here is a single DataFusion scan. Instead of running one query per
//! column (which would re-read the files once per column), the module builds a
//! wide `SELECT` with every aggregate aliased `c<i>_<stat>` and then transposes the
//! single result row back into one row per column.

use std::collections::HashMap;

use anyhow::{Context, Result};
use datafusion::arrow::array::{ArrayRef, Float64Array, RecordBatch, StringArray, UInt64Array};
use datafusion::arrow::datatypes::{DataType, Field, Schema};
use datafusion::execution::context::SessionContext;
use datafusion::scalar::ScalarValue;
use serde::Serialize;

use crate::render::type_name;
use crate::sqlutil::ident;

/// Options for a profiling run.
#[derive(Debug, Clone)]
pub struct ProfileOptions {
    /// SQL whose output schema is profiled.
    pub input_sql: String,
    /// Human-readable label for the report.
    pub source: String,
    /// Include 25th/50th/75th percentiles for numeric columns.
    pub quantiles: bool,
}

/// Profile of one column.
#[derive(Debug, Clone, Serialize)]
pub struct ColumnProfile {
    /// Column name.
    pub column: String,
    /// Arrow type name.
    pub data_type: String,
    /// Number of NULL values.
    pub nulls: u64,
    /// Share of NULL values, `0.0..=1.0`.
    pub null_ratio: f64,
    /// Number of distinct non-NULL values.
    pub distinct: u64,
    /// Smallest value, rendered as text.
    pub min: Option<String>,
    /// Largest value, rendered as text.
    pub max: Option<String>,
    /// Arithmetic mean, numeric columns only.
    pub mean: Option<f64>,
    /// 25th percentile, numeric columns only.
    pub p25: Option<f64>,
    /// Median, numeric columns only.
    pub p50: Option<f64>,
    /// 75th percentile, numeric columns only.
    pub p75: Option<f64>,
    /// Most frequent non-NULL value, via the custom `mode` UDAF.
    pub top: Option<String>,
}

/// Profile of one dataset.
#[derive(Debug, Clone, Serialize)]
pub struct ProfileReport {
    /// Dataset or query label.
    pub source: String,
    /// Total rows.
    pub rows: u64,
    /// Column count.
    pub column_count: usize,
    /// Per-column profiles, in schema order.
    pub columns: Vec<ColumnProfile>,
    /// The aggregate query that produced the report.
    pub query: String,
}

impl ProfileReport {
    /// Flatten the report into a single `RecordBatch` for the shared renderer.
    pub fn to_batch(&self) -> Result<RecordBatch> {
        let schema = Schema::new(vec![
            Field::new("column", DataType::Utf8, false),
            Field::new("type", DataType::Utf8, false),
            Field::new("nulls", DataType::UInt64, true),
            Field::new("null_ratio", DataType::Float64, true),
            Field::new("distinct", DataType::UInt64, true),
            Field::new("min", DataType::Utf8, true),
            Field::new("max", DataType::Utf8, true),
            Field::new("mean", DataType::Float64, true),
            Field::new("p25", DataType::Float64, true),
            Field::new("p50", DataType::Float64, true),
            Field::new("p75", DataType::Float64, true),
            Field::new("top", DataType::Utf8, true),
        ]);
        let columns: Vec<ArrayRef> = vec![
            std::sync::Arc::new(StringArray::from_iter_values(
                self.columns.iter().map(|c| c.column.clone()),
            )),
            std::sync::Arc::new(StringArray::from_iter_values(
                self.columns.iter().map(|c| c.data_type.clone()),
            )),
            std::sync::Arc::new(UInt64Array::from_iter_values(
                self.columns.iter().map(|c| c.nulls),
            )),
            std::sync::Arc::new(Float64Array::from_iter_values(
                self.columns.iter().map(|c| round(c.null_ratio)),
            )),
            std::sync::Arc::new(UInt64Array::from_iter_values(
                self.columns.iter().map(|c| c.distinct),
            )),
            std::sync::Arc::new(StringArray::from(
                self.columns
                    .iter()
                    .map(|c| c.min.clone())
                    .collect::<Vec<_>>(),
            )),
            std::sync::Arc::new(StringArray::from(
                self.columns
                    .iter()
                    .map(|c| c.max.clone())
                    .collect::<Vec<_>>(),
            )),
            std::sync::Arc::new(Float64Array::from(
                self.columns
                    .iter()
                    .map(|c| c.mean.map(round))
                    .collect::<Vec<_>>(),
            )),
            std::sync::Arc::new(Float64Array::from(
                self.columns
                    .iter()
                    .map(|c| c.p25.map(round))
                    .collect::<Vec<_>>(),
            )),
            std::sync::Arc::new(Float64Array::from(
                self.columns
                    .iter()
                    .map(|c| c.p50.map(round))
                    .collect::<Vec<_>>(),
            )),
            std::sync::Arc::new(Float64Array::from(
                self.columns
                    .iter()
                    .map(|c| c.p75.map(round))
                    .collect::<Vec<_>>(),
            )),
            std::sync::Arc::new(StringArray::from(
                self.columns
                    .iter()
                    .map(|c| c.top.clone())
                    .collect::<Vec<_>>(),
            )),
        ];
        Ok(RecordBatch::try_new(std::sync::Arc::new(schema), columns)?)
    }
}

fn round(value: f64) -> f64 {
    (value * 1_000_000.0).round() / 1_000_000.0
}

/// Build and run the profiling query.
pub async fn profile(ctx: &SessionContext, options: &ProfileOptions) -> Result<ProfileReport> {
    let planned = ctx
        .sql(&options.input_sql)
        .await
        .with_context(|| format!("cannot plan profile input `{}`", options.source))?;
    let fields: Vec<(String, DataType)> = planned
        .schema()
        .fields()
        .iter()
        .map(|f| (f.name().clone(), f.data_type().clone()))
        .collect();
    anyhow::ensure!(!fields.is_empty(), "profile input has no columns");

    let sql = build_query(&options.input_sql, &fields, options.quantiles);
    let batches = ctx.sql(&sql).await?.collect().await?;
    let batch = batches
        .into_iter()
        .find(|b| b.num_rows() > 0)
        .context("profiling query returned no rows")?;

    let schema = batch.schema();
    let index: HashMap<&str, usize> = schema
        .fields()
        .iter()
        .enumerate()
        .map(|(i, f)| (f.name().as_str(), i))
        .collect();

    let rows = u64_at(&batch, index["total_rows"]).unwrap_or(0);
    let mut columns = Vec::with_capacity(fields.len());
    for (position, (name, data_type)) in fields.iter().enumerate() {
        let numeric = data_type.is_numeric();
        let pick = |suffix: &str| index.get(format!("c{position}_{suffix}").as_str()).copied();
        let nulls = pick("nulls").and_then(|i| u64_at(&batch, i)).unwrap_or(0);
        columns.push(ColumnProfile {
            column: name.clone(),
            data_type: type_name(data_type),
            nulls,
            null_ratio: if rows == 0 {
                0.0
            } else {
                nulls as f64 / rows as f64
            },
            distinct: pick("distinct")
                .and_then(|i| u64_at(&batch, i))
                .unwrap_or(0),
            min: pick("min").and_then(|i| text_at(&batch, i)),
            max: pick("max").and_then(|i| text_at(&batch, i)),
            mean: if numeric {
                pick("mean").and_then(|i| f64_at(&batch, i))
            } else {
                None
            },
            p25: if numeric {
                pick("p25").and_then(|i| f64_at(&batch, i))
            } else {
                None
            },
            p50: if numeric {
                pick("p50").and_then(|i| f64_at(&batch, i))
            } else {
                None
            },
            p75: if numeric {
                pick("p75").and_then(|i| f64_at(&batch, i))
            } else {
                None
            },
            top: pick("top").and_then(|i| text_at(&batch, i)),
        });
    }

    Ok(ProfileReport {
        source: options.source.clone(),
        rows,
        column_count: columns.len(),
        columns,
        query: sql,
    })
}

/// Assemble the wide aggregate statement.
fn build_query(input_sql: &str, fields: &[(String, DataType)], quantiles: bool) -> String {
    let mut expressions = vec!["COUNT(*) AS total_rows".to_string()];
    for (position, (name, data_type)) in fields.iter().enumerate() {
        let column = ident(name);
        expressions.push(format!("(COUNT(*) - COUNT({column})) AS c{position}_nulls"));
        expressions.push(format!("COUNT(DISTINCT {column}) AS c{position}_distinct"));
        if !data_type.is_nested() {
            expressions.push(format!("CAST(MIN({column}) AS VARCHAR) AS c{position}_min"));
            expressions.push(format!("CAST(MAX({column}) AS VARCHAR) AS c{position}_max"));
            expressions.push(format!("mode({column}) AS c{position}_top"));
        }
        if data_type.is_numeric() {
            let cast = format!("CAST({column} AS DOUBLE)");
            expressions.push(format!("AVG({cast}) AS c{position}_mean"));
            let percentiles: &[(&str, f64)] = if quantiles {
                &[("p25", 0.25), ("p50", 0.5), ("p75", 0.75)]
            } else {
                &[("p50", 0.5)]
            };
            for (suffix, q) in percentiles {
                expressions.push(format!(
                    "approx_percentile_cont({cast}, {q}) AS c{position}_{suffix}"
                ));
            }
        }
    }
    format!(
        "SELECT {} FROM ({input_sql}) AS lakewatch_input",
        expressions.join(",\n       ")
    )
}

fn u64_at(batch: &RecordBatch, index: usize) -> Option<u64> {
    let scalar = ScalarValue::try_from_array(batch.column(index), 0).ok()?;
    match scalar {
        ScalarValue::UInt64(v) => v,
        ScalarValue::Int64(v) => v.map(|v| v.max(0) as u64),
        ScalarValue::Int32(v) => v.map(|v| v.max(0) as u64),
        ScalarValue::UInt32(v) => v.map(u64::from),
        ScalarValue::Float64(v) => v.map(|v| v.max(0.0) as u64),
        other => other.to_string().parse().ok(),
    }
}

fn f64_at(batch: &RecordBatch, index: usize) -> Option<f64> {
    let scalar = ScalarValue::try_from_array(batch.column(index), 0).ok()?;
    match scalar {
        ScalarValue::Float64(v) => v,
        ScalarValue::Float32(v) => v.map(f64::from),
        ScalarValue::Int64(v) => v.map(|v| v as f64),
        other if other.is_null() => None,
        other => other.to_string().parse().ok(),
    }
}

fn text_at(batch: &RecordBatch, index: usize) -> Option<String> {
    let scalar = ScalarValue::try_from_array(batch.column(index), 0).ok()?;
    if scalar.is_null() {
        return None;
    }
    match scalar {
        ScalarValue::Utf8(v) | ScalarValue::LargeUtf8(v) => v,
        ScalarValue::Utf8View(v) => v,
        other => Some(other.to_string()),
    }
}
