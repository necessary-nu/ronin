//! Version-7 Ninja build log reader and writer.

use crate::error::{PersistenceError, PersistenceOperation};
use crate::graph::{EdgeId, Graph, NodeId};
use crate::htab::RapidHashMap;
use crate::runtime::{CommandHash, FileTime, RuntimeState};
use crate::util::{BStr, BString, ByteSlice};
use std::collections::HashSet;
use std::fs::{File, OpenOptions};
use std::io::{self, BufWriter, Read, Write};
use std::path::{Path, PathBuf};

const MAX_LOG_LINE: usize = 256 << 10;
const LOG_HEADER: &[u8] = b"# ninja log v7";

/// One recorded output, keyed by its path in [`BuildLog::entries`].
///
/// The path lives in the map key alone; duplicating it here cost a second
/// allocation and copy for every line of a log that can hold one entry per
/// build output.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct LogEntry {
    pub(crate) start_time: i32,
    pub(crate) end_time: i32,
    pub(crate) mtime: i64,
    pub(crate) command_hash: u64,
}

pub(crate) struct BuildLog {
    writer: Option<BufWriter<File>>,
    path: PathBuf,
    pub(crate) entries: RapidHashMap<BString, LogEntry>,
}

impl BuildLog {
    // [spec:ronin:def:log.loginit-fn]
    // [spec:ronin:sem:log.loginit-fn]
    // [spec:ronin:req:compat.persistent-state]
    pub(crate) fn open(builddir: Option<&Path>) -> io::Result<Self> {
        let path = builddir.map_or_else(
            || PathBuf::from(".ninja_log"),
            |directory| directory.join(".ninja_log"),
        );
        let read_file = OpenOptions::new().read(true).open(&path);
        let mut valid = false;
        let mut entries = RapidHashMap::default();
        match read_file {
            Ok(mut read_file) => {
                // Read once and borrow each line out of the buffer. The
                // incremental reader this replaces copied every line into a
                // scratch `Vec` before parsing it, which is pure overhead once
                // the whole file is in hand — and the file is bounded by the
                // build's own output count.
                let mut content = Vec::new();
                read_file.read_to_end(&mut content)?;
                let mut lines = content.split(|byte| *byte == b'\n').peekable();
                // The header is compared verbatim, so a CRLF log is rejected
                // here exactly as it was before.
                valid = lines.next() == Some(LOG_HEADER);
                if valid {
                    while let Some(line) = lines.next() {
                        // The final element is either the empty slice after a
                        // trailing newline or a line no newline terminated —
                        // a log truncated by a crash. Both were skipped
                        // before and are skipped now.
                        if lines.peek().is_none() {
                            break;
                        }
                        let line = line.strip_suffix(b"\r").unwrap_or(line);
                        if let Some((output, entry)) = parse_entry(line) {
                            entries.insert(output, entry);
                        }
                    }
                }
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        let writer_file = if valid {
            open_log(&path)?
        } else {
            crate::persistence::atomic_rewrite(&path, |writer| {
                writer.write_all(LOG_HEADER)?;
                writer.write_all(b"\n")
            })?
        };
        Ok(Self {
            writer: Some(BufWriter::new(writer_file)),
            path,
            entries,
        })
    }

    // [spec:ronin:def:log.logclose-fn]
    // [spec:ronin:sem:log.logclose-fn]
    pub(crate) fn finish(mut self) -> io::Result<()> {
        self.writer
            .take()
            .map_or(Ok(()), |mut writer| writer.flush())
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    pub(crate) fn hydrate_runtime(
        &self,
        graph: &Graph,
        runtime: &mut RuntimeState,
        nodes: impl Iterator<Item = NodeId>,
    ) {
        for node in nodes {
            let graph_node = graph.node(node);
            if graph_node.gen.is_none() {
                continue;
            }
            if let Some(entry) = self.entries.get(graph.node_path(node)) {
                let state = runtime.node_mut(node);
                state.set_log_mtime(FileTime::observed(entry.mtime));
                state.set_logged_command_hash(CommandHash::from_raw(entry.command_hash));
            }
        }
    }
}

// [spec:ronin:def:log.nextfield-fn]
// [spec:ronin:sem:log.nextfield-fn]
fn nextfield<'a>(line: &mut &'a [u8]) -> Option<&'a [u8]> {
    if line.is_empty() {
        return None;
    }
    let index = line
        .iter()
        .position(|byte| matches!(byte, b'\t' | b'\n'))
        .unwrap_or(line.len());
    let (field, rest) = line.split_at(index);
    *line = rest.get(1..).unwrap_or(rest);
    Some(field)
}

fn open_log(path: &Path) -> io::Result<File> {
    OpenOptions::new()
        .create(true)
        .read(true)
        .append(true)
        .open(path)
}

/// Read a signed decimal field straight from its bytes.
///
/// A log field is ASCII by construction, so validating it as UTF-8 only to
/// walk the same bytes again is wasted work. This accepts exactly what
/// `str::parse::<i64>` accepts, so a corrupt field still yields `None` and the
/// line is still skipped — `byte_parsers_match_the_str_implementations`
/// holds that equivalence.
fn parse_decimal(field: &[u8]) -> Option<i64> {
    let (negative, digits) = match field.split_first() {
        Some((b'-', rest)) => (true, rest),
        Some((b'+', rest)) => (false, rest),
        _ => (false, field),
    };
    if digits.is_empty() {
        return None;
    }
    let mut value: i64 = 0;
    for &byte in digits {
        let digit = byte.wrapping_sub(b'0');
        if digit > 9 {
            return None;
        }
        let digit = i64::from(digit);
        value = value.checked_mul(10)?;
        // Accumulate downwards for negatives so `i64::MIN` round-trips.
        value = if negative {
            value.checked_sub(digit)?
        } else {
            value.checked_add(digit)?
        };
    }
    Some(value)
}

/// Read a hexadecimal field straight from its bytes.
///
/// Matches `u64::from_str_radix(.., 16)`, which accepts a leading `+` and
/// rejects a leading `-` because the target is unsigned.
fn parse_hex(field: &[u8]) -> Option<u64> {
    let digits = match field.split_first() {
        Some((b'+', rest)) => rest,
        _ => field,
    };
    if digits.is_empty() {
        return None;
    }
    let mut value: u64 = 0;
    for &byte in digits {
        let digit = match byte {
            b'0'..=b'9' => byte - b'0',
            b'a'..=b'f' => byte - b'a' + 10,
            b'A'..=b'F' => byte - b'A' + 10,
            _ => return None,
        };
        value = value.checked_mul(16)?.checked_add(u64::from(digit))?;
    }
    Some(value)
}

fn parse_entry(line: &[u8]) -> Option<(BString, LogEntry)> {
    if line.len() > MAX_LOG_LINE || line == LOG_HEADER {
        return None;
    }
    let mut rest = line;
    let start_time = i32::try_from(parse_decimal(nextfield(&mut rest)?)?).ok()?;
    let end_time = i32::try_from(parse_decimal(nextfield(&mut rest)?)?).ok()?;
    let mtime = parse_decimal(nextfield(&mut rest)?)?;
    let output = BString::from(nextfield(&mut rest)?);
    let command_hash = parse_hex(nextfield(&mut rest)?)?;
    Some((
        output,
        LogEntry {
            start_time,
            end_time,
            mtime,
            command_hash,
        },
    ))
}

fn write_entry(writer: &mut dyn Write, output: &BStr, entry: &LogEntry) -> io::Result<()> {
    write!(
        writer,
        "{}\t{}\t{}\t",
        entry.start_time, entry.end_time, entry.mtime
    )?;
    writer.write_all(output.as_bytes())?;
    writeln!(writer, "\t{:x}", entry.command_hash)
}

fn rewrite_inner(
    log: &mut BuildLog,
    entries: RapidHashMap<BString, LogEntry>,
    #[cfg(test)] fault: Option<crate::persistence::RewriteStage>,
) -> io::Result<()> {
    log.writer.as_mut().expect("open build log").flush()?;
    let write_contents = |writer: &mut dyn Write| {
        writer.write_all(LOG_HEADER)?;
        writer.write_all(b"\n")?;
        for (output, entry) in &entries {
            write_entry(writer, output.as_bstr(), entry)?;
        }
        Ok(())
    };
    #[cfg(test)]
    let replacement = if let Some(stage) = fault {
        crate::persistence::atomic_rewrite_with_fault(&log.path, stage, write_contents)?
    } else {
        crate::persistence::atomic_rewrite(&log.path, write_contents)?
    };
    #[cfg(not(test))]
    let replacement = crate::persistence::atomic_rewrite(&log.path, write_contents)?;
    log.writer = Some(BufWriter::new(replacement));
    log.entries = entries;
    Ok(())
}

fn rewrite(log: &mut BuildLog, entries: RapidHashMap<BString, LogEntry>) -> io::Result<()> {
    #[cfg(test)]
    {
        rewrite_inner(log, entries, None)
    }
    #[cfg(not(test))]
    {
        rewrite_inner(log, entries)
    }
}

#[cfg(test)]
fn rewrite_with_fault(
    log: &mut BuildLog,
    entries: RapidHashMap<BString, LogEntry>,
    stage: crate::persistence::RewriteStage,
) -> io::Result<()> {
    rewrite_inner(log, entries, Some(stage))
}

fn record_entries(log: &mut BuildLog, entries: Vec<(BString, LogEntry)>) -> io::Result<()> {
    let mut encoded = Vec::new();
    for (output, entry) in &entries {
        write_entry(&mut encoded, output.as_bstr(), entry)?;
    }
    let writer = log.writer.as_mut().expect("open build log");
    writer.write_all(&encoded)?;
    writer.flush()?;
    for (output, entry) in entries {
        log.entries.insert(output, entry);
    }
    Ok(())
}

#[cfg(test)]
fn record_entry(log: &mut BuildLog, output: BString, entry: LogEntry) -> io::Result<()> {
    record_entries(log, vec![(output, entry)])
}

// [spec:ronin:def:log.logrecord-fn]
// [spec:ronin:sem:log.logrecord-fn]
#[cfg(test)]
pub(crate) fn logrecord(
    log: &mut BuildLog,
    graph: &Graph,
    runtime: &RuntimeState,
    node: NodeId,
) -> io::Result<()> {
    let state = runtime.node(node);
    record_entry(
        log,
        graph.node_path(node).to_owned(),
        LogEntry {
            start_time: 0,
            end_time: 0,
            mtime: state.log_mtime().raw(),
            command_hash: state.logged_command_hash().raw(),
        },
    )
}

pub(crate) fn logrecordedge(
    log: &mut BuildLog,
    graph: &Graph,
    edge: EdgeId,
    command_hash: CommandHash,
    start_time: i32,
    end_time: i32,
    record_mtime: i64,
) -> Result<(), PersistenceError> {
    let outputs = graph.edge(edge).out.clone();
    let entries = outputs
        .into_iter()
        .map(|output| {
            (
                graph.node_path(output).to_owned(),
                LogEntry {
                    start_time,
                    end_time,
                    mtime: record_mtime,
                    command_hash: command_hash.raw(),
                },
            )
        })
        .collect();
    record_entries(log, entries).map_err(|source| {
        PersistenceError::io(
            PersistenceOperation::RecordBuildLog,
            log.path.clone(),
            source,
        )
    })
}

pub(crate) fn logrestat<F>(log: &mut BuildLog, filters: &[BString], mut stat: F) -> io::Result<()>
where
    F: FnMut(&Path) -> io::Result<i64>,
{
    let filters = filters
        .iter()
        .map(|filter| filter.as_bytes())
        .collect::<HashSet<_>>();
    let mut entries = log.entries.clone();
    for (output, entry) in &mut entries {
        if filters.is_empty() || filters.contains(output.as_bytes()) {
            entry.mtime = stat(output.to_path().expect("byte paths are valid on Unix"))?;
        }
    }
    rewrite(log, entries)
}

pub(crate) fn logrecompact<F>(log: &mut BuildLog, mut is_dead: F) -> io::Result<()>
where
    F: FnMut(&BStr) -> bool,
{
    let entries = log
        .entries
        .iter()
        .filter(|(path, _)| !is_dead(path.as_bytes().as_bstr()))
        .map(|(path, entry)| (path.clone(), entry.clone()))
        .collect();
    rewrite(log, entries)
}

pub(crate) fn logentry(log: &BuildLog, output: impl AsRef<[u8]>) -> Option<&LogEntry> {
    log.entries.get(output.as_ref())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::{mkedge, mknode};
    use crate::util::xasprintf;
    use std::fmt::Write as _;

    /// The byte parsers must agree with the `str` ones they replaced.
    ///
    /// A malformed field returns `None`, which makes `parse_entry` skip the
    /// line and the reader recover — so a parser that is merely *nearly* right
    /// does not fail loudly, it silently drops or invents log entries. Compare
    /// against the standard-library implementations directly rather than only
    /// through the reader.
    #[test]
    fn byte_parsers_match_the_str_implementations() {
        for case in [
            "0",
            "1",
            "-1",
            "+1",
            "007",
            "-0",
            "2147483647",
            "-2147483648",
            "2147483648",
            "9223372036854775807",
            "-9223372036854775808",
            "9223372036854775808",
            "",
            "-",
            "+",
            "abc",
            "1a",
            " 1",
            "1 ",
            "1-1",
        ] {
            assert_eq!(
                parse_decimal(case.as_bytes()),
                case.parse::<i64>().ok(),
                "decimal {case:?}"
            );
        }

        for case in [
            "0",
            "ff",
            "FF",
            "+ff",
            "-ff",
            "ffffffffffffffff",
            "10000000000000000",
            "",
            "+",
            "-",
            "g",
            "0x1",
            " f",
            "f ",
        ] {
            assert_eq!(
                parse_hex(case.as_bytes()),
                u64::from_str_radix(case, 16).ok(),
                "hex {case:?}"
            );
        }
    }
    use std::fs;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static NEXT_LOG_TEST: AtomicUsize = AtomicUsize::new(0);

    struct TempLog {
        directory: PathBuf,
        path: PathBuf,
    }

    impl TempLog {
        fn new(name: &str) -> Self {
            for _ in 0..1024 {
                let sequence = NEXT_LOG_TEST.fetch_add(1, Ordering::Relaxed);
                let directory = std::env::temp_dir().join(format!(
                    "ronin-ninja-build-log-{}-{name}-{sequence}",
                    std::process::id()
                ));
                match fs::create_dir(&directory) {
                    Ok(()) => {
                        let path = directory.join(".ninja_log");
                        return Self { directory, path };
                    }
                    Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
                    Err(error) => panic!("could not create build-log test directory: {error}"),
                }
            }
            panic!("could not allocate a unique build-log test directory")
        }
    }

    impl Drop for TempLog {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.directory);
        }
    }

    #[test]
    fn splits_tab_separated_fields() {
        let mut line = b"one\ttwo".as_slice();
        assert_eq!(nextfield(&mut line), Some(b"one".as_slice()));
        assert_eq!(nextfield(&mut line), Some(b"two".as_slice()));
        assert_eq!(nextfield(&mut line), None);
    }

    // [spec:ronin:req:compat.persistent-state/test]
    #[test]
    fn ninja_build_log_tolerates_every_truncation() {
        let temp = TempLog::new("truncate");
        let complete = concat!(
            "# ninja log v7\n",
            "15\t18\t18\tout\t1234\n",
            "20\t25\t25\tmid\t5678\n",
        );
        for size in (1..=complete.len()).rev() {
            fs::write(&temp.path, &complete.as_bytes()[..size]).unwrap();
            BuildLog::open(Some(&temp.directory))
                .unwrap()
                .finish()
                .unwrap();
        }
    }

    #[test]
    fn ninja_build_log_restat_filters_and_updates_mtime() {
        let temp = TempLog::new("restat");
        fs::write(&temp.path, "# ninja log v7\n1\t2\t3\tout\tc0ffee\n").unwrap();
        let mut log = BuildLog::open(Some(&temp.directory)).unwrap();
        assert_eq!(logentry(&log, "out").unwrap().mtime, 3);
        logrestat(&mut log, &[BString::from("out2")], |_| Ok(4)).unwrap();
        assert_eq!(logentry(&log, "out").unwrap().mtime, 3);
        logrestat(&mut log, &[], |_| Ok(4)).unwrap();
        assert_eq!(logentry(&log, "out").unwrap().mtime, 4);
        log.finish().unwrap();
    }

    // [spec:ronin:req:runtime.persistence-transactions/test]
    #[test]
    fn build_log_rewrite_failures_preserve_state_and_writer() {
        for stage in crate::persistence::RewriteStage::ALL {
            let temp = TempLog::new("transaction-failure");
            fs::write(&temp.path, "# ninja log v7\n1\t2\t3\tout\tc0ffee\n").unwrap();
            let original_file = fs::read(&temp.path).unwrap();
            let mut log = BuildLog::open(Some(&temp.directory)).unwrap();
            let original_entries = log.entries.clone();
            let mut staged_entries = original_entries.clone();
            staged_entries.get_mut(b"out".as_slice()).unwrap().mtime = 99;

            let error = rewrite_with_fault(&mut log, staged_entries, stage).unwrap_err();
            assert!(error
                .to_string()
                .contains("injected atomic rewrite failure"));
            assert_eq!(log.entries, original_entries);
            assert_eq!(fs::read(&temp.path).unwrap(), original_file);

            record_entry(
                &mut log,
                BString::from("probe"),
                LogEntry {
                    start_time: 3,
                    end_time: 4,
                    mtime: 4,
                    command_hash: 0x1234,
                },
            )
            .unwrap();
            log.finish().unwrap();
            assert!(fs::read(&temp.path)
                .unwrap()
                .windows(b"\tprobe\t1234\n".len())
                .any(|window| window == b"\tprobe\t1234\n"));
        }
    }

    #[test]
    fn ninja_build_log_ignores_very_long_input_line() {
        let temp = TempLog::new("long-line");
        let mut contents = String::from("# ninja log v7\n123\t456\t456\tout\tcommand start");
        while contents.len() < 512 << 10 {
            contents.push_str(" more_command");
        }
        contents.push('\n');
        contents.push_str("456\t789\t789\tout2\tbeef\n");
        fs::write(&temp.path, contents).unwrap();

        let log = BuildLog::open(Some(&temp.directory)).unwrap();
        assert!(logentry(&log, "out").is_none());
        let entry = logentry(&log, "out2").unwrap();
        assert_eq!(entry.start_time, 456);
        assert_eq!(entry.end_time, 789);
        assert_eq!(entry.mtime, 789);
        assert_eq!(entry.command_hash, 0xbeef);
        log.finish().unwrap();
    }

    #[test]
    fn ninja_build_log_records_every_multi_target_output() {
        let temp = TempLog::new("multi-target");
        let mut graph = Graph::default();
        let root = crate::env::mkenv(&mut graph, None);
        let edge = mkedge(&mut graph, root);
        let output = mknode(&mut graph, xasprintf(format_args!("out")));
        let depfile = mknode(&mut graph, xasprintf(format_args!("out.d")));
        {
            let edge = graph.edge_mut(edge);
            edge.out.extend([output, depfile]);
            edge.set_explicit_output_count(2);
        }

        let mut log = BuildLog::open(Some(&temp.directory)).unwrap();
        logrecordedge(
            &mut log,
            &graph,
            edge,
            CommandHash::from_raw(0x1234),
            21,
            22,
            23,
        )
        .unwrap();
        let output = logentry(&log, "out").unwrap();
        let depfile = logentry(&log, "out.d").unwrap();
        assert_eq!(output.start_time, 21);
        assert_eq!(depfile.start_time, 21);
        assert_eq!(output.end_time, 22);
        assert_eq!(depfile.end_time, 22);
        assert_eq!(output.mtime, 23);
        assert_eq!(depfile.mtime, 23);
        log.finish().unwrap();
    }

    #[test]
    fn ninja_build_log_preserves_non_utf8_outputs() {
        let temp = TempLog::new("non-utf8");
        let mut graph = Graph::default();
        let root = crate::env::mkenv(&mut graph, None);
        let edge = mkedge(&mut graph, root);
        let output = mknode(&mut graph, BString::from(b"out-\xff".as_slice()));
        graph.node_mut(output).gen = Some(edge);
        graph.edge_mut(edge).out.push(output);
        let mut runtime = RuntimeState::new(&graph);
        runtime
            .node_mut(output)
            .set_log_mtime(crate::runtime::FileTime::observed(17));
        runtime
            .node_mut(output)
            .set_logged_command_hash(CommandHash::from_raw(0x1234));
        let mut log = BuildLog::open(Some(&temp.directory)).unwrap();
        logrecord(&mut log, &graph, &runtime, output).unwrap();
        log.finish().unwrap();

        let mut reloaded_graph = Graph::default();
        let root = crate::env::mkenv(&mut reloaded_graph, None);
        let edge = mkedge(&mut reloaded_graph, root);
        let output = mknode(&mut reloaded_graph, BString::from(b"out-\xff".as_slice()));
        reloaded_graph.node_mut(output).gen = Some(edge);
        reloaded_graph.edge_mut(edge).out.push(output);
        let reloaded = BuildLog::open(Some(&temp.directory)).unwrap();
        let mut runtime = RuntimeState::new(&reloaded_graph);
        reloaded.hydrate_runtime(&reloaded_graph, &mut runtime, reloaded_graph.node_ids());
        assert_eq!(logentry(&reloaded, b"out-\xff").unwrap().mtime, 17);
        assert_eq!(
            runtime.node(output).logged_command_hash(),
            CommandHash::from_raw(0x1234)
        );
        reloaded.finish().unwrap();
    }

    #[test]
    fn ninja_build_log_recompacts_latest_live_entries() {
        let temp = TempLog::new("recompact");
        let mut contents = String::from("# ninja log v7\n");
        for end_time in 18..218 {
            let _ = writeln!(contents, "15\t{end_time}\t{end_time}\tout\t1234");
        }
        contents.push_str("21\t22\t22\tout2\t5678\n");
        fs::write(&temp.path, contents).unwrap();

        let mut log = BuildLog::open(Some(&temp.directory)).unwrap();
        assert_eq!(log.entries.len(), 2);
        assert_eq!(logentry(&log, "out").unwrap().end_time, 217);
        logrecompact(&mut log, |path| path == "out2").unwrap();
        assert_eq!(log.entries.len(), 1);
        assert!(logentry(&log, "out").is_some());
        assert!(logentry(&log, "out2").is_none());
        log.finish().unwrap();

        let reloaded = BuildLog::open(Some(&temp.directory)).unwrap();
        assert_eq!(reloaded.entries.len(), 1);
        assert!(logentry(&reloaded, "out").is_some());
        assert!(logentry(&reloaded, "out2").is_none());
        reloaded.finish().unwrap();
    }
}
