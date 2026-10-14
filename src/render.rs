//! Formatting `RecordBatch`es for humans, files and other programs.

use std::io::Write;
use std::path::Path;
use std::str::FromStr;

use anyhow::{Context, Result};
use datafusion::arrow::array::RecordBatch;
use datafusion::arrow::datatypes::{DataType, TimeUnit};
use datafusion::arrow::util::pretty::pretty_format_batches;
use datafusion::scalar::ScalarValue;
use serde_json::{Map, Value};

/// Where a result set should go.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputFormat {
    Table,
    Json,
    Ndjson,
    Csv,
    Parquet,
}

impl FromStr for OutputFormat {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self> {
        Ok(match s.to_ascii_lowercase().as_str() {
            "table" | "pretty" => OutputFormat::Table,
            "json" => OutputFormat::Json,
            "ndjson" | "jsonl" => OutputFormat::Ndjson,
            "csv" => OutputFormat::Csv,
            "parquet" => OutputFormat::Parquet,
            other => anyhow::bail!(
                "unknown format `{other}` (expected table, json, ndjson, csv or parquet)"
            ),
        })
    }
}

/// Render `batches` in `format`, either to stdout or to `out`.
pub fn write_batches(
    batches: &[RecordBatch],
    format: OutputFormat,
    out: Option<&Path>,
) -> Result<()> {
    let rows: usize = batches.iter().map(|b| b.num_rows()).sum();
    match format {
        OutputFormat::Table => {
            let rendered = if batches.is_empty() || batches.iter().all(|b| b.num_columns() == 0) {
                String::new()
            } else {
                pretty_format_batches(batches)
                    .context("cannot render result set")?
                    .to_string()
            };
            emit(&rendered, out)?;
            if out.is_none() {
                eprintln!("({rows} row{})", if rows == 1 { "" } else { "s" });
            }
        }
        OutputFormat::Json | OutputFormat::Ndjson => {
            let mut buf = String::new();
            if format == OutputFormat::Json {
                buf.push_str("[\n");
            }
            let mut first = true;
            for batch in batches {
                for row in rows_of(batch) {
                    if format == OutputFormat::Json {
                        buf.push_str(if first { "  " } else { ",\n  " });
                        buf.push_str(&serde_json::to_string(&row)?);
                    } else {
                        buf.push_str(&serde_json::to_string(&row)?);
                        buf.push('\n');
                    }
                    first = false;
                }
            }
            if format == OutputFormat::Json {
                buf.push_str("\n]\n");
            }
            emit(&buf, out)?;
        }
        OutputFormat::Csv => {
            let mut buf = String::new();
            if let Some(batch) = batches.first() {
                let header: Vec<String> = batch
                    .schema()
                    .fields()
                    .iter()
                    .map(|f| csv_field(f.name()))
                    .collect();
                buf.push_str(&header.join(","));
                buf.push('\n');
            }
            for batch in batches {
                for row in rows_of(batch) {
                    let cells: Vec<String> = row
                        .values()
                        .map(|v| match v {
                            Value::Null => String::new(),
                            Value::String(s) => csv_field(s),
                            other => other.to_string(),
                        })
                        .collect();
                    buf.push_str(&cells.join(","));
                    buf.push('\n');
                }
            }
            emit(&buf, out)?;
        }
        OutputFormat::Parquet => {
            let dir = out.context("`--format parquet` requires `--out <DIR>`")?;
            anyhow::bail!(
                "parquet output writes a directory, not a single file: `{}`",
                dir.display()
            );
        }
    }
    Ok(())
}

/// Write `text` to `out`, or to stdout when no file is given.
pub fn emit(text: &str, out: Option<&Path>) -> Result<()> {
    match out {
        Some(path) => {
            let mut file = std::fs::File::create(path)
                .with_context(|| format!("cannot write `{}`", path.display()))?;
            file.write_all(text.as_bytes())?;
        }
        None => {
            let stdout = std::io::stdout();
            let mut lock = stdout.lock();
            lock.write_all(text.as_bytes())?;
        }
    }
    Ok(())
}

fn csv_field(raw: &str) -> String {
    if raw.contains(',') || raw.contains('"') || raw.contains('\n') || raw.contains('\r') {
        format!("\"{}\"", raw.replace('"', "\"\""))
    } else {
        raw.to_string()
    }
}

/// Every row of a batch as a JSON object.
pub fn rows_of(batch: &RecordBatch) -> Vec<Map<String, Value>> {
    let mut rows = Vec::with_capacity(batch.num_rows());
    for row in 0..batch.num_rows() {
        let mut object = Map::new();
        for (col, field) in batch.schema().fields().iter().enumerate() {
            let value = ScalarValue::try_from_array(batch.column(col), row)
                .map(|sv| scalar_to_json(&sv))
                .unwrap_or(Value::Null);
            object.insert(field.name().clone(), value);
        }
        rows.push(object);
    }
    rows
}

/// Convert one Arrow scalar into JSON without losing the common types.
pub fn scalar_to_json(value: &ScalarValue) -> Value {
    match value {
        ScalarValue::Null => Value::Null,
        ScalarValue::Boolean(v) => v.map_or(Value::Null, Value::Bool),
        ScalarValue::Int8(v) => v.map_or(Value::Null, |v| Value::from(v as i64)),
        ScalarValue::Int16(v) => v.map_or(Value::Null, |v| Value::from(v as i64)),
        ScalarValue::Int32(v) => v.map_or(Value::Null, |v| Value::from(v as i64)),
        ScalarValue::Int64(v) => v.map_or(Value::Null, Value::from),
        ScalarValue::UInt8(v) => v.map_or(Value::Null, |v| Value::from(v as u64)),
        ScalarValue::UInt16(v) => v.map_or(Value::Null, |v| Value::from(v as u64)),
        ScalarValue::UInt32(v) => v.map_or(Value::Null, |v| Value::from(v as u64)),
        ScalarValue::UInt64(v) => v.map_or(Value::Null, Value::from),
        ScalarValue::Float32(v) => v.map_or(Value::Null, |v| {
            serde_json::Number::from_f64(v as f64).map_or(Value::Null, Value::Number)
        }),
        ScalarValue::Float64(v) => v.map_or(Value::Null, |v| {
            serde_json::Number::from_f64(v).map_or(Value::Null, Value::Number)
        }),
        ScalarValue::Utf8(v) | ScalarValue::LargeUtf8(v) => match v {
            Some(s) => Value::String(s.clone()),
            None => Value::Null,
        },
        ScalarValue::Utf8View(v) => match v {
            Some(s) => Value::String(s.clone()),
            None => Value::Null,
        },
        ScalarValue::Date32(days) => days.map_or(Value::Null, |d| {
            Value::String(
                chrono::NaiveDate::from_num_days_from_ce_opt(d + 719_163)
                    .map(|d| d.format("%Y-%m-%d").to_string())
                    .unwrap_or_else(|| d.to_string()),
            )
        }),
        ScalarValue::TimestampSecond(v, _) => timestamp_json(v, 1_000_000_000),
        ScalarValue::TimestampMillisecond(v, _) => timestamp_json(v, 1_000_000),
        ScalarValue::TimestampMicrosecond(v, _) => timestamp_json(v, 1_000),
        ScalarValue::TimestampNanosecond(v, _) => timestamp_json(v, 1),
        ScalarValue::Binary(v) | ScalarValue::LargeBinary(v) | ScalarValue::BinaryView(v) => {
            match v {
                Some(bytes) => Value::String(format!("<{} bytes>", bytes.len())),
                None => Value::Null,
            }
        }
        other => Value::String(other.to_string()),
    }
}

fn timestamp_json(value: &Option<i64>, per_second: i64) -> Value {
    match value {
        None => Value::Null,
        Some(raw) => {
            let micros = raw.div_euclid(per_second / 1000);
            let formatted = chrono::DateTime::from_timestamp_micros(micros)
                .map(|dt| dt.format("%Y-%m-%dT%H:%M:%S%.6fZ").to_string())
                .unwrap_or_else(|| raw.to_string());
            Value::String(formatted)
        }
    }
}

/// Human-readable Arrow type name, used by `tables` and `profile`.
pub fn type_name(data_type: &DataType) -> String {
    match data_type {
        DataType::Timestamp(unit, tz) => match tz {
            Some(tz) => format!("timestamp[{unit:?},{tz}]"),
            None => format!("timestamp[{unit:?}]"),
        },
        DataType::Decimal128(p, s) => format!("decimal({p},{s})"),
        other => {
            let _ = TimeUnit::Second;
            format!("{other:?}").to_ascii_lowercase()
        }
    }
}
