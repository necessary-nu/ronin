//! Version-7 Ninja build log reader and writer.

use crate::graph::{nodeget, EdgeRef, Graph, NodeRef};
use crate::util::ByteSlice;
use std::collections::BTreeMap;
use std::fs::{File, OpenOptions};
use std::io::{self, BufRead, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};

const LOG_VERSION: i32 = 7;
const MAX_LOG_LINE: usize = 256 << 10;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LogEntry {
    pub start_time: i32,
    pub end_time: i32,
    pub mtime: i64,
    pub output: String,
    pub command_hash: u64,
}

pub struct BuildLog {
    writer: Option<BufWriter<File>>,
    path: PathBuf,
    entries: BTreeMap<String, LogEntry>,
}

// [spec:samurai:def:log.nextfield-fn]
// [spec:samurai:sem:log.nextfield-fn]
fn nextfield<'a>(line: &mut &'a str) -> Option<&'a str> {
    if line.is_empty() {
        return None;
    }
    let index = line.find(['\t', '\n']).unwrap_or(line.len());
    let (field, rest) = line.split_at(index);
    *line = rest
        .strip_prefix('\t')
        .or_else(|| rest.strip_prefix('\n'))
        .unwrap_or(rest);
    Some(field)
}

fn open_log(path: &Path) -> io::Result<File> {
    OpenOptions::new()
        .create(true)
        .read(true)
        .append(true)
        .open(path)
}

fn parse_entry(line: &str) -> Option<LogEntry> {
    if line.len() > MAX_LOG_LINE || line == format!("# ninja log v{LOG_VERSION}") {
        return None;
    }
    let mut rest = line;
    let start_time = nextfield(&mut rest)?.parse().ok()?;
    let end_time = nextfield(&mut rest)?.parse().ok()?;
    let mtime = nextfield(&mut rest)?.parse().ok()?;
    let output = nextfield(&mut rest)?.to_string();
    let command_hash = u64::from_str_radix(nextfield(&mut rest)?, 16).ok()?;
    Some(LogEntry {
        start_time,
        end_time,
        mtime,
        output,
        command_hash,
    })
}

fn write_entry(writer: &mut BufWriter<File>, entry: &LogEntry) -> io::Result<()> {
    writeln!(
        writer,
        "{}\t{}\t{}\t{}\t{:x}",
        entry.start_time, entry.end_time, entry.mtime, entry.output, entry.command_hash
    )
}

fn rewrite(log: &mut BuildLog) -> io::Result<()> {
    if let Some(mut writer) = log.writer.take() {
        writer.flush()?;
    }
    let mut writer = BufWriter::new(File::create(&log.path)?);
    writeln!(writer, "# ninja log v{LOG_VERSION}")?;
    for entry in log.entries.values() {
        write_entry(&mut writer, entry)?;
    }
    writer.flush()?;
    log.writer = Some(BufWriter::new(open_log(&log.path)?));
    Ok(())
}

// [spec:samurai:def:log.loginit-fn]
// [spec:samurai:sem:log.loginit-fn]
pub fn loginit(builddir: Option<&Path>, graph: &Graph) -> io::Result<BuildLog> {
    let path = builddir.map_or_else(
        || std::path::PathBuf::from(".ninja_log"),
        |dir| dir.join(".ninja_log"),
    );
    let read_file = OpenOptions::new().read(true).open(&path);
    let mut valid = false;
    let mut entries = BTreeMap::new();
    if let Ok(read_file) = read_file {
        let mut lines = BufReader::new(read_file).lines();
        valid = lines.next().transpose()?.as_deref() == Some("# ninja log v7");
        if valid {
            for line in lines {
                let line = line?;
                if let Some(entry) = parse_entry(&line) {
                    if let Some(node) = nodeget(graph, entry.output.as_bytes()) {
                        let mut node = node.borrow_mut();
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
    let writer_file = if valid {
        open_log(&path)?
    } else {
        let mut file = File::create(&path)?;
        writeln!(file, "# ninja log v{LOG_VERSION}")?;
        file
    };
    Ok(BuildLog {
        writer: Some(BufWriter::new(writer_file)),
        path,
        entries,
    })
}

fn record_entry(log: &mut BuildLog, entry: LogEntry) -> io::Result<()> {
    write_entry(log.writer.as_mut().expect("open build log"), &entry)?;
    log.entries.insert(entry.output.clone(), entry);
    Ok(())
}

// [spec:samurai:def:log.logrecord-fn]
// [spec:samurai:sem:log.logrecord-fn]
pub fn logrecord(log: &mut BuildLog, node: &NodeRef) -> io::Result<()> {
    let node = node.borrow();
    record_entry(
        log,
        LogEntry {
            start_time: 0,
            end_time: 0,
            mtime: node.logmtime,
            output: String::from_utf8_lossy(node.path.as_bytes()).into_owned(),
            command_hash: node.hash,
        },
    )
}

pub fn logrecordedge(
    log: &mut BuildLog,
    edge: &EdgeRef,
    start_time: i32,
    end_time: i32,
    record_mtime: i64,
) -> io::Result<()> {
    let (outputs, command_hash) = {
        let edge = edge.borrow();
        (edge.out.clone(), edge.hash)
    };
    for output in outputs {
        let output = output.borrow();
        record_entry(
            log,
            LogEntry {
                start_time,
                end_time,
                mtime: record_mtime,
                output: String::from_utf8_lossy(output.path.as_bytes()).into_owned(),
                command_hash,
            },
        )?;
    }
    Ok(())
}

pub fn logrestat<F>(log: &mut BuildLog, filters: &[&str], mut stat: F) -> io::Result<()>
where
    F: FnMut(&Path) -> io::Result<i64>,
{
    for entry in log.entries.values_mut() {
        if filters.is_empty() || filters.contains(&entry.output.as_str()) {
            entry.mtime = stat(Path::new(&entry.output))?;
        }
    }
    rewrite(log)
}

pub fn logrecompact<F>(log: &mut BuildLog, mut is_dead: F) -> io::Result<()>
where
    F: FnMut(&str) -> bool,
{
    log.entries.retain(|path, _| !is_dead(path));
    rewrite(log)
}

pub fn logentry<'a>(log: &'a BuildLog, output: &str) -> Option<&'a LogEntry> {
    log.entries.get(output)
}

// [spec:samurai:def:log.logclose-fn]
// [spec:samurai:sem:log.logclose-fn]
pub fn logclose(mut log: BuildLog) -> io::Result<()> {
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
        let mut line = "one\ttwo";
        assert_eq!(nextfield(&mut line), Some("one"));
        assert_eq!(nextfield(&mut line), Some("two"));
        assert_eq!(nextfield(&mut line), None);
    }

    #[test]
    fn ninja_build_log_tolerates_every_truncation() {
        let temp = TempLog::new("truncate");
        let complete = concat!(
            "# ninja log v7\n",
            "15\t18\t18\tout\t1234\n",
            "20\t25\t25\tmid\t5678\n",
        );
        let graph = graphinit();
        for size in (1..=complete.len()).rev() {
            fs::write(&temp.path, &complete.as_bytes()[..size]).unwrap();
            logclose(loginit(Some(&temp.directory), &graph).unwrap()).unwrap();
        }
    }

    #[test]
    fn ninja_build_log_restat_filters_and_updates_mtime() {
        let temp = TempLog::new("restat");
        fs::write(&temp.path, "# ninja log v7\n1\t2\t3\tout\tc0ffee\n").unwrap();
        let graph = graphinit();
        let mut log = loginit(Some(&temp.directory), &graph).unwrap();
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

        let graph = graphinit();
        let log = loginit(Some(&temp.directory), &graph).unwrap();
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
        let root = crate::env::mkenv(None);
        let edge = mkedge(&mut graph, root);
        let output = mknode(&mut graph, xasprintf(format_args!("out")));
        let depfile = mknode(&mut graph, xasprintf(format_args!("out.d")));
        output.borrow_mut().mtime = 22;
        depfile.borrow_mut().mtime = 22;
        {
            let mut edge = edge.borrow_mut();
            edge.out.extend([output, depfile]);
            edge.outimpidx = 2;
            edge.hash = 0x1234;
        }

        let mut log = loginit(Some(&temp.directory), &graph).unwrap();
        logrecordedge(&mut log, &edge, 21, 22, 23).unwrap();
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
    fn ninja_build_log_recompacts_latest_live_entries() {
        let temp = TempLog::new("recompact");
        let mut contents = String::from("# ninja log v7\n");
        for end_time in 18..218 {
            contents.push_str(&format!("15\t{end_time}\t{end_time}\tout\t1234\n"));
        }
        contents.push_str("21\t22\t22\tout2\t5678\n");
        fs::write(&temp.path, contents).unwrap();

        let graph = graphinit();
        let mut log = loginit(Some(&temp.directory), &graph).unwrap();
        assert_eq!(log.entries.len(), 2);
        assert_eq!(logentry(&log, "out").unwrap().end_time, 217);
        logrecompact(&mut log, |path| path == "out2").unwrap();
        assert_eq!(log.entries.len(), 1);
        assert!(logentry(&log, "out").is_some());
        assert!(logentry(&log, "out2").is_none());
        logclose(log).unwrap();

        let reloaded = loginit(Some(&temp.directory), &graph).unwrap();
        assert_eq!(reloaded.entries.len(), 1);
        assert!(logentry(&reloaded, "out").is_some());
        assert!(logentry(&reloaded, "out2").is_none());
        logclose(reloaded).unwrap();
    }
}
