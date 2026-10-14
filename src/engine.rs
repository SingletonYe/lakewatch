//! The query engine: catalog + functions + a DataFusion `SessionContext`.

use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result};
use datafusion::arrow::array::RecordBatch;
use datafusion::execution::config::SessionConfig;
use datafusion::execution::context::SessionContext;

use crate::catalog::{self, TableEntry};
use crate::functions;

/// A ready-to-query view over one local lake.
pub struct Engine {
    /// The DataFusion session every command executes against.
    pub ctx: SessionContext,
    /// The tables discovered under the root directory.
    pub tables: Vec<TableEntry>,
    /// Absolute path of the lake root.
    pub root: std::path::PathBuf,
}

impl Engine {
    /// Discover a lake rooted at `root` and register every dataset it contains.
    pub async fn open(root: &Path) -> Result<Self> {
        let root = root
            .canonicalize()
            .with_context(|| format!("cannot resolve lake root `{}`", root.display()))?;
        anyhow::ensure!(
            root.is_dir(),
            "lake root `{}` is not a directory",
            root.display()
        );

        let tables = catalog::discover(&root)?;

        // One partition per core keeps scans parallel without over-subscribing a
        // laptop; the planner still picks the cheapest plan for each query.
        let cfg = SessionConfig::new()
            .with_target_partitions(default_partitions())
            .with_information_schema(true);

        let ctx = SessionContext::new_with_config(cfg);
        functions::register(&ctx);
        catalog::register_all(&ctx, &root, &tables).await?;

        Ok(Self { ctx, tables, root })
    }

    /// Run a SQL statement and collect every resulting batch.
    pub async fn sql(&self, sql: &str) -> Result<Vec<RecordBatch>> {
        let df = self.ctx.sql(sql).await?;
        Ok(df.collect().await?)
    }

    /// Run a SQL statement and return the value of the first cell as text.
    pub async fn scalar_text(&self, sql: &str) -> Result<Option<String>> {
        let batches = self.sql(sql).await?;
        let Some(batch) = batches.iter().find(|b| b.num_rows() > 0) else {
            return Ok(None);
        };
        let value = datafusion::scalar::ScalarValue::try_from_array(batch.column(0), 0)?;
        if value.is_null() {
            Ok(None)
        } else {
            Ok(Some(match value {
                datafusion::scalar::ScalarValue::Utf8(v) => v.unwrap_or_default(),
                other => other.to_string(),
            }))
        }
    }

    /// Look up a discovered table by name.
    pub fn table(&self, name: &str) -> Option<&TableEntry> {
        self.tables.iter().find(|t| t.name == name)
    }
}

fn default_partitions() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        .min(8)
}

/// Build a `SessionContext` without registering any table. Handy for tests and
/// for the "no lake on disk" code paths.
pub fn bare_context() -> SessionContext {
    let ctx = SessionContext::new_with_config(
        SessionConfig::new()
            .with_target_partitions(default_partitions())
            .with_information_schema(true),
    );
    functions::register(&ctx);
    ctx
}

/// Register an in-memory batch as a table (used by tests and `--stdin` paths).
pub fn register_batches(ctx: &SessionContext, name: &str, batches: Vec<RecordBatch>) -> Result<()> {
    let provider = datafusion::datasource::memory::MemTable::try_new(
        batches
            .first()
            .map(|b| b.schema())
            .context("cannot infer schema from zero batches")?,
        vec![batches],
    )?;
    ctx.register_table(name, Arc::new(provider))?;
    Ok(())
}
