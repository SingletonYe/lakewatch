//! lakewatch — SQL analytics and data-quality checks for a local data lake,
//! built on [Apache DataFusion](https://github.com/apache/datafusion).
//!
//! The crate is organised as a small pipeline:
//!
//! * [`catalog`] discovers CSV/Parquet files under a root directory and registers
//!   each logical dataset as a DataFusion `ListingTable`.
//! * [`functions`] contributes the custom scalar functions (UDFs) and aggregate
//!   functions (UDAFs) that ship with the tool.
//! * [`engine`] wires the catalog and the functions into a `SessionContext`.
//! * [`profile`] and [`check`] are the two batteries-included workflows on top of
//!   that context: column profiling and declarative data-quality assertions.
//! * [`render`] turns `RecordBatch`es into table / json / ndjson / csv / parquet output.

pub mod catalog;
pub mod check;
pub mod engine;
pub mod functions;
pub mod profile;
pub mod render;
pub mod sqlutil;
