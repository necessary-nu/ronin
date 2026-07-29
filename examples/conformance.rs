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
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
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

// [spec:samurai:req:compat.upstream-conformance]
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
    println!("differential: 10 CLI/tool cases matched");
    println!("persistence: Ninja → Ronin → Ninja log/deps round trip passed");
    Ok(())
}

fn main() {
    let result = Config::parse().and_then(|config| run(&config));
    if let Err(error) = result {
        eprintln!("conformance: {error}");
        std::process::exit(1);
    }
}
