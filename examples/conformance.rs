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
            // Under the checkout rather than /tmp, which a reboot empties.
            ninja_source: root.join("reference/ninja"),
            ninja_build: root.join("reference/ninja-build"),
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
    /// The process exit status this shape must leave with, as measured from
    /// Ninja and written down here.
    ///
    /// Comparing the two tools to each other already catches a status that
    /// drifts on one side. What it cannot catch is a case that quietly stops
    /// being the case it is named after: two of these once ran
    /// `kill -KILL $$`, where `$$` is a manifest escape for one `$`, so both
    /// tools agreed on the status of a `kill` that was never given a pid.
    /// A recorded number is the shape's identity, and a case that stops
    /// producing it says so.
    status: i32,
}

/// One process-boundary case the upstream unit binary does not exercise.
struct InvocationCase {
    name: &'static str,
    manifest: Option<&'static str>,
    arguments: &'static [&'static str],
    makeflags: Option<&'static str>,
    /// The process exit status this shape must leave with. See `BuildCase`.
    status: i32,
}

const INVOCATION_CASES: &[InvocationCase] = &[
    InvocationCase {
        name: "a missing manifest names the selected source",
        manifest: None,
        arguments: &["-f", "absent.custom"],
        makeflags: None,
        status: 1,
    },
    InvocationCase {
        name: "a stale inherited jobserver falls back locally",
        manifest: Some("build all: phony\ndefault all\n"),
        arguments: &[],
        makeflags: Some(" -j2 --jobserver-auth=fifo:@DIR@/missing-jobserver"),
        status: 0,
    },
    InvocationCase {
        name: "a dry run does not join an inherited jobserver",
        manifest: Some("build all: phony\ndefault all\n"),
        arguments: &["-n"],
        makeflags: Some(" -j2 --jobserver-auth=fifo:@DIR@/missing-jobserver"),
        status: 0,
    },
];

const BUILD_CASES: &[BuildCase] = &[
    BuildCase {
        name: "failure propagates the command status",
        manifest: "rule f\n  command = exit 7\nbuild a: f\n",
        arguments: &["-C", "@DIR@"],
        extra: &[],
        stale_manifest: false,
        status: 7,
    },
    BuildCase {
        name: "quiet suppresses the directory line but not the failure",
        manifest: "rule f\n  command = exit 7\nbuild a: f\n",
        arguments: &["-C", "@DIR@", "--quiet"],
        extra: &[],
        stale_manifest: false,
        status: 7,
    },
    BuildCase {
        name: "a killed command reports the shell's status",
        manifest: "rule f\n  command = kill -KILL $$$$\nbuild a: f\n",
        arguments: &["-C", "@DIR@"],
        extra: &[],
        stale_manifest: false,
        status: 137,
    },
    BuildCase {
        name: "a terminated command reads as an interrupt",
        manifest: "rule f\n  command = kill -TERM $$$$\nbuild a: f\n",
        arguments: &["-C", "@DIR@"],
        extra: &[],
        stale_manifest: false,
        status: 130,
    },
    BuildCase {
        name: "a quit command is an ordinary failure",
        manifest: "rule f\n  command = kill -QUIT $$$$\nbuild a: f\n",
        arguments: &["-C", "@DIR@"],
        extra: &[],
        stale_manifest: false,
        status: 3,
    },
    BuildCase {
        name: "a segmentation fault is an ordinary failure",
        manifest: "rule f\n  command = kill -SEGV $$$$\nbuild a: f\n",
        arguments: &["-C", "@DIR@"],
        extra: &[],
        stale_manifest: false,
        status: 11,
    },
    BuildCase {
        name: "a hung-up command reads as an interrupt",
        manifest: "rule f\n  command = kill -HUP $$$$\nbuild a: f\n",
        arguments: &["-C", "@DIR@"],
        extra: &[],
        stale_manifest: false,
        status: 130,
    },
    BuildCase {
        // Ninja's ExitStatus enum spends 130 on ExitInterrupted and then reads
        // every finished command's status back through it, so a command that
        // exits 130 by itself is an interrupt: no FAILED line, and the build
        // stops where it stands.
        name: "a command that exits 130 reads as an interrupt",
        manifest: "rule f\n  command = exit 130\nbuild a: f\n",
        arguments: &["-C", "@DIR@"],
        extra: &[],
        stale_manifest: false,
        status: 130,
    },
    BuildCase {
        name: "a large command status survives the round trip",
        manifest: "rule f\n  command = exit 111\nbuild a: f\n",
        arguments: &["-C", "@DIR@"],
        extra: &[],
        stale_manifest: false,
        status: 111,
    },
    BuildCase {
        name: "a command that does not exist reports the shell's status",
        manifest: "rule f\n  command = ronin-no-such-command\nbuild a: f\n",
        arguments: &["-C", "@DIR@"],
        extra: &[],
        stale_manifest: false,
        status: 127,
    },
    BuildCase {
        // An interrupt is checked before the completion is even counted as a
        // failure, so no allowance keeps the build going and no later status
        // replaces the one that says why it stopped.
        name: "an interrupt outranks keep-going and any later failure",
        manifest: "rule f\n  command = kill -TERM $$$$\nrule g\n  command = exit 5\n\
                   build a: f\nbuild b: g\ndefault a b\n",
        arguments: &["-C", "@DIR@", "-k", "0", "-j", "1"],
        extra: &[],
        stale_manifest: false,
        status: 130,
    },
    BuildCase {
        name: "an ordinary failure does not stop keep-going before an interrupt",
        manifest: "rule f\n  command = exit 5\nrule g\n  command = kill -TERM $$$$\n\
                   build a: f\nbuild b: g\ndefault a b\n",
        arguments: &["-C", "@DIR@", "-k", "0", "-j", "1"],
        extra: &[],
        stale_manifest: false,
        status: 130,
    },
    BuildCase {
        name: "a signal-killed command still lets keep-going finish the rest",
        manifest: "rule f\n  command = kill -SEGV $$$$\nrule ok\n  command = cp $in $out\n\
                   build a: f\nbuild b: ok source\ndefault a b\n",
        arguments: &["-C", "@DIR@", "-k", "0", "-j", "1"],
        extra: &[],
        stale_manifest: false,
        status: 11,
    },
    BuildCase {
        name: "a dry run over a failing command still succeeds",
        manifest: "rule f\n  command = exit 3\nbuild a: f\n",
        arguments: &["-C", "@DIR@", "-n"],
        extra: &[],
        stale_manifest: false,
        status: 0,
    },
    BuildCase {
        // Ninja reads a depfile that was never written as an empty one, so a
        // compiler that emits nothing for a unit with no includes keeps the
        // status it exited with.
        name: "a depfile that was never written is read as empty",
        manifest: "rule cc\n  command = cp $in $out\n  depfile = $out.d\n  deps = gcc\n\
                   build a: cc source\n",
        arguments: &["-C", "@DIR@"],
        extra: &[],
        stale_manifest: false,
        status: 0,
    },
    BuildCase {
        // The depfile's directory is the build tool's to create; the response
        // file's is not. Ninja is asymmetric here and the statuses show it.
        name: "a depfile's directory is created before the command runs",
        manifest: "rule cc\n  command = cp $in $out && printf 'a: source\\n' > $depfile\n\
                   \x20 depfile = deeper/nested/a.d\n  deps = gcc\nbuild a: cc source\n",
        arguments: &["-C", "@DIR@"],
        extra: &[],
        stale_manifest: false,
        status: 0,
    },
    BuildCase {
        // The other half of that asymmetry, and the reason it is bearable:
        // when the directory is missing the response file cannot be written,
        // and Ninja names the call, the path and the step that refused. The
        // diagnostic is on stderr, where it is printed the moment the write
        // fails; stdout gets only the summary, whose reason is empty because
        // the build loop's error string was never written to.
        name: "a response file with nowhere to go names the write that failed",
        manifest: "rule cc\n  command = touch $out\n  rspfile = absent/deeper/a.rsp\n\
                   \x20 rspfile_content = arguments\nbuild a: cc\n",
        arguments: &["-C", "@DIR@"],
        extra: &[],
        stale_manifest: false,
        status: 1,
    },
    BuildCase {
        // The same sentence with a different errno, so the case pins the
        // shape of the message rather than one system message inside it.
        name: "a response file that is a directory names the write that failed",
        manifest: "rule cc\n  command = touch $out\n  rspfile = nested\n\
                   \x20 rspfile_content = arguments\nbuild a: cc\n",
        arguments: &["-C", "@DIR@"],
        extra: &[],
        stale_manifest: false,
        status: 1,
    },
    BuildCase {
        // Ninja leaves its build loop the moment an edge cannot be started,
        // without consulting the failure allowance: `-k 0` keeps a build going
        // past commands that ran and failed, not past a file the disk refused.
        // Nothing after the first edge runs, so the run says only that.
        name: "keep-going does not survive a response file that cannot be written",
        manifest: "rule cc\n  command = touch $out\n  rspfile = absent/a.rsp\n\
                   \x20 rspfile_content = arguments\nrule touch\n  command = touch $out\n\
                   build a: cc\nbuild b: touch\nbuild c: touch\ndefault a b c\n",
        arguments: &["-C", "@DIR@", "-k", "0", "-j", "1"],
        extra: &[],
        stale_manifest: false,
        status: 1,
    },
    BuildCase {
        // `WriteFile` is stdio, so a payload that fits the stream's buffer
        // never reaches the device until the close and a full disk is reported
        // against the close rather than the write.
        name: "a short response file on a full device is reported against the close",
        manifest: "rule cc\n  command = touch $out\n  rspfile = /dev/full\n\
                   \x20 rspfile_content = arguments\nbuild a: cc\n",
        arguments: &["-C", "@DIR@"],
        extra: &[],
        stale_manifest: false,
        status: 1,
    },
    BuildCase {
        // Past the buffer the payload is handed to the kernel by the write
        // itself, which is where the same full device is reported instead.
        // Built up through bindings because the size is the point: 8 KiB is
        // clear of the 4 KiB block a device node reports.
        name: "a long response file on a full device is reported against the write",
        manifest: "sixtyfour = 0123456789abcdef0123456789abcdef\
                   0123456789abcdef0123456789abcdef\n\
                   kibibyte = $sixtyfour$sixtyfour$sixtyfour$sixtyfour\
                   $sixtyfour$sixtyfour$sixtyfour$sixtyfour\
                   $sixtyfour$sixtyfour$sixtyfour$sixtyfour\
                   $sixtyfour$sixtyfour$sixtyfour$sixtyfour\n\
                   payload = $kibibyte$kibibyte$kibibyte$kibibyte\
                   $kibibyte$kibibyte$kibibyte$kibibyte\n\
                   rule cc\n  command = touch $out\n  rspfile = /dev/full\n\
                   \x20 rspfile_content = $payload\nbuild a: cc\n",
        arguments: &["-C", "@DIR@"],
        extra: &[],
        stale_manifest: false,
        status: 1,
    },
    BuildCase {
        name: "a console command's status leaves with the build",
        manifest: "rule f\n  command = exit 111\n  pool = console\nbuild a: f\n",
        arguments: &["-C", "@DIR@"],
        extra: &[],
        stale_manifest: false,
        status: 111,
    },
    BuildCase {
        name: "keep-going carries the last failure out",
        manifest: TWO_FAILURES,
        arguments: &["-C", "@DIR@", "-k", "0", "-j", "1"],
        extra: &[],
        stale_manifest: false,
        status: 5,
    },
    BuildCase {
        name: "stopping at the first failure carries that one out",
        manifest: TWO_FAILURES,
        arguments: &["-C", "@DIR@", "-j", "1"],
        extra: &[],
        stale_manifest: false,
        status: 3,
    },
    BuildCase {
        name: "an exhausted allowance above one reports the plural",
        manifest: TWO_FAILURES,
        arguments: &["-C", "@DIR@", "-k", "2", "-j", "1"],
        extra: &[],
        stale_manifest: false,
        status: 5,
    },
    BuildCase {
        name: "an unexhausted allowance reports lost progress",
        manifest: TWO_FAILURES,
        arguments: &["-C", "@DIR@", "-k", "3", "-j", "1"],
        extra: &[],
        stale_manifest: false,
        status: 5,
    },
    BuildCase {
        name: "work behind a failure cannot proceed",
        manifest: "rule f\n  command = exit 3\nrule cp\n  command = cp $in $out\n\
                   build a: f\nbuild b: cp a\ndefault b\n",
        arguments: &["-C", "@DIR@", "-k", "0", "-j", "1"],
        extra: &[],
        stale_manifest: false,
        status: 3,
    },
    BuildCase {
        name: "a console command owns the terminal while it fails",
        manifest: "rule f\n  command = exit 7\n  pool = console\nbuild a: f\n",
        arguments: &["-C", "@DIR@"],
        extra: &[],
        stale_manifest: false,
        status: 7,
    },
    BuildCase {
        name: "a missing input is an error, not a build outcome",
        manifest: "rule cp\n  command = cp $in $out\nbuild a: cp nosuch\n",
        arguments: &["-C", "@DIR@"],
        extra: &[],
        stale_manifest: false,
        status: 1,
    },
    BuildCase {
        name: "the directory is announced before it is entered",
        manifest: COPY_ONE,
        arguments: &["-C", "@DIR@"],
        extra: &[],
        stale_manifest: false,
        status: 0,
    },
    BuildCase {
        name: "a directory that cannot be entered is still named",
        manifest: COPY_ONE,
        arguments: &["-C", "@DIR@/nope"],
        extra: &[],
        stale_manifest: false,
        status: 1,
    },
    BuildCase {
        name: "repeated -C does not compose",
        manifest: COPY_ONE,
        arguments: &["-C", "@DIR@", "-C", "nested"],
        extra: &[],
        stale_manifest: false,
        status: 1,
    },
    BuildCase {
        name: "a tool suppresses the directory line",
        manifest: COPY_ONE,
        arguments: &["-C", "@DIR@", "-t", "targets"],
        extra: &[],
        stale_manifest: false,
        status: 0,
    },
    BuildCase {
        name: "a dry run still announces the directory",
        manifest: COPY_ONE,
        arguments: &["-C", "@DIR@", "-n"],
        extra: &[],
        stale_manifest: false,
        status: 0,
    },
    BuildCase {
        name: "a self-referencing phony names the cycle and the flag",
        manifest: "build a: phony a\nbuild b: phony a\ndefault b\n",
        arguments: &["-C", "@DIR@", "-w", "phonycycle=err"],
        extra: &[],
        stale_manifest: false,
        status: 1,
    },
    BuildCase {
        name: "a phony cycle through two nodes names both",
        manifest: "build a: phony c\nbuild c: phony a\ndefault a\n",
        arguments: &["-C", "@DIR@"],
        extra: &[],
        stale_manifest: false,
        status: 1,
    },
    BuildCase {
        name: "a cycle through real rules names the path",
        manifest: "rule cp\n  command = cp $in $out\n\
                   build a: cp b\nbuild b: cp c\nbuild c: cp a\ndefault a\n",
        arguments: &["-C", "@DIR@"],
        extra: &[],
        stale_manifest: false,
        status: 1,
    },
    BuildCase {
        // Ninja reports the cycle from the node that closes it, not from
        // whichever other output of that edge was asked for.
        name: "a cycle through a multi-output edge starts where it closes",
        manifest: "rule cat\n  command = cat $in > $out\n\
                   build a b: cat c\nbuild c: cat a\ndefault b\n",
        arguments: &["-C", "@DIR@"],
        extra: &[],
        stale_manifest: false,
        status: 1,
    },
    BuildCase {
        name: "an unexpected rule binding is located and named",
        manifest: "rule cc\n  command = gcc\n  nonsense = x\nbuild a: cc\n",
        arguments: &["-C", "@DIR@"],
        extra: &[],
        stale_manifest: false,
        status: 1,
    },
    BuildCase {
        name: "a duplicate rule is located",
        manifest: "rule cc\n  command = a\nrule cc\n  command = b\n",
        arguments: &["-C", "@DIR@"],
        extra: &[],
        stale_manifest: false,
        status: 1,
    },
    BuildCase {
        name: "a rule with no command is reported after its block",
        manifest: "rule cc\n",
        arguments: &["-C", "@DIR@"],
        extra: &[],
        stale_manifest: false,
        status: 1,
    },
    BuildCase {
        name: "a half-specified response file is reported after its block",
        manifest: "rule cc\n  command = gcc\n  rspfile = f\nbuild a: cc\n",
        arguments: &["-C", "@DIR@"],
        extra: &[],
        stale_manifest: false,
        status: 1,
    },
    BuildCase {
        name: "a pool with no depth is reported after its block",
        manifest: "pool p\n",
        arguments: &["-C", "@DIR@"],
        extra: &[],
        stale_manifest: false,
        status: 1,
    },
    BuildCase {
        name: "an unknown build rule is located at the name",
        manifest: "build a: nosuchrule\n",
        arguments: &["-C", "@DIR@"],
        extra: &[],
        stale_manifest: false,
        status: 1,
    },
    BuildCase {
        name: "a missing colon names what was found and hints",
        manifest: "build a\n",
        arguments: &["-C", "@DIR@"],
        extra: &[],
        stale_manifest: false,
        status: 1,
    },
    BuildCase {
        name: "an unexpected indent carries no source context",
        manifest: "  command = x\n",
        arguments: &["-C", "@DIR@"],
        extra: &[],
        stale_manifest: false,
        status: 1,
    },
    // An indent is a token of its own and it starts at the first of its
    // spaces, so an unexpected one names the *indented* line and never carries
    // context — its column is zero. The case above is the one shape where
    // reporting it against the line before gives the same answer, because
    // there is no line before it. Each of these is a shape where it does not.
    BuildCase {
        name: "an unexpected indent names the indented line, not the one that closed the block",
        manifest: "rule cc\n  command = x\ndepfile = y\n  deps = gcc\nbuild a: cc\n",
        arguments: &["-C", "@DIR@"],
        extra: &[],
        stale_manifest: false,
        status: 1,
    },
    BuildCase {
        name: "an unexpected indent after a top-level binding names its own line",
        manifest: "x = 1\n  y = 2\n",
        arguments: &["-C", "@DIR@"],
        extra: &[],
        stale_manifest: false,
        status: 1,
    },
    BuildCase {
        name: "an unexpected indent after a blank line names its own line",
        manifest: "x = 1\n\n  y = 2\n",
        arguments: &["-C", "@DIR@"],
        extra: &[],
        stale_manifest: false,
        status: 1,
    },
    BuildCase {
        name: "an unexpected indent after a comment line names its own line",
        manifest: "x = 1\n# comment\n  y = 2\n",
        arguments: &["-C", "@DIR@"],
        extra: &[],
        stale_manifest: false,
        status: 1,
    },
    BuildCase {
        // Upstream's own parser case for this: the indented blank line ends
        // the rule, so the binding under it belongs to nothing.
        name: "an indented blank line ends a rule and the indent below it is unexpected",
        manifest: "rule r\n  command = r\n  \n  generator = 1\n",
        arguments: &["-C", "@DIR@"],
        extra: &[],
        stale_manifest: false,
        status: 1,
    },
    BuildCase {
        name: "an unexpected indent after a build statement's block names its own line",
        manifest: "rule cc\n  command = x\nbuild a: cc\n  pool = console\nx = 1\n  y = 2\n",
        arguments: &["-C", "@DIR@"],
        extra: &[],
        stale_manifest: false,
        status: 1,
    },
    BuildCase {
        // The line above is too long to quote, so *both* tools print no
        // context and only the line number says which line is meant.
        // Anchoring an indent at the line before it is invisible here.
        name: "an unexpected indent below a line too long to quote still names its own line",
        manifest:
            "x = aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\
                   \n  y = 2\n",
        arguments: &["-C", "@DIR@"],
        extra: &[],
        stale_manifest: false,
        status: 1,
    },
    // A `$`-escaped line ending is eaten *after* a token, never in front of
    // one, so it cannot be part of an indent. A line holding nothing but
    // spaces and a continuation is an indent in its own right.
    BuildCase {
        name: "a line holding only an indent and a continuation is an indent",
        manifest: "x = 1\n  $\nbuild a: phony\n",
        arguments: &["-C", "@DIR@"],
        extra: &[],
        stale_manifest: false,
        status: 1,
    },
    BuildCase {
        // The continuation lands on a blank line, so reading it as part of the
        // whitespace leaves a manifest that parses. It is not one.
        name: "an indent continued onto a blank line is still an indent",
        manifest: "x = 1\n  $\n\ny = 2\nbuild a: phony\n",
        arguments: &["-C", "@DIR@"],
        extra: &[],
        stale_manifest: false,
        status: 1,
    },
    BuildCase {
        // Inside a block the continuation *is* eaten, after the indent, so the
        // binding's name is looked for on the line it reached.
        name: "a continuation inside a block looks for the binding on the line it reached",
        manifest: "rule cc\n  command = x\n  $\n  # comment\n  deps = gcc\nbuild a: cc\n",
        arguments: &["-C", "@DIR@"],
        extra: &[],
        stale_manifest: false,
        status: 1,
    },
    // A comment needs its terminating newline to be a comment at all: the
    // lexer's rule is `[ ]*"#"[^\000\n]*"\n"`. One that runs off the end of
    // the file matches nothing, and what is left is the spaces in front of it.
    BuildCase {
        name: "an indented comment at the end of the file is an indent",
        manifest: "rule cc\n  command = x\n  # comment",
        arguments: &["-C", "@DIR@"],
        extra: &[],
        stale_manifest: false,
        status: 1,
    },
    BuildCase {
        name: "a comment at the end of the file is the byte no token begins with",
        manifest: "x = 1\n# comment",
        arguments: &["-C", "@DIR@"],
        extra: &[],
        stale_manifest: false,
        status: 1,
    },
    BuildCase {
        // Half a line ending is not one, so the spaces in front of it are all
        // that matched: an indent, rather than the lexing error a carriage
        // return is on its own.
        name: "an indent in front of a lone carriage return is an indent",
        manifest: "x = 1\n  \ry = 2\n",
        arguments: &["-C", "@DIR@"],
        extra: &[],
        stale_manifest: false,
        status: 1,
    },
    // A check a block defers is located where the peek for an indent put the
    // scanner back, which is the start of the line that failed the peek —
    // spaces included.
    BuildCase {
        name: "a rule's missing command is located at the indented blank line that ended it",
        manifest: "rule cc\n  depfile = y\n  \nbuild a: cc\n",
        arguments: &["-C", "@DIR@"],
        extra: &[],
        stale_manifest: false,
        status: 1,
    },
    BuildCase {
        name: "a build statement's deferred check is located at the indented blank line",
        manifest: "rule cc\n  command = x\nbuild a: cc\n  pool = nope\n  \nbuild b: phony\n",
        arguments: &["-C", "@DIR@"],
        extra: &[],
        stale_manifest: false,
        status: 1,
    },
    BuildCase {
        name: "a rule's missing command is located at a comment that ends the file",
        manifest: "rule cc\n  depfile = y\n# c",
        arguments: &["-C", "@DIR@"],
        extra: &[],
        stale_manifest: false,
        status: 1,
    },
    BuildCase {
        name: "a manifest that cannot be included names the file",
        manifest: "include nosuchfile.ninja\n",
        arguments: &["-C", "@DIR@"],
        extra: &[],
        stale_manifest: false,
        status: 1,
    },
    BuildCase {
        name: "an invalid pool depth does not quote the value",
        manifest: "pool p\n  depth = x\n",
        arguments: &["-C", "@DIR@"],
        extra: &[],
        stale_manifest: false,
        status: 1,
    },
    // Everything a build statement's own bindings can change is checked once
    // that block is read, so Ninja's lexer has already moved past the statement
    // and the diagnostic names the *following* line with no caret under it.
    // Each of these is one of those, and the line number is the part that is
    // both easy to get wrong and impossible to notice.
    BuildCase {
        name: "an empty output path is reported after the statement's block",
        manifest: "rule r\n  command = c\nbuild $undefined: r source\nbuild b: r source\n",
        arguments: &["-C", "@DIR@"],
        extra: &[],
        stale_manifest: false,
        status: 1,
    },
    BuildCase {
        name: "an empty input path is reported after the statement's block",
        manifest: "rule r\n  command = c\nbuild a: r $undefined\nbuild b: r source\n",
        arguments: &["-C", "@DIR@"],
        extra: &[],
        stale_manifest: false,
        status: 1,
    },
    BuildCase {
        name: "a duplicate output is reported after the statement's block",
        manifest: "rule r\n  command = c\nbuild a: r source\nbuild a: r other\nbuild b: r source\n",
        arguments: &["-C", "@DIR@"],
        extra: &[],
        stale_manifest: false,
        status: 1,
    },
    BuildCase {
        // The statement's own bindings sit between it and the line the
        // diagnostic names, which is the shape a generated manifest has.
        name: "a duplicate output names the line after the bindings, not the statement",
        manifest: "rule r\n  command = c\nbuild a: r source\nbuild a: r other\n  x = 1\n  y = 2\n\
                   build b: r source\n",
        arguments: &["-C", "@DIR@"],
        extra: &[],
        stale_manifest: false,
        status: 1,
    },
    BuildCase {
        // Not "multiple rules generate": nothing else generates it, the
        // statement simply names it twice.
        name: "one statement naming an output twice is its own complaint",
        manifest: "rule r\n  command = c\nbuild a b | a: r source\nbuild c: r source\n",
        arguments: &["-C", "@DIR@"],
        extra: &[],
        stale_manifest: false,
        status: 1,
    },
    BuildCase {
        name: "an unknown pool is reported after the statement's block",
        manifest: "rule r\n  command = c\nbuild a: r source\n  pool = nosuchpool\n\
                   build b: r source\n",
        arguments: &["-C", "@DIR@"],
        extra: &[],
        stale_manifest: false,
        status: 1,
    },
    BuildCase {
        // The pool is resolved before any path is interned, so a statement
        // wrong in both ways is reported as the pool.
        name: "an unknown pool outranks a duplicate output in the same statement",
        manifest: "rule r\n  command = c\nbuild a: r source\nbuild a: r other\n\
                     pool = nosuchpool\nbuild b: r source\n",
        arguments: &["-C", "@DIR@"],
        extra: &[],
        stale_manifest: false,
        status: 1,
    },
    BuildCase {
        name: "a dyndep that is not an input is reported after the statement's block",
        manifest: "rule r\n  command = c\nbuild a: r source\n  dyndep = elsewhere\n\
                   build b: r source\n",
        arguments: &["-C", "@DIR@"],
        extra: &[],
        stale_manifest: false,
        status: 1,
    },
    BuildCase {
        // A statement's paths see its own bindings, so this one names `sub/a`
        // rather than failing on an empty path.
        name: "a statement's paths expand against its own bindings",
        manifest: "rule r\n  command = c\nbuild $dir/a: r source\n  dir = sub\n",
        arguments: &["-C", "@DIR@", "-t", "targets", "all"],
        extra: &[],
        stale_manifest: false,
        status: 0,
    },
    // A tab is ordinary text everywhere except where a statement belongs. It
    // therefore never indents, which is why a tab-indented rule body is not a
    // body at all and the rule is reported as having no command.
    BuildCase {
        name: "a tab-indented rule body leaves the rule without a command",
        manifest: "rule r\n\tcommand = c\nbuild a: r source\n",
        arguments: &["-C", "@DIR@"],
        extra: &[],
        stale_manifest: false,
        status: 1,
    },
    BuildCase {
        name: "a tab where a statement belongs is the lexing error",
        manifest: "rule r\n  command = c\n\tx = 1\nbuild a: r source\n",
        arguments: &["-C", "@DIR@"],
        extra: &[],
        stale_manifest: false,
        status: 1,
    },
    BuildCase {
        name: "a tab inside a path is part of the path",
        manifest: "rule r\n  command = c\nbuild a: r so\turce\n",
        arguments: &["-C", "@DIR@"],
        extra: &[],
        stale_manifest: false,
        status: 1,
    },
    BuildCase {
        name: "an unterminated last line is located like any other lexer failure",
        manifest: "rule r\n  command = c\nbuild a: r source",
        arguments: &["-C", "@DIR@"],
        extra: &[],
        stale_manifest: false,
        status: 1,
    },
    BuildCase {
        // A separator this position does not take is left where it is, so what
        // complains is the colon that was expected — naming the whole `||`.
        name: "a separator in the output position is what the colon says it found",
        manifest: "rule r\n  command = c\nbuild a || b: r source\n",
        arguments: &["-C", "@DIR@"],
        extra: &[],
        stale_manifest: false,
        status: 1,
    },
    BuildCase {
        name: "a name after a rule's own is what the newline says it found",
        manifest: "rule r x\n  command = c\n",
        arguments: &["-C", "@DIR@"],
        extra: &[],
        stale_manifest: false,
        status: 1,
    },
    BuildCase {
        // Nothing to include is not a syntax error: the empty name is opened
        // and reported as the missing file it is.
        name: "an include naming nothing fails on the empty name",
        manifest: "include\n",
        arguments: &["-C", "@DIR@"],
        extra: &[],
        stale_manifest: false,
        status: 1,
    },
    BuildCase {
        name: "a malformed escape puts the caret on the dollar",
        manifest: "x = a$!\n",
        arguments: &["-C", "@DIR@"],
        extra: &[],
        stale_manifest: false,
        status: 1,
    },
    BuildCase {
        // This one does not, because Ninja raises it without marking the token
        // first, so it still points at whatever was read before the value.
        name: "the newline escape's version complaint points at the assignment",
        manifest: "x = a$^b\n",
        arguments: &["-C", "@DIR@"],
        extra: &[],
        stale_manifest: false,
        status: 1,
    },
    BuildCase {
        // Half a line ending is not a line ending. This is what a CRLF
        // manifest looks like once something has eaten one of the newlines.
        name: "a carriage return with no newline behind it is a lexing error",
        manifest: "rule r\n  command = c\rbuild a: r source\n",
        arguments: &["-C", "@DIR@"],
        extra: &[],
        stale_manifest: false,
        status: 1,
    },
    BuildCase {
        name: "a token that belongs mid-statement is named where a statement was due",
        manifest: "= 1\n",
        arguments: &["-C", "@DIR@"],
        extra: &[],
        stale_manifest: false,
        status: 1,
    },
    BuildCase {
        // The warning is raised while reading a statement the parser then gets
        // past; the failure comes later. Both are printed, in that order.
        name: "a warning raised before a fatal manifest error survives it",
        manifest: "build a: phony a\nbuild b: nosuchrule\n",
        arguments: &["-C", "@DIR@"],
        extra: &[],
        stale_manifest: false,
        status: 1,
    },
    BuildCase {
        name: "a failing manifest regeneration is an error against the manifest",
        manifest: "rule regen\n  command = exit 4\n  generator = 1\n\
                   build build.ninja: regen conf\nrule cp\n  command = cp $in $out\n\
                   build a: cp source\ndefault a\n",
        arguments: &["-C", "@DIR@"],
        extra: &[("conf", "conf\n")],
        stale_manifest: true,
        status: 1,
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

/// Check the status the two tools agreed on against the one the case records.
///
/// Reached only once they already agree, so this says nothing about
/// compatibility and everything about whether the case is still the case it is
/// named after. `None` is a tool that died by a signal rather than exiting,
/// which no shape here is meant to produce.
fn recorded_status(name: &str, recorded: i32, observed: Option<i32>) -> Result<(), String> {
    if observed == Some(recorded) {
        return Ok(());
    }
    Err(format!(
        "case '{name}' left with {observed:?} where it is recorded as exiting {recorded}; \
         either the shape stopped exercising what it is named after, or Ninja's status moved"
    ))
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
        return recorded_status(case.name, case.status, ninja_result.0);
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

// [spec:ronin:req:compat.upstream-conformance]
// [spec:ronin:req:compat.process-integration]
fn compare_invocation_case(
    ninja: &Path,
    ronin: &Path,
    case: &InvocationCase,
) -> Result<(), String> {
    let mut rendered = Vec::new();
    for tool in [ninja, ronin] {
        let directory = TemporaryDirectory::new("invocation").map_err(|error| error.to_string())?;
        if let Some(manifest) = case.manifest {
            fs::write(directory.0.join("build.ninja"), manifest)
                .map_err(|error| error.to_string())?;
        }
        let arguments = case
            .arguments
            .iter()
            .map(|argument| argument.replace("@DIR@", &directory.0.display().to_string()))
            .collect::<Vec<_>>();
        let mut command = Command::new(tool);
        command
            .args(&arguments)
            .current_dir(&directory.0)
            .env_remove("MAKEFLAGS");
        if let Some(makeflags) = case.makeflags {
            command.env(
                "MAKEFLAGS",
                makeflags.replace("@DIR@", &directory.0.display().to_string()),
            );
        }
        let output = command
            .output()
            .map_err(|error| format!("running {}: {error}", tool.display()))?;
        rendered.push((
            output.status.code(),
            normalize(&output.stdout, &directory.0),
            normalize(&output.stderr, &directory.0),
        ));
    }
    let (ninja_result, ronin_result) = (&rendered[0], &rendered[1]);
    if ninja_result == ronin_result {
        return recorded_status(case.name, case.status, ninja_result.0);
    }
    Err(format!(
        "invocation case '{}' differs\n\
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
    for case in INVOCATION_CASES {
        compare_invocation_case(&ninja, &config.ronin, case)?;
    }
    println!("differential: 10 CLI/tool cases matched");
    println!(
        "build outcome: {} failure and directory cases matched, each on its recorded exit status",
        BUILD_CASES.len()
    );
    println!(
        "invocation boundary: {} source and inherited-runtime cases matched, \
         each on its recorded exit status",
        INVOCATION_CASES.len()
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
    use super::{
        normalize, parse_log_records, recorded_status, LogRecord, BUILD_CASES, INVOCATION_CASES,
    };
    use std::collections::BTreeSet;
    use std::path::Path;

    // [spec:ronin:req:product.build-outcome/test]
    #[test]
    fn recorded_status_accepts_the_measured_code() {
        assert!(recorded_status("a case", 130, Some(130)).is_ok());
        assert!(recorded_status("a case", 0, Some(0)).is_ok());
    }

    /// The regression this check exists for. Three cases named after signals
    /// were written `kill -KILL $$`, which a manifest escapes to one `$`, so
    /// they ran `kill -KILL $` — a usage error exiting 2 that both tools
    /// agreed on. Comparing the tools to each other could never see it;
    /// a recorded 137 does.
    // [spec:ronin:req:product.build-outcome/test]
    #[test]
    fn recorded_status_rejects_a_drifted_shape() {
        let error = recorded_status("a killed command", 137, Some(2))
            .expect_err("a status that is not the recorded one is a failure");
        assert!(error.contains("a killed command"), "{error}");
        assert!(error.contains("137"), "{error}");
        assert!(error.contains("Some(2)"), "{error}");
    }

    /// No shape here asks either tool to die by a signal, so a status of
    /// `None` is a tool that crashed rather than a case that passed.
    // [spec:ronin:req:product.build-outcome/test]
    #[test]
    fn recorded_status_rejects_a_signalled_tool() {
        assert!(recorded_status("a case", 1, None).is_err());
    }

    /// Every case is looked up by name in its failure message, and the
    /// recorded statuses were measured one name at a time.
    // [spec:ronin:req:compat.upstream-conformance/test]
    #[test]
    fn case_names_are_unique() {
        let mut names = BTreeSet::new();
        for name in BUILD_CASES
            .iter()
            .map(|case| case.name)
            .chain(INVOCATION_CASES.iter().map(|case| case.name))
        {
            assert!(names.insert(name), "duplicate case name: {name}");
        }
    }

    /// A manifest escape for a literal `$` is `$$`, so a command that means
    /// to reach the shell's `$$` has to spell it `$$$$`. Writing `$$` there
    /// is how three signal cases came to prove nothing.
    // [spec:ronin:req:compat.upstream-conformance/test]
    #[test]
    fn signal_cases_escape_the_shell_pid() {
        for case in BUILD_CASES {
            if !case.manifest.contains("kill -") {
                continue;
            }
            assert!(
                case.manifest.contains("$$$$"),
                "case '{}' signals a pid the shell never expands",
                case.name
            );
        }
    }

    /// The two `/dev/full` cases are one pair: the short payload has to stay
    /// inside the stream's buffer so its failure is reported against the
    /// close, and the long one has to clear the block a device reports so its
    /// failure is reported against the write. The long payload is spelled as
    /// nested bindings to keep the manifest readable, which is exactly how it
    /// could quietly shrink back into being a second copy of the short case.
    // [spec:ronin:req:compat.upstream-conformance/test]
    #[test]
    fn full_device_cases_straddle_a_block() {
        let case = |name| {
            BUILD_CASES
                .iter()
                .find(|case| case.name == name)
                .unwrap_or_else(|| panic!("case '{name}' is in the table"))
        };
        let long = case("a long response file on a full device is reported against the write");
        // `sixtyfour` is one 64-byte literal, and the two bindings above the
        // payload multiply their references to it.
        let expanded = 64
            * long.manifest.matches("$sixtyfour").count()
            * long.manifest.matches("$kibibyte").count();
        assert!(expanded >= 8192, "the payload expands to {expanded} bytes");

        let short = case("a short response file on a full device is reported against the close");
        assert!(
            short.manifest.contains("rspfile_content = arguments"),
            "the short payload has to stay inside the buffer"
        );
        for case in [short, long] {
            assert!(
                case.manifest.contains("rspfile = /dev/full"),
                "case '{}' no longer writes to a full device",
                case.name
            );
        }
    }

    // [spec:ronin:req:compat.process-integration/test]
    #[test]
    fn normalizes_invocation_output() {
        let directory = Path::new("/tmp/ronin-invocation-case");
        assert_eq!(
            normalize(
                b"ninja: Jobserver mode detected: fifo:/tmp/ronin-invocation-case/outer\n",
                directory,
            ),
            b"@TOOL@: Jobserver mode detected: fifo:@DIR@/outer\n"
        );
        assert_eq!(
            normalize(b"ronin: error: unavailable\n", directory),
            b"@TOOL@: error: unavailable\n"
        );
    }

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

    /// `normalize` removes the product name and the temporary directory and
    /// nothing else, so a case whose output carries the tool's *version*
    /// can never match: Ninja calls itself 1.14.0.git and Ronin does not.
    /// That is a property of the manifest, so it is checked here rather
    /// than discovered as a mismatch that looks like a real difference.
    // [spec:ronin:req:compat.upstream-conformance/test]
    #[test]
    fn build_cases_never_compare_the_tools_version() {
        for case in BUILD_CASES {
            assert!(
                !case.manifest.contains("ninja_required_version"),
                "build case '{}' would compare the tool's version",
                case.name
            );
        }
    }
}
