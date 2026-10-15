//! The query engine: catalog + functions + a DataFusion `SessionContext`.

use std::path::Path;

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
    ///
    /// `partitions` overrides the number of target partitions (`None` = one per
    /// core, capped at eight). `session_options` are DataFusion setting
    /// overrides applied on top of those defaults, as `(key, value)` pairs.
    pub async fn open(
        root: &Path,
        partitions: Option<usize>,
        session_options: &[(String, String)],
    ) -> Result<Self> {
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
        let mut cfg = SessionConfig::new()
            .with_target_partitions(
                partitions
                    .filter(|n| *n > 0)
                    .unwrap_or_else(default_partitions),
            )
            .with_information_schema(true)
            // A lake on a laptop is read from local disk; spreading one file over
            // several partitions costs more in scheduling than it saves.
            .with_repartition_file_scans(false);

        // Applied last, so an explicit option always wins over the defaults above.
        // `ConfigOptions::set` rejects unknown keys and malformed values, and we
        // surface that as an ordinary error rather than letting `SessionConfig`
        // panic on the caller's typo.
        for (key, value) in session_options {
            cfg.options_mut()
                .set(key, value)
                .with_context(|| format!("invalid --session-option `{key}={value}`"))?;
        }

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
