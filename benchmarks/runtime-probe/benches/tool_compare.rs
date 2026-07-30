//! Whole-tool high-concurrency comparison harness.

use std::fmt::Write as _;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

struct Config {
    tools: Vec<Tool>,
    children: usize,
    parallelism: usize,
    sleep_ms: u64,
    warmups: usize,
    repetitions: usize,
}

struct Tool {
    name: String,
    path: PathBuf,
}

#[derive(Clone, Copy)]
struct Measurement {
    elapsed: Duration,
    peak_threads: usize,
    peak_rss_kib: u64,
}

struct TemporaryDirectory(PathBuf);

impl TemporaryDirectory {
    fn new() -> io::Result<Self> {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "ronin-runtime-comparison-{}-{nonce}",
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
    let mut tools = Vec::new();
    let mut children = 1_024;
    let mut parallelism = 256;
    let mut sleep_ms = 20;
    let mut warmups = 1;
    let mut repetitions = 5;
    let mut arguments = std::env::args().skip(1);
    while let Some(argument) = arguments.next() {
        let value = |arguments: &mut std::iter::Skip<std::env::Args>, name: &str| {
            arguments
                .next()
                .ok_or_else(|| format!("missing value for {name}"))
        };
        match argument.as_str() {
            "--tool" => {
                let specification = value(&mut arguments, "--tool")?;
                let (name, path) = specification
                    .split_once('=')
                    .ok_or("--tool must be NAME=PATH")?;
                tools.push(Tool {
                    name: name.to_owned(),
                    path: PathBuf::from(path),
                });
            }
            "--children" => {
                children = value(&mut arguments, "--children")?
                    .parse()
                    .map_err(|_| "invalid --children value")?;
            }
            "--parallelism" => {
                parallelism = value(&mut arguments, "--parallelism")?
                    .parse()
                    .map_err(|_| "invalid --parallelism value")?;
            }
            "--sleep-ms" => {
                sleep_ms = value(&mut arguments, "--sleep-ms")?
                    .parse()
                    .map_err(|_| "invalid --sleep-ms value")?;
            }
            "--warmups" => {
                warmups = value(&mut arguments, "--warmups")?
                    .parse()
                    .map_err(|_| "invalid --warmups value")?;
            }
            "--repetitions" => {
                repetitions = value(&mut arguments, "--repetitions")?
                    .parse()
                    .map_err(|_| "invalid --repetitions value")?;
            }
            _ => return Err(format!("unknown argument: {argument}")),
        }
    }
    if tools.len() < 2 {
        return Err("provide at least two --tool NAME=PATH arguments".into());
    }
    if children == 0 || parallelism == 0 || repetitions == 0 {
        return Err("children, parallelism, and repetitions must be positive".into());
    }
    for tool in &tools {
        if !tool.path.is_file() {
            return Err(format!(
                "{} does not exist: {}",
                tool.name,
                tool.path.display()
            ));
        }
    }
    Ok(Config {
        tools,
        children,
        parallelism,
        sleep_ms,
        warmups,
        repetitions,
    })
}

fn write_workload(directory: &Path, children: usize, sleep_ms: u64) -> io::Result<()> {
    fs::create_dir_all(directory)?;
    let mut manifest = format!(
        "rule step\n  command = sleep {:.3}; printf x; touch $out\n",
        sleep_ms as f64 / 1_000.0
    );
    for index in 0..children {
        let _ = writeln!(manifest, "build out/{index}: step");
    }
    manifest.push_str("build all: phony");
    for index in 0..children {
        let _ = write!(manifest, " out/{index}");
    }
    manifest.push_str("\ndefault all\n");
    fs::write(directory.join("build.ninja"), manifest)
}

fn reset_workload(directory: &Path) -> io::Result<()> {
    let output = directory.join("out");
    match fs::remove_dir_all(&output) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    fs::create_dir(output)
}

fn sample_linux_process(process_id: u32, peak_threads: &mut usize, peak_rss_kib: &mut u64) {
    let Ok(status) = fs::read_to_string(format!("/proc/{process_id}/status")) else {
        return;
    };
    for line in status.lines() {
        if let Some(value) = line.strip_prefix("Threads:") {
            *peak_threads = (*peak_threads).max(value.trim().parse().unwrap_or_default());
        } else if let Some(value) = line.strip_prefix("VmHWM:") {
            *peak_rss_kib = (*peak_rss_kib).max(
                value
                    .split_whitespace()
                    .next()
                    .and_then(|value| value.parse().ok())
                    .unwrap_or_default(),
            );
        }
    }
}

fn measure(tool: &Tool, directory: &Path, parallelism: usize) -> Result<Measurement, String> {
    reset_workload(directory).map_err(|error| error.to_string())?;
    let started = Instant::now();
    let mut child = Command::new(&tool.path)
        .arg("-j")
        .arg(parallelism.to_string())
        .current_dir(directory)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| format!("failed to start {}: {error}", tool.name))?;
    let mut peak_threads = 0;
    let mut peak_rss_kib = 0;
    let status = loop {
        sample_linux_process(child.id(), &mut peak_threads, &mut peak_rss_kib);
        if let Some(status) = child.try_wait().map_err(|error| error.to_string())? {
            break status;
        }
        thread::sleep(Duration::from_micros(100));
    };
    if !status.success() {
        return Err(format!("{} workload failed with {status}", tool.name));
    }
    Ok(Measurement {
        elapsed: started.elapsed(),
        peak_threads,
        peak_rss_kib,
    })
}

fn median<T: Ord + Copy>(values: &mut [T]) -> T {
    values.sort_unstable();
    values[values.len() / 2]
}

fn startup_ns(tool: &Tool) -> Result<u128, String> {
    for _ in 0..3 {
        let status = Command::new(&tool.path)
            .arg("--version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map_err(|error| error.to_string())?;
        if !status.success() {
            return Err(format!("{} --version failed", tool.name));
        }
    }
    let mut samples = Vec::with_capacity(31);
    for _ in 0..31 {
        let started = Instant::now();
        let status = Command::new(&tool.path)
            .arg("--version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map_err(|error| error.to_string())?;
        if !status.success() {
            return Err(format!("{} --version failed", tool.name));
        }
        samples.push(started.elapsed().as_nanos());
    }
    Ok(median(&mut samples))
}

fn tool_version(tool: &Tool) -> Result<String, String> {
    let output = Command::new(&tool.path)
        .arg("--version")
        .output()
        .map_err(|error| error.to_string())?;
    if !output.status.success() {
        return Err(format!("{} --version failed", tool.name));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

fn run() -> Result<(), String> {
    let config = parse_arguments()?;
    let temporary = TemporaryDirectory::new().map_err(|error| error.to_string())?;
    let directories = config
        .tools
        .iter()
        .map(|tool| {
            let directory = temporary.0.join(&tool.name);
            write_workload(&directory, config.children, config.sleep_ms)
                .map_err(|error| error.to_string())?;
            Ok(directory)
        })
        .collect::<Result<Vec<_>, String>>()?;

    println!(
        "# children={} parallelism={} sleep_ms={} warmups={} repetitions={}",
        config.children, config.parallelism, config.sleep_ms, config.warmups, config.repetitions
    );
    println!(
        "tool,version,binary_bytes,startup_median_ns,workload_median_ms,workload_min_ms,\
         workload_max_ms,peak_threads_median,peak_rss_kib_median"
    );
    for (tool, directory) in config.tools.iter().zip(&directories) {
        for _ in 0..config.warmups {
            measure(tool, directory, config.parallelism)?;
        }
    }
    let mut samples = config
        .tools
        .iter()
        .map(|_| Vec::with_capacity(config.repetitions))
        .collect::<Vec<_>>();
    for repetition in 0..config.repetitions {
        for offset in 0..config.tools.len() {
            let tool_index = (repetition + offset) % config.tools.len();
            samples[tool_index].push(measure(
                &config.tools[tool_index],
                &directories[tool_index],
                config.parallelism,
            )?);
        }
    }
    for ((tool, _directory), samples) in config.tools.iter().zip(&directories).zip(&mut samples) {
        let mut elapsed = samples
            .iter()
            .map(|measurement| measurement.elapsed)
            .collect::<Vec<_>>();
        let minimum = *elapsed.iter().min().expect("samples are non-empty");
        let maximum = *elapsed.iter().max().expect("samples are non-empty");
        let middle = median(&mut elapsed);
        let mut threads = samples
            .iter()
            .map(|measurement| measurement.peak_threads)
            .collect::<Vec<_>>();
        let mut rss = samples
            .iter()
            .map(|measurement| measurement.peak_rss_kib)
            .collect::<Vec<_>>();
        let size = fs::metadata(&tool.path)
            .map_err(|error| error.to_string())?
            .len();
        println!(
            "{},{},{},{},{:.3},{:.3},{:.3},{},{}",
            tool.name,
            tool_version(tool)?.replace(',', "_"),
            size,
            startup_ns(tool)?,
            middle.as_secs_f64() * 1_000.0,
            minimum.as_secs_f64() * 1_000.0,
            maximum.as_secs_f64() * 1_000.0,
            median(&mut threads),
            median(&mut rss)
        );
    }
    Ok(())
}

fn main() {
    if let Err(error) = run() {
        eprintln!("tool comparison: {error}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tool(name: &str, path: &str) -> Tool {
        Tool {
            name: name.to_owned(),
            path: PathBuf::from(path),
        }
    }

    #[test]
    fn workload_contains_every_edge_and_default() {
        let temporary = TemporaryDirectory::new().unwrap();
        let directory = temporary.0.join("workload");
        write_workload(&directory, 3, 25).unwrap();
        let manifest = fs::read_to_string(directory.join("build.ninja")).unwrap();

        assert!(manifest.contains("sleep 0.025; printf x; touch $out"));
        assert!(manifest.contains("build out/0: step"));
        assert!(manifest.contains("build out/1: step"));
        assert!(manifest.contains("build out/2: step"));
        assert!(manifest.contains("build all: phony out/0 out/1 out/2"));
        assert!(manifest.ends_with("default all\n"));
    }

    #[test]
    fn reset_removes_outputs_and_recreates_an_empty_directory() {
        let temporary = TemporaryDirectory::new().unwrap();
        let directory = temporary.0.join("reset");
        fs::create_dir_all(directory.join("out")).unwrap();
        fs::write(directory.join("out/stale"), "stale").unwrap();

        reset_workload(&directory).unwrap();

        assert!(directory.join("out").is_dir());
        assert!(!directory.join("out/stale").exists());
        assert_eq!(fs::read_dir(directory.join("out")).unwrap().count(), 0);
    }

    #[test]
    fn reset_creates_a_missing_output_directory() {
        let temporary = TemporaryDirectory::new().unwrap();
        let directory = temporary.0.join("missing");
        fs::create_dir(&directory).unwrap();

        reset_workload(&directory).unwrap();

        assert!(directory.join("out").is_dir());
    }

    #[test]
    fn median_selects_the_middle_or_upper_middle_value() {
        assert_eq!(median(&mut [9, 1, 5]), 5);
        assert_eq!(median(&mut [4, 1, 3, 2]), 3);
        assert_eq!(
            median(&mut [
                Duration::from_millis(8),
                Duration::from_millis(2),
                Duration::from_millis(5),
            ]),
            Duration::from_millis(5)
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn process_sampling_observes_the_current_process() {
        let mut threads = 0;
        let mut rss = 0;
        sample_linux_process(std::process::id(), &mut threads, &mut rss);
        assert!(threads >= 1);
        assert!(rss > 0);
    }

    #[test]
    fn measurement_accepts_a_successful_tool_and_samples_it() {
        let temporary = TemporaryDirectory::new().unwrap();
        let directory = temporary.0.join("measure");
        fs::create_dir(&directory).unwrap();
        let measurement = measure(&tool("true", "/bin/true"), &directory, 4).unwrap();

        assert!(measurement.elapsed > Duration::ZERO);
        #[cfg(target_os = "linux")]
        assert!(measurement.peak_threads <= 1);
    }

    #[test]
    fn startup_and_version_measurements_use_the_requested_binary() {
        let true_tool = tool("true", "/bin/true");
        assert!(startup_ns(&true_tool).unwrap() > 0);

        let echo_tool = tool("echo", "/bin/echo");
        assert!(!tool_version(&echo_tool).unwrap().is_empty());
    }

    #[test]
    fn temporary_directory_is_unique_and_writable() {
        let first = TemporaryDirectory::new().unwrap();
        let second = TemporaryDirectory::new().unwrap();
        assert_ne!(first.0, second.0);
        assert!(first.0.is_dir());
        assert!(second.0.is_dir());
        fs::write(first.0.join("sample"), "data").unwrap();
        assert_eq!(fs::read_to_string(first.0.join("sample")).unwrap(), "data");
    }
}
