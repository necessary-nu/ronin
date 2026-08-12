//! Which Make made the recording, and whether the Make in front of us is it.
//!
//! `GNU Make 4.4.1` is printed by the source the Free Software Foundation
//! released, by Debian's `make-dfsg 4.4.1-2`, by Fedora's build and by Arch's,
//! and those four programs do not answer every question alike: Debian's asks
//! `ar` for a non-deterministic archive under `.POSIX:` where the released
//! source does not, Fedora's names a different host, Arch's carries Guile. A
//! recording that names only the version cannot say which of them made it, and
//! a re-record on the wrong host would overwrite it without a word.
//!
//! So the corpus keeps the oracle's answers to the questions builds of 4.4.1
//! are known to differ on, and recording refuses when the Make in front of it
//! answers differently. [spec:ronin:req:make.oracle-provenance]

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Names the Make the recorder runs. It takes any command, a wrapper script
/// included, so a build that only exists inside a container can be measured.
pub const ORACLE_VARIABLE: &str = "MAKE_PORT_ORACLE";

/// Moves the pin: recording rewrites the record from what this Make answers,
/// and the value is the prose that will name the build. Deliberate, and it
/// arrives as a diff to review rather than as a silent overwrite.
pub const MOVE_VARIABLE: &str = "MAKE_PORT_ORACLE_MOVE";

/// The host's Make, which is a distribution's build and so is not the oracle.
/// Left as the default because a recorder who has not chosen should be told
/// what is wrong with the obvious choice rather than handed a working default.
const DEFAULT_ORACLE: &str = "/usr/bin/make";

/// The record, beside the cases it describes. `collect` walks directories, so
/// a file here is not mistaken for one.
const RECORD: &str = "oracle.provenance";

/// Variables whose value is a property of the invocation or the environment
/// rather than of the build. Recording them would make the record say where it
/// ran instead of what ran.
const NOT_THE_BUILD: [&str; 12] = [
    ".DEFAULT_GOAL",
    ".FEATURES",
    ".INCLUDE_DIRS",
    ".VARIABLES",
    "CURDIR",
    "GNUMAKEFLAGS",
    "MAKEFILE_LIST",
    "MAKEFLAGS",
    "MAKELEVEL",
    "MAKE_COMMAND",
    "MFLAGS",
    "SHELL",
];

/// What a Make answers about itself.
pub struct Provenance {
    /// Prose naming the source the build came from. Nothing can check it, so
    /// it travels with the record rather than being probed; what is checked is
    /// everything below it.
    pub build: String,
    pub version: String,
    pub host: String,
    pub features: BTreeSet<String>,
    /// Every variable the build installs at `default` origin.
    pub defaults: BTreeMap<String, String>,
    /// Only where `.POSIX:` gives a different answer from the above, which is
    /// the whole of what declaring it changes about the built-in variables.
    pub posix: BTreeMap<String, String>,
}

/// The Make to run, from the environment or the host's.
pub fn selected() -> PathBuf {
    std::env::var_os(ORACLE_VARIABLE).map_or_else(|| PathBuf::from(DEFAULT_ORACLE), PathBuf::from)
}

/// Ask a Make the questions the record is made of.
pub fn probe(oracle: &Path) -> Provenance {
    let plain = ask(oracle, false);
    let under_posix = ask(oracle, true);
    let posix = under_posix
        .answers
        .into_iter()
        .filter(|(name, value)| plain.answers.get(name) != Some(value))
        .collect();

    Provenance {
        build: String::new(),
        version: version(oracle),
        host: plain.host,
        features: plain.features,
        defaults: plain.answers,
        posix,
    }
}

/// The recorded identity, or the reason there is none to read.
pub fn read(corpus: &Path) -> Result<Provenance, String> {
    let path = corpus.join(RECORD);
    let text = fs::read_to_string(&path)
        .map_err(|error| format!("no oracle record at {} ({error})", path.display()))?;
    Ok(parse(&text))
}

pub fn write(corpus: &Path, provenance: &Provenance) {
    let path = corpus.join(RECORD);
    fs::write(&path, render(provenance))
        .unwrap_or_else(|error| panic!("writing {}: {error}", path.display()));
}

/// Every way the Make in front of us is not the one the record names.
// [spec:ronin:req:make.oracle-provenance]
pub fn differences(recorded: &Provenance, observed: &Provenance) -> Vec<String> {
    let mut found = Vec::new();
    for (what, was, is) in [
        ("version", &recorded.version, &observed.version),
        ("host", &recorded.host, &observed.host),
    ] {
        if was != is {
            found.push(format!(
                "{what} is {is:?}, and the record was made by {was:?}"
            ));
        }
    }
    for feature in recorded.features.difference(&observed.features) {
        found.push(format!("feature {feature} is missing"));
    }
    for feature in observed.features.difference(&recorded.features) {
        found.push(format!(
            "feature {feature} is offered and the record has none"
        ));
    }
    compare("", &recorded.defaults, &observed.defaults, &mut found);
    compare(
        "under .POSIX: ",
        &recorded.posix,
        &observed.posix,
        &mut found,
    );
    found
}

fn compare(
    context: &str,
    recorded: &BTreeMap<String, String>,
    observed: &BTreeMap<String, String>,
    into: &mut Vec<String>,
) {
    for (name, was) in recorded {
        match observed.get(name) {
            Some(is) if is == was => {}
            Some(is) => into.push(format!(
                "{context}{name} is {is:?}, and the record has {was:?}"
            )),
            None => into.push(format!(
                "{context}{name} is not defined, and the record has {was:?}"
            )),
        }
    }
    for (name, is) in observed {
        if !recorded.contains_key(name) {
            into.push(format!(
                "{context}{name} is {is:?}, and the record has no such default"
            ));
        }
    }
}

/// What one run of the probe answered.
struct Answers {
    host: String,
    features: BTreeSet<String>,
    answers: BTreeMap<String, String>,
}

/// Ask through a makefile rather than through the command line, because the
/// built-in variables are installed for a recipe to read and `$(file ...)`
/// writes them without a shell between us and the bytes.
fn ask(oracle: &Path, posix: bool) -> Answers {
    let scratch = std::env::temp_dir().join("ronin-make-oracle-probe");
    let _ = fs::remove_dir_all(&scratch);
    fs::create_dir_all(&scratch).expect("a directory to probe the oracle in");

    let hidden = NOT_THE_BUILD.join(" ");
    let makefile = format!(
        "{}answer:\n\t@$(file >answers)\
         $(foreach v,$(filter-out {hidden},$(sort $(.VARIABLES))),\
         $(if $(filter default,$(origin $v)),$(file >>answers,default $v $(value $v))))\
         $(foreach f,$(sort $(.FEATURES)),$(file >>answers,feature $f))\
         $(file >>answers,host $(MAKE_HOST)):\n",
        if posix { ".POSIX:\n" } else { "" }
    );
    fs::write(scratch.join("Makefile"), makefile).expect("writing the probe");

    let output = Command::new(oracle)
        .current_dir(&scratch)
        .env("LC_ALL", "C")
        .env_remove("MAKEFLAGS")
        .env_remove("MAKELEVEL")
        .output()
        .unwrap_or_else(|error| panic!("running {}: {error}", oracle.display()));
    assert!(
        output.status.success(),
        "{} could not answer the provenance probe:\n{}",
        oracle.display(),
        String::from_utf8_lossy(&output.stderr)
    );

    let text = fs::read_to_string(scratch.join("answers")).expect("the probe's answers");
    let mut collected = Answers {
        host: String::new(),
        features: BTreeSet::new(),
        answers: BTreeMap::new(),
    };
    for line in text.lines() {
        record_answer(line, &mut collected);
    }
    collected
}

fn record_answer(line: &str, into: &mut Answers) {
    let (kind, rest) = split(line);
    match kind {
        "host" => into.host = rest.to_owned(),
        "feature" => {
            into.features.insert(rest.to_owned());
        }
        "default" => {
            let (name, value) = split(rest);
            into.answers.insert(name.to_owned(), value.to_owned());
        }
        _ => {}
    }
}

fn version(oracle: &Path) -> String {
    let reported = Command::new(oracle)
        .arg("--version")
        .output()
        .unwrap_or_else(|error| panic!("asking {} its version: {error}", oracle.display()));
    String::from_utf8_lossy(&reported.stdout)
        .lines()
        .next()
        .unwrap_or_default()
        .trim()
        .to_owned()
}

pub fn render(provenance: &Provenance) -> String {
    let mut text = String::new();
    let _ = writeln!(text, "build {}", provenance.build);
    let _ = writeln!(text, "version {}", provenance.version);
    let _ = writeln!(text, "host {}", provenance.host);
    for feature in &provenance.features {
        let _ = writeln!(text, "feature {feature}");
    }
    for (name, value) in &provenance.defaults {
        let _ = writeln!(text, "default {name} {value}");
    }
    for (name, value) in &provenance.posix {
        let _ = writeln!(text, "posix {name} {value}");
    }
    text
}

pub fn parse(text: &str) -> Provenance {
    let mut provenance = Provenance {
        build: String::new(),
        version: String::new(),
        host: String::new(),
        features: BTreeSet::new(),
        defaults: BTreeMap::new(),
        posix: BTreeMap::new(),
    };
    for line in text.lines() {
        let (kind, rest) = split(line);
        let (name, value) = split(rest);
        match kind {
            "build" => provenance.build = rest.to_owned(),
            "version" => provenance.version = rest.to_owned(),
            "host" => provenance.host = rest.to_owned(),
            "feature" => {
                provenance.features.insert(rest.to_owned());
            }
            "default" => {
                provenance
                    .defaults
                    .insert(name.to_owned(), value.to_owned());
            }
            "posix" => {
                provenance.posix.insert(name.to_owned(), value.to_owned());
            }
            other => panic!("{RECORD}: unknown record `{other}`"),
        }
    }
    provenance
}

/// The first word and everything after it, with an empty value where a
/// variable's is empty and the trailing space was trimmed by an editor.
fn split(line: &str) -> (&str, &str) {
    line.split_once(' ').unwrap_or((line, ""))
}

/// The Debian departure the corpus was recorded through before the oracle
/// became the released source: `-rvU` under `.POSIX:` where GNU says `-rv`.
/// Two builds that agree on their version, their host and every other default
/// still have to be told apart by it.
// [spec:ronin:req:make.oracle-provenance/test]
#[test]
fn patched_posix_defaults_fail_the_record() {
    let released = parse("version GNU Make 4.4.1\nhost x86_64-pc-linux-gnu\ndefault ARFLAGS -rv\n");
    let patched = parse(
        "version GNU Make 4.4.1\nhost x86_64-pc-linux-gnu\ndefault ARFLAGS -rv\nposix ARFLAGS -rvU\n",
    );

    assert!(differences(&released, &released).is_empty());
    assert_eq!(
        differences(&released, &patched),
        vec![r#"under .POSIX: ARFLAGS is "-rvU", and the record has no such default"#]
    );
}

// [spec:ronin:req:make.oracle-provenance/test]
#[test]
fn the_record_survives_a_round_trip() {
    let text = "build upstream 4.4.1, from the release tarball\nversion GNU Make 4.4.1\n\
                host x86_64-pc-linux-gnu\nfeature guile\nfeature load\ndefault AR ar\n\
                default COFLAGS \ndefault RM rm -f\nposix CC c99\n";

    assert_eq!(render(&parse(text)), text);
}
