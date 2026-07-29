use std::env;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const PINNED_NINJA_REVISION: &str = "b51a1e37c2fb89bbefa600bd155e1ce13983f09d";

// [spec:samurai:req:performance.reproducible-baseline]
const WORKLOAD_VERSION: u32 = 1;
const COMMAND_EDGES: usize = 4_000;
const DEEP_EDGES: usize = 2_000;
const WIDE_EDGES: usize = 4_000;
const CANONICAL_PATHS: usize = 4_000;
const DEPENDENCY_EDGES: usize = 300;
const SCHEDULER_EDGES: usize = 128;

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

struct Config {
    ronin: PathBuf,
    ninja: PathBuf,
    samurai: Option<PathBuf>,
    ninja_source: PathBuf,
    repetitions: usize,
    warmups: usize,
    output: Option<PathBuf>,
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
            "--help" | "-h" => {
                println!(
                    "usage: cargo run --release --example baseline -- \
                     [--ronin PATH] [--ninja PATH] [--samurai PATH|--without-samurai] \
                     [--ninja-source PATH] [--warmups N] [--repetitions N] [--output PATH]"
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

fn write_manifest(directory: &Path, manifest: &str) -> io::Result<()> {
    fs::create_dir_all(directory)?;
    fs::write(directory.join("build.ninja"), manifest)
}

fn command_evaluation(directory: &Path) -> io::Result<Workload> {
    let mut manifest =
        String::from("rule cc\n  command = cc -DINDEX=$index -Iinclude $in -o $out\n");
    for index in 0..COMMAND_EDGES {
        manifest.push_str(&format!(
            "build out/{index}.o: cc src/{index}.c\n  index = {index}\n"
        ));
    }
    manifest.push_str("build all: phony");
    for index in 0..COMMAND_EDGES {
        manifest.push_str(&format!(" out/{index}.o"));
    }
    manifest.push_str("\ndefault all\n");
    write_manifest(directory, &manifest)?;
    Ok(Workload {
        name: "manifest-command-evaluation",
        directory: directory.to_owned(),
        arguments: vec!["-t".into(), "commands".into(), "all".into()],
        reset: Reset::None,
    })
}

fn deep_graph(directory: &Path) -> io::Result<Workload> {
    let mut manifest = String::from("build node/0: phony\n");
    for index in 1..DEEP_EDGES {
        manifest.push_str(&format!("build node/{index}: phony node/{}\n", index - 1));
    }
    manifest.push_str(&format!("default node/{}\n", DEEP_EDGES - 1));
    write_manifest(directory, &manifest)?;
    Ok(Workload {
        name: "deep-graph-evaluation",
        directory: directory.to_owned(),
        arguments: Vec::new(),
        reset: Reset::None,
    })
}

fn wide_noop(directory: &Path) -> io::Result<Workload> {
    let mut manifest = String::new();
    for index in 0..WIDE_EDGES {
        manifest.push_str(&format!("build leaf/{index}: phony\n"));
    }
    manifest.push_str("build all: phony");
    for index in 0..WIDE_EDGES {
        manifest.push_str(&format!(" leaf/{index}"));
    }
    manifest.push_str("\ndefault all\n");
    write_manifest(directory, &manifest)?;
    Ok(Workload {
        name: "wide-noop-build",
        directory: directory.to_owned(),
        arguments: Vec::new(),
        reset: Reset::None,
    })
}

fn path_canonicalization(directory: &Path) -> io::Result<Workload> {
    let mut manifest = String::new();
    for index in 0..CANONICAL_PATHS {
        manifest.push_str(&format!(
            "build scratch/{index}/../canonical-{index}: phony\n"
        ));
    }
    write_manifest(directory, &manifest)?;
    Ok(Workload {
        name: "path-canonicalization",
        directory: directory.to_owned(),
        arguments: vec!["-t".into(), "targets".into(), "all".into()],
        reset: Reset::None,
    })
}

fn dependency_log(directory: &Path, tool: &Tool) -> Result<Workload, String> {
    fs::create_dir_all(directory.join("src")).map_err(|error| error.to_string())?;
    fs::create_dir_all(directory.join("out")).map_err(|error| error.to_string())?;
    fs::create_dir_all(directory.join("include")).map_err(|error| error.to_string())?;
    fs::write(directory.join("include/common.h"), b"/* baseline */\n")
        .map_err(|error| error.to_string())?;

    let mut manifest = String::from(
        "rule compile\n  command = printf '$out: $in include/common.h\\n' > $out.d && touch $out\n  depfile = $out.d\n  deps = gcc\n",
    );
    for index in 0..DEPENDENCY_EDGES {
        fs::write(directory.join(format!("src/{index}.c")), b"int baseline;\n")
            .map_err(|error| error.to_string())?;
        manifest.push_str(&format!("build out/{index}.o: compile src/{index}.c\n"));
    }
    manifest.push_str("build all: phony");
    for index in 0..DEPENDENCY_EDGES {
        manifest.push_str(&format!(" out/{index}.o"));
    }
    manifest.push_str("\ndefault all\n");
    write_manifest(directory, &manifest).map_err(|error| error.to_string())?;

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

fn scheduler(directory: &Path) -> io::Result<Workload> {
    fs::create_dir_all(directory.join("jobs"))?;
    let mut manifest = String::from("rule step\n  command = touch $out\n");
    for index in 0..SCHEDULER_EDGES {
        manifest.push_str(&format!("build jobs/{index}: step\n"));
    }
    manifest.push_str("build all: phony");
    for index in 0..SCHEDULER_EDGES {
        manifest.push_str(&format!(" jobs/{index}"));
    }
    manifest.push_str("\ndefault all\n");
    write_manifest(directory, &manifest)?;
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

fn time_once(tool: &Tool, workload: &Workload) -> Result<Duration, String> {
    prepare(workload)?;
    let started = Instant::now();
    let status = Command::new(&tool.path)
        .args(&workload.arguments)
        .current_dir(&workload.directory)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|error| format!("failed to run {}: {error}", tool.path.display()))?;
    let elapsed = started.elapsed();
    if status.success() {
        Ok(elapsed)
    } else {
        Err(format!(
            "{} failed on {} with {status}",
            tool.name, workload.name
        ))
    }
}

fn median(samples: &mut [Duration]) -> Duration {
    samples.sort_unstable();
    samples[samples.len() / 2]
}

fn metadata(config: &Config, ninja_revision: &str) -> Result<String, String> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let ronin_revision = git_output(root, &["rev-parse", "HEAD"])?;
    let dirty = !git_output(root, &["status", "--porcelain"])?.is_empty();
    let platform = command_output(Path::new("uname"), &["-a"])?;
    let rustc = command_output(Path::new("rustc"), &["--version"])?;
    Ok(format!(
        "# schema=ronin-performance-baseline-v1\n\
         # workload_version={WORKLOAD_VERSION}\n\
         # ronin_revision={ronin_revision}\n\
         # ronin_dirty={dirty}\n\
         # ninja_revision={ninja_revision}\n\
         # build_profile=release\n\
         # platform={platform}\n\
         # rustc={rustc}\n\
         # warmups={}\n\
         # repetitions={}\n\
         # noise_control=sequential tool execution; stdout/stderr discarded; {} warmup(s); median wall time; no CPU pinning\n\
         # sizes=command:{COMMAND_EDGES},deep:{DEEP_EDGES},wide:{WIDE_EDGES},canonical:{CANONICAL_PATHS},deps:{DEPENDENCY_EDGES},scheduler:{SCHEDULER_EDGES}\n",
        config.warmups, config.repetitions, config.warmups
    ))
}

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
    report.push_str("tool,tool_version,workload,median_ms,min_ms,max_ms\n");

    for tool in &tools {
        let workloads = workload_catalog(&temporary.0, tool)?;
        for workload in &workloads {
            for _ in 0..config.warmups {
                time_once(tool, workload)?;
            }
            let mut samples = Vec::with_capacity(config.repetitions);
            for _ in 0..config.repetitions {
                samples.push(time_once(tool, workload)?);
            }
            let minimum = *samples.iter().min().unwrap();
            let maximum = *samples.iter().max().unwrap();
            let middle = median(&mut samples);
            report.push_str(&format!(
                "{},{},{},{:.3},{:.3},{:.3}\n",
                tool.name,
                tool.version.replace(',', "_"),
                workload.name,
                middle.as_secs_f64() * 1_000.0,
                minimum.as_secs_f64() * 1_000.0,
                maximum.as_secs_f64() * 1_000.0
            ));
        }
    }

    if let Some(path) = config.output {
        fs::write(&path, &report)
            .map_err(|error| format!("failed to write {}: {error}", path.display()))?;
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
