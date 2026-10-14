//! Tiny helpers for building SQL text safely.

/// Quote an identifier for interpolation into SQL.
pub fn ident(raw: &str) -> String {
    format!("\"{}\"", raw.replace('"', "\"\""))
}

/// Quote a string literal for interpolation into SQL.
pub fn literal(raw: &str) -> String {
    format!("'{}'", raw.replace('\'', "''"))
}
