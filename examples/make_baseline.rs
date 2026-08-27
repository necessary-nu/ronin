//! Wall-time comparison of Ronin's Make front end against GNU Make 4.4.1.
//!
//! `examples/baseline.rs` measures Ronin's NINJA mode against pinned stock
//! Ninja, and it is the only performance gate this repository had. Make mode
//! was never compared with the tool it replaces, so nothing could say whether
//! using Ronin as `make` is faster or slower than GNU Make, and nothing would
//! have noticed a regression in either direction.
//!
//! This is that comparison. Both tools run the same targets in the same trees
//! at the same `-j`, sampled interleaved rather than one tool's whole run and
//! then the other's, because the load on a shared machine drifts over a minute
//! and a block-sampled comparison records the drift as a result.
//!
//! The recorded baseline in `benchmarks/make-baseline-v1.csv` is what was
//! MEASURED, not what would be preferred. Where Ronin is slower the recorded
//! ratio says so, and the gate's job is to keep it from getting worse.
// [spec:ronin:req:performance.make-oracle-baseline]

use std::collections::BTreeMap;
use std::env;
use std::fmt::Write as _;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

#[path = "support/make_workloads.rs"]
mod make_workloads;
use make_workloads::{
    MAKE_WORKLOAD_VERSION, RECURSION_DEPTH, RECURSION_FANOUT, RECURSION_LEAF_TARGETS, WIDE_RULES,
    recursion_units,
};

const RECORDED_BASELINE: &str = include_str!("../benchmarks/make-baseline-v1.csv");

/// How much worse than the recorded Ronin/GNU ratio a run may be before the
/// gate refuses it. The Ninja gate's tolerance, for the same reason: a margin
/// wide enough to absorb host noise and narrow enough that a real regression
/// cannot hide inside it.
const MAX_RECORDED_RUNTIME_RATIO: f64 = 1.20;

/// Workloads every gated run must measure. A run that skips one of these is
/// refused rather than validated against the rows it happened to produce.
const GATED_WORKLOADS: [&str; 4] = ["wide-noop", "recursive-noop", "vim-noop", "zsh-incremental"];

/// The clean build of vim: the number that matters most to somebody actually
/// using this as their `make`, and far too slow to run on every release pass.
/// Recorded in the baseline, measured only under `--clean-build`.
const CLEAN_BUILD_WORKLOAD: &str = "vim-clean-build";

/// Default ceiling on the one-minute load average. Wall time measured on a
/// busy machine is not a measurement, and a gate that records one anyway is
/// worse than no gate: it teaches everyone to ignore it.
const DEFAULT_MAX_LOAD: f64 = 4.0;

struct Tool {
    name: &'static str,
    path: PathBuf,
    version: String,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Shape {
    /// The tree is up to date and neither tool may build anything.
    NoOp,
    /// The tree never settles, and every run rebuilds the same small piece of
    /// it. zsh is like this under GNU Make itself: a whole-tree `make` in an
    /// already-built zsh 5.9.2 recompiles one object, updates `stamp-modobjs`
    /// and relinks `Src/zsh`, every single time, with GNU Make and with Ronin
    /// alike. Calling that a no-op would be a lie, and gating it as one would
    /// fail forever, so it is measured as what it is: the incremental steady
    /// state a zsh developer actually lives in. Roughly a second and a half of
    /// that is `gcc` and `ld`, identical for both tools, so this workload's
    /// ratio UNDERSTATES the front end's own overhead — which is exactly why
    /// the two synthetic shapes beside it exist.
    Incremental,
    /// The tree is emptied before each sample and both tools build it whole.
    CleanBuild,
}

struct Workload {
    name: &'static str,
    /// One directory per tool, by tool index. The synthetic shapes get a tree
    /// each, because Ronin writes build state into the directory it builds and
    /// a shared one would have each tool measuring the other's leavings. vim
    /// and zsh are hundreds of megabytes already configured, so both tools run
    /// in the one tree; a no-op writes no output there, and the sentinel below
    /// is what proves it.
    directories: Vec<PathBuf>,
    arguments: Vec<String>,
    shape: Shape,
    /// A built file, relative to the workload directory, whose modification
    /// time answers whether the run did what the shape claims. A no-op that
    /// rebuilds it is not a no-op and its times mean nothing; a clean build
    /// that does not is not a build.
    sentinel: PathBuf,
    /// Cleaned with this argument list before each clean-build sample.
    clean_arguments: Vec<String>,
}

struct Record {
    tool: &'static str,
    workload: &'static str,
    median_ms: f64,
}

struct Baseline {
    gnu_ms: f64,
    ronin_ms: f64,
}

struct Config {
    gnu: PathBuf,
    ronin: PathBuf,
    projects: PathBuf,
    scratch: PathBuf,
    repetitions: usize,
    warmups: usize,
    jobs: usize,
    max_load: f64,
    output: Option<PathBuf>,
    validate: bool,
    clean_build: bool,
}

impl Default for Config {
    fn default() -> Self {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        Self {
            gnu: root.join("reference/make-oracle/make-4.4.1/make"),
            // Make mode is reached by the invoked name and by nothing else, so
            // this has to be a path whose file name is `make`. The wrapper
            // script makes one; the default names where it makes it.
            ronin: root.join("target/make-performance-bin/make"),
            projects: root.join("reference/make-projects"),
            // Under the checkout rather than the system temporary directory:
            // /tmp here is a tmpfs, and a stat-bound workload measured in RAM
            // against real trees measured on disk is two different questions
            // reported as one table.
            scratch: root.join("target/make-performance"),
            repetitions: 5,
            warmups: 1,
            jobs: 8,
            max_load: DEFAULT_MAX_LOAD,
            output: None,
            validate: false,
            clean_build: false,
        }
    }
}

#[allow(
    clippy::too_many_lines,
    reason = "one switch table, and splitting it would put the option names in two places"
)]
fn parse_arguments() -> Result<Config, String> {
    let mut config = Config::default();
    let mut arguments = env::args().skip(1);
    while let Some(argument) = arguments.next() {
        let mut value = |name: &str| {
            arguments
                .next()
                .ok_or_else(|| format!("missing value for {name}"))
        };
        match argument.as_str() {
            "--gnu-make" => config.gnu = PathBuf::from(value("--gnu-make")?),
            "--ronin-make" => config.ronin = PathBuf::from(value("--ronin-make")?),
            "--projects" => config.projects = PathBuf::from(value("--projects")?),
            "--scratch" => config.scratch = PathBuf::from(value("--scratch")?),
            "--repetitions" => {
                config.repetitions = value("--repetitions")?
                    .parse()
                    .map_err(|_| "invalid --repetitions value".to_owned())?;
            }
            "--warmups" => {
                config.warmups = value("--warmups")?
                    .parse()
                    .map_err(|_| "invalid --warmups value".to_owned())?;
            }
            "--jobs" => {
                config.jobs = value("--jobs")?
                    .parse()
                    .map_err(|_| "invalid --jobs value".to_owned())?;
            }
            "--max-load" => {
                config.max_load = value("--max-load")?
                    .parse()
                    .map_err(|_| "invalid --max-load value".to_owned())?;
            }
            "--output" => config.output = Some(PathBuf::from(value("--output")?)),
            "--validate" => config.validate = true,
            "--clean-build" => config.clean_build = true,
            "--help" | "-h" => {
                println!(
                    "usage: cargo run --release --example make_baseline -- \
                     [--gnu-make PATH] [--ronin-make PATH] [--projects DIR] [--scratch DIR] \
                     [--warmups N] [--repetitions N] [--jobs N] [--max-load LOAD] \
                     [--output PATH] [--clean-build] [--validate]"
                );
                std::process::exit(0);
            }
            _ => return Err(format!("unknown argument: {argument}")),
        }
    }
    if config.repetitions == 0 {
        return Err("--repetitions must be positive".into());
    }
    if config.jobs == 0 {
        return Err("--jobs must be positive".into());
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

fn tool(name: &'static str, path: PathBuf) -> Result<Tool, String> {
    if !path.is_file() {
        return Err(format!("{name} binary does not exist: {}", path.display()));
    }
    let version = command_output(&path, &["--version"])?
        .lines()
        .next()
        .unwrap_or_default()
        .to_owned();
    Ok(Tool {
        name,
        path,
        version,
    })
}

/// The one-minute load average, or `None` where the kernel does not publish
/// one. Read once before sampling and once after, because a run that started
/// quiet and finished under somebody else's build recorded that build.
fn load_average() -> Option<f64> {
    let loadavg = fs::read_to_string("/proc/loadavg").ok()?;
    loadavg.split_whitespace().next()?.parse().ok()
}

/// Refuse to start on a busy machine.
///
/// Checked only before sampling, and that is not an oversight. Once sampling
/// has begun the one-minute average includes the harness's own workloads —
/// a `-j8` clean build of vim drives it past any threshold worth setting by
/// itself — so the reading afterwards measures this gate rather than the
/// competition for the machine. It is recorded for the reader and not gated
/// on.
fn require_quiet_host(config: &Config) -> Result<f64, String> {
    let Some(load) = load_average() else {
        return Ok(f64::NAN);
    };
    if load > config.max_load {
        return Err(format!(
            "one-minute load average is {load:.2}, above the {:.2} this gate will measure \
             at. Wall time from a busy machine is not a measurement. Wait for the host to \
             go quiet, or raise --max-load deliberately and say so in the record.",
            config.max_load
        ));
    }
    Ok(load)
}

/// The single directory under `projects` whose name starts with `prefix`.
///
/// Matched rather than named so that the pinned versions live in exactly one
/// place — `scripts/check-make-projects.sh`, which fetches and configures the
/// trees. A second copy of the version string here would drift the first time
/// one moved, and would drift silently, into a gate that then measured a tree
/// nobody was building.
fn project_tree(projects: &Path, prefix: &str) -> Result<PathBuf, String> {
    let entries = fs::read_dir(projects).map_err(|error| {
        format!(
            "reading {}: {error}. Run scripts/check-make-projects.sh first: this gate measures \
             the trees it configures and builds.",
            projects.display()
        )
    })?;
    let mut matched = entries
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| {
            path.is_dir()
                && path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with(prefix))
        })
        .collect::<Vec<_>>();
    matched.sort();
    match matched.len() {
        0 => Err(format!(
            "no {prefix}* tree under {}. Run scripts/check-make-projects.sh first.",
            projects.display()
        )),
        1 => Ok(matched.remove(0)),
        count => Err(format!(
            "{count} {prefix}* trees under {}; the gate cannot tell which one is pinned. \
             Remove the stale ones.",
            projects.display()
        )),
    }
}

fn synthetic_directories(
    scratch: &Path,
    tools: &[Tool],
    name: &str,
    write: impl Fn(&Path) -> io::Result<()>,
) -> Result<Vec<PathBuf>, String> {
    tools
        .iter()
        .map(|tool| {
            let directory = scratch.join(name).join(tool.name);
            if directory.exists() {
                fs::remove_dir_all(&directory).map_err(|error| error.to_string())?;
            }
            write(&directory)
                .map_err(|error| format!("writing {name} workload for {}: {error}", tool.name))?;
            Ok(directory)
        })
        .collect()
}

fn shared_directories(tools: &[Tool], directory: &Path) -> Vec<PathBuf> {
    tools.iter().map(|_| directory.to_owned()).collect()
}

fn workload_catalog(config: &Config, tools: &[Tool]) -> Result<Vec<Workload>, String> {
    let jobs = format!("-j{}", config.jobs);
    let vim = project_tree(&config.projects, "vim-")?;
    let zsh = project_tree(&config.projects, "zsh-")?;
    let mut catalog = vec![
        Workload {
            name: "wide-noop",
            directories: synthetic_directories(
                &config.scratch,
                tools,
                "wide",
                make_workloads::wide,
            )?,
            arguments: vec![jobs.clone()],
            shape: Shape::NoOp,
            sentinel: PathBuf::from("build/0"),
            clean_arguments: Vec::new(),
        },
        Workload {
            name: "recursive-noop",
            directories: synthetic_directories(
                &config.scratch,
                tools,
                "recursive",
                make_workloads::recursive,
            )?,
            arguments: vec![jobs.clone()],
            shape: Shape::NoOp,
            sentinel: PathBuf::from("sub0/sub0/sub0/build/0"),
            clean_arguments: Vec::new(),
        },
        Workload {
            name: "vim-noop",
            directories: shared_directories(tools, &vim),
            arguments: vec![jobs.clone()],
            shape: Shape::NoOp,
            sentinel: PathBuf::from("src/vim"),
            clean_arguments: Vec::new(),
        },
        Workload {
            name: "zsh-incremental",
            directories: shared_directories(tools, &zsh),
            arguments: vec![jobs.clone()],
            shape: Shape::Incremental,
            sentinel: PathBuf::from("Src/zsh"),
            clean_arguments: Vec::new(),
        },
    ];
    if config.clean_build {
        catalog.push(Workload {
            name: CLEAN_BUILD_WORKLOAD,
            directories: shared_directories(tools, &vim),
            arguments: vec![jobs],
            shape: Shape::CleanBuild,
            sentinel: PathBuf::from("src/vim"),
            clean_arguments: vec!["clean".to_owned()],
        });
    }
    Ok(catalog)
}

/// Run a tool in a workload directory with the invocation environment a Make
/// inherits from its parent cleared.
///
/// `MAKEFLAGS` is the one that matters: the harness itself may have been
/// started under a Make, and a leaked `MAKEFLAGS` carries that Make's switches
/// and its jobserver file descriptors into both tools, which changes what is
/// measured and can wedge the run outright.
fn invoke(tool: &Tool, directory: &Path, arguments: &[String]) -> Command {
    let mut command = Command::new(&tool.path);
    command
        .args(arguments)
        .current_dir(directory)
        .env_remove("MAKEFLAGS")
        .env_remove("MFLAGS")
        .env_remove("MAKELEVEL")
        .env_remove("MAKE_TERMOUT")
        .env_remove("MAKE_TERMERR")
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    command
}

fn run_checked(tool: &Tool, directory: &Path, arguments: &[String]) -> Result<(), String> {
    let status = invoke(tool, directory, arguments)
        .status()
        .map_err(|error| format!("failed to run {}: {error}", tool.path.display()))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!(
            "{} exited {status} in {}",
            tool.name,
            directory.display()
        ))
    }
}

fn modified(path: &Path) -> Result<std::time::SystemTime, String> {
    fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .map_err(|error| format!("reading {}: {error}", path.display()))
}

/// One sample: the wall time of one whole invocation.
///
/// The process is waited for rather than polled, which is the difference from
/// the Ninja harness beside this one. That harness reads `/proc` every hundred
/// microseconds to sample peak memory, and the cost of doing so lands on the
/// slower tool in proportion to how much slower it is. Here the only question
/// is wall time, so nothing runs between the spawn and the wait.
fn time_once(
    tool: &Tool,
    workload: &Workload,
    directory: &Path,
    gnu: &Tool,
    verify: bool,
) -> Result<Duration, String> {
    if workload.shape == Shape::CleanBuild {
        // Cleaned with GNU Make whichever tool is about to build, so both
        // start from a tree emptied the same way rather than from each tool's
        // own idea of what `clean` removes.
        run_checked(gnu, directory, &workload.clean_arguments)?;
    }
    let sentinel = directory.join(&workload.sentinel);
    let before = (workload.shape != Shape::CleanBuild)
        .then(|| modified(&sentinel))
        .transpose()?;

    let started = Instant::now();
    let status = invoke(tool, directory, &workload.arguments)
        .status()
        .map_err(|error| format!("failed to run {}: {error}", tool.path.display()))?;
    let elapsed = started.elapsed();

    if !status.success() {
        return Err(format!(
            "{} exited {status} on {}",
            tool.name, workload.name
        ));
    }
    if verify {
        verify_shape(tool, workload, &sentinel, before)?;
    }
    Ok(elapsed)
}

/// Prove the run did what its shape claims, because a time taken from a run
/// that did something else is worse than no time at all: it validates, it
/// records, and it answers a question nobody asked.
fn verify_shape(
    tool: &Tool,
    workload: &Workload,
    sentinel: &Path,
    before: Option<std::time::SystemTime>,
) -> Result<(), String> {
    // Reading it at all is the whole check for a clean build: the tree was
    // emptied before the run, so a sentinel that exists now is one this run
    // built. A tool that reported success and built nothing fails here.
    let after = modified(sentinel)?;
    let complaint = match (workload.shape, before) {
        (Shape::NoOp, Some(before)) if after != before => "rebuilt",
        (Shape::Incremental, Some(before)) if after == before => "did not rebuild",
        _ => return Ok(()),
    };
    Err(format!(
        "{} {complaint} {} on the {} workload, so that run is not the shape the \
         workload records and its time answers a different question",
        tool.name,
        workload.sentinel.display(),
        workload.name
    ))
}

fn median(samples: &mut [Duration]) -> Duration {
    samples.sort_unstable();
    samples[samples.len() / 2]
}

fn recorded_baseline() -> Result<BTreeMap<&'static str, Baseline>, String> {
    let mut baseline = BTreeMap::new();
    for (index, line) in RECORDED_BASELINE.lines().enumerate() {
        if index == 0 {
            if line != "workload,gnu_median_ms,ronin_median_ms" {
                return Err("recorded Make baseline has an invalid header".into());
            }
            continue;
        }
        let mut fields = line.split(',');
        let workload = fields
            .next()
            .ok_or_else(|| format!("recorded Make baseline line {} is invalid", index + 1))?;
        let gnu_ms = fields
            .next()
            .ok_or_else(|| format!("recorded baseline for {workload} lacks a GNU Make time"))?
            .parse::<f64>()
            .map_err(|_| format!("recorded GNU Make baseline for {workload} is invalid"))?;
        let ronin_ms = fields
            .next()
            .ok_or_else(|| format!("recorded baseline for {workload} lacks a Ronin time"))?
            .parse::<f64>()
            .map_err(|_| format!("recorded Ronin baseline for {workload} is invalid"))?;
        if fields.next().is_some() {
            return Err(format!("recorded baseline for {workload} has extra fields"));
        }
        if baseline
            .insert(workload, Baseline { gnu_ms, ronin_ms })
            .is_some()
        {
            return Err(format!("recorded Make baseline repeats {workload}"));
        }
    }
    Ok(baseline)
}

/// Refuse a run whose Ronin/GNU ratio is materially worse than the recorded
/// one.
///
/// There is deliberately no absolute "Ronin must beat GNU Make" check, which
/// is the difference from the Ninja gate. Ronin does not beat GNU Make on
/// these workloads today, and a threshold that says otherwise would fail on
/// the day it was written and be turned off on the day after. What is recorded
/// is what was measured, and what is gated is the direction of travel.
fn validate(records: &[Record]) -> Result<(), String> {
    let baseline = recorded_baseline()?;
    let ronin = records
        .iter()
        .filter(|record| record.tool == "ronin")
        .map(|record| (record.workload, record))
        .collect::<BTreeMap<_, _>>();
    let gnu = records
        .iter()
        .filter(|record| record.tool == "gnu-make")
        .map(|record| (record.workload, record))
        .collect::<BTreeMap<_, _>>();
    for gated in GATED_WORKLOADS {
        if !ronin.contains_key(gated) {
            return Err(format!("the run did not measure {gated}"));
        }
    }
    if ronin.keys().ne(gnu.keys()) {
        return Err("the two tools did not measure the same workloads".into());
    }

    for (workload, measured) in &ronin {
        let Some(baseline) = baseline.get(workload) else {
            return Err(format!("{workload} has no recorded baseline"));
        };
        let current_ratio = measured.median_ms / gnu[workload].median_ms;
        let recorded_ratio = baseline.ronin_ms / baseline.gnu_ms;
        if current_ratio > recorded_ratio * MAX_RECORDED_RUNTIME_RATIO {
            return Err(format!(
                "{workload} Ronin/GNU ratio regressed to {current_ratio:.2}x \
                 from the recorded {recorded_ratio:.2}x"
            ));
        }
    }
    Ok(())
}

fn metadata(config: &Config, tools: &[Tool], load: f64) -> Result<String, String> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let repository = root.to_str().ok_or("non-UTF-8 repository path")?;
    let revision = command_output(Path::new("git"), &["-C", repository, "rev-parse", "HEAD"])?;
    let dirty = !command_output(
        Path::new("git"),
        &["-C", repository, "status", "--porcelain"],
    )?
    .is_empty();
    let platform = command_output(Path::new("uname"), &["-a"])?;
    let rustc = command_output(Path::new("rustc"), &["--version"])?;
    let mut header = format!(
        "# schema=ronin-make-performance-baseline-v1\n\
         # workload_version={MAKE_WORKLOAD_VERSION}\n\
         # ronin_revision={revision}\n\
         # ronin_dirty={dirty}\n\
         # build_profile=release\n\
         # platform={platform}\n\
         # rustc={rustc}\n\
         # jobs={}\n\
         # warmups={}\n\
         # repetitions={}\n\
         # load_average_before={load:.2}\n\
         # max_load={:.2}\n\
         # noise_control=interleaved tool samples; stdout/stderr discarded; blocking wait, \
         no sampling thread; MAKEFLAGS/MFLAGS/MAKELEVEL cleared; median wall time; no CPU pinning\n\
         # validation=Ronin/GNU runtime ratio <= {:.0}% of the recorded ratio; no absolute \
         threshold, because Ronin is slower than GNU Make on these workloads and the recorded \
         numbers say so\n\
         # sizes=wide:{WIDE_RULES},recursive:{}units(fanout {RECURSION_FANOUT} depth \
         {RECURSION_DEPTH} leaf {RECURSION_LEAF_TARGETS})\n",
        config.jobs,
        config.warmups,
        config.repetitions,
        config.max_load,
        MAX_RECORDED_RUNTIME_RATIO * 100.0,
        recursion_units(),
    );
    for tool in tools {
        let _ = writeln!(header, "# {}={}", tool.name, tool.version);
    }
    Ok(header)
}

#[allow(
    clippy::too_many_lines,
    reason = "setup, interleaved sampling, validation and reporting form one reproducible run"
)]
fn run() -> Result<(), String> {
    let config = parse_arguments()?;
    let tools = vec![
        tool("gnu-make", config.gnu.clone())?,
        tool("ronin", config.ronin.clone())?,
    ];
    let gnu = &tools[0];
    let load = require_quiet_host(&config)?;

    fs::create_dir_all(&config.scratch)
        .map_err(|error| format!("creating {}: {error}", config.scratch.display()))?;
    let catalog = workload_catalog(&config, &tools)?;

    let mut report = metadata(&config, &tools, load)?;
    report.push_str("tool,workload,median_ms,min_ms,max_ms,samples\n");
    let mut records = Vec::new();

    for workload in &catalog {
        // Priming, and it is not only a warmup: the trees arrive built by
        // whichever tool ran last, and a no-op is only a no-op once each tool
        // has had one pass to settle whatever state it keeps. The shape is
        // deliberately NOT verified here — this is the pass whose job is to
        // make the shape true, so checking it would refuse every first run in
        // a tree the other tool built.
        for (index, tool) in tools.iter().enumerate() {
            for _ in 0..config.warmups.max(1) {
                time_once(tool, workload, &workload.directories[index], gnu, false)?;
            }
        }
        let mut samples = vec![Vec::with_capacity(config.repetitions); tools.len()];
        for repetition in 0..config.repetitions {
            for offset in 0..tools.len() {
                // Rotate which tool goes first each repetition, so neither one
                // systematically inherits the other's cache state.
                let index = (repetition + offset) % tools.len();
                samples[index].push(time_once(
                    &tools[index],
                    workload,
                    &workload.directories[index],
                    gnu,
                    true,
                )?);
            }
        }
        for (tool, mut samples) in tools.iter().zip(samples) {
            let minimum = samples.iter().copied().min().unwrap_or_default();
            let maximum = samples.iter().copied().max().unwrap_or_default();
            let count = samples.len();
            let middle = median(&mut samples);
            let _ = writeln!(
                report,
                "{},{},{:.3},{:.3},{:.3},{count}",
                tool.name,
                workload.name,
                middle.as_secs_f64() * 1_000.0,
                minimum.as_secs_f64() * 1_000.0,
                maximum.as_secs_f64() * 1_000.0,
            );
            records.push(Record {
                tool: tool.name,
                workload: workload.name,
                median_ms: middle.as_secs_f64() * 1_000.0,
            });
        }
    }

    let after = load_average().unwrap_or(f64::NAN);
    let _ = writeln!(
        report,
        "# load_average_after={after:.2} (includes this gate's own workloads)"
    );
    for workload in &catalog {
        let ronin = records
            .iter()
            .find(|record| record.tool == "ronin" && record.workload == workload.name);
        let gnu_record = records
            .iter()
            .find(|record| record.tool == "gnu-make" && record.workload == workload.name);
        if let (Some(ronin), Some(gnu_record)) = (ronin, gnu_record) {
            let _ = writeln!(
                report,
                "# ratio {}={:.2}x",
                workload.name,
                ronin.median_ms / gnu_record.median_ms
            );
        }
    }

    if let Some(path) = config.output.as_ref() {
        fs::write(path, &report)
            .map_err(|error| format!("failed to write {}: {error}", path.display()))?;
    }
    if config.validate {
        validate(&records)?;
        eprintln!(
            "validation: Ronin/GNU runtime ratio within {:.0}% of the recorded ratio",
            MAX_RECORDED_RUNTIME_RATIO * 100.0
        );
    }
    print!("{report}");
    Ok(())
}

fn main() {
    if let Err(error) = run() {
        eprintln!("make-baseline: {error}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    /// A directory of this test's own, removed when it drops.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(label: &str) -> Self {
            let nonce = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos();
            let path = env::temp_dir().join(format!(
                "ronin-make-baseline-{label}-{}-{nonce}",
                std::process::id()
            ));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn workload(name: &'static str, shape: Shape, sentinel: &str) -> Workload {
        Workload {
            name,
            directories: Vec::new(),
            arguments: Vec::new(),
            shape,
            sentinel: PathBuf::from(sentinel),
            clean_arguments: Vec::new(),
        }
    }

    fn subject() -> Tool {
        Tool {
            name: "ronin",
            path: PathBuf::from("/nonexistent"),
            version: String::new(),
        }
    }

    /// The invariant that pays for itself: the first smoke run of this harness
    /// was refused because GNU Make rebuilt `Src/zsh` on a workload described
    /// as a no-op. Without it the gate would have recorded a time for a run
    /// that did more work than the run it was compared against, and would have
    /// gone on recording it.
    // [spec:ronin:req:performance.make-oracle-baseline/test]
    #[test]
    fn a_rebuilt_no_op_is_refused() {
        let scratch = Scratch::new("noop");
        let sentinel = scratch.0.join("built");
        fs::write(&sentinel, b"first\n").unwrap();
        let before = modified(&sentinel).unwrap();
        let workload = workload("probe-noop", Shape::NoOp, "built");

        assert!(verify_shape(&subject(), &workload, &sentinel, Some(before)).is_ok());

        // Rewritten with a later timestamp, which is what a build that was
        // supposed to do nothing leaves behind.
        fs::write(&sentinel, b"second\n").unwrap();
        let touched = before + Duration::from_secs(1);
        filetime_set(&sentinel, touched);
        let error = verify_shape(&subject(), &workload, &sentinel, Some(before)).unwrap_err();
        assert!(error.contains("rebuilt"), "{error}");
        assert!(error.contains("probe-noop"), "{error}");
    }

    /// The other direction, and it is not symmetry for its own sake: zsh's
    /// tree never settles, so its workload is gated on the sentinel MOVING.
    /// A tool that quietly decided the tree was up to date would otherwise
    /// post a very good time for having done none of the work.
    // [spec:ronin:req:performance.make-oracle-baseline/test]
    #[test]
    fn an_idle_incremental_run_is_refused() {
        let scratch = Scratch::new("incremental");
        let sentinel = scratch.0.join("built");
        fs::write(&sentinel, b"first\n").unwrap();
        let before = modified(&sentinel).unwrap();
        let workload = workload("probe-incremental", Shape::Incremental, "built");

        let error = verify_shape(&subject(), &workload, &sentinel, Some(before)).unwrap_err();
        assert!(error.contains("did not rebuild"), "{error}");

        filetime_set(&sentinel, before + Duration::from_secs(1));
        assert!(verify_shape(&subject(), &workload, &sentinel, Some(before)).is_ok());
    }

    /// A clean build's whole claim is that the sentinel is there afterwards,
    /// because the tree was emptied before the run started.
    // [spec:ronin:req:performance.make-oracle-baseline/test]
    #[test]
    fn a_clean_build_that_built_nothing_is_refused() {
        let scratch = Scratch::new("clean");
        let sentinel = scratch.0.join("built");
        let workload = workload("probe-clean", Shape::CleanBuild, "built");

        assert!(verify_shape(&subject(), &workload, &sentinel, None).is_err());
        fs::write(&sentinel, b"linked\n").unwrap();
        assert!(verify_shape(&subject(), &workload, &sentinel, None).is_ok());
    }

    /// `recursion_units` is what the report header tells the reader the
    /// workload's size is, and it is computed rather than counted. If the
    /// generator and the arithmetic ever disagree, every recorded ratio is
    /// attributed to the wrong number of Makefiles.
    // [spec:ronin:req:performance.make-oracle-baseline/test]
    #[test]
    fn the_recursive_shape_writes_the_units_counted() {
        let scratch = Scratch::new("recursive");
        let root = scratch.0.join("tree");
        make_workloads::recursive(&root).unwrap();
        assert_eq!(count_makefiles(&root), recursion_units());
    }

    /// Both synthetic shapes are gated as no-ops, which is only true if every
    /// output is at least as new as the source it is built from.
    // [spec:ronin:req:performance.make-oracle-baseline/test]
    #[test]
    fn the_wide_shape_leaves_outputs_up_to_date() {
        let scratch = Scratch::new("wide");
        let root = scratch.0.join("tree");
        make_workloads::wide(&root).unwrap();
        assert!(root.join("Makefile").is_file());
        for index in [0, WIDE_RULES / 2, WIDE_RULES - 1] {
            let source = modified(&root.join(format!("src/{index}"))).unwrap();
            let output = modified(&root.join(format!("build/{index}"))).unwrap();
            assert!(output >= source, "build/{index} is older than its source");
        }
    }

    /// The pinned trees are matched by prefix so that their versions live only
    /// in `scripts/check-make-projects.sh`. Two matches has to be an error and
    /// not a coin toss: the gate would otherwise measure whichever tree sorted
    /// first, which may not be the one anything else builds.
    // [spec:ronin:req:performance.make-oracle-baseline/test]
    #[test]
    fn two_pinned_trees_are_an_error() {
        let scratch = Scratch::new("trees");
        assert!(
            project_tree(&scratch.0, "vim-")
                .unwrap_err()
                .contains("no vim-")
        );

        fs::create_dir(scratch.0.join("vim-9.2.0957")).unwrap();
        assert_eq!(
            project_tree(&scratch.0, "vim-").unwrap(),
            scratch.0.join("vim-9.2.0957")
        );

        fs::create_dir(scratch.0.join("vim-9.1.0000")).unwrap();
        let error = project_tree(&scratch.0, "vim-").unwrap_err();
        assert!(error.contains('2') && error.contains("stale"), "{error}");
    }

    fn count_makefiles(directory: &Path) -> usize {
        let mut total = usize::from(directory.join("Makefile").is_file());
        for entry in fs::read_dir(directory).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                total += count_makefiles(&path);
            }
        }
        total
    }

    /// Set a file's modification time without reaching for a crate to do it.
    fn filetime_set(path: &Path, when: SystemTime) {
        let seconds = when
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
            .to_string();
        let status = Command::new("touch")
            .args(["-d", &format!("@{seconds}")])
            .arg(path)
            .status()
            .unwrap();
        assert!(status.success());
    }

    fn passing_records() -> Vec<Record> {
        recorded_baseline()
            .unwrap()
            .into_iter()
            .filter(|(workload, _)| GATED_WORKLOADS.contains(workload))
            .flat_map(|(workload, baseline)| {
                [
                    Record {
                        tool: "gnu-make",
                        workload,
                        median_ms: baseline.gnu_ms,
                    },
                    Record {
                        tool: "ronin",
                        workload,
                        median_ms: baseline.ronin_ms,
                    },
                ]
            })
            .collect()
    }

    // [spec:ronin:req:performance.make-oracle-baseline/test]
    #[test]
    fn validation_rejects_a_make_ratio_regression() {
        let records = passing_records();
        assert!(validate(&records).is_ok());

        let mut regressed = passing_records();
        let workload = GATED_WORKLOADS[0];
        let gnu_ms = regressed
            .iter()
            .find(|record| record.tool == "gnu-make" && record.workload == workload)
            .unwrap()
            .median_ms;
        let recorded = &recorded_baseline().unwrap()[workload];
        regressed
            .iter_mut()
            .find(|record| record.tool == "ronin" && record.workload == workload)
            .unwrap()
            .median_ms =
            recorded.ronin_ms / recorded.gnu_ms * MAX_RECORDED_RUNTIME_RATIO * 1.01 * gnu_ms;
        assert!(validate(&regressed).unwrap_err().contains("regressed"));
    }

    // [spec:ronin:req:performance.make-oracle-baseline/test]
    #[test]
    fn validation_refuses_an_unmeasured_workload() {
        let dropped = GATED_WORKLOADS[GATED_WORKLOADS.len() - 1];
        let records = passing_records()
            .into_iter()
            .filter(|record| record.workload != dropped)
            .collect::<Vec<_>>();
        assert!(validate(&records).unwrap_err().contains(dropped));
    }

    // [spec:ronin:req:performance.make-oracle-baseline/test]
    #[test]
    fn the_recorded_baseline_covers_every_workload() {
        let baseline = recorded_baseline().unwrap();
        for workload in GATED_WORKLOADS {
            assert!(baseline.contains_key(workload), "{workload} has no row");
        }
        assert!(baseline.contains_key(CLEAN_BUILD_WORKLOAD));
    }
}
