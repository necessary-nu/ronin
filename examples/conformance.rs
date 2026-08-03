use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};

const PINNED_REVISION: &str = "b51a1e37c2fb89bbefa600bd155e1ce13983f09d";
const INVENTORY: &str = include_str!("../tests/ninja_suite_inventory.tsv");
static NEXT_DIRECTORY: AtomicUsize = AtomicUsize::new(0);

struct Config {
    ninja_source: PathBuf,
    ninja_build: PathBuf,
    ronin: PathBuf,
}

impl Default for Config {
    fn default() -> Self {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        Self {
            ninja_source: PathBuf::from("/tmp/ninja"),
            ninja_build: PathBuf::from("/tmp/ninja-build"),
            ronin: root.join("target/release/ronin"),
        }
    }
}

impl Config {
    fn parse() -> Result<Self, String> {
        let mut config = Self::default();
        let mut arguments = env::args().skip(1);
        while let Some(argument) = arguments.next() {
            let value = |arguments: &mut std::iter::Skip<env::Args>, name: &str| {
                arguments
                    .next()
                    .ok_or_else(|| format!("missing value for {name}"))
            };
            match argument.as_str() {
                "--ninja-source" => {
                    config.ninja_source = PathBuf::from(value(&mut arguments, "--ninja-source")?);
                }
                "--ninja-build" => {
                    config.ninja_build = PathBuf::from(value(&mut arguments, "--ninja-build")?);
                }
                "--ronin" => config.ronin = PathBuf::from(value(&mut arguments, "--ronin")?),
                "--help" | "-h" => {
                    println!(
                        "usage: conformance [--ninja-source DIR] [--ninja-build DIR] [--ronin FILE]"
                    );
                    std::process::exit(0);
                }
                _ => return Err(format!("unknown argument '{argument}'")),
            }
        }
        Ok(config)
    }
}

struct TemporaryDirectory(PathBuf);

impl TemporaryDirectory {
    fn new(label: &str) -> io::Result<Self> {
        for _ in 0..1024 {
            let sequence = NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed);
            let path = env::temp_dir().join(format!(
                "ronin-conformance-{label}-{}-{sequence}",
                std::process::id()
            ));
            match fs::create_dir(&path) {
                Ok(()) => return Ok(Self(path)),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
                Err(error) => return Err(error),
            }
        }
        Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "could not allocate a unique conformance directory",
        ))
    }
}

impl Drop for TemporaryDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn command_output(program: &Path, arguments: &[&str], directory: &Path) -> Result<Output, String> {
    Command::new(program)
        .args(arguments)
        .current_dir(directory)
        .output()
        .map_err(|error| format!("running {}: {error}", program.display()))
}

fn successful_output(
    program: &Path,
    arguments: &[&str],
    directory: &Path,
) -> Result<Output, String> {
    let output = command_output(program, arguments, directory)?;
    if output.status.success() {
        Ok(output)
    } else {
        Err(format!(
            "{} {:?} failed with {:?}\nstdout:\n{}\nstderr:\n{}",
            program.display(),
            arguments,
            output.status.code(),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        ))
    }
}

fn pinned_revision(source: &Path) -> Result<String, String> {
    let output = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(source)
        .output()
        .map_err(|error| format!("reading Ninja revision: {error}"))?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).into_owned());
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

fn expected_inventory() -> Result<(BTreeMap<String, usize>, BTreeSet<String>), String> {
    let mut suites = BTreeMap::new();
    let mut overrides = BTreeSet::new();
    for (index, line) in INVENTORY.lines().enumerate() {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let fields = line.split('\t').collect::<Vec<_>>();
        if fields.len() != 5 {
            return Err(format!("inventory line {} has invalid columns", index + 1));
        }
        if fields[4].is_empty() {
            return Err(format!("inventory line {} lacks evidence", index + 1));
        }
        match fields[0] {
            "suite" => {
                if !matches!(fields[3], "mapped" | "rust-native") {
                    return Err(format!("suite {} is not classified", fields[1]));
                }
                let count = fields[2]
                    .parse::<usize>()
                    .map_err(|_| format!("invalid count for suite {}", fields[1]))?;
                if suites.insert(fields[1].to_owned(), count).is_some() {
                    return Err(format!("duplicate suite {}", fields[1]));
                }
            }
            "test" => {
                if !matches!(
                    fields[3],
                    "mapped" | "rust-native" | "platform-inapplicable"
                ) {
                    return Err(format!("test {} is not classified", fields[1]));
                }
                if !overrides.insert(fields[1].to_owned()) {
                    return Err(format!("duplicate test override {}", fields[1]));
                }
            }
            kind => return Err(format!("unknown inventory kind '{kind}'")),
        }
    }
    Ok((suites, overrides))
}

fn parse_gtest_list(list: &[u8]) -> Result<BTreeMap<String, Vec<String>>, String> {
    let list = std::str::from_utf8(list).map_err(|error| error.to_string())?;
    let mut suites = BTreeMap::<String, Vec<String>>::new();
    let mut current = None;
    for line in list.lines() {
        if !line.starts_with(' ') {
            let suite = line
                .strip_suffix('.')
                .ok_or_else(|| format!("invalid gtest suite line '{line}'"))?;
            suites.entry(suite.to_owned()).or_default();
            current = Some(suite.to_owned());
        } else if let Some(suite) = &current {
            let test = line
                .trim()
                .split_once("  #")
                .map_or_else(|| line.trim(), |(name, _)| name);
            suites.get_mut(suite).unwrap().push(test.to_owned());
        } else {
            return Err("gtest output starts with a test instead of a suite".into());
        }
    }
    Ok(suites)
}

fn verify_inventory(actual: &BTreeMap<String, Vec<String>>) -> Result<(), String> {
    let (expected, overrides) = expected_inventory()?;
    let actual_counts = actual
        .iter()
        .map(|(suite, tests)| (suite.clone(), tests.len()))
        .collect::<BTreeMap<_, _>>();
    if actual_counts != expected {
        return Err(format!(
            "Ninja suite inventory drift\nexpected: {expected:?}\nactual: {actual_counts:?}"
        ));
    }
    let actual_tests = actual
        .iter()
        .flat_map(|(suite, tests)| tests.iter().map(move |test| format!("{suite}.{test}")))
        .collect::<BTreeSet<_>>();
    for test in &overrides {
        if !actual_tests.contains(test) {
            return Err(format!(
                "inventory override '{test}' is not an upstream test"
            ));
        }
    }
    let total = actual.values().map(Vec::len).sum::<usize>();
    println!(
        "inventory: {total} upstream tests across {} suites; {} explicit per-test override(s)",
        actual.len(),
        overrides.len()
    );
    Ok(())
}

fn run_upstream_suite(ninja_test: &Path, source: &Path) -> Result<(), String> {
    let output = command_output(ninja_test, &[], source)?;
    if output.status.success() {
        println!("upstream: all 425 pinned Ninja tests passed");
        Ok(())
    } else {
        Err(format!(
            "{} failed with {:?}\nstdout:\n{}\nstderr:\n{}",
            ninja_test.display(),
            output.status.code(),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        ))
    }
}

fn write_fixture(directory: &Path) -> Result<(), String> {
    fs::write(
        directory.join("build.ninja"),
        concat!(
            "rule cc\n",
            "  command = printf object > $out && printf '$out: $in\\n' > $depfile\n",
            "  description = compile $out\n",
            "  depfile = $out.d\n",
            "  deps = gcc\n",
            "rule link\n",
            "  command = cp $in $out\n",
            "  description = link $out\n",
            "build object: cc source\n",
            "build result: link object\n",
            "build all: phony result\n",
            "default all\n"
        ),
    )
    .map_err(|error| error.to_string())?;
    fs::write(directory.join("source"), "source\n").map_err(|error| error.to_string())
}

fn compare_case(
    ninja: &Path,
    ronin: &Path,
    directory: &Path,
    arguments: &[&str],
) -> Result<(), String> {
    let ninja_output = command_output(ninja, arguments, directory)?;
    let ronin_output = command_output(ronin, arguments, directory)?;
    if ninja_output.status.code() == ronin_output.status.code()
        && ninja_output.stdout == ronin_output.stdout
        && ninja_output.stderr == ronin_output.stderr
    {
        return Ok(());
    }
    Err(format!(
        "differential mismatch for {arguments:?}\n\
         Ninja rc={:?}\nstdout:\n{}\nstderr:\n{}\n\
         Ronin rc={:?}\nstdout:\n{}\nstderr:\n{}",
        ninja_output.status.code(),
        String::from_utf8_lossy(&ninja_output.stdout),
        String::from_utf8_lossy(&ninja_output.stderr),
        ronin_output.status.code(),
        String::from_utf8_lossy(&ronin_output.stdout),
        String::from_utf8_lossy(&ronin_output.stderr)
    ))
}

/// One build whose failure and directory handling must match Ninja exactly.
///
/// These run from a parent directory with `-C`, because where the build says it
/// is going is as much a part of the contract as what it says when it stops:
/// editors resolve the relative paths in compiler diagnostics against that line.
struct BuildCase {
    name: &'static str,
    manifest: &'static str,
    arguments: &'static [&'static str],
    /// Files to create besides `source`.
    extra: &'static [(&'static str, &'static str)],
    /// Backdate the manifest, so a generator edge has something to do. Without
    /// it a manifest-regeneration case is already up to date and proves nothing.
    stale_manifest: bool,
}

const BUILD_CASES: &[BuildCase] = &[
    BuildCase {
        name: "failure propagates the command status",
        manifest: "rule f\n  command = exit 7\nbuild a: f\n",
        arguments: &["-C", "@DIR@"],
        extra: &[],
        stale_manifest: false,
    },
    BuildCase {
        name: "quiet suppresses the directory line but not the failure",
        manifest: "rule f\n  command = exit 7\nbuild a: f\n",
        arguments: &["-C", "@DIR@", "--quiet"],
        extra: &[],
        stale_manifest: false,
    },
    BuildCase {
        name: "a killed command reports the shell's status",
        manifest: "rule f\n  command = kill -KILL $$\nbuild a: f\n",
        arguments: &["-C", "@DIR@"],
        extra: &[],
        stale_manifest: false,
    },
    BuildCase {
        name: "a terminated command reads as an interrupt",
        manifest: "rule f\n  command = kill -TERM $$\nbuild a: f\n",
        arguments: &["-C", "@DIR@"],
        extra: &[],
        stale_manifest: false,
    },
    BuildCase {
        name: "a quit command is an ordinary failure",
        manifest: "rule f\n  command = kill -QUIT $$\nbuild a: f\n",
        arguments: &["-C", "@DIR@"],
        extra: &[],
        stale_manifest: false,
    },
    BuildCase {
        name: "keep-going carries the last failure out",
        manifest: TWO_FAILURES,
        arguments: &["-C", "@DIR@", "-k", "0", "-j", "1"],
        extra: &[],
        stale_manifest: false,
    },
    BuildCase {
        name: "stopping at the first failure carries that one out",
        manifest: TWO_FAILURES,
        arguments: &["-C", "@DIR@", "-j", "1"],
        extra: &[],
        stale_manifest: false,
    },
    BuildCase {
        name: "an exhausted allowance above one reports the plural",
        manifest: TWO_FAILURES,
        arguments: &["-C", "@DIR@", "-k", "2", "-j", "1"],
        extra: &[],
        stale_manifest: false,
    },
    BuildCase {
        name: "an unexhausted allowance reports lost progress",
        manifest: TWO_FAILURES,
        arguments: &["-C", "@DIR@", "-k", "3", "-j", "1"],
        extra: &[],
        stale_manifest: false,
    },
    BuildCase {
        name: "work behind a failure cannot proceed",
        manifest: "rule f\n  command = exit 3\nrule cp\n  command = cp $in $out\n\
                   build a: f\nbuild b: cp a\ndefault b\n",
        arguments: &["-C", "@DIR@", "-k", "0", "-j", "1"],
        extra: &[],
        stale_manifest: false,
    },
    BuildCase {
        name: "a console command owns the terminal while it fails",
        manifest: "rule f\n  command = exit 7\n  pool = console\nbuild a: f\n",
        arguments: &["-C", "@DIR@"],
        extra: &[],
        stale_manifest: false,
    },
    BuildCase {
        name: "a missing input is an error, not a build outcome",
        manifest: "rule cp\n  command = cp $in $out\nbuild a: cp nosuch\n",
        arguments: &["-C", "@DIR@"],
        extra: &[],
        stale_manifest: false,
    },
    BuildCase {
        name: "the directory is announced before it is entered",
        manifest: COPY_ONE,
        arguments: &["-C", "@DIR@"],
        extra: &[],
        stale_manifest: false,
    },
    BuildCase {
        name: "a directory that cannot be entered is still named",
        manifest: COPY_ONE,
        arguments: &["-C", "@DIR@/nope"],
        extra: &[],
        stale_manifest: false,
    },
    BuildCase {
        name: "repeated -C does not compose",
        manifest: COPY_ONE,
        arguments: &["-C", "@DIR@", "-C", "nested"],
        extra: &[],
        stale_manifest: false,
    },
    BuildCase {
        name: "a tool suppresses the directory line",
        manifest: COPY_ONE,
        arguments: &["-C", "@DIR@", "-t", "targets"],
        extra: &[],
        stale_manifest: false,
    },
    BuildCase {
        name: "a dry run still announces the directory",
        manifest: COPY_ONE,
        arguments: &["-C", "@DIR@", "-n"],
        extra: &[],
        stale_manifest: false,
    },
    BuildCase {
        name: "a failing manifest regeneration is an error against the manifest",
        manifest: "rule regen\n  command = exit 4\n  generator = 1\n\
                   build build.ninja: regen conf\nrule cp\n  command = cp $in $out\n\
                   build a: cp source\ndefault a\n",
        arguments: &["-C", "@DIR@"],
        extra: &[("conf", "conf\n")],
        stale_manifest: true,
    },
];

const COPY_ONE: &str = "rule cp\n  command = cp $in $out\nbuild a: cp source\n";
const TWO_FAILURES: &str = "rule f\n  command = exit 3\nrule g\n  command = exit 5\n\
                            build a: f\nbuild b: g\ndefault a b\n";

/// Removes the differences the compatibility contract requires.
///
/// The product name is Ronin's own and the temporary path differs per run;
/// everything else is compared byte for byte. The name is only replaced at the
/// start of a line and only with its colon, so a stray mention inside a path or
/// a message stays a real difference.
fn normalize(output: &[u8], directory: &Path) -> Vec<u8> {
    let text = String::from_utf8_lossy(output).into_owned();
    let text = text.replace(&directory.display().to_string(), "@DIR@");
    let mut normalized = String::with_capacity(text.len());
    for line in text.split_inclusive('\n') {
        match line
            .strip_prefix("ninja: ")
            .or_else(|| line.strip_prefix("ronin: "))
        {
            Some(rest) => {
                normalized.push_str("@TOOL@: ");
                normalized.push_str(rest);
            }
            None => normalized.push_str(line),
        }
    }
    normalized.into_bytes()
}

fn compare_build_case(ninja: &Path, ronin: &Path, case: &BuildCase) -> Result<(), String> {
    let mut rendered = Vec::new();
    for tool in [ninja, ronin] {
        let parent = TemporaryDirectory::new("case").map_err(|error| error.to_string())?;
        let directory = parent.0.join("work");
        fs::create_dir(&directory).map_err(|error| error.to_string())?;
        fs::create_dir(directory.join("nested")).map_err(|error| error.to_string())?;
        for (name, contents) in case.extra {
            fs::write(directory.join(name), contents).map_err(|error| error.to_string())?;
        }
        fs::write(directory.join("source"), "source\n").map_err(|error| error.to_string())?;
        fs::write(directory.join("build.ninja"), case.manifest)
            .map_err(|error| error.to_string())?;
        if case.stale_manifest {
            backdate(&directory.join("build.ninja"))?;
        }
        let arguments = case
            .arguments
            .iter()
            .map(|argument| argument.replace("@DIR@", &directory.display().to_string()))
            .collect::<Vec<_>>();
        let borrowed = arguments.iter().map(String::as_str).collect::<Vec<_>>();
        let output = command_output(tool, &borrowed, &parent.0)?;
        rendered.push((
            output.status.code(),
            normalize(&output.stdout, &directory),
            normalize(&output.stderr, &directory),
        ));
    }
    let (ninja_result, ronin_result) = (&rendered[0], &rendered[1]);
    if ninja_result == ronin_result {
        return Ok(());
    }
    Err(format!(
        "build case '{}' differs\n\
         Ninja rc={:?}\nstdout:\n{}\nstderr:\n{}\n\
         Ronin rc={:?}\nstdout:\n{}\nstderr:\n{}",
        case.name,
        ninja_result.0,
        String::from_utf8_lossy(&ninja_result.1),
        String::from_utf8_lossy(&ninja_result.2),
        ronin_result.0,
        String::from_utf8_lossy(&ronin_result.1),
        String::from_utf8_lossy(&ronin_result.2)
    ))
}

/// Ages a file so an edge that depends on a newer one has work to do.
fn backdate(path: &Path) -> Result<(), String> {
    let file = fs::File::options()
        .write(true)
        .open(path)
        .map_err(|error| error.to_string())?;
    let stale = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_000_000_000);
    file.set_times(fs::FileTimes::new().set_modified(stale))
        .map_err(|error| error.to_string())
}

fn verify_state_signatures(directory: &Path) -> Result<(), String> {
    let log = fs::read(directory.join(".ninja_log")).map_err(|error| error.to_string())?;
    if !log.starts_with(b"# ninja log v7\n") {
        return Err("build log does not have the Ninja v7 signature".into());
    }
    let deps = fs::read(directory.join(".ninja_deps")).map_err(|error| error.to_string())?;
    if !deps.starts_with(b"# ninjadeps\n") {
        return Err("deps log does not have the Ninja signature".into());
    }
    Ok(())
}

/// One `.ninja_log` record, as the file stores it.
#[derive(Debug, PartialEq, Eq)]
struct LogRecord {
    start: i64,
    end: i64,
    output: String,
    command_hash: String,
}

/// Read `.ninja_log` into records, ignoring the comment header.
///
/// The mtime column is deliberately dropped: it is the output's real
/// modification time, so it differs between two runs of the same build for
/// reasons that say nothing about compatibility.
fn log_records(directory: &Path) -> Result<Vec<LogRecord>, String> {
    let log =
        fs::read_to_string(directory.join(".ninja_log")).map_err(|error| error.to_string())?;
    parse_log_records(&log)
}

fn parse_log_records(log: &str) -> Result<Vec<LogRecord>, String> {
    let mut records = Vec::new();
    for line in log.lines().filter(|line| !line.starts_with('#')) {
        let fields = line.split('\t').collect::<Vec<_>>();
        let [start, end, _mtime, output, hash] = fields[..] else {
            return Err(format!("unparsable build log line: {line:?}"));
        };
        records.push(LogRecord {
            start: start.parse().map_err(|_| format!("bad start: {line:?}"))?,
            end: end.parse().map_err(|_| format!("bad end: {line:?}"))?,
            output: output.to_owned(),
            command_hash: hash.to_owned(),
        });
    }
    records.sort_by(|left, right| left.output.cmp(&right.output));
    Ok(records)
}

/// Build the fixture from scratch and return what the tool recorded.
fn record_clean_build(tool: &Path, directory: &Path) -> Result<Vec<LogRecord>, String> {
    for stale in [".ninja_log", ".ninja_deps", "object", "result", "object.d"] {
        let path = directory.join(stale);
        if path.exists() {
            fs::remove_file(&path).map_err(|error| error.to_string())?;
        }
    }
    successful_output(tool, &[], directory)?;
    log_records(directory)
}

/// Compare what each tool writes into `.ninja_log`, not merely its signature.
///
/// The signature check alone let two real defects through: Ronin recorded
/// every command with zeroed start and end times, which silently cost Ninja
/// its progress prediction when it read one of our logs, and a dry run
/// appended a full set of entries for commands it had not run. Both are
/// invisible to a check that only reads the first line of the file.
// [spec:ronin:req:compat.persistent-state]
fn compare_log_content(ninja: &Path, ronin: &Path, directory: &Path) -> Result<(), String> {
    let from_ninja = record_clean_build(ninja, directory)?;
    let from_ronin = record_clean_build(ronin, directory)?;

    let ninja_shape = from_ninja
        .iter()
        .map(|record| (&record.output, &record.command_hash))
        .collect::<Vec<_>>();
    let ronin_shape = from_ronin
        .iter()
        .map(|record| (&record.output, &record.command_hash))
        .collect::<Vec<_>>();
    if ninja_shape != ronin_shape {
        return Err(format!(
            "build log contents differ\nNinja: {from_ninja:?}\nRonin: {from_ronin:?}"
        ));
    }

    // Durations are what Ninja reads back to weight its progress prediction,
    // so a plausible ordering matters even though the values cannot match.
    for record in &from_ronin {
        if record.start > record.end {
            return Err(format!(
                "Ronin recorded a command ending before it began: {record:?}"
            ));
        }
    }
    if from_ronin
        .iter()
        .all(|record| record.start == 0 && record.end == 0)
    {
        return Err("Ronin recorded no command durations at all".into());
    }

    // A dry run must add nothing, and the tree has to have work in it for
    // that to mean anything: on an up-to-date tree a dry run plans nothing and
    // every tool passes trivially, which is how this defect survived being
    // looked for the first time.
    for stale in ["object", "result"] {
        fs::remove_file(directory.join(stale)).map_err(|error| error.to_string())?;
    }
    let before = fs::read(directory.join(".ninja_log")).map_err(|error| error.to_string())?;
    for tool in [ninja, ronin] {
        let planned = successful_output(tool, &["-n"], directory)?;
        if planned.stdout.is_empty() {
            return Err(format!(
                "{} planned no commands, so the dry-run check proves nothing",
                tool.display()
            ));
        }
        let after = fs::read(directory.join(".ninja_log")).map_err(|error| error.to_string())?;
        if before != after {
            return Err(format!(
                "{} changed the build log during a dry run",
                tool.display()
            ));
        }
    }
    Ok(())
}

// [spec:ronin:req:compat.upstream-conformance]
fn run(config: &Config) -> Result<(), String> {
    let revision = pinned_revision(&config.ninja_source)?;
    if revision != PINNED_REVISION {
        return Err(format!(
            "Ninja revision {revision} does not match pin {PINNED_REVISION}"
        ));
    }
    let ninja = config.ninja_build.join("ninja");
    let ninja_test = config.ninja_build.join("ninja_test");
    for path in [&ninja, &ninja_test, &config.ronin] {
        if !path.is_file() {
            return Err(format!("required binary {} does not exist", path.display()));
        }
    }

    let list = successful_output(&ninja_test, &["--gtest_list_tests"], &config.ninja_source)?;
    let tests = parse_gtest_list(&list.stdout)?;
    verify_inventory(&tests)?;
    run_upstream_suite(&ninja_test, &config.ninja_source)?;

    let fixture = TemporaryDirectory::new("state").map_err(|error| error.to_string())?;
    write_fixture(&fixture.0)?;
    successful_output(&ninja, &[], &fixture.0)?;
    successful_output(&config.ronin, &[], &fixture.0)?;
    verify_state_signatures(&fixture.0)?;

    for arguments in [
        &["-t", "commands", "all"][..],
        &["-t", "inputs", "all"][..],
        &["-t", "multi-inputs", "all"][..],
        &["-t", "rules", "-d"][..],
        &["-t", "targets", "all"][..],
        &["-t", "query", "result"][..],
        &["-t", "compdb", "cc"][..],
        &["-t", "compdb-targets", "all"][..],
        &["-t", "deps"][..],
        &["-t", "missingdeps", "all"][..],
    ] {
        compare_case(&ninja, &config.ronin, &fixture.0, arguments)?;
    }

    fs::remove_file(fixture.0.join("object")).map_err(|error| error.to_string())?;
    fs::remove_file(fixture.0.join("result")).map_err(|error| error.to_string())?;
    successful_output(&config.ronin, &[], &fixture.0)?;
    let ninja_noop = successful_output(&ninja, &[], &fixture.0)?;
    if ninja_noop.stdout != b"ninja: no work to do.\n" || !ninja_noop.stderr.is_empty() {
        return Err(format!(
            "Ninja did not accept Ronin's state\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&ninja_noop.stdout),
            String::from_utf8_lossy(&ninja_noop.stderr)
        ));
    }
    verify_state_signatures(&fixture.0)?;
    compare_log_content(&ninja, &config.ronin, &fixture.0)?;
    for case in BUILD_CASES {
        compare_build_case(&ninja, &config.ronin, case)?;
    }
    println!("differential: 10 CLI/tool cases matched");
    println!(
        "build outcome: {} failure and directory cases matched",
        BUILD_CASES.len()
    );
    println!("persistence: Ninja → Ronin → Ninja log/deps round trip passed");
    println!("log content: build-log records and dry-run inertness matched");
    Ok(())
}

fn main() {
    let result = Config::parse().and_then(|config| run(&config));
    if let Err(error) = result {
        eprintln!("conformance: {error}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::{parse_log_records, LogRecord};

    // [spec:ronin:req:compat.persistent-state/test]
    #[test]
    fn log_records_drop_the_header_and_the_mtime_column() {
        let records = parse_log_records("# ninja log v7\n0\t305\t17856\tobject\tabc\n")
            .expect("a well-formed log parses");
        assert_eq!(
            records,
            vec![LogRecord {
                start: 0,
                end: 305,
                output: "object".to_owned(),
                command_hash: "abc".to_owned(),
            }]
        );
    }

    // [spec:ronin:req:compat.persistent-state/test]
    #[test]
    fn log_records_sort_by_output_so_scheduling_order_does_not_matter() {
        let records = parse_log_records("0\t1\t9\tresult\tzz\n1\t2\t9\tobject\taa\n")
            .expect("a well-formed log parses");
        let outputs = records
            .iter()
            .map(|record| record.output.as_str())
            .collect::<Vec<_>>();
        assert_eq!(outputs, ["object", "result"]);
    }

    // [spec:ronin:req:compat.persistent-state/test]
    #[test]
    fn a_malformed_log_line_is_rejected_rather_than_skipped() {
        assert!(parse_log_records("0\t1\tobject\n").is_err());
        assert!(parse_log_records("x\t1\t9\tobject\taa\n").is_err());
    }
}
