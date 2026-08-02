//! Deterministic in-process allocation accounting for the baseline workloads.
//!
//! Runs release Ronin in process against the version-1 baseline workload
//! shapes with a counting global allocator, reporting allocator requests and
//! requested bytes per workload and per build statement. Unlike wall-time
//! measurement, allocation counts are deterministic, so a single run per
//! workload suffices and recorded baselines gate material increases.

// [spec:ronin:req:performance.allocation-accounting]

use std::alloc::{GlobalAlloc, Layout, System};
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fmt::Write as _;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

#[path = "support/workloads.rs"]
mod workloads;
use workloads::{
    CANONICAL_PATHS, CLEAN_TREE_EDGES, COMMAND_EDGES, DEEP_EDGES, DEPENDENCY_EDGES,
    LARGE_MANIFEST_EDGES, SCHEDULER_EDGES, WIDE_EDGES, WORKLOAD_VERSION,
};

const SCHEMA: &str = "ronin-alloc-metrics-v1";

/// A recorded value may grow by at most this percentage before `--check`
/// fails. Allocation counts are deterministic, so the margin only absorbs
/// scheduler completion-order wobble and platform path-length differences.
const MAX_RECORDED_PERCENT: u64 = 110;

static ALLOCATION_REQUESTS: AtomicU64 = AtomicU64::new(0);
static REQUESTED_BYTES: AtomicU64 = AtomicU64::new(0);

struct CountingAllocator;

impl CountingAllocator {
    fn count(size: usize) {
        ALLOCATION_REQUESTS.fetch_add(1, Ordering::Relaxed);
        REQUESTED_BYTES.fetch_add(size as u64, Ordering::Relaxed);
    }
}

// SAFETY: every method forwards the caller's layout unchanged to the system
// allocator, so the system allocator's contract is preserved verbatim.
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        Self::count(layout.size());
        // SAFETY: the caller's layout obligations are forwarded unchanged.
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        Self::count(layout.size());
        // SAFETY: the caller's layout obligations are forwarded unchanged.
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        Self::count(new_size);
        // SAFETY: the caller's pointer and layout obligations are forwarded
        // unchanged.
        unsafe { System.realloc(pointer, layout, new_size) }
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        // SAFETY: the caller's pointer and layout obligations are forwarded
        // unchanged.
        unsafe { System.dealloc(pointer, layout) }
    }
}

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

struct Workload {
    name: &'static str,
    directory: PathBuf,
    arguments: Vec<String>,
    build_statements: usize,
}

struct Sample {
    requests: u64,
    bytes: u64,
    minor_faults: Option<u64>,
}

struct Record {
    workload: &'static str,
    build_statements: usize,
    sample: Sample,
}

#[derive(Default)]
struct Config {
    record: Option<PathBuf>,
    check: Option<PathBuf>,
}

struct TemporaryDirectory(PathBuf);

impl TemporaryDirectory {
    fn new() -> io::Result<Self> {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "ronin-alloc-metrics-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir(&path)?;
        Ok(Self(path))
    }
}

impl Drop for TemporaryDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn parse_arguments() -> Result<Config, String> {
    let mut config = Config::default();
    let mut arguments = std::env::args().skip(1);
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--record" => {
                let path = arguments
                    .next()
                    .ok_or_else(|| "--record requires a path".to_owned())?;
                config.record = Some(PathBuf::from(path));
            }
            "--check" => {
                let path = arguments
                    .next()
                    .ok_or_else(|| "--check requires a path".to_owned())?;
                config.check = Some(PathBuf::from(path));
            }
            "--help" => {
                println!("usage: alloc_metrics [--record <csv-path>] [--check <csv-path>]");
                std::process::exit(0);
            }
            other => return Err(format!("unknown argument: {other}")),
        }
    }
    Ok(config)
}

const fn build_profile() -> &'static str {
    if cfg!(debug_assertions) {
        "debug"
    } else {
        "release"
    }
}

#[cfg(target_os = "linux")]
fn minor_faults() -> Option<u64> {
    let status = fs::read_to_string("/proc/self/stat").ok()?;
    let after_command = &status[status.rfind(')')? + 1..];
    after_command.split_whitespace().nth(7)?.parse().ok()
}

#[cfg(not(target_os = "linux"))]
fn minor_faults() -> Option<u64> {
    None
}

fn run_ronin(directory: &Path, arguments: &[String]) -> Result<(), String> {
    let runner = ronin::Runner::new(directory).map_err(|error| error.to_string())?;
    let mut os_arguments = vec![OsString::from("ronin")];
    os_arguments.extend(arguments.iter().map(OsString::from));
    let mut output = io::sink();
    let mut diagnostics = io::sink();
    let result = runner
        .run_os_with_sinks(&os_arguments, &mut output, &mut diagnostics)
        .map_err(|error| error.to_string())?;
    if result.exit_code == 0 {
        Ok(())
    } else {
        Err(format!(
            "ronin {} in {} exited with code {}",
            arguments.join(" "),
            directory.display(),
            result.exit_code
        ))
    }
}

fn command_evaluation(directory: &Path) -> io::Result<Workload> {
    workloads::command_evaluation(directory)?;
    Ok(Workload {
        name: "manifest-command-evaluation",
        directory: directory.to_owned(),
        arguments: vec!["-t".into(), "commands".into(), "all".into()],
        build_statements: COMMAND_EDGES + 1,
    })
}

fn deep_graph(directory: &Path) -> io::Result<Workload> {
    workloads::deep_graph(directory)?;
    Ok(Workload {
        name: "deep-graph-evaluation",
        directory: directory.to_owned(),
        arguments: Vec::new(),
        build_statements: DEEP_EDGES,
    })
}

fn wide_noop(directory: &Path) -> io::Result<Workload> {
    workloads::wide_noop(directory)?;
    Ok(Workload {
        name: "wide-noop-build",
        directory: directory.to_owned(),
        arguments: Vec::new(),
        build_statements: WIDE_EDGES + 1,
    })
}

fn path_canonicalization(directory: &Path) -> io::Result<Workload> {
    workloads::path_canonicalization(directory)?;
    Ok(Workload {
        name: "path-canonicalization",
        directory: directory.to_owned(),
        arguments: vec!["-t".into(), "targets".into(), "all".into()],
        build_statements: CANONICAL_PATHS,
    })
}

fn dependency_log(directory: &Path) -> Result<Workload, String> {
    workloads::dependency_log_sources(directory).map_err(|error| error.to_string())?;
    run_ronin(directory, &[])?;
    if !directory.join(".ninja_deps").is_file() {
        return Err("priming build did not create .ninja_deps".to_owned());
    }
    Ok(Workload {
        name: "dependency-log-load",
        directory: directory.to_owned(),
        arguments: Vec::new(),
        build_statements: DEPENDENCY_EDGES + 1,
    })
}

/// Scan many explicit targets in one invocation.
///
/// Every other workload names one target, so each scan traverses the graph
/// once and per-traversal scratch never repeats. Naming many targets is the
/// shape that repeated traversal actually takes, as do restat completions and
/// dyndep reloads.
const MULTI_TARGETS: usize = 200;

fn multi_target_scan(directory: &Path) -> io::Result<Workload> {
    workloads::wide_noop(directory)?;
    Ok(Workload {
        name: "multi-target-scan",
        directory: directory.to_owned(),
        arguments: (0..MULTI_TARGETS)
            .map(|index| format!("leaf/{index}"))
            .collect(),
        build_statements: MULTI_TARGETS,
    })
}

/// Scan edges whose dependencies come from depfiles rather than the deps log.
///
/// `dependency-log-load` measures `.ninja_deps` ingestion, which never parses a
/// depfile. Declaring `depfile` without `deps` keeps every rebuild scan on the
/// depfile parser, which is the path a real build takes after each compile.
const DEPFILE_EDGES: usize = 100;
const DEPFILE_HEADERS: usize = 40;

fn depfile_scan(directory: &Path) -> Result<Workload, String> {
    let error = |error: io::Error| error.to_string();
    fs::create_dir_all(directory.join("src")).map_err(error)?;
    fs::create_dir_all(directory.join("out")).map_err(error)?;
    fs::create_dir_all(directory.join("include")).map_err(error)?;

    let mut headers = String::new();
    for index in 0..DEPFILE_HEADERS {
        let path = format!("include/h{index}.h");
        fs::write(directory.join(&path), b"/* header */\n").map_err(error)?;
        let _ = write!(headers, " {path}");
    }

    let mut manifest = String::from(
        "rule compile\n  command = cp $out.d.in $out.d && touch $out\n  depfile = $out.d\n",
    );
    for index in 0..DEPFILE_EDGES {
        fs::write(directory.join(format!("src/{index}.c")), b"int scan;\n").map_err(error)?;
        fs::write(
            directory.join(format!("out/{index}.o.d.in")),
            format!("out/{index}.o: src/{index}.c{headers}\n"),
        )
        .map_err(error)?;
        let _ = writeln!(manifest, "build out/{index}.o: compile src/{index}.c");
    }
    manifest.push_str("build all: phony");
    for index in 0..DEPFILE_EDGES {
        let _ = write!(manifest, " out/{index}.o");
    }
    manifest.push_str("\ndefault all\n");
    fs::create_dir_all(directory).map_err(error)?;
    fs::write(directory.join("build.ninja"), &manifest).map_err(error)?;

    run_ronin(directory, &[])?;
    if !directory.join("out/0.o.d").is_file() {
        return Err("priming build did not leave depfiles in place".to_owned());
    }
    Ok(Workload {
        name: "depfile-scan",
        directory: directory.to_owned(),
        arguments: Vec::new(),
        build_statements: DEPFILE_EDGES + 1,
    })
}

/// Re-scan a tree that is already up to date.
///
/// Every other workload leaves the graph dirty, so this is the only one that
/// reaches the build-log reader or evaluates a clean edge.
fn clean_tree(directory: &Path) -> Result<Workload, String> {
    workloads::clean_tree_sources(directory).map_err(|error| error.to_string())?;
    run_ronin(directory, &[])?;
    if !directory.join(".ninja_log").is_file() {
        return Err("priming build did not create .ninja_log".to_owned());
    }
    Ok(Workload {
        name: "clean-tree-noop",
        directory: directory.to_owned(),
        arguments: Vec::new(),
        build_statements: CLEAN_TREE_EDGES + 1,
    })
}

/// Parse and evaluate a manifest at real-project scale.
///
/// Allocation traffic per build statement is only known at fixture size, and
/// the size axis is exactly where Ronin's standing against C samurai erodes.
fn large_manifest(directory: &Path) -> io::Result<Workload> {
    workloads::large_manifest(directory)?;
    Ok(Workload {
        name: "large-manifest-parse",
        directory: directory.to_owned(),
        arguments: vec!["-t".into(), "commands".into(), "all".into()],
        build_statements: LARGE_MANIFEST_EDGES + 1,
    })
}

fn scheduler(directory: &Path) -> io::Result<Workload> {
    workloads::scheduler(directory)?;
    Ok(Workload {
        name: "scheduler-barrier",
        directory: directory.to_owned(),
        arguments: vec!["-j".into(), "8".into()],
        build_statements: SCHEDULER_EDGES + 1,
    })
}

fn workload_catalog(root: &Path) -> Result<Vec<Workload>, String> {
    Ok(vec![
        command_evaluation(&root.join("command-evaluation")).map_err(|error| error.to_string())?,
        deep_graph(&root.join("deep-graph")).map_err(|error| error.to_string())?,
        wide_noop(&root.join("wide-noop")).map_err(|error| error.to_string())?,
        path_canonicalization(&root.join("canonicalization")).map_err(|error| error.to_string())?,
        dependency_log(&root.join("dependency-log"))?,
        clean_tree(&root.join("clean-tree"))?,
        large_manifest(&root.join("large-manifest")).map_err(|error| error.to_string())?,
        depfile_scan(&root.join("depfile-scan"))?,
        multi_target_scan(&root.join("multi-target")).map_err(|error| error.to_string())?,
        scheduler(&root.join("scheduler")).map_err(|error| error.to_string())?,
    ])
}

fn measure(workload: &Workload) -> Result<Sample, String> {
    let start_faults = minor_faults();
    let start_requests = ALLOCATION_REQUESTS.load(Ordering::Relaxed);
    let start_bytes = REQUESTED_BYTES.load(Ordering::Relaxed);
    run_ronin(&workload.directory, &workload.arguments)?;
    let requests = ALLOCATION_REQUESTS.load(Ordering::Relaxed) - start_requests;
    let bytes = REQUESTED_BYTES.load(Ordering::Relaxed) - start_bytes;
    let minor_faults = minor_faults()
        .zip(start_faults)
        .map(|(end, start)| end.saturating_sub(start));
    Ok(Sample {
        requests,
        bytes,
        minor_faults,
    })
}

#[allow(
    clippy::cast_precision_loss,
    reason = "per-statement ratios are display-only and far below 2^53"
)]
fn per_statement(value: u64, build_statements: usize) -> f64 {
    value as f64 / build_statements as f64
}

fn print_report(records: &[Record]) {
    println!("# Ronin allocation metrics");
    println!("# schema={SCHEMA}");
    println!("# workload_version={WORKLOAD_VERSION}");
    println!("# build_profile={}", build_profile());
    println!(
        "# platform={}-{}",
        std::env::consts::OS,
        std::env::consts::ARCH
    );
    println!();
    println!(
        "{:<30} {:>8} {:>12} {:>14} {:>10} {:>12} {:>12}",
        "workload", "builds", "requests", "bytes", "req/build", "bytes/build", "minor-faults"
    );
    for record in records {
        let faults = record
            .sample
            .minor_faults
            .map_or_else(|| "-".to_owned(), |faults| faults.to_string());
        println!(
            "{:<30} {:>8} {:>12} {:>14} {:>10.1} {:>12.1} {:>12}",
            record.workload,
            record.build_statements,
            record.sample.requests,
            record.sample.bytes,
            per_statement(record.sample.requests, record.build_statements),
            per_statement(record.sample.bytes, record.build_statements),
            faults,
        );
    }
}

fn encode_csv(records: &[Record]) -> String {
    let mut output = String::new();
    let _ = writeln!(output, "# schema={SCHEMA}");
    let _ = writeln!(output, "# workload_version={WORKLOAD_VERSION}");
    let _ = writeln!(output, "# build_profile={}", build_profile());
    let _ = writeln!(
        output,
        "# platform={}-{}",
        std::env::consts::OS,
        std::env::consts::ARCH
    );
    let _ = writeln!(
        output,
        "workload,build_statements,allocation_requests,requested_bytes"
    );
    for record in records {
        let _ = writeln!(
            output,
            "{},{},{},{}",
            record.workload, record.build_statements, record.sample.requests, record.sample.bytes
        );
    }
    output
}

struct RecordedBaseline {
    build_profile: String,
    rows: BTreeMap<String, (u64, u64)>,
}

fn parse_recorded(content: &str) -> Result<RecordedBaseline, String> {
    let mut build_profile = None;
    let mut rows = BTreeMap::new();
    for line in content.lines() {
        if let Some(value) = line.strip_prefix("# build_profile=") {
            build_profile = Some(value.to_owned());
            continue;
        }
        if line.starts_with('#') || line.starts_with("workload,") || line.is_empty() {
            continue;
        }
        let fields = line.split(',').collect::<Vec<_>>();
        let [workload, _, requests, bytes] = fields.as_slice() else {
            return Err(format!("malformed recorded row: {line}"));
        };
        let requests = requests
            .parse()
            .map_err(|_| format!("malformed request count: {line}"))?;
        let bytes = bytes
            .parse()
            .map_err(|_| format!("malformed byte count: {line}"))?;
        rows.insert((*workload).to_owned(), (requests, bytes));
    }
    Ok(RecordedBaseline {
        build_profile: build_profile.ok_or_else(|| "recorded build_profile missing".to_owned())?,
        rows,
    })
}

fn validate(records: &[Record], recorded: &RecordedBaseline) -> Result<(), String> {
    if recorded.build_profile != build_profile() {
        return Err(format!(
            "recorded baseline is {} but this run is {}",
            recorded.build_profile,
            build_profile()
        ));
    }
    let mut failures = Vec::new();
    for record in records {
        let Some((recorded_requests, recorded_bytes)) = recorded.rows.get(record.workload) else {
            failures.push(format!(
                "{}: not present in recorded baseline",
                record.workload
            ));
            continue;
        };
        let request_limit = recorded_requests.saturating_mul(MAX_RECORDED_PERCENT) / 100;
        let byte_limit = recorded_bytes.saturating_mul(MAX_RECORDED_PERCENT) / 100;
        if record.sample.requests > request_limit {
            failures.push(format!(
                "{}: {} allocation requests exceed the recorded {} by more than {}%",
                record.workload,
                record.sample.requests,
                recorded_requests,
                MAX_RECORDED_PERCENT - 100
            ));
        }
        if record.sample.bytes > byte_limit {
            failures.push(format!(
                "{}: {} requested bytes exceed the recorded {} by more than {}%",
                record.workload,
                record.sample.bytes,
                recorded_bytes,
                MAX_RECORDED_PERCENT - 100
            ));
        }
    }
    for workload in recorded.rows.keys() {
        if !records.iter().any(|record| record.workload == workload) {
            failures.push(format!("{workload}: recorded workload was not measured"));
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(failures.join("\n"))
    }
}

fn run() -> Result<(), String> {
    let config = parse_arguments()?;
    let root = TemporaryDirectory::new().map_err(|error| error.to_string())?;
    let workloads = workload_catalog(&root.0)?;
    let mut records = Vec::new();
    for workload in &workloads {
        let sample = measure(workload)?;
        records.push(Record {
            workload: workload.name,
            build_statements: workload.build_statements,
            sample,
        });
    }
    print_report(&records);
    if let Some(path) = &config.record {
        fs::write(path, encode_csv(&records)).map_err(|error| error.to_string())?;
        println!("\nrecorded baseline written to {}", path.display());
    }
    if let Some(path) = &config.check {
        let content = fs::read_to_string(path).map_err(|error| error.to_string())?;
        let recorded = parse_recorded(&content)?;
        validate(&records, &recorded)?;
        println!(
            "\nallocation check passed against {} at {}% tolerance",
            path.display(),
            MAX_RECORDED_PERCENT - 100
        );
    }
    Ok(())
}

fn main() {
    if let Err(error) = run() {
        eprintln!("alloc-metrics: {error}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(workload: &'static str, requests: u64, bytes: u64) -> Record {
        Record {
            workload,
            build_statements: 100,
            sample: Sample {
                requests,
                bytes,
                minor_faults: None,
            },
        }
    }

    fn recorded(rows: &[(&str, u64, u64)]) -> RecordedBaseline {
        RecordedBaseline {
            build_profile: build_profile().to_owned(),
            rows: rows
                .iter()
                .map(|(workload, requests, bytes)| ((*workload).to_owned(), (*requests, *bytes)))
                .collect(),
        }
    }

    #[test]
    fn validation_accepts_counts_within_tolerance() {
        let records = [record("wide-noop-build", 109, 1090)];
        let baseline = recorded(&[("wide-noop-build", 100, 1000)]);
        assert!(validate(&records, &baseline).is_ok());
    }

    #[test]
    fn validation_rejects_material_increases_and_missing_workloads() {
        let records = [record("wide-noop-build", 111, 1000)];
        let baseline = recorded(&[("wide-noop-build", 100, 1000)]);
        let error = validate(&records, &baseline).unwrap_err();
        assert!(error.contains("allocation requests exceed"));

        let baseline = recorded(&[
            ("wide-noop-build", 111, 1000),
            ("deep-graph-evaluation", 1, 1),
        ]);
        let error = validate(&records, &baseline).unwrap_err();
        assert!(error.contains("recorded workload was not measured"));
    }

    #[test]
    fn recorded_baselines_round_trip() {
        let records = [record("wide-noop-build", 5, 50)];
        let encoded = encode_csv(&records);
        let parsed = parse_recorded(&encoded).unwrap();
        assert_eq!(parsed.build_profile, build_profile());
        assert_eq!(parsed.rows["wide-noop-build"], (5, 50));
    }

    /// The large-manifest generator must emit a parsable manifest.
    ///
    /// Its build lines are written with a string continuation, which strips
    /// the newline *and* the following indentation — so a missing trailing
    /// space silently welds the output onto the rule name and yields a
    /// manifest that still parses but describes a different graph.
    #[test]
    fn large_manifest_statements_are_well_formed() {
        let directory = std::env::temp_dir().join("ronin-alloc-large-manifest-shape");
        let _ = std::fs::remove_dir_all(&directory);
        workloads::large_manifest(&directory).unwrap();
        let manifest = std::fs::read_to_string(directory.join("build.ninja")).unwrap();

        assert!(manifest.contains(
            "build obj/components/mod0/target_0.o: cxx ../../src/components/mod0/source_0.cc\n"
        ));
        assert_eq!(
            manifest.matches(": cxx ").count(),
            LARGE_MANIFEST_EDGES,
            "every statement must name the rule with a separating space"
        );
        assert!(manifest.ends_with("\ndefault all\n"));
        let _ = std::fs::remove_dir_all(&directory);
    }

    /// The clean-tree workload is only meaningful if the priming build can
    /// actually make every output up to date.
    ///
    /// Its whole purpose is to be the one fixture whose graph evaluates clean,
    /// so a generator that emitted an output with no input, or omitted a
    /// source file, would silently turn it back into the dirty scan every
    /// other workload already measures.
    #[test]
    fn clean_tree_sources_are_buildable_and_complete() {
        let directory = std::env::temp_dir().join("ronin-alloc-clean-tree-shape");
        let _ = std::fs::remove_dir_all(&directory);
        workloads::clean_tree_sources(&directory).unwrap();

        let manifest = std::fs::read_to_string(directory.join("build.ninja")).unwrap();
        assert_eq!(
            manifest.matches("build out/").count(),
            CLEAN_TREE_EDGES,
            "every edge must produce one output"
        );
        for index in [0, CLEAN_TREE_EDGES / 2, CLEAN_TREE_EDGES - 1] {
            assert!(
                directory.join(format!("src/{index}.c")).is_file(),
                "input src/{index}.c must exist before the priming build"
            );
            assert!(
                manifest.contains(&format!("build out/{index}.o: copy src/{index}.c\n")),
                "edge {index} must name its input so the output can be newer than it"
            );
        }
        assert!(
            manifest.contains("\ndefault all\n"),
            "the aggregate target must be the default"
        );
        let _ = std::fs::remove_dir_all(&directory);
    }
}
