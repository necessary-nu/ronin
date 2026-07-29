//! Version-7 Ninja build log reader and writer.

#[cfg(test)]
use crate::graph::NodeId;
use crate::graph::{nodeget, EdgeId, Graph};
use crate::util::{BStr, BString, ByteSlice};
use std::collections::{HashMap, HashSet};
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufRead, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};

const MAX_LOG_LINE: usize = 256 << 10;
const LOG_HEADER: &[u8] = b"# ninja log v7";

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct LogEntry {
    pub(crate) start_time: i32,
    pub(crate) end_time: i32,
    pub(crate) mtime: i64,
    pub(crate) output: BString,
    pub(crate) command_hash: u64,
}

pub(crate) struct BuildLog {
    writer: Option<BufWriter<File>>,
    path: PathBuf,
    pub(crate) entries: HashMap<BString, LogEntry>,
}

// [spec:samurai:def:log.nextfield-fn]
// [spec:samurai:sem:log.nextfield-fn]
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

fn parse_entry(line: &[u8]) -> Option<LogEntry> {
    if line.len() > MAX_LOG_LINE || line == LOG_HEADER {
        return None;
    }
    let mut rest = line;
    let start_time = std::str::from_utf8(nextfield(&mut rest)?)
        .ok()?
        .parse()
        .ok()?;
    let end_time = std::str::from_utf8(nextfield(&mut rest)?)
        .ok()?
        .parse()
        .ok()?;
    let mtime = std::str::from_utf8(nextfield(&mut rest)?)
        .ok()?
        .parse()
        .ok()?;
    let output = BString::from(nextfield(&mut rest)?);
    let command_hash =
        u64::from_str_radix(std::str::from_utf8(nextfield(&mut rest)?).ok()?, 16).ok()?;
    Some(LogEntry {
        start_time,
        end_time,
        mtime,
        output,
        command_hash,
    })
}

fn read_line(reader: &mut impl BufRead, line: &mut Vec<u8>) -> io::Result<Option<bool>> {
    line.clear();
    loop {
        let buffer = reader.fill_buf()?;
        if buffer.is_empty() {
            return Ok((!line.is_empty()).then_some(false));
        }
        let newline = buffer.iter().position(|byte| *byte == b'\n');
        let consumed = newline.map_or(buffer.len(), |index| index + 1);
        if line.len() <= MAX_LOG_LINE {
            let keep = consumed
                .saturating_sub(usize::from(newline.is_some()))
                .min(MAX_LOG_LINE + 1 - line.len());
            line.extend_from_slice(&buffer[..keep]);
        }
        reader.consume(consumed);
        if newline.is_some() {
            return Ok(Some(true));
        }
    }
}

fn write_entry(writer: &mut BufWriter<File>, entry: &LogEntry) -> io::Result<()> {
    write!(
        writer,
        "{}\t{}\t{}\t",
        entry.start_time, entry.end_time, entry.mtime
    )?;
    writer.write_all(entry.output.as_bytes())?;
    writeln!(writer, "\t{:x}", entry.command_hash)
}

fn rewrite(log: &mut BuildLog) -> io::Result<()> {
    if let Some(mut writer) = log.writer.take() {
        writer.flush()?;
    }
    let mut temp_name = log.path.as_os_str().to_os_string();
    temp_name.push(".recompact");
    let temp_path = PathBuf::from(temp_name);
    if let Err(error) = fs::remove_file(&temp_path) {
        if error.kind() != io::ErrorKind::NotFound {
            return Err(error);
        }
    }
    let mut writer = BufWriter::new(File::create(&temp_path)?);
    writer.write_all(LOG_HEADER)?;
    writer.write_all(b"\n")?;
    for entry in log.entries.values() {
        write_entry(&mut writer, entry)?;
    }
    writer.flush()?;
    writer.get_ref().sync_all()?;
    drop(writer);
    fs::rename(&temp_path, &log.path)?;
    log.writer = Some(BufWriter::new(open_log(&log.path)?));
    Ok(())
}

// [spec:samurai:def:log.loginit-fn]
// [spec:samurai:sem:log.loginit-fn]
// [spec:samurai:req:compat.persistent-state]
pub(crate) fn loginit(builddir: Option<&Path>, graph: &mut Graph) -> io::Result<BuildLog> {
    let path = builddir.map_or_else(
        || std::path::PathBuf::from(".ninja_log"),
        |dir| dir.join(".ninja_log"),
    );
    let read_file = OpenOptions::new().read(true).open(&path);
    let mut valid = false;
    let mut entries = HashMap::new();
    if let Ok(read_file) = read_file {
        let mut reader = BufReader::new(read_file);
        let mut line = Vec::new();
        valid = read_line(&mut reader, &mut line)?.is_some() && line == LOG_HEADER;
        if valid {
            while let Some(terminated) = read_line(&mut reader, &mut line)? {
                if terminated {
                    let line = line.strip_suffix(b"\r").unwrap_or(&line);
                    if let Some(entry) = parse_entry(line) {
                        if let Some(node) = nodeget(graph, entry.output.as_bytes()) {
                            let node = graph.node_mut(node);
                            if node.gen.is_some() {
                                node.logmtime = entry.mtime;
                                node.hash = entry.command_hash;
                            }
                        }
                        entries.insert(entry.output.clone(), entry);
                    }
                }
            }
        }
    }
    let writer_file = if valid {
        open_log(&path)?
    } else {
        let mut file = File::create(&path)?;
        file.write_all(LOG_HEADER)?;
        file.write_all(b"\n")?;
        file
    };
    Ok(BuildLog {
        writer: Some(BufWriter::new(writer_file)),
        path,
        entries,
    })
}

fn record_entry(log: &mut BuildLog, entry: LogEntry) -> io::Result<()> {
    let writer = log.writer.as_mut().expect("open build log");
    write_entry(writer, &entry)?;
    writer.flush()?;
    log.entries.insert(entry.output.clone(), entry);
    Ok(())
}

// [spec:samurai:def:log.logrecord-fn]
// [spec:samurai:sem:log.logrecord-fn]
#[cfg(test)]
pub(crate) fn logrecord(log: &mut BuildLog, graph: &Graph, node: NodeId) -> io::Result<()> {
    let node = graph.node(node);
    record_entry(
        log,
        LogEntry {
            start_time: 0,
            end_time: 0,
            mtime: node.logmtime,
            output: node.path.clone(),
            command_hash: node.hash,
        },
    )
}

pub(crate) fn logrecordedge(
    log: &mut BuildLog,
    graph: &Graph,
    edge: EdgeId,
    start_time: i32,
    end_time: i32,
    record_mtime: i64,
) -> io::Result<()> {
    let (outputs, command_hash) = {
        let edge = graph.edge(edge);
        (edge.out.clone(), edge.hash)
    };
    for output in outputs {
        let output = graph.node(output);
        record_entry(
            log,
            LogEntry {
                start_time,
                end_time,
                mtime: record_mtime,
                output: output.path.clone(),
                command_hash,
            },
        )?;
    }
    Ok(())
}

pub(crate) fn logrestat<F>(log: &mut BuildLog, filters: &[&str], mut stat: F) -> io::Result<()>
where
    F: FnMut(&Path) -> io::Result<i64>,
{
    let filters = filters
        .iter()
        .map(|filter| filter.as_bytes())
        .collect::<HashSet<_>>();
    for entry in log.entries.values_mut() {
        if filters.is_empty() || filters.contains(entry.output.as_bytes()) {
            entry.mtime = stat(
                entry
                    .output
                    .to_path()
                    .expect("byte paths are valid on Unix"),
            )?;
        }
    }
    rewrite(log)
}

pub(crate) fn logrecompact<F>(log: &mut BuildLog, mut is_dead: F) -> io::Result<()>
where
    F: FnMut(&BStr) -> bool,
{
    log.entries
        .retain(|path, _| !is_dead(path.as_bytes().as_bstr()));
    rewrite(log)
}

#[cfg(test)]
pub(crate) fn logentry(log: &BuildLog, output: impl AsRef<[u8]>) -> Option<&LogEntry> {
    log.entries.get(output.as_ref())
}

// [spec:samurai:def:log.logclose-fn]
// [spec:samurai:sem:log.logclose-fn]
pub(crate) fn logclose(mut log: BuildLog) -> io::Result<()> {
    if let Some(mut writer) = log.writer.take() {
        writer.flush()
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::{graphinit, mkedge, mknode};
    use crate::util::xasprintf;
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
                    Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
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

    // [spec:samurai:req:compat.persistent-state/test]
    #[test]
    fn ninja_build_log_tolerates_every_truncation() {
        let temp = TempLog::new("truncate");
        let complete = concat!(
            "# ninja log v7\n",
            "15\t18\t18\tout\t1234\n",
            "20\t25\t25\tmid\t5678\n",
        );
        let mut graph = graphinit();
        for size in (1..=complete.len()).rev() {
            fs::write(&temp.path, &complete.as_bytes()[..size]).unwrap();
            logclose(loginit(Some(&temp.directory), &mut graph).unwrap()).unwrap();
        }
    }

    #[test]
    fn ninja_build_log_restat_filters_and_updates_mtime() {
        let temp = TempLog::new("restat");
        fs::write(&temp.path, "# ninja log v7\n1\t2\t3\tout\tc0ffee\n").unwrap();
        let mut graph = graphinit();
        let mut log = loginit(Some(&temp.directory), &mut graph).unwrap();
        assert_eq!(logentry(&log, "out").unwrap().mtime, 3);
        logrestat(&mut log, &["out2"], |_| Ok(4)).unwrap();
        assert_eq!(logentry(&log, "out").unwrap().mtime, 3);
        logrestat(&mut log, &[], |_| Ok(4)).unwrap();
        assert_eq!(logentry(&log, "out").unwrap().mtime, 4);
        logclose(log).unwrap();
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

        let mut graph = graphinit();
        let log = loginit(Some(&temp.directory), &mut graph).unwrap();
        assert!(logentry(&log, "out").is_none());
        let entry = logentry(&log, "out2").unwrap();
        assert_eq!(entry.start_time, 456);
        assert_eq!(entry.end_time, 789);
        assert_eq!(entry.mtime, 789);
        assert_eq!(entry.command_hash, 0xbeef);
        logclose(log).unwrap();
    }

    #[test]
    fn ninja_build_log_records_every_multi_target_output() {
        let temp = TempLog::new("multi-target");
        let mut graph = graphinit();
        let root = crate::env::mkenv(&mut graph, None);
        let edge = mkedge(&mut graph, root);
        let output = mknode(&mut graph, xasprintf(format_args!("out")));
        let depfile = mknode(&mut graph, xasprintf(format_args!("out.d")));
        graph.node_mut(output).mtime = 22;
        graph.node_mut(depfile).mtime = 22;
        {
            let edge = graph.edge_mut(edge);
            edge.out.extend([output, depfile]);
            edge.outimpidx = 2;
            edge.hash = 0x1234;
        }

        let mut log = loginit(Some(&temp.directory), &mut graph).unwrap();
        logrecordedge(&mut log, &graph, edge, 21, 22, 23).unwrap();
        let output = logentry(&log, "out").unwrap();
        let depfile = logentry(&log, "out.d").unwrap();
        assert_eq!(output.start_time, 21);
        assert_eq!(depfile.start_time, 21);
        assert_eq!(output.end_time, 22);
        assert_eq!(depfile.end_time, 22);
        assert_eq!(output.mtime, 23);
        assert_eq!(depfile.mtime, 23);
        logclose(log).unwrap();
    }

    #[test]
    fn ninja_build_log_preserves_non_utf8_outputs() {
        let temp = TempLog::new("non-utf8");
        let mut graph = graphinit();
        let root = crate::env::mkenv(&mut graph, None);
        let edge = mkedge(&mut graph, root);
        let output = mknode(&mut graph, BString::from(b"out-\xff".as_slice()));
        graph.node_mut(output).gen = Some(edge);
        graph.node_mut(output).logmtime = 17;
        graph.node_mut(output).hash = 0x1234;
        graph.edge_mut(edge).out.push(output);
        let mut log = loginit(Some(&temp.directory), &mut graph).unwrap();
        logrecord(&mut log, &graph, output).unwrap();
        logclose(log).unwrap();

        let mut reloaded_graph = graphinit();
        let root = crate::env::mkenv(&mut reloaded_graph, None);
        let edge = mkedge(&mut reloaded_graph, root);
        let output = mknode(&mut reloaded_graph, BString::from(b"out-\xff".as_slice()));
        reloaded_graph.node_mut(output).gen = Some(edge);
        reloaded_graph.edge_mut(edge).out.push(output);
        let reloaded = loginit(Some(&temp.directory), &mut reloaded_graph).unwrap();
        assert_eq!(logentry(&reloaded, b"out-\xff").unwrap().mtime, 17);
        assert_eq!(reloaded_graph.node(output).hash, 0x1234);
        logclose(reloaded).unwrap();
    }

    #[test]
    fn ninja_build_log_recompacts_latest_live_entries() {
        let temp = TempLog::new("recompact");
        let mut contents = String::from("# ninja log v7\n");
        for end_time in 18..218 {
            contents.push_str(&format!("15\t{end_time}\t{end_time}\tout\t1234\n"));
        }
        contents.push_str("21\t22\t22\tout2\t5678\n");
        fs::write(&temp.path, contents).unwrap();

        let mut graph = graphinit();
        let mut log = loginit(Some(&temp.directory), &mut graph).unwrap();
        assert_eq!(log.entries.len(), 2);
        assert_eq!(logentry(&log, "out").unwrap().end_time, 217);
        logrecompact(&mut log, |path| path == "out2").unwrap();
        assert_eq!(log.entries.len(), 1);
        assert!(logentry(&log, "out").is_some());
        assert!(logentry(&log, "out2").is_none());
        logclose(log).unwrap();

        let reloaded = loginit(Some(&temp.directory), &mut graph).unwrap();
        assert_eq!(reloaded.entries.len(), 1);
        assert!(logentry(&reloaded, "out").is_some());
        assert!(logentry(&reloaded, "out2").is_none());
        logclose(reloaded).unwrap();
    }
}
