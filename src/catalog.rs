//! Lake discovery: walk a root directory and turn files into DataFusion tables.
//!
//! The convention is deliberately boring so that it stays guessable:
//!
//! ```text
//! <root>/customers.csv              -> table "customers"
//! <root>/orders/orders_2024_01.csv  -> table "orders"   (all files in the dir)
//! <root>/events/2024/part-0.parquet -> table "events_2024"
//! ```
//!
//! Every file sharing a directory *and* a format forms one logical table, which
//! is exactly the shape `ListingTable` wants: one schema, many objects.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use datafusion::datasource::file_format::csv::CsvFormat;
use datafusion::datasource::file_format::parquet::ParquetFormat;
use datafusion::datasource::file_format::FileFormat;
use datafusion::datasource::listing::{
    ListingOptions, ListingTable, ListingTableConfig, ListingTableUrl,
};
use datafusion::execution::context::SessionContext;
use serde::Serialize;

/// One discovered dataset.
#[derive(Debug, Clone, Serialize)]
pub struct TableEntry {
    /// SQL-addressable table name.
    pub name: String,
    /// `csv` or `parquet`.
    pub format: String,
    /// Directory holding the data files, relative to the lake root.
    pub path: String,
    /// File names, relative to the lake root.
    pub files: Vec<String>,
    /// Total size on disk, in bytes.
    pub bytes: u64,
}

impl TableEntry {
    /// Absolute location the `ListingTable` should read from.
    ///
    /// A group of one file is addressed as that exact object; a group of many
    /// files is addressed as the directory plus a format filter. The distinction
    /// matters when a directory mixes formats, and it is what keeps two
    /// single-file tables in the lake root from reading each other's files.
    pub fn location(&self, root: &Path) -> String {
        if self.files.len() == 1 {
            root.join(&self.files[0]).to_string_lossy().to_string()
        } else {
            root.join(&self.path).to_string_lossy().to_string()
        }
    }

    /// `ListingOptions` for this table's format.
    fn listing_options(&self) -> Result<ListingOptions> {
        let format: Arc<dyn FileFormat> = match self.format.as_str() {
            "csv" => Arc::new(
                CsvFormat::default()
                    .with_has_header(true)
                    .with_delimiter(b','),
            ),
            "parquet" => Arc::new(ParquetFormat::default()),
            other => anyhow::bail!("unsupported format `{other}`"),
        };
        Ok(ListingOptions::new(format).with_file_extension(format!(".{}", self.format)))
    }
}

/// Recursively discover every CSV/Parquet dataset under `root`.
pub fn discover(root: &Path) -> Result<Vec<TableEntry>> {
    let mut groups: BTreeMap<(PathBuf, String, String), Vec<PathBuf>> = BTreeMap::new();

    walk(root, &mut |path| {
        let ext = match path.extension().and_then(|e| e.to_str()) {
            Some(e) => e.to_ascii_lowercase(),
            None => return Ok(()),
        };
        if ext != "csv" && ext != "parquet" {
            return Ok(());
        }
        let dir = path.parent().unwrap_or(root).to_path_buf();
        let rel_dir = dir
            .strip_prefix(root)
            .unwrap_or(Path::new(""))
            .to_path_buf();
        let stem = path
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default();
        groups
            .entry((dir, ext.clone(), base_name(&rel_dir, &stem)))
            .or_default()
            .push(path.to_path_buf());
        Ok(())
    })?;

    let mut entries = Vec::new();
    let mut used: Vec<String> = Vec::new();

    for ((dir, format, base), files) in groups {
        let rel_dir = dir
            .strip_prefix(root)
            .unwrap_or(Path::new(""))
            .to_path_buf();
        let name = unique_name(&base, &format, &mut used);

        let mut rel_files = Vec::new();
        let mut bytes = 0u64;
        for file in &files {
            if let Ok(meta) = file.metadata() {
                bytes += meta.len();
            }
            rel_files.push(
                file.strip_prefix(root)
                    .unwrap_or(file)
                    .to_string_lossy()
                    .to_string(),
            );
        }
        rel_files.sort();

        entries.push(TableEntry {
            name,
            format,
            path: if rel_dir.as_os_str().is_empty() {
                ".".to_string()
            } else {
                rel_dir.to_string_lossy().replace('\\', "/")
            },
            files: rel_files,
            bytes,
        });
    }

    entries.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(entries)
}

/// Register a single table; needs the lake root to resolve relative paths.
pub async fn register_one(ctx: &SessionContext, root: &Path, entry: &TableEntry) -> Result<()> {
    let url = ListingTableUrl::parse(entry.location(root))
        .with_context(|| format!("invalid path for table `{}`", entry.name))?;
    let options = entry.listing_options()?;
    let state = ctx.state();
    let config = ListingTableConfig::new(url)
        .with_listing_options(options)
        .infer_schema(&state)
        .await
        .with_context(|| format!("cannot infer schema for table `{}`", entry.name))?;
    let table = ListingTable::try_new(config)?;
    ctx.register_table(entry.name.as_str(), Arc::new(table))?;
    Ok(())
}

/// Register every table in `tables` rooted at `root`.
pub async fn register_all(ctx: &SessionContext, root: &Path, tables: &[TableEntry]) -> Result<()> {
    for entry in tables {
        register_one(ctx, root, entry).await?;
    }
    Ok(())
}

/// Table name for a file group.
///
/// Files that live in a sub-directory are grouped by that directory, so a
/// partitioned dataset (`orders/2024-01.csv`, `orders/2024-02.csv`) is one table.
/// Files sitting directly in the lake root are grouped by their own stem, so
/// `customers.csv` and `products.csv` stay two tables.
fn base_name(rel_dir: &Path, stem: &str) -> String {
    if rel_dir.as_os_str().is_empty() {
        let cleaned = sanitize(stem);
        return if cleaned == "lake" {
            "lake".to_string()
        } else {
            cleaned
        };
    }
    let joined = rel_dir
        .components()
        .map(|c| c.as_os_str().to_string_lossy().to_string())
        .collect::<Vec<_>>()
        .join("_");
    sanitize(&joined)
}

/// Make a string safe to use as an unquoted SQL identifier.
fn sanitize(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for ch in raw.chars() {
        if ch.is_ascii_alphanumeric() || ch == '_' {
            out.push(ch.to_ascii_lowercase());
        } else {
            out.push('_');
        }
    }
    let out = out.trim_matches('_').to_string();
    let out = if out.is_empty() {
        "lake".to_string()
    } else {
        out
    };
    if out
        .chars()
        .next()
        .map(|c| c.is_ascii_digit())
        .unwrap_or(false)
    {
        format!("t_{out}")
    } else {
        out
    }
}

/// Disambiguate `name` against names already handed out.
fn unique_name(base: &str, format: &str, used: &mut Vec<String>) -> String {
    if !used.iter().any(|u| u == base) {
        used.push(base.to_string());
        return base.to_string();
    }
    let suffixed = format!("{base}_{format}");
    if !used.iter().any(|u| u == &suffixed) {
        used.push(suffixed.clone());
        return suffixed;
    }
    let mut n = 2;
    loop {
        let candidate = format!("{suffixed}_{n}");
        if !used.iter().any(|u| u == &candidate) {
            used.push(candidate.clone());
            return candidate;
        }
        n += 1;
    }
}

/// Depth-first walk that skips hidden entries and symlinked directories.
fn walk(dir: &Path, visit: &mut impl FnMut(&Path) -> Result<()>) -> Result<()> {
    let mut children: Vec<PathBuf> = Vec::new();
    for entry in std::fs::read_dir(dir)
        .with_context(|| format!("cannot read directory `{}`", dir.display()))?
    {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.') {
            continue;
        }
        let file_type = entry.file_type()?;
        if file_type.is_symlink() {
            continue;
        }
        let path = entry.path();
        if file_type.is_dir() {
            children.push(path);
        } else if file_type.is_file() {
            visit(&path)?;
        }
    }
    children.sort();
    for child in children {
        walk(&child, visit)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitizes_identifiers() {
        assert_eq!(sanitize("Orders 2024"), "orders_2024");
        assert_eq!(sanitize("a-b/c"), "a_b_c");
        assert_eq!(sanitize("123"), "t_123");
        assert_eq!(sanitize("!!!"), "lake");
    }

    #[test]
    fn names_tables_from_directories_and_stems() {
        assert_eq!(base_name(Path::new(""), "customers"), "customers");
        assert_eq!(base_name(Path::new("orders"), "orders_2024_01"), "orders");
        assert_eq!(base_name(Path::new("events/2024"), "part-0"), "events_2024");
    }

    #[test]
    fn disambiguates_repeated_names() {
        let mut used = Vec::new();
        assert_eq!(unique_name("orders", "csv", &mut used), "orders");
        assert_eq!(
            unique_name("orders", "parquet", &mut used),
            "orders_parquet"
        );
        assert_eq!(unique_name("orders", "csv", &mut used), "orders_csv");
    }
}
