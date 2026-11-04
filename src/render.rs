//! Formatting `RecordBatch`es for humans, files and other programs.

use std::collections::HashSet;
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
                let header: Vec<String> =
                    unique_names(batch).iter().map(|n| csv_field(n)).collect();
                buf.push_str(&header.join(","));
                buf.push('\n');
            }
            for batch in batches {
                for cells in cells_of(batch) {
                    let rendered: Vec<String> = cells
                        .iter()
                        .map(|v| match v {
                            Value::Null => String::new(),
                            Value::String(s) => csv_field(s),
                            other => other.to_string(),
                        })
                        .collect();
                    buf.push_str(&rendered.join(","));
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
///
/// JSON objects cannot hold two keys with the same name, but a query can legitimately return two
/// columns with the same name (`SELECT a.s, b.s FROM a JOIN b \u2026`). Rather than dropping one of
/// them, the second and later occurrences are renamed by [`unique_names`].
pub fn rows_of(batch: &RecordBatch) -> Vec<Map<String, Value>> {
    let names = unique_names(batch);
    let mut rows = Vec::with_capacity(batch.num_rows());
    for row in 0..batch.num_rows() {
        let mut object = Map::new();
        for (col, name) in names.iter().enumerate() {
            let value = ScalarValue::try_from_array(batch.column(col), row)
                .map(|sv| scalar_to_json(&sv))
                .unwrap_or(Value::Null);
            object.insert(name.clone(), value);
        }
        rows.push(object);
    }
    rows
}

/// Every row of a batch, with the cells in column order.
///
/// The CSV writer takes its header from the schema and its rows from here, so the two have to
/// agree cell for cell. Counting by name instead of by position is what let a three-column result
/// print a three-name header above two-value rows.
pub fn cells_of(batch: &RecordBatch) -> Vec<Vec<Value>> {
    let mut rows = Vec::with_capacity(batch.num_rows());
    for row in 0..batch.num_rows() {
        let mut cells = Vec::with_capacity(batch.num_columns());
        for col in 0..batch.num_columns() {
            let value = ScalarValue::try_from_array(batch.column(col), row)
                .map(|sv| scalar_to_json(&sv))
                .unwrap_or(Value::Null);
            cells.push(value);
        }
        rows.push(cells);
    }
    rows
}

/// The field names of `batch`, with duplicates made unique.
///
/// The second and later occurrence of a name gets a `_2`, `_3`, \u2026 suffix, chosen so the result
/// is unique even when the batch already contains a column with the suffixed name.
fn unique_names(batch: &RecordBatch) -> Vec<String> {
    let mut used: HashSet<String> = HashSet::new();
    let mut names = Vec::with_capacity(batch.num_columns());
    for field in batch.schema().fields() {
        let base = field.name().as_str();
        let mut candidate = base.to_string();
        let mut n = 2;
        while !used.insert(candidate.clone()) {
            candidate = format!("{base}_{n}");
            n += 1;
        }
        names.push(candidate);
    }
    names
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
        ScalarValue::TimestampSecond(v, _) => timestamp_json(v, 1),
        ScalarValue::TimestampMillisecond(v, _) => timestamp_json(v, 1_000),
        ScalarValue::TimestampMicrosecond(v, _) => timestamp_json(v, 1_000_000),
        ScalarValue::TimestampNanosecond(v, _) => timestamp_json(v, 1_000_000_000),
        ScalarValue::Binary(v) | ScalarValue::LargeBinary(v) | ScalarValue::BinaryView(v) => {
            match v {
                Some(bytes) => Value::String(format!("<{} bytes>", bytes.len())),
                None => Value::Null,
            }
        }
        other => Value::String(other.to_string()),
    }
}

/// Render a timestamp as an ISO-8601 string.
///
/// `units_per_second` is how many of the value's units make one second, so the
/// value is first normalised to nanoseconds in `i128` (an `i64` of nanoseconds
/// only spans ~292 years) and then to microseconds for formatting.
fn timestamp_json(value: &Option<i64>, units_per_second: i64) -> Value {
    match value {
        None => Value::Null,
        Some(raw) => {
            let nanos = (*raw as i128) * (1_000_000_000i128 / units_per_second as i128);
            let micros = nanos.div_euclid(1_000) as i64;
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

#[cfg(test)]
mod tests {
    use super::*;

    fn rendered(value: ScalarValue) -> serde_json::Value {
        scalar_to_json(&value)
    }

    #[test]
    fn renders_every_timestamp_precision() {
        // 2024-03-15T12:34:56.789Z expressed in each Arrow unit.
        assert_eq!(
            rendered(ScalarValue::TimestampSecond(Some(1_710_506_096), None)),
            serde_json::json!("2024-03-15T12:34:56.000000Z")
        );
        assert_eq!(
            rendered(ScalarValue::TimestampMillisecond(
                Some(1_710_506_096_789),
                None
            )),
            serde_json::json!("2024-03-15T12:34:56.789000Z")
        );
        assert_eq!(
            rendered(ScalarValue::TimestampMicrosecond(
                Some(1_710_506_096_789_000),
                None
            )),
            serde_json::json!("2024-03-15T12:34:56.789000Z")
        );
        // Regression: nanosecond values used to divide by zero here.
        assert_eq!(
            rendered(ScalarValue::TimestampNanosecond(
                Some(1_710_506_096_789_000_000),
                None
            )),
            serde_json::json!("2024-03-15T12:34:56.789000Z")
        );
    }

    #[test]
    fn renders_timestamps_beyond_the_i64_nanosecond_range() {
        // 2260-01-01: 9.15e18 nanoseconds does not fit in the arithmetic an i64
        // intermediate would use, but must still render.
        assert_eq!(
            rendered(ScalarValue::TimestampNanosecond(
                Some(9_151_488_000_000_000_000),
                None
            )),
            serde_json::json!("2260-01-01T00:00:00.000000Z")
        );
    }

    #[test]
    fn renders_null_and_date_values() {
        assert_eq!(
            rendered(ScalarValue::TimestampNanosecond(None, None)),
            serde_json::Value::Null
        );
        // 2024-03-15 as days since the Unix epoch.
        assert_eq!(
            rendered(ScalarValue::Date32(Some(19_797))),
            serde_json::json!("2024-03-15")
        );
    }

    #[test]
    fn renders_scalars_without_losing_type() {
        assert_eq!(rendered(ScalarValue::Null), serde_json::Value::Null);
        assert_eq!(
            rendered(ScalarValue::Boolean(Some(true))),
            serde_json::json!(true)
        );
        assert_eq!(
            rendered(ScalarValue::Int64(Some(-7))),
            serde_json::json!(-7)
        );
        assert_eq!(rendered(ScalarValue::UInt64(Some(7))), serde_json::json!(7));
        assert_eq!(
            rendered(ScalarValue::Float64(Some(1.5))),
            serde_json::json!(1.5)
        );
        assert_eq!(
            rendered(ScalarValue::Utf8(Some("x".to_string()))),
            serde_json::json!("x")
        );
        assert_eq!(
            rendered(ScalarValue::Float64(Some(f64::NAN))),
            serde_json::Value::Null
        );
    }

    fn duplicate_name_batch() -> RecordBatch {
        use datafusion::arrow::array::{ArrayRef, Int64Array};
        use datafusion::arrow::datatypes::{Field, Schema};
        use std::sync::Arc;

        let schema = Arc::new(Schema::new(vec![
            Field::new("s", DataType::Int64, true),
            Field::new("s", DataType::Int64, true),
        ]));
        RecordBatch::try_new(
            schema,
            vec![
                Arc::new(Int64Array::from(vec![1])) as ArrayRef,
                Arc::new(Int64Array::from(vec![2])) as ArrayRef,
            ],
        )
        .unwrap()
    }

    /// A query can return two columns with the same name (`SELECT a.s, b.s FROM a JOIN b …`).
    /// `rows_of` used to key its object by column name, so the second `s` overwrote the first and
    /// the CSV row came out one cell narrower than its own header.
    #[test]
    fn keeps_columns_that_share_a_name() {
        let batch = duplicate_name_batch();

        let cells = cells_of(&batch);
        assert_eq!(cells.len(), 1);
        assert_eq!(cells[0].len(), 2, "both cells survive, counted by position");
        assert_eq!(cells[0][0], serde_json::json!(1));
        assert_eq!(cells[0][1], serde_json::json!(2));

        let rows = rows_of(&batch);
        assert_eq!(rows[0].len(), 2, "no key is dropped");
        assert_eq!(rows[0].get("s"), Some(&serde_json::json!(1)));
        assert_eq!(rows[0].get("s_2"), Some(&serde_json::json!(2)));

        assert_eq!(
            unique_names(&batch),
            vec!["s".to_string(), "s_2".to_string()]
        );
    }

    /// The header comes from the schema and the rows from `cells_of`, so the two must always be
    /// the same width — which is exactly what the duplicate-name bug broke.
    #[test]
    fn csv_header_matches_row_width_with_duplicate_names() {
        let dir = std::env::temp_dir().join("lakewatch-duplicate-column-names.csv");
        write_batches(&[duplicate_name_batch()], OutputFormat::Csv, Some(&dir)).unwrap();
        let text = std::fs::read_to_string(&dir).unwrap();
        let _ = std::fs::remove_file(&dir);

        let lines: Vec<&str> = text.trim_end().split('\n').collect();
        assert_eq!(lines[0], "s,s_2");
        assert_eq!(lines[1], "1,2");
        assert_eq!(
            lines[0].split(',').count(),
            lines[1].split(',').count(),
            "header and rows must be the same width"
        );
    }
}
