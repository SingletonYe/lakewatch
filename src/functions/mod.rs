//! Custom DataFusion functions shipped with lakewatch.
//!
//! Three scalar functions (UDFs) and two aggregate functions (UDAFs) are added
//! to every session, so they are available from plain SQL:
//!
//! | function | kind | signature | purpose |
//! |---|---|---|---|
//! | `is_email` | scalar | `(utf8) -> boolean` | null-safe email validation |
//! | `mask_email` | scalar | `(utf8) -> utf8` | PII masking for exported reports |
//! | `normalize_text` | scalar | `(utf8) -> utf8` | trim / lowercase / collapse whitespace |
//! | `mode` | aggregate | `(any) -> utf8` | most frequent value, as text |
//! | `null_ratio` | aggregate | `(any) -> float64` | share of NULLs in the group |

mod aggregate;
mod scalar;

pub use aggregate::{ModeUdaf, NullRatioUdaf};
pub use scalar::{is_email, mask_email, normalize_text};

use datafusion::execution::context::SessionContext;

/// Install every lakewatch function on `ctx`.
pub fn register(ctx: &SessionContext) {
    ctx.register_udf(is_email());
    ctx.register_udf(mask_email());
    ctx.register_udf(normalize_text());
    ctx.register_udaf(aggregate::mode_udaf());
    ctx.register_udaf(aggregate::null_ratio_udaf());
}

/// Names of the functions, for `lakewatch functions`.
pub fn catalog() -> Vec<(&'static str, &'static str, &'static str, &'static str)> {
    vec![
        (
            "is_email",
            "scalar",
            "(utf8) -> boolean",
            "True when the text is a structurally valid email address",
        ),
        (
            "mask_email",
            "scalar",
            "(utf8) -> utf8",
            "Keep the first character and the domain: `a***@example.com`",
        ),
        (
            "normalize_text",
            "scalar",
            "(utf8) -> utf8",
            "Trim, lowercase and collapse internal whitespace",
        ),
        (
            "mode",
            "aggregate",
            "(any) -> utf8",
            "Most frequent non-null value in the group, rendered as text",
        ),
        (
            "null_ratio",
            "aggregate",
            "(any) -> float64",
            "Fraction of NULL values in the group",
        ),
    ]
}
