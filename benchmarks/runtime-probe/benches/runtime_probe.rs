//! Isolated process-supervision strategy benchmark.

use std::fs;
use std::io::{self, Read};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

#[derive(Default)]
struct Metrics {
    output_bytes: usize,
    peak_threads: usize,
    peak_rss_kib: u64,
    wakeups: usize,
}

impl Metrics {
    fn sample(&mut self) {
        let Ok(status) = fs::read_to_string("/proc/self/status") else {
            return;
        };
        for line in status.lines() {
            if let Some(value) = line.strip_prefix("Threads:") {
                self.peak_threads = self
                    .peak_threads
                    .max(value.trim().parse().unwrap_or_default());
            } else if let Some(value) = line.strip_prefix("VmHWM:") {
                self.peak_rss_kib = self.peak_rss_kib.max(
                    value
                        .split_whitespace()
                        .next()
                        .and_then(|value| value.parse().ok())
                        .unwrap_or_default(),
                );
            }
        }
    }
}

fn shell_command(sleep_ms: u64) -> String {
    if sleep_ms == 0 {
        "printf x".to_owned()
    } else {
        format!("sleep {:.3}; printf x", sleep_ms as f64 / 1_000.0)
    }
}

fn run_threaded(total: usize, parallelism: usize, sleep_ms: u64) -> io::Result<Metrics> {
    let command = shell_command(sleep_ms);
    let (sender, receiver) = mpsc::channel();
    let mut metrics = Metrics::default();
    let mut started = 0;
    let mut finished = 0;
    while finished < total {
        while started < total && started - finished < parallelism {
            let sender = sender.clone();
            let command = command.clone();
            std::thread::spawn(move || {
                let result = run_threaded_child(&command);
                let _ = sender.send(result);
            });
            started += 1;
        }
        metrics.sample();
        metrics.output_bytes += receiver.recv().map_err(io::Error::other)??;
        metrics.wakeups += 1;
        finished += 1;
    }
    Ok(metrics)
}

#[cfg(unix)]
fn run_threaded_child(command: &str) -> io::Result<usize> {
    use std::os::fd::OwnedFd;
    use std::os::unix::net::UnixStream;

    let (mut output, writer) = UnixStream::pair()?;
    let stdout: OwnedFd = writer.try_clone()?.into();
    let stderr: OwnedFd = writer.into();
    let mut child = Command::new("/bin/sh")
        .arg("-c")
        .arg(command)
        .stdin(Stdio::null())
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr))
        .spawn()?;
    let mut bytes = Vec::new();
    output.read_to_end(&mut bytes)?;
    if !child.wait()?.success() {
        return Err(io::Error::other("threaded child failed"));
    }
    Ok(bytes.len())
}

#[cfg(not(unix))]
fn run_threaded_child(command: &str) -> io::Result<usize> {
    let output = Command::new("/bin/sh").arg("-c").arg(command).output()?;
    if !output.status.success() {
        return Err(io::Error::other("threaded child failed"));
    }
    Ok(output.stdout.len() + output.stderr.len())
}

#[cfg(feature = "tokio-runtime")]
fn run_tokio(total: usize, parallelism: usize, sleep_ms: u64) -> io::Result<Metrics> {
    use std::os::fd::OwnedFd;
    use std::os::unix::net::UnixStream;
    use tokio::io::AsyncReadExt;

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    runtime.block_on(async move {
        let command = shell_command(sleep_ms);
        let mut tasks = tokio::task::JoinSet::new();
        let mut metrics = Metrics::default();
        let mut started = 0;
        let mut finished = 0;
        while finished < total {
            while started < total && started - finished < parallelism {
                let command = command.clone();
                tasks.spawn(async move {
                    let (output, writer) = UnixStream::pair()?;
                    output.set_nonblocking(true)?;
                    let stdout: OwnedFd = writer.try_clone()?.into();
                    let stderr: OwnedFd = writer.into();
                    let mut child = tokio::process::Command::new("/bin/sh")
                        .arg("-c")
                        .arg(command)
                        .stdin(Stdio::null())
                        .stdout(Stdio::from(stdout))
                        .stderr(Stdio::from(stderr))
                        .spawn()?;
                    let mut output = tokio::net::UnixStream::from_std(output)?;
                    let mut bytes = Vec::new();
                    output.read_to_end(&mut bytes).await?;
                    Ok::<_, io::Error>((child.wait().await?, bytes))
                });
                started += 1;
            }
            metrics.sample();
            let output = tasks
                .join_next()
                .await
                .ok_or_else(|| io::Error::other("Tokio task set closed"))?
                .map_err(io::Error::other)??;
            if !output.0.success() {
                return Err(io::Error::other("Tokio child failed"));
            }
            metrics.output_bytes += output.1.len();
            metrics.wakeups += 1;
            finished += 1;
        }
        Ok(metrics)
    })
}

#[cfg(not(feature = "tokio-runtime"))]
fn run_tokio(_total: usize, _parallelism: usize, _sleep_ms: u64) -> io::Result<Metrics> {
    Err(io::Error::other(
        "rebuild with --features tokio-runtime to run Tokio",
    ))
}

#[cfg(unix)]
struct PolledChild {
    #[cfg(feature = "evented-runtime")]
    key: usize,
    child: Child,
    output: std::os::unix::net::UnixStream,
    status: Option<std::process::ExitStatus>,
    eof: bool,
    output_bytes: usize,
}

#[cfg(unix)]
fn spawn_polled(_key: usize, command: &str) -> io::Result<PolledChild> {
    use std::os::fd::OwnedFd;
    use std::os::unix::net::UnixStream;

    let (output, writer) = UnixStream::pair()?;
    output.set_nonblocking(true)?;
    let stdout: OwnedFd = writer.try_clone()?.into();
    let stderr: OwnedFd = writer.into();
    let child = Command::new("/bin/sh")
        .arg("-c")
        .arg(command)
        .stdin(Stdio::null())
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr))
        .spawn()?;
    Ok(PolledChild {
        #[cfg(feature = "evented-runtime")]
        key: _key,
        child,
        output,
        status: None,
        eof: false,
        output_bytes: 0,
    })
}

#[cfg(unix)]
fn run_polling(total: usize, parallelism: usize, sleep_ms: u64) -> io::Result<Metrics> {
    let command = shell_command(sleep_ms);
    let mut metrics = Metrics::default();
    let mut active = Vec::new();
    let mut started = 0;
    let mut finished = 0;
    while finished < total {
        while started < total && active.len() < parallelism {
            active.push(spawn_polled(started, &command)?);
            started += 1;
        }
        metrics.sample();
        metrics.wakeups += 1;
        let mut progress = false;
        let mut index = 0;
        while index < active.len() {
            let child = &mut active[index];
            let mut buffer = [0; 4096];
            loop {
                match child.output.read(&mut buffer) {
                    Ok(0) => {
                        child.eof = true;
                        break;
                    }
                    Ok(count) => {
                        child.output_bytes += count;
                        progress = true;
                    }
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
                    Err(error) => return Err(error),
                }
            }
            if child.status.is_none() {
                child.status = child.child.try_wait()?;
                progress |= child.status.is_some();
            }
            if child.status.is_some() && child.eof {
                let child = active.swap_remove(index);
                if !child.status.expect("status checked").success() {
                    return Err(io::Error::other("polled child failed"));
                }
                metrics.output_bytes += child.output_bytes;
                finished += 1;
            } else {
                index += 1;
            }
        }
        if !progress {
            std::thread::sleep(Duration::from_micros(100));
        }
    }
    Ok(metrics)
}

#[cfg(all(unix, feature = "evented-runtime"))]
fn run_evented(total: usize, parallelism: usize, sleep_ms: u64) -> io::Result<Metrics> {
    use polling::{Event, Events, Poller};

    let command = shell_command(sleep_ms);
    let poller = Poller::new()?;
    let mut events = Events::new();
    let mut metrics = Metrics::default();
    let mut active = Vec::new();
    let mut started = 0;
    let mut finished = 0;
    while finished < total {
        while started < total && active.len() < parallelism {
            let child = spawn_polled(started, &command)?;
            // SAFETY: `active` owns each stream until it is removed from the
            // poller; moving a UnixStream does not invalidate its descriptor.
            unsafe {
                poller.add(&child.output, Event::readable(child.key))?;
            }
            active.push(child);
            started += 1;
        }
        metrics.sample();
        events.clear();
        poller.wait(&mut events, None)?;
        metrics.wakeups += 1;
        for event in events.iter() {
            let Some(index) = active.iter().position(|child| child.key == event.key) else {
                continue;
            };
            let child = &mut active[index];
            let mut buffer = [0; 4096];
            loop {
                match child.output.read(&mut buffer) {
                    Ok(0) => {
                        child.eof = true;
                        break;
                    }
                    Ok(count) => child.output_bytes += count,
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
                    Err(error) => return Err(error),
                }
            }
            if child.eof {
                poller.delete(&child.output)?;
                let mut child = active.swap_remove(index);
                let status = child.child.wait()?;
                if !status.success() {
                    return Err(io::Error::other("evented child failed"));
                }
                metrics.output_bytes += child.output_bytes;
                finished += 1;
            } else {
                poller.modify(&child.output, Event::readable(child.key))?;
            }
        }
    }
    Ok(metrics)
}

#[cfg(not(all(unix, feature = "evented-runtime")))]
fn run_evented(_total: usize, _parallelism: usize, _sleep_ms: u64) -> io::Result<Metrics> {
    Err(io::Error::other(
        "rebuild with --features evented-runtime on Unix",
    ))
}

#[cfg(not(unix))]
fn run_polling(_total: usize, _parallelism: usize, _sleep_ms: u64) -> io::Result<Metrics> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "polling probe requires Unix",
    ))
}

fn main() -> io::Result<()> {
    let mut arguments = std::env::args().skip(1);
    let strategy = arguments
        .next()
        .ok_or_else(|| io::Error::other("missing strategy"))?;
    let total = arguments
        .next()
        .ok_or_else(|| io::Error::other("missing total"))?
        .parse()
        .map_err(io::Error::other)?;
    let parallelism = arguments
        .next()
        .ok_or_else(|| io::Error::other("missing parallelism"))?
        .parse()
        .map_err(io::Error::other)?;
    let sleep_ms = arguments
        .next()
        .unwrap_or_else(|| "0".to_owned())
        .parse()
        .map_err(io::Error::other)?;
    let started = Instant::now();
    let mut metrics = match strategy.as_str() {
        "threaded" => run_threaded(total, parallelism, sleep_ms)?,
        "tokio" => run_tokio(total, parallelism, sleep_ms)?,
        "polling" => run_polling(total, parallelism, sleep_ms)?,
        "evented" => run_evented(total, parallelism, sleep_ms)?,
        _ => return Err(io::Error::other("unknown strategy")),
    };
    metrics.sample();
    if metrics.output_bytes != total {
        return Err(io::Error::other("output was lost"));
    }
    println!(
        "strategy={strategy} total={total} parallelism={parallelism} sleep_ms={sleep_ms} \
         elapsed_ms={:.3} peak_threads={} peak_rss_kib={} wakeups={}",
        started.elapsed().as_secs_f64() * 1_000.0,
        metrics.peak_threads,
        metrics.peak_rss_kib,
        metrics.wakeups
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_metrics(metrics: Metrics, expected_bytes: usize) {
        assert_eq!(metrics.output_bytes, expected_bytes);
        assert!(metrics.wakeups > 0);
        #[cfg(target_os = "linux")]
        {
            assert!(metrics.peak_threads > 0);
            assert!(metrics.peak_rss_kib > 0);
        }
    }

    #[test]
    fn shell_commands_render_zero_and_fractional_sleeps() {
        assert_eq!(shell_command(0), "printf x");
        assert_eq!(shell_command(1), "sleep 0.001; printf x");
        assert_eq!(shell_command(250), "sleep 0.250; printf x");
    }

    #[test]
    fn threaded_strategy_captures_every_child_byte() {
        assert_metrics(run_threaded(8, 4, 0).unwrap(), 8);
    }

    #[cfg(unix)]
    #[test]
    fn busy_polling_strategy_captures_every_child_byte() {
        assert_metrics(run_polling(8, 4, 0).unwrap(), 8);
    }

    #[cfg(all(unix, feature = "evented-runtime"))]
    #[test]
    fn evented_strategy_captures_every_child_byte() {
        assert_metrics(run_evented(8, 4, 0).unwrap(), 8);
    }

    #[cfg(feature = "tokio-runtime")]
    #[test]
    fn tokio_strategy_captures_every_child_byte() {
        assert_metrics(run_tokio(8, 4, 0).unwrap(), 8);
    }

    #[cfg(unix)]
    #[test]
    fn polled_child_uses_the_requested_key_and_combined_output() {
        let command = "printf out; printf err >&2";
        let mut child = spawn_polled(41, command).unwrap();
        #[cfg(feature = "evented-runtime")]
        assert_eq!(child.key, 41);
        child.output.set_nonblocking(false).unwrap();
        let mut output = Vec::new();
        child.output.read_to_end(&mut output).unwrap();
        assert!(child.child.wait().unwrap().success());
        assert_eq!(output, b"outerr");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn metrics_sample_observes_the_probe_process() {
        let mut metrics = Metrics::default();
        metrics.sample();
        assert!(metrics.peak_threads >= 1);
        assert!(metrics.peak_rss_kib > 0);
    }

    #[test]
    fn zero_child_runs_complete_without_wakeups() {
        let threaded = run_threaded(0, 1, 0).unwrap();
        assert_eq!(threaded.output_bytes, 0);
        assert_eq!(threaded.wakeups, 0);

        #[cfg(unix)]
        {
            let polling = run_polling(0, 1, 0).unwrap();
            assert_eq!(polling.output_bytes, 0);
            assert_eq!(polling.wakeups, 0);
        }
    }
}
