use std::collections::BTreeMap;
use std::env;
use std::fmt::Write as _;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const PINNED_NINJA_REVISION: &str = "b51a1e37c2fb89bbefa600bd155e1ce13983f09d";
const RECORDED_BASELINE: &str = include_str!("../benchmarks/baseline-v1.csv");
const MAX_RECORDED_RUNTIME_RATIO: f64 = 1.20;
const MAX_NINJA_RUNTIME_RATIO: f64 = 1.20;
const MAX_NINJA_RSS_RATIO: f64 = 2.00;

// [spec:samurai:req:performance.reproducible-baseline]
#[path = "support/workloads.rs"]
mod workloads;
use workloads::{
    CANONICAL_PATHS, CLEAN_TREE_EDGES, COMMAND_EDGES, DEEP_EDGES, DEPENDENCY_EDGES,
    LARGE_MANIFEST_EDGES, SCHEDULER_EDGES, WIDE_EDGES, WORKLOAD_VERSION,
};

#[derive(Clone)]
struct Tool {
    name: &'static str,
    path: PathBuf,
    version: String,
}

#[derive(Clone, Copy)]
enum Reset {
    None,
    SchedulerOutputs,
}

struct Workload {
    name: &'static str,
    directory: PathBuf,
    arguments: Vec<String>,
    reset: Reset,
}

struct Measurement {
    elapsed: Duration,
    peak_rss_kib: Option<u64>,
}

struct Record {
    tool: &'static str,
    workload: &'static str,
    median_ms: f64,
    median_peak_rss_kib: Option<u64>,
}

struct Baseline {
    ronin_ms: f64,
    ninja_ms: f64,
}

struct Config {
    ronin: PathBuf,
    ninja: PathBuf,
    samurai: Option<PathBuf>,
    ninja_source: PathBuf,
    repetitions: usize,
    warmups: usize,
    output: Option<PathBuf>,
    validate: bool,
}

impl Default for Config {
    fn default() -> Self {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let samurai = PathBuf::from("/tmp/ronin-samu-reference");
        Self {
            ronin: root.join("target/release/ronin"),
            ninja: PathBuf::from("/tmp/ninja-build/ninja"),
            samurai: samurai.exists().then_some(samurai),
            ninja_source: PathBuf::from("/tmp/ninja"),
            repetitions: 5,
            warmups: 1,
            output: None,
            validate: false,
        }
    }
}

struct TemporaryDirectory(PathBuf);

impl TemporaryDirectory {
    fn new() -> io::Result<Self> {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let path = env::temp_dir().join(format!(
            "ronin-baseline-v{WORKLOAD_VERSION}-{}-{nonce}",
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
    let mut arguments = env::args().skip(1);
    while let Some(argument) = arguments.next() {
        let value = |arguments: &mut std::iter::Skip<env::Args>, name: &str| {
            arguments
                .next()
                .ok_or_else(|| format!("missing value for {name}"))
        };
        match argument.as_str() {
            "--ronin" => config.ronin = PathBuf::from(value(&mut arguments, "--ronin")?),
            "--ninja" => config.ninja = PathBuf::from(value(&mut arguments, "--ninja")?),
            "--samurai" => {
                config.samurai = Some(PathBuf::from(value(&mut arguments, "--samurai")?));
            }
            "--without-samurai" => config.samurai = None,
            "--ninja-source" => {
                config.ninja_source = PathBuf::from(value(&mut arguments, "--ninja-source")?);
            }
            "--repetitions" => {
                config.repetitions = value(&mut arguments, "--repetitions")?
                    .parse()
                    .map_err(|_| "invalid --repetitions value".to_owned())?;
            }
            "--warmups" => {
                config.warmups = value(&mut arguments, "--warmups")?
                    .parse()
                    .map_err(|_| "invalid --warmups value".to_owned())?;
            }
            "--output" => {
                config.output = Some(PathBuf::from(value(&mut arguments, "--output")?));
            }
            "--validate" => config.validate = true,
            "--help" | "-h" => {
                println!(
                    "usage: cargo run --release --example baseline -- \
                     [--ronin PATH] [--ninja PATH] [--samurai PATH|--without-samurai] \
                     [--ninja-source PATH] [--warmups N] [--repetitions N] [--output PATH] \
                     [--validate]"
                );
                std::process::exit(0);
            }
            _ => return Err(format!("unknown argument: {argument}")),
        }
    }
    if config.repetitions == 0 {
        return Err("--repetitions must be positive".into());
    }
    Ok(config)
}

fn command_output(program: &Path, arguments: &[&str]) -> Result<String, String> {
    let output = Command::new(program)
        .args(arguments)
        .output()
        .map_err(|error| format!("failed to run {}: {error}", program.display()))?;
    if !output.status.success() {
        return Err(format!(
            "{} failed: {}",
            program.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

fn git_output(repository: &Path, arguments: &[&str]) -> Result<String, String> {
    let mut command_arguments = vec![
        "-C",
        repository.to_str().ok_or("non-UTF-8 repository path")?,
    ];
    command_arguments.extend_from_slice(arguments);
    command_output(Path::new("git"), &command_arguments)
}

fn validate_ninja_pin(config: &Config) -> Result<String, String> {
    let revision = git_output(&config.ninja_source, &["rev-parse", "HEAD"])?;
    if revision != PINNED_NINJA_REVISION {
        return Err(format!(
            "Ninja source is {revision}, expected {PINNED_NINJA_REVISION}"
        ));
    }
    Ok(revision)
}

fn tool(name: &'static str, path: PathBuf) -> Result<Tool, String> {
    if !path.is_file() {
        return Err(format!("{name} binary does not exist: {}", path.display()));
    }
    let version = command_output(&path, &["--version"])?;
    Ok(Tool {
        name,
        path,
        version,
    })
}

fn command_evaluation(directory: &Path) -> io::Result<Workload> {
    workloads::command_evaluation(directory)?;
    Ok(Workload {
        name: "manifest-command-evaluation",
        directory: directory.to_owned(),
        arguments: vec!["-t".into(), "commands".into(), "all".into()],
        reset: Reset::None,
    })
}

fn deep_graph(directory: &Path) -> io::Result<Workload> {
    workloads::deep_graph(directory)?;
    Ok(Workload {
        name: "deep-graph-evaluation",
        directory: directory.to_owned(),
        arguments: Vec::new(),
        reset: Reset::None,
    })
}

fn wide_noop(directory: &Path) -> io::Result<Workload> {
    workloads::wide_noop(directory)?;
    Ok(Workload {
        name: "wide-noop-build",
        directory: directory.to_owned(),
        arguments: Vec::new(),
        reset: Reset::None,
    })
}

fn path_canonicalization(directory: &Path) -> io::Result<Workload> {
    workloads::path_canonicalization(directory)?;
    Ok(Workload {
        name: "path-canonicalization",
        directory: directory.to_owned(),
        arguments: vec!["-t".into(), "targets".into(), "all".into()],
        reset: Reset::None,
    })
}

fn dependency_log(directory: &Path, tool: &Tool) -> Result<Workload, String> {
    workloads::dependency_log_sources(directory).map_err(|error| error.to_string())?;
    run_checked(tool, directory, &[])?;
    if !directory.join(".ninja_deps").is_file() {
        return Err(format!("{} did not create .ninja_deps", tool.name));
    }
    Ok(Workload {
        name: "dependency-log-load",
        directory: directory.to_owned(),
        arguments: Vec::new(),
        reset: Reset::None,
    })
}

/// Re-scan a tree that is already up to date.
///
/// Every other workload leaves the graph dirty, so this is the only one that
/// reaches the build-log reader or evaluates a clean edge.
fn clean_tree(directory: &Path, tool: &Tool) -> Result<Workload, String> {
    workloads::clean_tree_sources(directory).map_err(|error| error.to_string())?;
    run_checked(tool, directory, &[])?;
    if !directory.join(".ninja_log").is_file() {
        return Err(format!("{} did not create .ninja_log", tool.name));
    }
    Ok(Workload {
        name: "clean-tree-noop",
        directory: directory.to_owned(),
        arguments: Vec::new(),
        reset: Reset::None,
    })
}

/// Parse and evaluate a manifest at real-project scale.
///
/// Runs through the commands tool so it measures parsing and evaluation with
/// no stats and no execution. That matters beyond keeping the probe clean: a
/// fixture whose sources are absent puts the two tools on wildly asymmetric
/// error paths — samurai reports the missing input after three stat calls,
/// Ronin after four hundred thousand — which once produced a confidently
/// wrong result. A scaling probe has to use a path both tools complete.
fn large_manifest(directory: &Path) -> io::Result<Workload> {
    workloads::large_manifest(directory)?;
    Ok(Workload {
        name: "large-manifest-parse",
        directory: directory.to_owned(),
        arguments: vec!["-t".into(), "commands".into(), "all".into()],
        reset: Reset::None,
    })
}

fn scheduler(directory: &Path) -> io::Result<Workload> {
    workloads::scheduler(directory)?;
    Ok(Workload {
        name: "scheduler-barrier",
        directory: directory.to_owned(),
        arguments: vec!["-j".into(), "8".into()],
        reset: Reset::SchedulerOutputs,
    })
}

fn workload_catalog(root: &Path, tool: &Tool) -> Result<Vec<Workload>, String> {
    let tool_root = root.join(tool.name);
    Ok(vec![
        command_evaluation(&tool_root.join("command-evaluation"))
            .map_err(|error| error.to_string())?,
        deep_graph(&tool_root.join("deep-graph")).map_err(|error| error.to_string())?,
        wide_noop(&tool_root.join("wide-noop")).map_err(|error| error.to_string())?,
        path_canonicalization(&tool_root.join("canonicalization"))
            .map_err(|error| error.to_string())?,
        dependency_log(&tool_root.join("dependency-log"), tool)?,
        clean_tree(&tool_root.join("clean-tree"), tool)?,
        large_manifest(&tool_root.join("large-manifest")).map_err(|error| error.to_string())?,
        scheduler(&tool_root.join("scheduler")).map_err(|error| error.to_string())?,
    ])
}

fn prepare(workload: &Workload) -> Result<(), String> {
    match workload.reset {
        Reset::None => Ok(()),
        Reset::SchedulerOutputs => {
            let directory = workload.directory.join("jobs");
            if directory.exists() {
                fs::remove_dir_all(&directory).map_err(|error| error.to_string())?;
            }
            fs::create_dir(&directory).map_err(|error| error.to_string())
        }
    }
}

fn run_checked(tool: &Tool, directory: &Path, arguments: &[String]) -> Result<(), String> {
    let output = Command::new(&tool.path)
        .args(arguments)
        .current_dir(directory)
        .output()
        .map_err(|error| format!("failed to run {}: {error}", tool.path.display()))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(format!(
            "{} failed in {}: {}",
            tool.name,
            directory.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        ))
    }
}

#[cfg(target_os = "linux")]
fn peak_rss_kib(process_id: u32) -> Option<u64> {
    let status = fs::read_to_string(format!("/proc/{process_id}/status")).ok()?;
    status.lines().find_map(|line| {
        line.strip_prefix("VmHWM:")
            .and_then(|value| value.split_whitespace().next())
            .and_then(|value| value.parse().ok())
    })
}

#[cfg(not(target_os = "linux"))]
fn peak_rss_kib(_process_id: u32) -> Option<u64> {
    None
}

fn time_once(tool: &Tool, workload: &Workload) -> Result<Measurement, String> {
    prepare(workload)?;
    let started = Instant::now();
    let mut child = Command::new(&tool.path)
        .args(&workload.arguments)
        .current_dir(&workload.directory)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| format!("failed to run {}: {error}", tool.path.display()))?;
    let mut peak_rss = None;
    let status = loop {
        if let Some(rss) = peak_rss_kib(child.id()) {
            peak_rss = Some(peak_rss.map_or(rss, |peak: u64| peak.max(rss)));
        }
        if let Some(status) = child.try_wait().map_err(|error| error.to_string())? {
            break status;
        }
        thread::sleep(Duration::from_micros(100));
    };
    let measurement = Measurement {
        elapsed: started.elapsed(),
        peak_rss_kib: peak_rss,
    };
    if status.success() {
        Ok(measurement)
    } else {
        Err(format!(
            "{} failed on {} with {status}",
            tool.name, workload.name
        ))
    }
}

fn median_duration(samples: &mut [Duration]) -> Duration {
    samples.sort_unstable();
    samples[samples.len() / 2]
}

fn median_u64(samples: &mut [u64]) -> Option<u64> {
    if samples.is_empty() {
        return None;
    }
    samples.sort_unstable();
    Some(samples[samples.len() / 2])
}

fn recorded_baseline() -> Result<BTreeMap<&'static str, Baseline>, String> {
    let mut baseline = BTreeMap::new();
    for (line_number, line) in RECORDED_BASELINE.lines().enumerate() {
        if line_number == 0 {
            if line != "workload,ronin_median_ms,ninja_median_ms" {
                return Err("recorded baseline has an invalid header".into());
            }
            continue;
        }
        let mut fields = line.split(',');
        let workload = fields
            .next()
            .ok_or_else(|| format!("recorded baseline line {} is invalid", line_number + 1))?;
        let ronin_ms = fields
            .next()
            .ok_or_else(|| format!("recorded baseline for {workload} lacks Ronin time"))?
            .parse::<f64>()
            .map_err(|_| format!("recorded Ronin baseline for {workload} is invalid"))?;
        let ninja_ms = fields
            .next()
            .ok_or_else(|| format!("recorded baseline for {workload} lacks Ninja time"))?
            .parse::<f64>()
            .map_err(|_| format!("recorded Ninja baseline for {workload} is invalid"))?;
        if fields.next().is_some() {
            return Err(format!("recorded baseline for {workload} has extra fields"));
        }
        if baseline
            .insert(workload, Baseline { ronin_ms, ninja_ms })
            .is_some()
        {
            return Err(format!("recorded baseline repeats {workload}"));
        }
    }
    Ok(baseline)
}

// [spec:samurai:req:performance.no-unexplained-regression]
#[allow(
    clippy::cast_precision_loss,
    reason = "RSS regression thresholds are deliberately approximate ratios"
)]
fn validate(records: &[Record]) -> Result<(), String> {
    let baseline = recorded_baseline()?;
    let current = records
        .iter()
        .filter(|record| record.tool == "ronin")
        .map(|record| (record.workload, record))
        .collect::<BTreeMap<_, _>>();
    let ninja = records
        .iter()
        .filter(|record| record.tool == "ninja")
        .map(|record| (record.workload, record))
        .collect::<BTreeMap<_, _>>();
    if current.len() != baseline.len() || current.keys().ne(baseline.keys()) {
        return Err("current Ronin workloads do not match the recorded baseline".into());
    }
    if ninja.keys().ne(baseline.keys()) {
        return Err("current Ninja workloads do not match the recorded baseline".into());
    }

    for (workload, baseline) in baseline {
        let current = current[workload];
        let ninja = ninja[workload];
        let current_ratio = current.median_ms / ninja.median_ms;
        let recorded_ratio = baseline.ronin_ms / baseline.ninja_ms;
        if current_ratio > recorded_ratio * MAX_RECORDED_RUNTIME_RATIO {
            return Err(format!(
                "{workload} Ronin/Ninja ratio regressed to {current_ratio:.2}x \
                 from the recorded {recorded_ratio:.2}x"
            ));
        }
        if current.median_ms > ninja.median_ms * MAX_NINJA_RUNTIME_RATIO {
            return Err(format!(
                "{workload} took {:.3} ms versus Ninja's {:.3} ms",
                current.median_ms, ninja.median_ms
            ));
        }
        match (current.median_peak_rss_kib, ninja.median_peak_rss_kib) {
            (Some(current_rss), Some(ninja_rss))
                if current_rss as f64 > ninja_rss as f64 * MAX_NINJA_RSS_RATIO =>
            {
                return Err(format!(
                    "{workload} used {current_rss} KiB peak RSS versus Ninja's {ninja_rss} KiB"
                ));
            }
            (None, _) | (_, None) if cfg!(target_os = "linux") => {
                return Err(format!("{workload} lacks a Linux peak-RSS sample"));
            }
            _ => {}
        }
    }
    Ok(())
}

fn metadata(config: &Config, ninja_revision: &str) -> Result<String, String> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let ronin_revision = git_output(root, &["rev-parse", "HEAD"])?;
    let dirty = !git_output(root, &["status", "--porcelain"])?.is_empty();
    let platform = command_output(Path::new("uname"), &["-a"])?;
    let rustc = command_output(Path::new("rustc"), &["--version"])?;
    Ok(format!(
        "# schema=ronin-performance-baseline-v2\n\
         # workload_version={WORKLOAD_VERSION}\n\
         # ronin_revision={ronin_revision}\n\
         # ronin_dirty={dirty}\n\
         # ninja_revision={ninja_revision}\n\
         # build_profile=release\n\
         # platform={platform}\n\
         # rustc={rustc}\n\
         # warmups={}\n\
         # repetitions={}\n\
         # noise_control=interleaved tool samples; stdout/stderr discarded; {} warmup(s); median wall time; Linux peak RSS sampled from /proc every 100 us; no CPU pinning\n\
         # validation_thresholds=Ronin/Ninja runtime ratio <= {:.0}% of recorded v1 ratio and Ronin runtime <= {:.0}% of pinned Ninja; peak RSS <= {:.0}% of pinned Ninja\n\
         # sizes=command:{COMMAND_EDGES},deep:{DEEP_EDGES},wide:{WIDE_EDGES},canonical:{CANONICAL_PATHS},deps:{DEPENDENCY_EDGES},clean:{CLEAN_TREE_EDGES},large:{LARGE_MANIFEST_EDGES},scheduler:{SCHEDULER_EDGES}\n",
        config.warmups,
        config.repetitions,
        config.warmups,
        MAX_RECORDED_RUNTIME_RATIO * 100.0,
        MAX_NINJA_RUNTIME_RATIO * 100.0,
        MAX_NINJA_RSS_RATIO * 100.0
    ))
}

#[allow(
    clippy::too_many_lines,
    reason = "benchmark setup, interleaved sampling, validation, and reporting form one reproducible run"
)]
fn run() -> Result<(), String> {
    let config = parse_arguments()?;
    let ninja_revision = validate_ninja_pin(&config)?;
    let mut tools = vec![
        tool("ronin", config.ronin.clone())?,
        tool("ninja", config.ninja.clone())?,
    ];
    if let Some(path) = &config.samurai {
        tools.push(tool("samurai-c", path.clone())?);
    }

    let temporary = TemporaryDirectory::new().map_err(|error| error.to_string())?;
    let mut report = metadata(&config, &ninja_revision)?;
    report.push_str(
        "tool,tool_version,workload,median_ms,min_ms,max_ms,\
         median_peak_rss_kib,min_peak_rss_kib,max_peak_rss_kib\n",
    );
    let mut records = Vec::new();
    let catalogs = tools
        .iter()
        .map(|tool| workload_catalog(&temporary.0, tool))
        .collect::<Result<Vec<_>, _>>()?;
    let workload_count = catalogs.first().map_or(0, Vec::len);
    if catalogs
        .iter()
        .any(|catalog| catalog.len() != workload_count)
    {
        return Err("tool workload catalogs differ in length".into());
    }

    for workload_index in 0..workload_count {
        let workload_name = catalogs[0][workload_index].name;
        if catalogs
            .iter()
            .any(|catalog| catalog[workload_index].name != workload_name)
        {
            return Err("tool workload catalogs differ in order".into());
        }
        for (tool, catalog) in tools.iter().zip(&catalogs) {
            for _ in 0..config.warmups {
                time_once(tool, &catalog[workload_index])?;
            }
        }
        let mut samples = (0..tools.len())
            .map(|_| Vec::with_capacity(config.repetitions))
            .collect::<Vec<_>>();
        for repetition in 0..config.repetitions {
            for offset in 0..tools.len() {
                let tool_index = (repetition + offset) % tools.len();
                samples[tool_index].push(time_once(
                    &tools[tool_index],
                    &catalogs[tool_index][workload_index],
                )?);
            }
        }
        for ((tool, catalog), samples) in tools.iter().zip(&catalogs).zip(samples) {
            let workload = &catalog[workload_index];
            let minimum = samples.iter().map(|sample| sample.elapsed).min().unwrap();
            let maximum = samples.iter().map(|sample| sample.elapsed).max().unwrap();
            let mut durations = samples
                .iter()
                .map(|sample| sample.elapsed)
                .collect::<Vec<_>>();
            let middle = median_duration(&mut durations);
            let mut peak_rss = samples
                .iter()
                .filter_map(|sample| sample.peak_rss_kib)
                .collect::<Vec<_>>();
            let minimum_rss = peak_rss.iter().min().copied();
            let maximum_rss = peak_rss.iter().max().copied();
            let middle_rss = median_u64(&mut peak_rss);
            let _ = writeln!(
                report,
                "{},{},{},{:.3},{:.3},{:.3},{},{},{}",
                tool.name,
                tool.version.replace(',', "_"),
                workload.name,
                middle.as_secs_f64() * 1_000.0,
                minimum.as_secs_f64() * 1_000.0,
                maximum.as_secs_f64() * 1_000.0,
                middle_rss.map_or_else(String::new, |rss| rss.to_string()),
                minimum_rss.map_or_else(String::new, |rss| rss.to_string()),
                maximum_rss.map_or_else(String::new, |rss| rss.to_string())
            );
            records.push(Record {
                tool: tool.name,
                workload: workload.name,
                median_ms: middle.as_secs_f64() * 1_000.0,
                median_peak_rss_kib: middle_rss,
            });
        }
    }

    if let Some(path) = config.output.as_ref() {
        fs::write(path, &report)
            .map_err(|error| format!("failed to write {}: {error}", path.display()))?;
    }
    if config.validate {
        validate(&records)?;
        eprintln!(
            "validation: normalized runtime ≤ {:.0}% of recorded ratio and ≤ {:.0}% of Ninja; \
             peak RSS ≤ {:.0}% of Ninja",
            MAX_RECORDED_RUNTIME_RATIO * 100.0,
            MAX_NINJA_RUNTIME_RATIO * 100.0,
            MAX_NINJA_RSS_RATIO * 100.0
        );
    }
    print!("{report}");
    Ok(())
}

fn main() {
    if let Err(error) = run() {
        eprintln!("baseline: {error}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn passing_records() -> Vec<Record> {
        recorded_baseline()
            .unwrap()
            .into_iter()
            .flat_map(|(workload, baseline)| {
                let ninja_ms = baseline.ninja_ms;
                let current_ratio = (baseline.ronin_ms / baseline.ninja_ms / 2.0).min(0.9);
                [
                    Record {
                        tool: "ronin",
                        workload,
                        median_ms: ninja_ms * current_ratio,
                        median_peak_rss_kib: Some(1_000),
                    },
                    Record {
                        tool: "ninja",
                        workload,
                        median_ms: ninja_ms,
                        median_peak_rss_kib: Some(1_000),
                    },
                ]
            })
            .collect()
    }

    // [spec:samurai:req:performance.no-unexplained-regression/test]
    #[test]
    fn validation_rejects_runtime_and_memory_regressions() {
        let records = passing_records();
        assert!(validate(&records).is_ok());

        let mut recorded_regression = passing_records();
        let workload = recorded_regression
            .iter()
            .find(|record| record.tool == "ronin")
            .unwrap()
            .workload;
        let baseline = &recorded_baseline().unwrap()[workload];
        let ninja = recorded_regression
            .iter()
            .find(|candidate| candidate.tool == "ninja" && candidate.workload == workload)
            .unwrap()
            .median_ms;
        recorded_regression
            .iter_mut()
            .find(|record| record.tool == "ronin" && record.workload == workload)
            .unwrap()
            .median_ms =
            baseline.ronin_ms / baseline.ninja_ms * MAX_RECORDED_RUNTIME_RATIO * 1.01 * ninja;
        assert!(validate(&recorded_regression)
            .unwrap_err()
            .contains("ratio regressed"));

        let mut ninja_regression = passing_records();
        // This case exercises the absolute Ronin-versus-Ninja check, which
        // `validate` reaches only after the recorded-ratio check passes. Both
        // can only hold at once for a workload whose recorded ratio exceeds
        // one, so pick the loosest rather than whichever happens to sort
        // first — otherwise adding a workload to the catalog silently
        // retargets this assertion at the other check.
        let workload = recorded_baseline()
            .unwrap()
            .into_iter()
            .max_by(|(_, left), (_, right)| {
                (left.ronin_ms / left.ninja_ms).total_cmp(&(right.ronin_ms / right.ninja_ms))
            })
            .unwrap()
            .0;
        let ninja_ms = ninja_regression
            .iter()
            .find(|candidate| candidate.tool == "ninja" && candidate.workload == workload)
            .unwrap()
            .median_ms;
        ninja_regression
            .iter_mut()
            .find(|record| record.tool == "ronin" && record.workload == workload)
            .unwrap()
            .median_ms = ninja_ms * 1.21;
        assert!(validate(&ninja_regression)
            .unwrap_err()
            .contains("versus Ninja"));

        let mut memory_regression = passing_records();
        memory_regression
            .iter_mut()
            .find(|record| record.tool == "ronin")
            .unwrap()
            .median_peak_rss_kib = Some(2_001);
        assert!(validate(&memory_regression)
            .unwrap_err()
            .contains("peak RSS"));
    }
}
