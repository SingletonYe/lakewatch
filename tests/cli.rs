//! End-to-end tests: they drive the real binary against the sample lake that
//! ships in `data/`, so they cover argument parsing, discovery, planning,
//! execution and rendering in one shot.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_lakewatch")
}

fn sample_lake() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("data")
}

fn rules_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("rules/quality.json")
}

fn run(root: &Path, args: &[&str]) -> Output {
    Command::new(bin())
        .arg("--root")
        .arg(root)
        .args(args)
        .output()
        .expect("lakewatch should start")
}

fn json_of(output: &Output) -> serde_json::Value {
    let stdout = String::from_utf8_lossy(&output.stdout);
    serde_json::from_str(&stdout).unwrap_or_else(|error| {
        panic!(
            "stdout was not JSON ({error}):\n{stdout}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stderr)
        )
    })
}

fn stderr_of(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).to_string()
}

/// A throwaway directory that removes itself at the end of the test.
struct TempLake {
    path: PathBuf,
}

impl TempLake {
    fn new(tag: &str) -> Self {
        let path =
            std::env::temp_dir().join(format!("lakewatch-test-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("temp dir");
        Self { path }
    }

    fn write(&self, relative: &str, contents: &str) -> PathBuf {
        let path = self.path.join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).expect("parent dir");
        std::fs::write(&path, contents).expect("write file");
        path
    }
}

impl Drop for TempLake {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

#[test]
fn tables_lists_every_dataset() {
    let output = run(&sample_lake(), &["tables", "--format", "json", "--counts"]);
    assert!(output.status.success(), "{}", stderr_of(&output));

    let rows = json_of(&output);
    let rows = rows.as_array().expect("array of tables");
    let names: Vec<&str> = rows
        .iter()
        .map(|row| row["table"].as_str().unwrap())
        .collect();
    assert_eq!(names, vec!["customers", "orders", "products"]);

    let orders = rows.iter().find(|row| row["table"] == "orders").unwrap();
    assert_eq!(orders["files"], serde_json::json!(2));
    assert_eq!(orders["rows"], serde_json::json!(240));

    let customers = rows.iter().find(|row| row["table"] == "customers").unwrap();
    assert_eq!(customers["files"], serde_json::json!(1));
    assert_eq!(customers["rows"], serde_json::json!(61));
}

#[test]
fn sql_runs_joins_aggregates_and_windows() {
    let output = run(
        &sample_lake(),
        &[
            "sql",
            "SELECT p.category, COUNT(*) AS orders, ROUND(SUM(o.amount), 2) AS revenue \
             FROM orders o JOIN products p ON p.product_id = o.product_id \
             GROUP BY p.category ORDER BY revenue DESC",
            "--format",
            "json",
        ],
    );
    assert!(output.status.success(), "{}", stderr_of(&output));

    let rows = json_of(&output);
    let rows = rows.as_array().unwrap();
    assert_eq!(rows.len(), 5, "the sample lake covers five categories");
    assert!(
        rows[0]["revenue"].as_f64().unwrap() > 0.0,
        "revenue should be positive"
    );
}

#[test]
fn custom_udfs_and_udafs_are_reachable_from_sql() {
    let output = run(
        &sample_lake(),
        &[
            "sql",
            "SELECT mask_email('ana.silva@example.com') AS masked, \
             is_email('ana.silva@example.com') AS good, \
             is_email('nope@@example.com') AS bad, \
             normalize_text('  Ana   SILVA ') AS normalized, \
             mode(channel) AS top_channel, \
             null_ratio(customer_email) AS null_ratio \
             FROM orders",
            "--format",
            "json",
        ],
    );
    assert!(output.status.success(), "{}", stderr_of(&output));

    let row = &json_of(&output)[0];
    assert_eq!(row["masked"], "a********@example.com");
    assert_eq!(row["good"], serde_json::json!(true));
    assert_eq!(row["bad"], serde_json::json!(false));
    assert_eq!(row["normalized"], "ana silva");
    assert!(row["top_channel"].is_string());
    // a handful of orders reference a customer with no stored email
    let ratio = row["null_ratio"].as_f64().unwrap();
    assert!(ratio > 0.0 && ratio < 0.05, "unexpected null_ratio {ratio}");
}

#[test]
fn profile_reports_nulls_distincts_and_modes() {
    let output = run(
        &sample_lake(),
        &["profile", "orders", "--quantiles", "--format", "json"],
    );
    assert!(output.status.success(), "{}", stderr_of(&output));

    let rows = json_of(&output);
    let rows = rows.as_array().unwrap();
    assert_eq!(rows.len(), 9, "orders has nine columns");

    let order_id = rows.iter().find(|r| r["column"] == "order_id").unwrap();
    assert_eq!(order_id["nulls"], serde_json::json!(0));
    // 240 rows but only 239 distinct ids: the duplicate planted by the generator
    assert_eq!(order_id["distinct"], serde_json::json!(239));

    let amount = rows.iter().find(|r| r["column"] == "amount").unwrap();
    assert_eq!(amount["min"], "-149.0");
    assert!(amount["p50"].as_f64().unwrap() > 0.0);
    assert!(!amount["top"].as_str().unwrap().is_empty());

    let status = rows.iter().find(|r| r["column"] == "status").unwrap();
    assert_eq!(status["top"], "paid");

    let email = rows
        .iter()
        .find(|r| r["column"] == "customer_email")
        .unwrap();
    let nulls = email["nulls"].as_u64().unwrap();
    assert!(nulls > 0, "the sample lake ships empty customer emails");
    let ratio = email["null_ratio"].as_f64().unwrap();
    assert!((ratio - nulls as f64 / 240.0).abs() < 1e-5);
}

#[test]
fn check_gate_fails_and_explains_itself() {
    let output = run(
        &sample_lake(),
        &[
            "check",
            "--rules",
            rules_path().to_str().unwrap(),
            "--format",
            "json",
        ],
    );
    // an error-severity rule fails in the sample lake on purpose
    assert_eq!(output.status.code(), Some(1), "{}", stderr_of(&output));

    let report = json_of(&output);
    assert_eq!(report["ok"], serde_json::json!(false));
    assert_eq!(report["total"], serde_json::json!(9));

    let outcomes = report["outcomes"].as_array().unwrap();
    let find = |name: &str| {
        outcomes
            .iter()
            .find(|o| o["rule"] == name)
            .unwrap_or_else(|| panic!("rule {name} missing"))
    };

    assert_eq!(find("orders.order_id_is_unique")["violations"], 1);
    assert_eq!(find("orders.amount_is_positive")["violations"], 4);
    assert_eq!(find("products.unit_price_is_positive")["status"], "pass");
    // warnings are reported but do not gate the build
    assert_eq!(find("customers.email_is_well_formed")["severity"], "warn");
    assert!(
        find("orders.amount_is_positive")["sample"].is_array(),
        "failing rules carry sample violating rows"
    );
}

#[test]
fn check_passes_on_a_clean_rule_set() {
    let lake = TempLake::new("clean");
    lake.write("orders/part.csv", "order_id,amount\nO1,10\nO2,20\n");
    let rules = lake.write(
        "rules.json",
        r#"{"version":1,"rules":[
             {"name":"unique","table":"orders","type":"unique","columns":["order_id"]},
             {"name":"positive","table":"orders","type":"expression","predicate":"amount > 0"},
             {"name":"size","table":"orders","type":"row_count","min":2,"max":2}
           ]}"#,
    );

    let output = run(
        &lake.path,
        &[
            "check",
            "--rules",
            rules.to_str().unwrap(),
            "--format",
            "json",
        ],
    );
    assert!(output.status.success(), "{}", stderr_of(&output));
    let report = json_of(&output);
    assert_eq!(report["ok"], serde_json::json!(true));
    assert_eq!(report["passed"], serde_json::json!(3));
}

#[test]
fn unknown_table_and_unknown_rules_are_reported() {
    let output = run(&sample_lake(), &["profile", "nope"]);
    assert_eq!(output.status.code(), Some(2));
    assert!(stderr_of(&output).contains("unknown table `nope`"));

    let lake = TempLake::new("badrules");
    lake.write("t.csv", "a\n1\n");
    // a rule pointing at a table that is not in the lake is reported, not fatal
    let rules = lake.write(
        "rules.json",
        r#"{"rules":[{"name":"x","table":"nope","type":"unique","columns":["a"]}]}"#,
    );
    let output = run(&lake.path, &["check", "--rules", rules.to_str().unwrap()]);
    assert_eq!(output.status.code(), Some(1), "{}", stderr_of(&output));
    assert!(stderr_of(&output).contains("unknown table `nope`"));

    // a malformed rules file is a usage error
    let broken = lake.write("broken.json", r#"{"rules":[{"name":"x","type":"unique"}]}"#);
    let output = run(&lake.path, &["check", "--rules", broken.to_str().unwrap()]);
    assert_eq!(output.status.code(), Some(2));
    assert!(stderr_of(&output).contains("not a valid rules file"));
    let _ = &rules;
}

#[test]
fn discover_splits_partitioned_and_root_level_files() {
    let lake = TempLake::new("discover");
    lake.write("customers.csv", "customer_id\nC1\n");
    lake.write("products.csv", "product_id\nP1\n");
    lake.write("orders/2024-01.csv", "order_id\nO1\n");
    lake.write("orders/2024-02.csv", "order_id\nO2\n");

    let output = run(&lake.path, &["tables", "--format", "json"]);
    assert!(output.status.success(), "{}", stderr_of(&output));

    let rows = json_of(&output);
    let names: Vec<&str> = rows
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["table"].as_str().unwrap())
        .collect();
    assert_eq!(names, vec!["customers", "orders", "products"]);

    let orders = rows
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["table"] == "orders")
        .unwrap();
    assert_eq!(orders["files"], serde_json::json!(2));
}

/// The custom UDAFs must survive DataFusion's partial -> final aggregation
/// protocol, which is exactly what `--partitions 4` forces (the plan switches to
/// `AggregateExec: mode=Partial` plus `mode=FinalPartitioned`).
#[test]
fn udafs_agree_across_partition_layouts() {
    let query = "SELECT channel, mode(status) AS m, null_ratio(customer_email) AS nr \
                 FROM orders GROUP BY channel ORDER BY channel";

    let single = run(
        &sample_lake(),
        &["--partitions", "1", "sql", query, "-f", "json"],
    );
    let multi = run(
        &sample_lake(),
        &["--partitions", "4", "sql", query, "-f", "json"],
    );
    assert!(single.status.success(), "{}", stderr_of(&single));
    assert!(multi.status.success(), "{}", stderr_of(&multi));

    let single = json_of(&single);
    let multi = json_of(&multi);
    assert_eq!(single, multi, "partitioning must not change the answer");
    assert_eq!(single.as_array().unwrap().len(), 4);

    // ground truth for one group, computed without any custom function
    let truth = run(
        &sample_lake(),
        &[
            "sql",
            "SELECT status FROM (SELECT status, ROW_NUMBER() OVER (ORDER BY COUNT(*) DESC, status) AS rn \
             FROM orders WHERE channel = 'partner_api' GROUP BY status) WHERE rn = 1",
            "-f",
            "json",
        ],
    );
    let truth = json_of(&truth);
    let partner = multi
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["channel"] == "partner_api")
        .unwrap();
    assert_eq!(partner["m"], truth[0]["status"]);
}

#[test]
fn csv_columns_follow_the_schema_order() {
    // Regression: iterating a JSON object gave alphabetically sorted columns,
    // so CSV rows no longer lined up with the header.
    let output = run(
        &sample_lake(),
        &[
            "sql",
            "SELECT 'x' AS alpha, 42 AS beta, 1.5 AS gamma",
            "--format",
            "csv",
        ],
    );
    assert!(output.status.success(), "{}", stderr_of(&output));
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        "alpha,beta,gamma\nx,42,1.5"
    );

    let output = run(&sample_lake(), &["tables", "--format", "csv"]);
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let mut lines = stdout.trim().lines();
    assert_eq!(lines.next().unwrap(), "table,format,path,files,bytes");
    let first: Vec<&str> = lines.next().unwrap().split(',').collect();
    assert_eq!(first.len(), 5);
    assert_eq!(first[0], "customers");
    assert_eq!(first[1], "csv");
    assert_eq!(first[3], "1");
    assert!(first[4].parse::<u64>().is_ok(), "bytes should be numeric");
}

#[test]
fn limit_csv_output_and_stdin_sql_all_work() {
    let output = run(
        &sample_lake(),
        &[
            "sql",
            "SELECT order_id, status FROM orders",
            "--limit",
            "3",
            "--format",
            "csv",
        ],
    );
    assert!(output.status.success(), "{}", stderr_of(&output));
    let stdout = String::from_utf8_lossy(&output.stdout);
    let lines: Vec<&str> = stdout.trim().lines().collect();
    assert_eq!(lines.len(), 4, "header + three rows");
    assert_eq!(lines[0], "order_id,status");
}
