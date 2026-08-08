//! Differential discovery harness for Make evaluation against GNU Make 4.4.1.
//!
//! Every case in the vendored kati corpus is run twice from the same absolute
//! directory — once under the pinned GNU Make, once under the Make front end —
//! and the exit status, stdout, stderr and created-file set are compared. The
//! corpus is not expected to be clean: kati is a partial clone aimed at one
//! build system, so the differences are classified in
//! `tests/make_corpus_inventory.tsv` rather than filtered away, and a
//! difference that has no entry there fails the inventory check. Exact runner
//! status and stdout/stderr are retained here to discover evaluator gaps; they
//! are not Ronin's Make build-intent conformance gate.
//!
//! See `[dec:ronin:make-compiles-to-ninja]` and
//! `[dec:ronin:ninja-compatibility-oracle]`.

#![deny(missing_docs, unreachable_pub)]

use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

/// The oracle this corpus is pinned to. Moving it reruns the corpus and
/// reclassifies whatever moves; it is not a default to fall back from.
const ORACLE_VERSION: &str = "GNU Make 4.4.1";

const INVENTORY_PATH: &str = "tests/make_corpus_inventory.tsv";
const INVENTORY: &str = include_str!("../tests/make_corpus_inventory.tsv");

/// Long enough that nothing in the corpus reaches it, short enough that a hang
/// is reported rather than waited on.
const CASE_TIMEOUT: Duration = Duration::from_mins(1);

/// The classes a difference is allowed to land in.
const CLASSES: [&str; 4] = ["defect", "recorded", "extension", "artefact"];

/// What the inventory holds, once parsed: the declared families, and what is
/// recorded about each differing case.
type Classification = (BTreeMap<String, Family>, BTreeMap<String, Recorded>);

/// The Make front end under test.
///
/// This is the whole of what retargeting the harness touches. Pointing
/// `binary` at Ronin and putting Make-mode selection in `arguments` moves the
/// corpus onto Ronin without altering enumeration, normalisation or
/// classification, none of which know which front end produced the output.
struct FrontEnd {
    binary: PathBuf,
    arguments: Vec<String>,
}

struct Config {
    make: PathBuf,
    front_end: FrontEnd,
    corpus: PathBuf,
    work: PathBuf,
    update: bool,
}

impl Default for Config {
    fn default() -> Self {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        Self {
            make: PathBuf::from("make"),
            front_end: FrontEnd {
                binary: root.join("target/release/rkati"),
                arguments: Vec::new(),
            },
            corpus: root.join("kati/testcase"),
            work: env::temp_dir().join("ronin-make-conformance"),
            update: false,
        }
    }
}

fn value(name: &str, arguments: &mut std::iter::Skip<env::Args>) -> Result<String, String> {
    arguments
        .next()
        .ok_or_else(|| format!("missing value for {name}"))
}

impl Config {
    fn parse() -> Result<Self, String> {
        let mut config = Self::default();
        let mut arguments = env::args().skip(1);
        while let Some(argument) = arguments.next() {
            match argument.as_str() {
                "--make" => config.make = PathBuf::from(value("--make", &mut arguments)?),
                "--front-end" => {
                    config.front_end.binary = PathBuf::from(value("--front-end", &mut arguments)?);
                }
                "--front-end-arg" => {
                    let extra = value("--front-end-arg", &mut arguments)?;
                    config.front_end.arguments.push(extra);
                }
                "--corpus" => config.corpus = PathBuf::from(value("--corpus", &mut arguments)?),
                "--work" => config.work = PathBuf::from(value("--work", &mut arguments)?),
                "--update" => config.update = true,
                "--help" | "-h" => {
                    println!(
                        "usage: make_conformance [--make FILE] [--front-end FILE] \
                         [--front-end-arg ARG]... [--corpus DIR] [--work DIR] [--update]"
                    );
                    std::process::exit(0);
                }
                _ => return Err(format!("unknown argument '{argument}'")),
            }
        }
        Ok(config)
    }
}

/// One corpus run: a `.mk` file built with one of its `testN` targets, or a
/// `.sh` script handed the tool under test.
struct Case {
    id: String,
    file: String,
    target: String,
    script: bool,
    /// Whether the case decides what to expect by looking for `kati` in the
    /// name of the tool it was handed. See [`self_detecting`].
    self_detecting: bool,
}

/// What one tool did with one case, already normalised, plus a digest of what
/// it did before normalisation, so the denominator can be stated without one.
struct Observation {
    status: String,
    stdout: String,
    stderr: String,
    files: String,
    verbatim: u64,
}

/// A case whose two observations disagree.
struct Divergence {
    id: String,
    kinds: Vec<&'static str>,
    digest: u64,
    oracle: Observation,
    actual: Observation,
}

/// The result of one case: whether the two tools agreed byte for byte, and
/// what remained once the harness's normalisations were applied.
enum Outcome {
    Identical,
    NormalisedAway,
    Differs(Box<Divergence>),
}

/// One family of difference, with the reason it is that family.
struct Family {
    description: String,
    cases: usize,
}

/// What the inventory records about one differing case.
struct Recorded {
    class: String,
    families: Vec<String>,
    digest: u64,
}

/// Refuse to run against whatever Make happens to be installed.
///
/// A gate that silently compares against a different oracle, or against no
/// oracle at all, is worse than one that does not run.
fn verify_oracle(make: &Path) -> Result<(), String> {
    let output = Command::new(make)
        .arg("--version")
        .output()
        .map_err(|error| format!("{} is not runnable: {error}", make.display()))?;
    if !output.status.success() {
        return Err(format!(
            "{} --version failed with {:?}",
            make.display(),
            output.status.code()
        ));
    }
    let banner = String::from_utf8_lossy(&output.stdout);
    let first = banner.lines().next().unwrap_or_default().trim();
    if first == ORACLE_VERSION {
        Ok(())
    } else {
        Err(format!(
            "the semantics oracle is pinned at '{ORACLE_VERSION}' but {} reports '{first}'.\n\
             Install the pinned version or pass --make; the corpus is not run against \
             an unpinned Make.",
            make.display()
        ))
    }
}

/// The `testN` targets a `.mk` file declares, in the vendored harness's terms.
///
/// `run_test.go` takes the first `test` followed by digits at the start of any
/// line; reproducing that exactly is what keeps the run count comparable with
/// the harness being replaced.
fn declared_targets(content: &str) -> Vec<String> {
    let mut found = BTreeSet::new();
    for line in content.lines() {
        if let Some(rest) = line.strip_prefix("test") {
            let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
            found.insert(format!("test{}", &rest[..digits]));
        }
    }
    if found.is_empty() {
        vec![String::new()]
    } else {
        found.into_iter().collect()
    }
}

/// Whether a script chooses its expectations by looking for `kati` in the name
/// of the tool it was handed.
///
/// Half the corpus's shell cases do this, and the branch they take when the
/// name does not match prints a canned expectation *instead of* running the
/// tool — the corpus's way of writing GNU Make's side of a comparison for a
/// feature GNU Make does not have. Hand such a case a front end with any other
/// name and both runs print the same canned text, so the case agrees with
/// itself and proves nothing. The harness has to refuse those rather than
/// count them.
fn self_detecting(script: &str) -> bool {
    script
        .lines()
        .any(|line| line.contains("${mk}") && line.contains("grep") && line.contains("kati"))
}

/// Whether the tool under test is the one the self-detecting scripts look for.
///
/// The same test the scripts run, against the same string: everything the
/// script is handed as `$@`.
fn names_kati(front_end: &FrontEnd) -> bool {
    front_end.binary.to_string_lossy().contains("kati")
        || front_end
            .arguments
            .iter()
            .any(|argument| argument.contains("kati"))
}

fn enumerate_cases(corpus: &Path) -> Result<Vec<Case>, String> {
    let mut names = fs::read_dir(corpus)
        .map_err(|error| format!("reading {}: {error}", corpus.display()))?
        .map(|entry| {
            entry
                .map(|entry| entry.file_name().to_string_lossy().into_owned())
                .map_err(|error| error.to_string())
        })
        .collect::<Result<Vec<_>, _>>()?;
    names.sort();
    let mut cases = Vec::new();
    for name in names {
        if name.starts_with('.') {
            continue;
        }
        let extension = Path::new(&name)
            .extension()
            .map(std::ffi::OsStr::as_encoded_bytes);
        if extension == Some(b"mk") {
            let content = fs::read_to_string(corpus.join(&name))
                .map_err(|error| format!("reading {name}: {error}"))?;
            for target in declared_targets(&content) {
                let label = if target.is_empty() {
                    "default"
                } else {
                    &target
                };
                cases.push(Case {
                    id: format!("{name}#{label}"),
                    file: name.clone(),
                    target,
                    script: false,
                    self_detecting: false,
                });
            }
        } else if extension == Some(b"sh") && !name.starts_with("ninja_") {
            // A `ninja_` script drives kati's Ninja emitter and then Ninja.
            // GNU Make cannot answer what that should produce, so those cases
            // belong to the manifest oracle, not this one.
            let script = fs::read_to_string(corpus.join(&name))
                .map_err(|error| format!("reading {name}: {error}"))?;
            cases.push(Case {
                id: format!("{name}#script"),
                file: name.clone(),
                target: String::new(),
                script: true,
                self_detecting: self_detecting(&script),
            });
        }
    }
    Ok(cases)
}

/// A directory name a Makefile can survive being run in.
///
/// `#` starts a comment wherever a Makefile expands a path, so a case id used
/// verbatim would make `$(dir $(CURDIR))` differ between the two tools for a
/// reason that is entirely about the harness.
fn case_directory_name(id: &str) -> String {
    id.replace('#', "__")
}

/// Run one tool on one case in `directory`, which is already empty.
fn observe(
    tool: &Path,
    extra: &[String],
    case: &Case,
    directory: &Path,
    corpus: &Path,
) -> Result<Observation, String> {
    let mut command = Command::new(if case.script { Path::new("bash") } else { tool });
    if case.script {
        // A script runs the tool itself, so what it is handed has to be the
        // whole command: the front end and whatever selects its mode.
        command.arg(corpus.join(&case.file)).arg(tool).args(extra);
    } else {
        fs::copy(corpus.join(&case.file), directory.join("Makefile"))
            .map_err(|error| format!("staging {}: {error}", case.file))?;
        let _ = std::os::unix::fs::symlink(corpus.join("submake"), directory.join("submake"));
        command.args(extra);
        // The one recursive case: with the recipe echoed, both tools would
        // print their own $(MAKE), which is a product-identity difference
        // rather than an evaluation one. The vendored harness silenced it the
        // same way.
        if case.file.starts_with("submake_") {
            command.arg("-s");
        }
        if !case.target.is_empty() {
            command.arg(&case.target);
        }
    }
    command.arg("SHELL=/bin/bash");

    let stdout_path = directory.join(".harness-stdout");
    let stderr_path = directory.join(".harness-stderr");
    let stdout = fs::File::create(&stdout_path).map_err(|error| error.to_string())?;
    let stderr = fs::File::create(&stderr_path).map_err(|error| error.to_string())?;
    command
        .current_dir(directory)
        // An inherited jobserver makes both tools believe they are a submake.
        .env_remove("MAKEFLAGS")
        .env_remove("MAKELEVEL")
        // GNU Make quotes with the locale's directional marks. Pinning the
        // locale keeps a quoting difference a real difference.
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr));

    let status = wait_for(&mut command)?;
    let stdout = fs::read(&stdout_path).map_err(|error| error.to_string())?;
    let stderr = fs::read(&stderr_path).map_err(|error| error.to_string())?;
    let files = directory_listing(directory)?;
    let verbatim = digest(&[
        &status,
        &String::from_utf8_lossy(&stdout),
        &String::from_utf8_lossy(&stderr),
        &files,
    ]);
    Ok(Observation {
        status,
        stdout: normalize(&stdout, directory),
        stderr: normalize(&stderr, directory),
        files,
        verbatim,
    })
}

/// Spawn and reap, reporting a hang instead of waiting on one.
///
/// Output is already redirected to files, so there is no pipe to drain and no
/// deadlock to avoid while polling.
fn wait_for(command: &mut Command) -> Result<String, String> {
    use std::os::unix::process::ExitStatusExt;
    let mut child = command.spawn().map_err(|error| error.to_string())?;
    let started = Instant::now();
    loop {
        match child.try_wait().map_err(|error| error.to_string())? {
            Some(status) => {
                return Ok(status.code().map_or_else(
                    || format!("signal {}", status.signal().unwrap_or(0)),
                    |code| code.to_string(),
                ))
            }
            None if started.elapsed() > CASE_TIMEOUT => {
                let _ = child.kill();
                let _ = child.wait();
                return Ok("timeout".to_owned());
            }
            None => std::thread::sleep(Duration::from_millis(5)),
        }
    }
}

/// The names the Makefile caused to exist, excluding each tool's own artefacts.
///
/// The exclusions are the vendored harness's, for the same reason: a stamp
/// file, an emitted manifest and its helper scripts are things the tool wrote
/// about itself, not products of the Makefile under test.
fn directory_listing(directory: &Path) -> Result<String, String> {
    let mut names = fs::read_dir(directory)
        .map_err(|error| error.to_string())?
        .map(|entry| {
            entry
                .map(|entry| entry.file_name().to_string_lossy().into_owned())
                .map_err(|error| error.to_string())
        })
        .collect::<Result<Vec<_>, _>>()?;
    names.retain(|name| {
        !(name.starts_with('.')
            || name.starts_with("kati")
            || Path::new(name)
                .extension()
                .map(std::ffi::OsStr::as_encoded_bytes)
                == Some(b"json")
            || matches!(
                name.as_str(),
                "Makefile" | "build.ninja" | "env.sh" | "ninja.sh" | "gmon.out" | "submake"
            ))
    });
    names.sort();
    Ok(names.join("\n"))
}

/// The two normalisations this harness applies, and nothing else.
///
/// The run directory is chosen by the harness, so it cannot be part of the
/// contract. The tool's own name at the head of a diagnostic cannot match
/// either: `[spec:ronin:req:product.make-identity]` says the front end
/// identifies itself as Ronin. Both are replaced rather than deleted, so
/// whether a prefix is present at all stays a difference — kati omits it on
/// most diagnostics, and that is a real one.
fn normalize(output: &[u8], directory: &Path) -> String {
    let text = String::from_utf8_lossy(output).replace(&directory.display().to_string(), "@DIR@");
    let mut normalized = String::with_capacity(text.len());
    for line in text.split_inclusive('\n') {
        match tool_identity(line) {
            Some(rest) => {
                normalized.push_str("@TOOL@");
                normalized.push_str(rest);
            }
            None => normalized.push_str(line),
        }
    }
    normalized
}

/// The tail of a line that opens with a tool naming itself, if it does.
fn tool_identity(line: &str) -> Option<&str> {
    for name in ["gmake", "ckati", "rkati", "make", "kati", "ronin"] {
        let Some(rest) = line.strip_prefix(name) else {
            continue;
        };
        if rest.starts_with(": ") {
            return Some(rest);
        }
        if let Some(level) = rest.strip_prefix('[') {
            if let Some(close) = level.find("]: ") {
                if close > 0 && level[..close].bytes().all(|b| b.is_ascii_digit()) {
                    return Some(rest);
                }
            }
        }
    }
    None
}

/// FNV-1a over the whole divergence, so a recorded difference that changes
/// shape is a finding rather than a silent pass.
fn digest(divergence: &[&str]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for part in divergence {
        for byte in part.as_bytes().iter().chain(std::iter::once(&0x1f)) {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x0100_0000_01b3);
        }
    }
    hash
}

fn compare(id: &str, oracle: Observation, actual: Observation) -> Outcome {
    let verbatim = oracle.verbatim == actual.verbatim;
    let mut kinds = Vec::new();
    if oracle.status != actual.status {
        kinds.push("exit");
    }
    if oracle.stdout != actual.stdout {
        kinds.push("stdout");
    }
    if oracle.stderr != actual.stderr {
        kinds.push("stderr");
    }
    if oracle.files != actual.files {
        kinds.push("files");
    }
    if kinds.is_empty() {
        return if verbatim {
            Outcome::Identical
        } else {
            Outcome::NormalisedAway
        };
    }
    let digest = digest(&[
        &oracle.status,
        &oracle.stdout,
        &oracle.stderr,
        &oracle.files,
        &actual.status,
        &actual.stdout,
        &actual.stderr,
        &actual.files,
    ]);
    Outcome::Differs(Box::new(Divergence {
        id: id.to_owned(),
        kinds,
        digest,
        oracle,
        actual,
    }))
}

fn parse_inventory(text: &str) -> Result<Classification, String> {
    let mut families = BTreeMap::new();
    let mut cases = BTreeMap::new();
    for (index, line) in text.lines().enumerate() {
        let line = line.trim_end();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let fields = line.split('\t').collect::<Vec<_>>();
        let number = index + 1;
        match fields.as_slice() {
            ["family", name, description] => {
                if description.is_empty() {
                    return Err(format!(
                        "inventory line {number}: family '{name}' has no reason"
                    ));
                }
                let family = Family {
                    description: (*description).to_owned(),
                    cases: 0,
                };
                if families.insert((*name).to_owned(), family).is_some() {
                    return Err(format!(
                        "inventory line {number}: duplicate family '{name}'"
                    ));
                }
            }
            ["case", id, class, names, digest] => {
                if !CLASSES.contains(class) {
                    return Err(format!("inventory line {number}: unknown class '{class}'"));
                }
                let digest = u64::from_str_radix(digest, 16)
                    .map_err(|_| format!("inventory line {number}: bad digest"))?;
                let recorded = Recorded {
                    class: (*class).to_owned(),
                    families: names.split('+').map(str::to_owned).collect(),
                    digest,
                };
                if cases.insert((*id).to_owned(), recorded).is_some() {
                    return Err(format!("inventory line {number}: duplicate case '{id}'"));
                }
            }
            _ => return Err(format!("inventory line {number}: unrecognised record")),
        }
    }
    for (id, recorded) in &cases {
        for name in &recorded.families {
            if !families.contains_key(name) {
                return Err(format!("case '{id}' names undeclared family '{name}'"));
            }
        }
    }
    Ok((families, cases))
}

/// Run every case, both tools, in the same directory for each.
fn run_corpus(config: &Config, cases: &[Case]) -> Result<(usize, Vec<Divergence>), String> {
    let workers = std::thread::available_parallelism().map_or(4, std::num::NonZero::get);
    let next = AtomicUsize::new(0);
    let verbatim = AtomicUsize::new(0);
    let results = std::sync::Mutex::new(Vec::new());
    let failure = std::sync::Mutex::new(None::<String>);
    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| loop {
                let index = next.fetch_add(1, Ordering::Relaxed);
                let Some(case) = cases.get(index) else {
                    return;
                };
                match run_case(config, case) {
                    Ok(Outcome::Identical) => {
                        verbatim.fetch_add(1, Ordering::Relaxed);
                    }
                    Ok(Outcome::NormalisedAway) => {}
                    Ok(Outcome::Differs(divergence)) => results.lock().unwrap().push(*divergence),
                    Err(error) => {
                        let mut slot = failure.lock().unwrap();
                        if slot.is_none() {
                            *slot = Some(format!("{}: {error}", case.id));
                        }
                        return;
                    }
                }
            });
        }
    });
    if let Some(error) = failure.into_inner().unwrap() {
        return Err(error);
    }
    let mut results = results.into_inner().unwrap();
    results.sort_by(|left, right| left.id.cmp(&right.id));
    Ok((verbatim.into_inner(), results))
}

fn run_case(config: &Config, case: &Case) -> Result<Outcome, String> {
    let directory = config
        .work
        .join("out")
        .join(case_directory_name(&case.id))
        .join("run");
    let none: Vec<String> = Vec::new();
    let mut observations = Vec::new();
    for (tool, extra) in [
        (&config.make, &none),
        (&config.front_end.binary, &config.front_end.arguments),
    ] {
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir_all(&directory).map_err(|error| error.to_string())?;
        observations.push(observe(tool, extra, case, &directory, &config.corpus)?);
    }
    let _ = fs::remove_dir_all(config.work.join("out").join(case_directory_name(&case.id)));
    let actual = observations.pop().expect("both tools observed");
    let oracle = observations.pop().expect("both tools observed");
    Ok(compare(&case.id, oracle, actual))
}

/// The layout the corpus expects around itself.
///
/// One case copies a helper out of `../../../testcase`, which is where the
/// vendored harness's `out/<case>/<tool>/` working directory put it. Without
/// the same depth the case degrades into a `cp` failure that says nothing
/// about Make.
fn prepare_work(config: &Config) -> Result<(), String> {
    let _ = fs::remove_dir_all(config.work.join("out"));
    fs::create_dir_all(&config.work).map_err(|error| error.to_string())?;
    let link = config.work.join("testcase");
    if !link.exists() {
        std::os::unix::fs::symlink(&config.corpus, &link)
            .map_err(|error| format!("linking the corpus into the work tree: {error}"))?;
    }
    Ok(())
}

fn write_inventory(
    path: &Path,
    families: &BTreeMap<String, Family>,
    known: &BTreeMap<String, Recorded>,
    divergences: &[Divergence],
) -> Result<(), String> {
    let mut text = String::from(INVENTORY_HEADER);
    for (name, family) in families {
        writeln!(text, "family\t{name}\t{}", family.description)
            .expect("a String never fails to format");
    }
    text.push('\n');
    for divergence in divergences {
        let recorded = known.get(&divergence.id);
        let class = recorded.map_or("defect", |recorded| recorded.class.as_str());
        let names = recorded.map_or_else(
            || "unclassified".to_owned(),
            |recorded| recorded.families.join("+"),
        );
        writeln!(
            text,
            "case\t{}\t{class}\t{names}\t{:016x}",
            divergence.id, divergence.digest
        )
        .expect("a String never fails to format");
    }
    fs::write(path, text).map_err(|error| format!("writing {}: {error}", path.display()))
}

const INVENTORY_HEADER: &str = "\
# Classified differences between GNU Make 4.4.1 and the Make front end over the
# vendored kati corpus.  Regenerate the case rows with
# `scripts/check-make-conformance.sh --update`; the class and family columns are
# a judgement and are edited by hand.
#
# class    defect     a real difference from GNU Make that Make mode must fix
#          recorded   the corpus itself annotates the case as not matching Make
#                     (`# TODO` at the head of the file, as run_test.go reads it)
#          extension  the case exercises a kati-only feature, so GNU Make cannot
#                     be the oracle; the corpus fakes Make's side of it
#          artefact   the difference is caused by the corpus or the harness
#
# digest   FNV-1a over both observations.  A recorded difference that changes
#          shape fails the run and has to be reclassified.
#
# family   record: name and the reason that family exists.
# case     record: id, class, `+`-joined families, digest.

";

/// Say which tool the numbers below are about, and which cases could not be
/// asked about it at all.
///
/// Both lines are provenance rather than decoration: two runs against
/// different front ends have different denominators, and a total quoted
/// without them cannot be compared with another.
fn announce(front_end: &FrontEnd, refused: &[Case]) {
    let mut spelled = front_end.binary.display().to_string();
    for argument in &front_end.arguments {
        spelled.push(' ');
        spelled.push_str(argument);
    }
    println!("front end:   {spelled}");
    if refused.is_empty() {
        return;
    }
    println!(
        "refused:     {} self-detecting cases. Each greps the name of the tool it was\n\
         \x20            handed for `kati`, and prints a canned expectation instead of\n\
         \x20            running anything when it does not match, so against this front\n\
         \x20            end both sides would print the same text and agree about nothing:",
        refused.len()
    );
    for case in refused {
        println!("  {}", case.id);
    }
}

fn report(
    total: usize,
    verbatim: usize,
    divergences: &[Divergence],
    families: &BTreeMap<String, Family>,
    known: &BTreeMap<String, Recorded>,
) -> Result<(), String> {
    println!("corpus:      {total} runs against {ORACLE_VERSION}");
    println!(
        "raw:         {verbatim} identical byte for byte, {} differing",
        total - verbatim
    );
    println!(
        "normalised:  {} identical, {} differing",
        total - divergences.len(),
        divergences.len()
    );

    let mut unknown = Vec::new();
    let mut changed = Vec::new();
    let mut by_class: BTreeMap<&str, usize> = BTreeMap::new();
    for divergence in divergences {
        match known.get(&divergence.id) {
            None => unknown.push(divergence),
            Some(recorded) if recorded.digest != divergence.digest => changed.push(divergence),
            Some(recorded) => *by_class.entry(recorded.class.as_str()).or_default() += 1,
        }
    }
    let seen = divergences
        .iter()
        .map(|divergence| divergence.id.as_str())
        .collect::<BTreeSet<_>>();
    let stale = known
        .keys()
        .filter(|id| !seen.contains(id.as_str()))
        .collect::<Vec<_>>();

    println!("classified:");
    for class in CLASSES {
        println!(
            "  {class:<10} {}",
            by_class.get(class).copied().unwrap_or(0)
        );
    }
    println!("families:");
    for (name, family) in families {
        println!("  {:<28} {:>4}  {}", name, family.cases, family.description);
    }

    let mut problems = Vec::new();
    for divergence in &unknown {
        problems.push(format!(
            "unclassified difference in {} [{}]\n{}",
            divergence.id,
            divergence.kinds.join("+"),
            render(divergence)
        ));
    }
    for divergence in &changed {
        problems.push(format!(
            "recorded difference in {} changed shape [{}]\n{}",
            divergence.id,
            divergence.kinds.join("+"),
            render(divergence)
        ));
    }
    for id in stale {
        problems.push(format!(
            "{id} is recorded as differing but now matches; remove its inventory row"
        ));
    }
    if problems.is_empty() {
        Ok(())
    } else {
        Err(problems.join("\n\n"))
    }
}

fn render(divergence: &Divergence) -> String {
    let mut text = String::new();
    if divergence.oracle.status != divergence.actual.status {
        writeln!(
            text,
            "  exit: make={} front-end={}",
            divergence.oracle.status, divergence.actual.status
        )
        .expect("a String never fails to format");
    }
    for (name, left, right) in [
        (
            "stdout",
            &divergence.oracle.stdout,
            &divergence.actual.stdout,
        ),
        (
            "stderr",
            &divergence.oracle.stderr,
            &divergence.actual.stderr,
        ),
        ("files", &divergence.oracle.files, &divergence.actual.files),
    ] {
        if left != right {
            writeln!(
                text,
                "  make {name}:\n{}\n  front-end {name}:\n{}",
                indent(left),
                indent(right)
            )
            .expect("a String never fails to format");
        }
    }
    text
}

fn indent(text: &str) -> String {
    if text.is_empty() {
        return "    <empty>".to_owned();
    }
    text.lines()
        .map(|line| format!("    {line}"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn run(config: &Config) -> Result<(), String> {
    verify_oracle(&config.make)?;
    if !config.front_end.binary.is_file() {
        return Err(format!(
            "the front end {} does not exist; build it first",
            config.front_end.binary.display()
        ));
    }
    let (mut families, known) = parse_inventory(INVENTORY)?;
    let cases = enumerate_cases(&config.corpus)?;
    if cases.is_empty() {
        return Err(format!(
            "no cases under {}; the corpus is the point of this gate",
            config.corpus.display()
        ));
    }
    let exercisable = names_kati(&config.front_end);
    let (refused, cases): (Vec<Case>, Vec<Case>) = cases
        .into_iter()
        .partition(|case| case.self_detecting && !exercisable);
    announce(&config.front_end, &refused);
    prepare_work(config)?;
    let (verbatim, divergences) = run_corpus(config, &cases)?;
    if config.update {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(INVENTORY_PATH);
        write_inventory(&path, &families, &known, &divergences)?;
        println!("wrote {} rows to {}", divergences.len(), path.display());
        return Ok(());
    }
    for family in families.values_mut() {
        family.cases = 0;
    }
    for divergence in &divergences {
        if let Some(recorded) = known.get(&divergence.id) {
            for name in &recorded.families {
                if let Some(family) = families.get_mut(name) {
                    family.cases += 1;
                }
            }
        }
    }
    report(cases.len(), verbatim, &divergences, &families, &known)
}

fn main() {
    let result = Config::parse().and_then(|config| run(&config));
    if let Err(error) = result {
        eprintln!("make-conformance: {error}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::{
        case_directory_name, declared_targets, names_kati, normalize, parse_inventory,
        self_detecting, tool_identity, FrontEnd,
    };
    use std::path::{Path, PathBuf};

    #[test]
    fn a_script_that_greps_its_tools_name_is_recognised() {
        assert!(self_detecting(
            "if echo \"${mk}\" | grep -qv \"kati\"; then\n  echo canned\nfi\n"
        ));
        assert!(self_detecting("if echo \"${mk}\" | grep -q kati; then\n"));
        // Naming kati is not the same as branching on the tool's own name.
        assert!(!self_detecting("# kati writes a stamp here\n${mk} 2>&1\n"));
        assert!(!self_detecting("grep -q kati out.txt\n"));
    }

    #[test]
    fn a_front_end_is_measured_against_the_name_the_script_will_see() {
        let front_end = |binary: &str, arguments: &[&str]| FrontEnd {
            binary: PathBuf::from(binary),
            arguments: arguments
                .iter()
                .map(|argument| (*argument).to_owned())
                .collect(),
        };
        assert!(names_kati(&front_end("target/release/rkati", &[])));
        // Ronin's Make mode is reached by a make-named path, so what the script
        // sees is that path and the scripts' own kati test does not match it.
        assert!(!names_kati(&front_end("target/release/make", &[])));
    }

    #[test]
    fn a_file_with_no_test_targets_yields_one_default_run() {
        assert_eq!(declared_targets("all:\n\techo hi\n"), vec![String::new()]);
    }

    #[test]
    fn declared_targets_match_the_vendored_harness_shape() {
        let content = "test2: test1\ntest1:\n\techo\ntest10:\n# test3 in a comment\n";
        assert_eq!(declared_targets(content), ["test1", "test10", "test2"]);
    }

    #[test]
    fn the_tool_name_is_replaced_and_not_deleted() {
        let directory = Path::new("/tmp/x");
        assert_eq!(
            normalize(b"make: *** No targets.  Stop.\n", directory),
            "@TOOL@: *** No targets.  Stop.\n"
        );
        assert_eq!(
            normalize(b"*** No targets.\n", directory),
            "*** No targets.\n"
        );
    }

    #[test]
    fn a_tool_name_inside_a_message_is_left_alone() {
        assert_eq!(tool_identity("cp: cannot stat 'make: x'"), None);
        assert_eq!(tool_identity("Makefile:2: *** foo."), None);
        assert_eq!(tool_identity("makefile: not a prefix"), None);
        assert_eq!(tool_identity("make[x]: not a level"), None);
        assert_eq!(
            tool_identity("make[1]: Entering directory"),
            Some("[1]: Entering directory")
        );
    }

    #[test]
    fn the_run_directory_cannot_contain_a_comment_character() {
        assert_eq!(case_directory_name("a.mk#test2"), "a.mk__test2");
    }

    #[test]
    fn a_case_naming_an_undeclared_family_is_rejected() {
        let text = "family\ta\treason\ncase\tx.mk#test\tdefect\tb\t0\n";
        assert!(parse_inventory(text).is_err());
        let text = "family\ta\treason\ncase\tx.mk#test\tdefect\ta\t0\n";
        assert!(parse_inventory(text).is_ok());
    }

    #[test]
    fn a_case_in_no_recognised_class_is_rejected() {
        let text = "family\ta\treason\ncase\tx.mk#test\twontfix\ta\t0\n";
        assert!(parse_inventory(text).is_err());
    }
}
