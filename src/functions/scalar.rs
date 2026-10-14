//! The scalar functions (UDFs).
//!
//! All three are `Volatility::Immutable`, which lets DataFusion constant-fold
//! them and push them into scans. Each one handles both the `ColumnarValue::Scalar`
//! and `ColumnarValue::Array` cases so the same function works on literals and on
//! columns, and casts its input to `Utf8` first so it also works on
//! `Utf8View`/`LargeUtf8`/dictionary-encoded columns.

use std::sync::Arc;

use datafusion::arrow::array::{Array, ArrayRef, BooleanArray, StringArray};
use datafusion::arrow::compute::cast;
use datafusion::arrow::datatypes::DataType;
use datafusion::common::{DataFusionError, Result as DfResult};
use datafusion::logical_expr::{create_udf, ColumnarValue, ScalarUDF, Volatility};
use datafusion::scalar::ScalarValue;

/// `is_email(text) -> boolean`
pub fn is_email() -> ScalarUDF {
    create_udf(
        "is_email",
        vec![DataType::Utf8],
        DataType::Boolean,
        Volatility::Immutable,
        Arc::new(|args: &[ColumnarValue]| map_utf8_bool(&args[0], |value| value.map(valid_email))),
    )
}

/// `mask_email(text) -> utf8`
pub fn mask_email() -> ScalarUDF {
    create_udf(
        "mask_email",
        vec![DataType::Utf8],
        DataType::Utf8,
        Volatility::Immutable,
        Arc::new(|args: &[ColumnarValue]| {
            map_utf8(&args[0], |value| {
                value.map(mask).or_else(|| Some("***".to_string()))
            })
        }),
    )
}

/// `normalize_text(text) -> utf8`
pub fn normalize_text() -> ScalarUDF {
    create_udf(
        "normalize_text",
        vec![DataType::Utf8],
        DataType::Utf8,
        Volatility::Immutable,
        Arc::new(|args: &[ColumnarValue]| map_utf8(&args[0], |value| value.map(normalize))),
    )
}

/// Apply `f` to every element of a text argument, preserving scalar-ness.
fn map_utf8<F>(arg: &ColumnarValue, f: F) -> DfResult<ColumnarValue>
where
    F: Fn(Option<&str>) -> Option<String> + Send + Sync,
{
    match arg {
        ColumnarValue::Scalar(scalar) => {
            let value = match scalar.cast_to(&DataType::Utf8)? {
                ScalarValue::Utf8(v) => v,
                _ => None,
            };
            Ok(ColumnarValue::Scalar(ScalarValue::Utf8(
                f(value.as_deref()),
            )))
        }
        ColumnarValue::Array(array) => {
            let casted: ArrayRef = if array.data_type() == &DataType::Utf8 {
                Arc::clone(array)
            } else {
                cast(array, &DataType::Utf8)?
            };
            let input = casted
                .as_any()
                .downcast_ref::<StringArray>()
                .ok_or_else(|| {
                    DataFusionError::Execution(
                        "lakewatch: expected a Utf8 array after cast".to_string(),
                    )
                })?;
            let output: StringArray = input.iter().map(f).collect();
            Ok(ColumnarValue::Array(Arc::new(output)))
        }
    }
}

/// `f` variant that returns a boolean; kept separate so the output Arrow type is
/// `BooleanArray` rather than a stringified flag.
fn map_utf8_bool<F>(arg: &ColumnarValue, f: F) -> DfResult<ColumnarValue>
where
    F: Fn(Option<&str>) -> Option<bool> + Send + Sync,
{
    match arg {
        ColumnarValue::Scalar(scalar) => {
            let value = match scalar.cast_to(&DataType::Utf8)? {
                ScalarValue::Utf8(v) => v,
                _ => None,
            };
            Ok(ColumnarValue::Scalar(ScalarValue::Boolean(f(
                value.as_deref()
            ))))
        }
        ColumnarValue::Array(array) => {
            let casted: ArrayRef = if array.data_type() == &DataType::Utf8 {
                Arc::clone(array)
            } else {
                cast(array, &DataType::Utf8)?
            };
            let input = casted
                .as_any()
                .downcast_ref::<StringArray>()
                .ok_or_else(|| {
                    DataFusionError::Execution(
                        "lakewatch: expected a Utf8 array after cast".to_string(),
                    )
                })?;
            let output: BooleanArray = input.iter().map(f).collect();
            Ok(ColumnarValue::Array(Arc::new(output)))
        }
    }
}

// ---------------------------------------------------------------------------
// Boolean-returning helpers are declared here so the closures above stay short.
// ---------------------------------------------------------------------------

/// Structural email check: `local@domain`, at least one dot in the domain, no
/// empty labels, conservative character set. NULL in, NULL out.
pub fn valid_email(raw: &str) -> bool {
    let value = raw.trim();
    if value.len() > 254 || value.is_empty() {
        return false;
    }
    let mut parts = value.split('@');
    let local = match parts.next() {
        Some(l) => l,
        None => return false,
    };
    let domain = match parts.next() {
        Some(d) => d,
        None => return false,
    };
    if parts.next().is_some() {
        return false;
    }
    if local.is_empty() || local.len() > 64 || domain.is_empty() || domain.len() > 253 {
        return false;
    }
    if domain.starts_with('.') || domain.ends_with('.') || domain.contains("..") {
        return false;
    }
    if !domain.contains('.') {
        return false;
    }
    let local_ok = local
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || "!#$%&'*+/=?^_`{|}~.-".contains(c))
        && !local.starts_with('.')
        && !local.ends_with('.')
        && !local.contains("..");
    if !local_ok {
        return false;
    }
    domain.split('.').all(|label| {
        !label.is_empty()
            && !label.starts_with('-')
            && !label.ends_with('-')
            && label.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
    })
}

/// `ana.silva@example.com` -> `a*********@example.com`
pub fn mask(raw: &str) -> String {
    let value = raw.trim();
    let (local, domain) = match value.split_once('@') {
        Some((l, d)) => (l, d),
        None => return "***".to_string(),
    };
    if local.is_empty() || domain.is_empty() {
        return "***".to_string();
    }
    let head: String = local.chars().take(1).collect();
    format!(
        "{head}{}@{domain}",
        "*".repeat(local.chars().count().saturating_sub(1))
    )
}

/// Trim, lowercase and collapse repeated whitespace.
pub fn normalize(raw: &str) -> String {
    raw.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_ordinary_addresses() {
        assert!(valid_email("ana.silva@example.com"));
        assert!(valid_email("a@b.co"));
        assert!(valid_email("first+tag@sub.domain.example.org"));
        assert!(valid_email("  padded@example.com  "));
    }

    #[test]
    fn rejects_broken_addresses() {
        for broken in [
            "",
            "plain",
            "@example.com",
            "user@",
            "user@@example.com",
            "user@example",
            "user@.example.com",
            "user@example..com",
            "user@-example.com",
            ".user@example.com",
            "user.@example.com",
            "us er@example.com",
        ] {
            assert!(!valid_email(broken), "`{broken}` should be rejected");
        }
    }

    #[test]
    fn masks_everything_but_the_first_character() {
        assert_eq!(mask("ana.silva@example.com"), "a********@example.com");
        assert_eq!(mask("a@example.com"), "a@example.com");
        assert_eq!(mask("not-an-email"), "***");
        assert_eq!(mask("@example.com"), "***");
    }

    #[test]
    fn normalizes_case_and_whitespace() {
        assert_eq!(normalize("  Ana   SILVA "), "ana silva");
        assert_eq!(normalize("one"), "one");
        assert_eq!(normalize("   "), "");
    }
}
