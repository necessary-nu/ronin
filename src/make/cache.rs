//! Where an invocation's compiled artifact is kept between runs.
//!
//! NOT IN THE TREE. `[spec:ronin:req:make.state-outside-the-tree]` says Make
//! mode leaves nothing behind in a directory it built in, because that
//! directory is one the build did not create and must leave as it found it.
//! Ninja users expect a `build.ninja` beside their sources; Make users expect
//! nothing at all, and this front end is a Make front end.
//!
//! So the artifact goes under `$XDG_CACHE_HOME/ronin/`, or `~/.cache/ronin/`
//! where the variable is unset, in a directory named for what the invocation
//! is. A run that can be told neither has no cache and reads its makefiles,
//! which is what every run did before this existed.
//!
//! WHAT THE NAME COVERS is what makes two runs the same compilation.
//! `make ARCH=x86 O=out` does not compile to the graph `make` does, so the
//! command-line variables are in it; nor does `make clean` compile to the
//! graph `make all` does, so the goals are. The build directory is
//! canonicalised first, because two spellings of one directory are one
//! compilation and a symlinked path that resolves elsewhere is not. And the
//! format version is in it, so an artifact this build cannot read is an
//! artifact it never opens rather than one it misreads.

use crate::htab::rapidhashv1;
use std::path::{Path, PathBuf};

/// What this build writes and will read back.
///
/// Bumped whenever the bytes change meaning. A run whose cache directory was
/// written by another version simply has no cache: the name is different, so
/// the file is not there, and nothing has to detect a format it cannot parse.
pub(crate) const FORMAT_VERSION: u32 = 2;

/// The directory this invocation's artifact belongs in.
///
/// `None` where the host will say neither `XDG_CACHE_HOME` nor `HOME`, which
/// is a run with nowhere to put one. Nothing is created here — a reader wants
/// to know the name whether or not anything has been written yet.
pub(crate) fn directory_for(
    build_directory: &Path,
    variables: &[impl AsRef<[u8]>],
    goals: &[impl AsRef<[u8]>],
) -> Option<PathBuf> {
    Some(root()?.join(name_for(build_directory, variables, goals)))
}

/// `$XDG_CACHE_HOME/ronin`, or `~/.cache/ronin`.
///
/// An `XDG_CACHE_HOME` that is not absolute is ignored rather than resolved
/// against the working directory: the specification says a relative value is
/// invalid, and resolving it here would put the cache under whichever
/// directory `-C` had reached.
fn root() -> Option<PathBuf> {
    root_of(
        std::env::var_os("XDG_CACHE_HOME").as_deref(),
        std::env::var_os("HOME").as_deref(),
    )
}

/// The cache root the two names settle on, as a function of them.
///
/// Separated from the host so the rule can be stated once and checked without
/// a test writing to the process environment, which every other thread in a
/// test binary is reading at the same time.
fn root_of(
    cache_home: Option<&std::ffi::OsStr>,
    home: Option<&std::ffi::OsStr>,
) -> Option<PathBuf> {
    let cache = match cache_home {
        Some(cache) if Path::new(cache).is_absolute() => PathBuf::from(cache),
        _ => PathBuf::from(home?).join(".cache"),
    };
    Some(cache.join("ronin"))
}

/// The one directory name that stands for this invocation.
///
/// Hex rather than anything readable, because the parts it is made of include
/// a path and arbitrary `VAR=value` bytes: a name derived from those would be
/// a name the filesystem may refuse, and one long enough to refuse on its own.
fn name_for(
    build_directory: &Path,
    variables: &[impl AsRef<[u8]>],
    goals: &[impl AsRef<[u8]>],
) -> String {
    let canonical = std::fs::canonicalize(build_directory)
        .unwrap_or_else(|_| build_directory.to_path_buf())
        .into_os_string();
    let mut key = Vec::new();
    key.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
    // Length-prefixed rather than separated, so no run of bytes inside a
    // variable's value can be read as the end of it. `-C a` with a goal `b`
    // and `-C a b` with no goal are different invocations and must not share
    // a name.
    push_field(&mut key, canonical.as_encoded_bytes());
    push_field(&mut key, &(variables.len() as u64).to_le_bytes());
    for variable in variables {
        push_field(&mut key, variable.as_ref());
    }
    push_field(&mut key, &(goals.len() as u64).to_le_bytes());
    for goal in goals {
        push_field(&mut key, goal.as_ref());
    }
    format!("{:016x}", rapidhashv1(key.as_slice()))
}

fn push_field(key: &mut Vec<u8>, field: &[u8]) {
    key.extend_from_slice(&(field.len() as u64).to_le_bytes());
    key.extend_from_slice(field);
}

/// The two files an invocation leaves behind, inside its cache directory:
/// what the composition read, and what it composed.
const RECORD: &str = "record";
const GRAPH: &str = "graph";

pub(crate) fn snapshot(graph: &crate::frontend::BuildGraph) -> Option<Vec<u8>> {
    let mut bytes = Vec::new();
    crate::graph::persist::write(graph, &mut bytes).ok()?;
    Some(bytes)
}

/// What the reader checks before it believes a byte of the rest.
///
/// The version is in the directory name too, so a mismatch here is a file
/// written by something else entirely rather than by an older Ronin.
const MAGIC: &[u8] = b"ronin-make-record\x00";

/// Leave a record of everything this composition read, for the next
/// invocation to check the ground against.
///
/// Written where the read has settled and before the build runs, because it
/// describes the READ. A recipe that goes on to rewrite one of those makefiles
/// moves its digest away from what is recorded here and the next run misses,
/// which is the answer that costs time rather than correctness.
///
/// Every failure is silent and leaves no record. There is nothing a user can
/// do about a cache directory that will not be written, and nothing is lost by
/// not writing it: the next run reads its makefiles, which is what every run
/// did before this existed.
pub(crate) fn record(
    build_directory: &Path,
    invocation: &crate::make::cli::Invocation,
    settled: &crate::make::Groundwork,
    graph: Option<Vec<u8>>,
) {
    let Some(directory) =
        directory_for(build_directory, invocation.variables(), invocation.goals())
    else {
        return;
    };
    write(&directory, settled, graph);
}

fn write(directory: &Path, settled: &crate::make::Groundwork, graph: Option<Vec<u8>>) {
    // A composition that answered one question two ways describes no single
    // ground, so there is nothing a later run could check it against. Leaving
    // the old record in place would be worse than leaving none: it describes a
    // composition that is no longer the one this tree produces. A graph that
    // could not be written down leaves no record either, because a record on
    // its own describes reads whose graph is not there to load.
    let Some(graph) = graph.filter(|_| settled.record.divergence().is_none()) else {
        let _ = std::fs::remove_file(directory.join(RECORD));
        let _ = std::fs::remove_file(directory.join(GRAPH));
        return;
    };
    if std::fs::create_dir_all(directory).is_err() {
        return;
    }
    // Two files, written one after the other, can be left by an interrupted
    // run as a new graph beside an old record or the other way round. The
    // record names the graph it describes by digest, so a reader that finds
    // the two apart refuses them rather than checking one against the other.
    let digest = rapidhashv1(graph.as_slice());
    let written =
        crate::persistence::atomic_rewrite(&directory.join(GRAPH), |out| out.write_all(&graph));
    if written.is_err() {
        let _ = std::fs::remove_file(directory.join(RECORD));
        return;
    }
    let _ = crate::persistence::atomic_rewrite(&directory.join(RECORD), |out| {
        encode(out, settled, digest)
    });
}

fn encode(
    out: &mut dyn std::io::Write,
    settled: &crate::make::Groundwork,
    graph: u64,
) -> std::io::Result<()> {
    out.write_all(MAGIC)?;
    out.write_all(&FORMAT_VERSION.to_le_bytes())?;
    out.write_all(&graph.to_le_bytes())?;
    let record = &settled.record;
    put_usize(out, record.units())?;
    for (unit, asked) in record.entries() {
        put_bytes(out, unit)?;
        put_bytes(out, asked.directory().as_os_str().as_encoded_bytes())?;

        put_usize(out, asked.len())?;
        for ((question, text), answered) in asked.answers() {
            out.write_all(&[question_tag(*question)])?;
            put_bytes(out, text)?;
            put_bytes(out, &answered.answer)?;
            match answered.status {
                None => out.write_all(&[0])?,
                Some(status) => {
                    out.write_all(&[1])?;
                    out.write_all(&status.to_le_bytes())?;
                }
            }
        }

        put_usize(out, asked.environment().count())?;
        for (name, value) in asked.environment() {
            put_bytes(out, name)?;
            match value {
                None => out.write_all(&[0])?,
                Some(value) => {
                    out.write_all(&[1])?;
                    put_bytes(out, value)?;
                }
            }
        }

        // The makefiles this unit read, each with a digest of the bytes it was
        // given. A digest rather than a timestamp because a timestamp answers
        // a different question: a file rewritten to the same contents is the
        // same read, and one restored from a backup with an old date is not.
        // `rapidhashv1` is fixed-seed, which is what lets one run compare
        // against another's.
        let sources = settled
            .read_units
            .get(unit)
            .map_or(&[][..], |journal| &journal.sources);
        put_usize(out, sources.len())?;
        for (name, contents) in sources {
            put_bytes(out, name.as_encoded_bytes())?;
            out.write_all(&rapidhashv1(contents.as_ref()).to_le_bytes())?;
        }
    }
    Ok(())
}

/// One byte standing for a kind of question.
///
/// Written out rather than derived from the enum's own ordering, so that
/// adding a variant to [`kati::session::GroundQuestion`] cannot silently
/// change what an already-written record means.
const fn question_tag(question: kati::session::GroundQuestion) -> u8 {
    use kati::session::GroundQuestion as Q;
    match question {
        Q::Shell => 1,
        Q::Wildcard => 2,
        Q::RealPath => 3,
        Q::FileRead => 4,
        Q::Glob => 5,
        Q::Include => 6,
    }
}

fn put_usize(out: &mut dyn std::io::Write, value: usize) -> std::io::Result<()> {
    out.write_all(&(value as u64).to_le_bytes())
}

fn put_bytes(out: &mut dyn std::io::Write, value: &[u8]) -> std::io::Result<()> {
    put_usize(out, value.len())?;
    out.write_all(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn name(directory: &str, variables: &[&str], goals: &[&str]) -> String {
        let variables: Vec<&[u8]> = variables.iter().map(|v| v.as_bytes()).collect();
        let goals: Vec<&[u8]> = goals.iter().map(|g| g.as_bytes()).collect();
        name_for(Path::new(directory), &variables, &goals)
    }

    #[test]
    fn one_invocation_names_one_directory() {
        assert_eq!(
            name("/tmp", &["A=1"], &["all"]),
            name("/tmp", &["A=1"], &["all"])
        );
    }

    #[test]
    fn a_command_line_variable_names_another() {
        assert_ne!(name("/tmp", &["ARCH=x86"], &[]), name("/tmp", &[], &[]));
    }

    #[test]
    fn a_different_goal_names_another() {
        assert_ne!(name("/tmp", &[], &["all"]), name("/tmp", &[], &["clean"]));
    }

    #[test]
    fn a_different_build_directory_names_another() {
        assert_ne!(name("/tmp", &[], &[]), name("/var", &[], &[]));
    }

    #[test]
    fn a_variable_is_not_a_goal_spelled_alike() {
        assert_ne!(name("/tmp", &["all"], &[]), name("/tmp", &[], &["all"]));
    }

    #[test]
    fn two_fields_cannot_run_into_one() {
        assert_ne!(
            name("/tmp", &["A=1", "B=2"], &[]),
            name("/tmp", &["A=1B=2"], &[])
        );
    }

    #[test]
    fn variable_order_is_part_of_the_name() {
        assert_ne!(
            name("/tmp", &["A=1", "A=2"], &[]),
            name("/tmp", &["A=2", "A=1"], &[])
        );
    }

    fn root_from(cache_home: Option<&str>, home: Option<&str>) -> Option<PathBuf> {
        root_of(
            cache_home.map(std::ffi::OsStr::new),
            home.map(std::ffi::OsStr::new),
        )
    }

    #[test]
    fn a_relative_cache_home_is_not_used() {
        assert_eq!(
            root_from(Some("relative/path"), Some("/home/someone")),
            Some(PathBuf::from("/home/someone/.cache/ronin")),
            "a relative XDG_CACHE_HOME is invalid, and resolving it would follow -C around"
        );
    }

    #[test]
    fn an_absolute_cache_home_is_used() {
        assert_eq!(
            root_from(Some("/elsewhere"), Some("/home/someone")),
            Some(PathBuf::from("/elsewhere/ronin"))
        );
    }

    #[test]
    fn a_host_that_says_neither_has_no_cache() {
        assert_eq!(root_from(None, None), None);
    }

    #[test]
    fn a_record_names_the_graph_beside_it() {
        let directory = tempfile::tempdir().expect("a scratch directory");
        let settled = crate::make::Groundwork::default();
        write(directory.path(), &settled, Some(b"the graph".to_vec()));
        let graph = std::fs::read(directory.path().join(GRAPH)).expect("the graph is written");
        assert_eq!(graph, b"the graph");
        let record = std::fs::read(directory.path().join(RECORD)).expect("the record is written");
        let (magic, rest) = record.split_at(MAGIC.len());
        assert_eq!(magic, MAGIC);
        let (version, rest) = rest.split_at(4);
        assert_eq!(version, FORMAT_VERSION.to_le_bytes());
        let digest = u64::from_le_bytes(rest[..8].try_into().expect("eight bytes"));
        assert_eq!(digest, rapidhashv1(graph.as_slice()));
    }

    #[test]
    fn a_composition_with_no_graph_leaves_neither_file() {
        let directory = tempfile::tempdir().expect("a scratch directory");
        let settled = crate::make::Groundwork::default();
        write(directory.path(), &settled, Some(b"the graph".to_vec()));
        write(directory.path(), &settled, None);
        assert!(!directory.path().join(GRAPH).exists());
        assert!(!directory.path().join(RECORD).exists());
    }
}
