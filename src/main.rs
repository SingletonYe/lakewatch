//! `lakewatch` — SQL analytics and data-quality checks for a local data lake,
//! powered by [Apache DataFusion](https://github.com/apache/datafusion).
//!
//! ```text
//! lakewatch tables                 # what is in this lake?
//! lakewatch sql "SELECT ..."       # ad-hoc SQL over CSV/Parquet
//! lakewatch profile orders         # column statistics, one scan
//! lakewatch check -r rules.json    # declarative data-quality gate
//! ```

use std::io::Read;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Instant;

use anyhow::{Context, Result};
use clap::{Args, Parser, Subcommand};
use datafusion::arrow::array::{ArrayRef, RecordBatch, StringArray, UInt64Array};
use datafusion::arrow::datatypes::{DataType, Field, Schema};

use lakewatch::check::{self, RulesFile, Status};
use lakewatch::engine::Engine;
use lakewatch::functions;
use lakewatch::profile::{self, ProfileOptions};
use lakewatch::render::{self, OutputFormat};

/// Exit code used when a data-quality gate fails.
const EXIT_CHECK_FAILED: u8 = 1;
/// Exit code used for usage or runtime errors.
const EXIT_RUNTIME_ERROR: u8 = 2;

#[derive(Parser, Debug)]
#[command(
    name = "lakewatch",
    version,
    about = "SQL analytics and data-quality checks for local CSV/Parquet lakes, powered by Apache DataFusion",
    propagate_version = true
)]
struct Cli {
    /// Directory holding the lake: every CSV/Parquet file below it becomes a table.
    #[arg(
        long,
        short = 'C',
        default_value = ".",
        global = true,
        value_name = "DIR"
    )]
    root: PathBuf,

    /// DataFusion target partitions (default: one per core, capped at 8).
    #[arg(long, global = true, value_name = "N")]
    partitions: Option<usize>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// List the tables discovered in the lake.
    Tables(TablesArgs),

    /// Run a SQL query against the lake.
    Sql(SqlArgs),

    /// Profile every column of a table or query.
    Profile(ProfileArgs),

    /// Evaluate a data-quality rules file. Exits 1 when a rule fails.
    Check(CheckArgs),

    /// Show the custom SQL functions provided by lakewatch.
    Functions,
}

#[derive(Args, Debug)]
struct TablesArgs {
    /// Output format: table, json, ndjson or csv.
    #[arg(long, short = 'f', default_value = "table")]
    format: String,

    /// Write the result to a file instead of stdout.
    #[arg(long, short = 'o', value_name = "FILE")]
    out: Option<PathBuf>,

    /// Also count rows per table (adds one `COUNT(*)` scan per table).
    #[arg(long)]
    counts: bool,
}

#[derive(Args, Debug)]
struct SqlArgs {
    /// SQL to execute.
    #[arg(value_name = "SQL")]
    query: Option<String>,

    /// Read the SQL from a file instead.
    #[arg(long, short = 'F', value_name = "FILE", conflicts_with_all = ["query", "stdin"])]
    file: Option<PathBuf>,

    /// Read the SQL from stdin.
    #[arg(long, conflicts_with_all = ["query", "file"])]
    stdin: bool,

    /// Output format: table, json, ndjson, csv or parquet.
    #[arg(long, short = 'f', default_value = "table")]
    format: String,

    /// Write the result to a file (a directory, for parquet).
    #[arg(long, short = 'o', value_name = "PATH")]
    out: Option<PathBuf>,

    /// Stop after N result rows.
    #[arg(long, short = 'n', value_name = "N")]
    limit: Option<usize>,

    /// Print the plan instead of running the query.
    #[arg(long)]
    explain: bool,

    /// Print the wall-clock duration to stderr.
    #[arg(long)]
    timing: bool,

    /// Substitute `${name}` placeholders in the SQL, e.g. `--var month=2024-01`.
    #[arg(long = "var", value_name = "NAME=VALUE", value_parser = parse_var)]
    vars: Vec<(String, String)>,
}

#[derive(Args, Debug)]
struct ProfileArgs {
    /// Table to profile.
    #[arg(value_name = "TABLE", conflicts_with = "query")]
    table: Option<String>,

    /// Profile the output of an arbitrary query instead of a whole table.
    #[arg(long, value_name = "SQL", conflicts_with = "table")]
    query: Option<String>,

    /// Output format: table, json, ndjson or csv.
    #[arg(long, short = 'f', default_value = "table")]
    format: String,

    /// Write the result to a file instead of stdout.
    #[arg(long, short = 'o', value_name = "FILE")]
    out: Option<PathBuf>,

    /// Also compute the 25th and 75th percentiles of numeric columns.
    #[arg(long)]
    quantiles: bool,

    /// Print the aggregate SQL that produced the report.
    #[arg(long)]
    show_query: bool,
}

#[derive(Args, Debug)]
struct CheckArgs {
    /// Rules file (JSON).
    #[arg(long, short = 'r', value_name = "FILE")]
    rules: PathBuf,

    /// Output format: table, json, ndjson or csv.
    #[arg(long, short = 'f', default_value = "table")]
    format: String,

    /// Write the result to a file instead of stdout.
    #[arg(long, short = 'o', value_name = "FILE")]
    out: Option<PathBuf>,

    /// Include the SQL behind every rule in the report.
    #[arg(long)]
    verbose: bool,
}

fn parse_var(raw: &str) -> Result<(String, String)> {
    let (name, value) = raw
        .split_once('=')
        .with_context(|| format!("`--var {raw}` must be NAME=VALUE"))?;
    Ok((name.to_string(), value.to_string()))
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("lakewatch: cannot start async runtime: {error}");
            return ExitCode::from(EXIT_RUNTIME_ERROR);
        }
    };

    match runtime.block_on(run(cli)) {
        Ok(code) => code,
        Err(error) => {
            eprintln!("lakewatch: {error:#}");
            ExitCode::from(EXIT_RUNTIME_ERROR)
        }
    }
}

async fn run(cli: Cli) -> Result<ExitCode> {
    if let Command::Functions = cli.command {
        print_functions();
        return Ok(ExitCode::SUCCESS);
    }

    let engine = Engine::open(&cli.root, cli.partitions).await?;
    let code = match cli.command {
        Command::Functions => Option::None,
        Command::Tables(args) => Some(cmd_tables(&engine, args).await?),
        Command::Sql(args) => Some(cmd_sql(&engine, args).await?),
        Command::Profile(args) => Some(cmd_profile(&engine, args).await?),
        Command::Check(args) => Some(cmd_check(&engine, args).await?),
    };
    Ok(code.unwrap_or(ExitCode::SUCCESS))
}

fn print_functions() {
    println!("Functions provided by lakewatch");
    println!("{}", "-".repeat(96));
    for (name, kind, signature, description) in functions::catalog() {
        println!("{name:<16} {kind:<9} {signature:<18} {description}");
    }
    println!();
    println!("They are plain SQL, e.g. `lakewatch sql \"SELECT mode(status) FROM orders\"`.");
}

async fn cmd_tables(engine: &Engine, args: TablesArgs) -> Result<ExitCode> {
    let format: OutputFormat = args.format.parse()?;

    let mut names = Vec::new();
    let mut formats = Vec::new();
    let mut paths = Vec::new();
    let mut files = Vec::new();
    let mut bytes = Vec::new();
    let mut rows: Vec<Option<u64>> = Vec::new();

    for table in &engine.tables {
        names.push(table.name.clone());
        formats.push(table.format.clone());
        paths.push(table.path.clone());
        files.push(table.files.len() as u64);
        bytes.push(table.bytes);
        if args.counts {
            let sql = format!(
                "SELECT COUNT(*) FROM {}",
                lakewatch::sqlutil::ident(&table.name)
            );
            let count = engine
                .sql(&sql)
                .await
                .ok()
                .and_then(|batches| tally(&batches));
            rows.push(count);
        }
    }

    let mut fields = vec![
        Field::new("table", DataType::Utf8, false),
        Field::new("format", DataType::Utf8, false),
        Field::new("path", DataType::Utf8, false),
        Field::new("files", DataType::UInt64, false),
        Field::new("bytes", DataType::UInt64, false),
    ];
    if args.counts {
        fields.push(Field::new("rows", DataType::UInt64, true));
    }
    let mut columns: Vec<ArrayRef> = vec![
        Arc::new(StringArray::from_iter_values(names)),
        Arc::new(StringArray::from_iter_values(formats)),
        Arc::new(StringArray::from_iter_values(paths)),
        Arc::new(UInt64Array::from_iter_values(files)),
        Arc::new(UInt64Array::from_iter_values(bytes)),
    ];
    if args.counts {
        columns.push(Arc::new(UInt64Array::from(rows)));
    }

    let batch = RecordBatch::try_new(Arc::new(Schema::new(fields)), columns)?;
    render::write_batches(&[batch], format, args.out.as_deref())?;
    Ok(ExitCode::SUCCESS)
}

fn tally(batches: &[RecordBatch]) -> Option<u64> {
    let batch = batches.iter().find(|b| b.num_rows() > 0)?;
    let scalar = datafusion::scalar::ScalarValue::try_from_array(batch.column(0), 0).ok()?;
    match scalar {
        datafusion::scalar::ScalarValue::UInt64(v) => v,
        datafusion::scalar::ScalarValue::Int64(v) => v.map(|v| v.max(0) as u64),
        other => other.to_string().parse().ok(),
    }
}

async fn cmd_sql(engine: &Engine, args: SqlArgs) -> Result<ExitCode> {
    let format: OutputFormat = args.format.parse()?;
    let mut sql = read_sql(&args)?;
    for (name, value) in &args.vars {
        sql = sql.replace(&format!("${{{name}}}"), value);
    }

    if args.explain {
        let batches = engine
            .ctx
            .sql(&sql)
            .await?
            .explain(false, false)?
            .collect()
            .await?;
        render::write_batches(&batches, OutputFormat::Table, None)?;
        return Ok(ExitCode::SUCCESS);
    }

    if format == OutputFormat::Parquet {
        let out = args
            .out
            .clone()
            .context("`--format parquet` requires `--out <DIR>`")?;
        let started = Instant::now();
        let options = datafusion::dataframe::DataFrameWriteOptions::new();
        engine
            .ctx
            .sql(&sql)
            .await?
            .write_parquet(out.to_string_lossy().as_ref(), options, None)
            .await?;
        let files = count_parquet_files(&out);
        eprintln!(
            "wrote {files} parquet file(s) to {} in {:.2}s (parquet output is a \
directory: one file per partition)",
            out.display(),
            started.elapsed().as_secs_f64()
        );
        return Ok(ExitCode::SUCCESS);
    }

    let started = Instant::now();
    let batches = match args.limit {
        Some(limit) => {
            engine
                .ctx
                .sql(&sql)
                .await?
                .limit(0, Some(limit))?
                .collect()
                .await?
        }
        None => engine.sql(&sql).await?,
    };
    let elapsed = started.elapsed();
    render::write_batches(&batches, format, args.out.as_deref())?;
    if args.timing {
        eprintln!("elapsed: {:.3}s", elapsed.as_secs_f64());
    }
    Ok(ExitCode::SUCCESS)
}

/// Number of parquet files a sink produced, for the `--format parquet` message.
fn count_parquet_files(dir: &std::path::Path) -> usize {
    std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .flatten()
                .filter(|e| {
                    e.path()
                        .extension()
                        .map(|ext| ext.eq_ignore_ascii_case("parquet"))
                        .unwrap_or(false)
                })
                .count()
        })
        .unwrap_or(0)
}

fn read_sql(args: &SqlArgs) -> Result<String> {
    if let Some(file) = &args.file {
        return std::fs::read_to_string(file)
            .with_context(|| format!("cannot read SQL file `{}`", file.display()));
    }
    if args.stdin {
        let mut buffer = String::new();
        std::io::stdin().read_to_string(&mut buffer)?;
        return Ok(buffer);
    }
    args.query
        .clone()
        .context("pass a SQL string, --file <FILE> or --stdin")
}

async fn cmd_profile(engine: &Engine, args: ProfileArgs) -> Result<ExitCode> {
    let format: OutputFormat = args.format.parse()?;
    let options = match (&args.table, &args.query) {
        (Some(table), _) => {
            if engine.table(table).is_none() {
                anyhow::bail!(
                    "unknown table `{table}` (known: {})",
                    engine
                        .tables
                        .iter()
                        .map(|t| t.name.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                );
            }
            ProfileOptions {
                input_sql: format!("SELECT * FROM {}", lakewatch::sqlutil::ident(table)),
                source: table.clone(),
                quantiles: args.quantiles,
            }
        }
        (None, Some(query)) => ProfileOptions {
            input_sql: query.clone(),
            source: "<query>".to_string(),
            quantiles: args.quantiles,
        },
        (None, None) => anyhow::bail!("pass a table name or `--query <SQL>`"),
    };

    let report = profile::profile(&engine.ctx, &options).await?;
    if args.show_query {
        eprintln!("-- profiling SQL\n{}\n", report.query);
    }
    eprintln!(
        "profiled {} column(s) of {} ({} row(s))",
        report.column_count, report.source, report.rows
    );
    let batch = report.to_batch()?;
    render::write_batches(&[batch], format, args.out.as_deref())?;
    Ok(ExitCode::SUCCESS)
}

async fn cmd_check(engine: &Engine, args: CheckArgs) -> Result<ExitCode> {
    let format: OutputFormat = args.format.parse()?;
    let rules = RulesFile::load(&args.rules)?;
    let report = check::run(&engine.ctx, &engine.tables, &rules, args.verbose).await?;

    match format {
        // A CI gate wants the whole report, including `ok` and the sample rows.
        OutputFormat::Json => {
            render::emit(
                &format!("{}\n", serde_json::to_string_pretty(&report)?),
                args.out.as_deref(),
            )?;
        }
        // One JSON object per rule, for streaming into a log pipeline.
        OutputFormat::Ndjson => {
            let mut text = String::new();
            for outcome in &report.outcomes {
                text.push_str(&serde_json::to_string(outcome)?);
                text.push('\n');
            }
            render::emit(&text, args.out.as_deref())?;
        }
        _ => render::write_batches(&[report.to_batch()?], format, args.out.as_deref())?,
    }

    eprintln!(
        "{} rule(s): {} passed, {} failed ({} of them warnings), {} could not be evaluated",
        report.total, report.passed, report.failed, report.warnings, report.errored
    );
    for outcome in report.outcomes.iter().filter(|o| o.status != Status::Pass) {
        eprintln!(
            "  [{}] {} — {}",
            match outcome.status {
                Status::Pass => "pass",
                Status::Fail => "FAIL",
                Status::Error => "ERROR",
            },
            outcome.rule,
            outcome.message
        );
    }

    if report.ok {
        Ok(ExitCode::SUCCESS)
    } else {
        Ok(ExitCode::from(EXIT_CHECK_FAILED))
    }
}
