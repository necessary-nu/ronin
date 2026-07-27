use super::*;

pub fn format_progress_status(state: &BuildState, template: &str) -> String {
    let mut output = String::new();
    let mut characters = template.chars();
    let elapsed = if state.finished == 0 {
        0.0
    } else {
        state.start.elapsed().as_secs_f64()
    };
    let format_duration = |seconds: f64| {
        let seconds = seconds.max(0.0).round() as u64;
        let hours = seconds / 3600;
        let minutes = seconds % 3600 / 60;
        let seconds = seconds % 60;
        if hours == 0 {
            format!("{minutes:02}:{seconds:02}")
        } else {
            format!("{hours}:{minutes:02}:{seconds:02}")
        }
    };
    while let Some(character) = characters.next() {
        if character != '%' {
            output.push(character);
            continue;
        }
        let Some(code) = characters.next() else {
            output.push('%');
            break;
        };
        match code {
            '%' => output.push('%'),
            's' => output.push_str(&state.started.to_string()),
            'f' => output.push_str(&state.finished.to_string()),
            't' => output.push_str(&state.total.to_string()),
            'r' => output.push_str(&(state.started - state.finished).to_string()),
            'u' => output.push_str(&(state.total - state.started).to_string()),
            'p' => output.push_str(&format!(
                "{:3}%",
                if state.total == 0 {
                    0
                } else {
                    100 * state.finished / state.total
                }
            )),
            'e' => output.push_str(&format!("{elapsed:.3}")),
            'E' => {
                if state.finished == 0 {
                    output.push('?');
                } else {
                    let remaining = state.total.saturating_sub(state.finished);
                    let estimate = elapsed * remaining as f64 / state.finished as f64;
                    output.push_str(&format_duration(estimate));
                }
            }
            'w' => output.push_str(&format_duration(elapsed)),
            _ => {
                output.push('%');
                output.push(code);
            }
        }
    }
    output
}

fn formatstatus(state: &BuildState) -> String {
    format_progress_status(state, &state.options.statusfmt)
}

fn printstatus(state: &BuildState, command: &SamuraiString) -> String {
    format!(
        "{}{}",
        formatstatus(state),
        String::from_utf8_lossy(&command.s[..command.n])
    )
}

fn jobstart(state: &mut BuildState, edge: EdgeRef, command: SamuraiString) -> Job {
    state.started += 1;
    Job {
        command,
        edge,
        output: Vec::new(),
        failed: false,
    }
}

fn nodedone(state: &mut BuildState, node: &NodeRef, prune: bool) {
    let uses = node
        .borrow()
        .uses
        .iter()
        .filter_map(|edge| edge.upgrade())
        .collect::<Vec<_>>();
    for edge in uses {
        let (prune_outputs, ready) = {
            let mut edge = edge.borrow_mut();
            if edge.flags & FLAG_WORK == 0 {
                continue;
            }
            let blocking_flag = if prune { FLAG_DIRTY_OUT } else { FLAG_DIRTY };
            let prune_outputs = if edge.flags & blocking_flag == 0 {
                edge.nprune = edge.nprune.saturating_sub(1);
                edge.nprune == 0
            } else {
                false
            };
            let ready = if prune_outputs {
                false
            } else {
                edge.nblock = edge.nblock.saturating_sub(1);
                edge.nblock == 0
            };
            (prune_outputs, ready)
        };
        if prune_outputs {
            let (outputs, counted_command) = {
                let edge = edge.borrow();
                (
                    edge.out.clone(),
                    edge.flags & FLAG_DIRTY != 0
                        && !edge.rule.as_ref().is_some_and(|rule| rule.name == "phony"),
                )
            };
            for output in outputs {
                nodedone(state, &output, true);
            }
            if counted_command {
                state.total = state.total.saturating_sub(1);
            }
        } else if ready {
            queue(state, edge);
        }
    }
}

fn shouldprune(edge: &EdgeRef, node: &NodeRef, old_mtime: i64) -> bool {
    if node.borrow().mtime != old_mtime {
        return false;
    }
    let (inputs, inorderidx) = {
        let edge = edge.borrow();
        (edge.input.clone(), edge.inorderidx)
    };
    let mut newest = None;
    for input in inputs.into_iter().take(inorderidx) {
        if crate::graph::nodestat(&input).is_err() {
            return false;
        }
        let mtime = input.borrow().mtime;
        if mtime != MTIME_MISSING
            && newest
                .as_ref()
                .is_none_or(|current: &NodeRef| current.borrow().mtime < mtime)
        {
            newest = Some(input);
        }
    }
    if let Some(newest) = newest {
        node.borrow_mut().logmtime = newest.borrow().mtime;
    }
    true
}

fn edgedone(state: &mut BuildState, edge: &EdgeRef) {
    let restat = crate::env::edgevar(edge, "restat", false).is_some_and(|value| value.n != 0);
    for output in &edge.borrow().out {
        let old = output.borrow().mtime;
        let _ = crate::graph::nodestat(output);
        let mtime = output.borrow().mtime;
        output.borrow_mut().logmtime = if mtime == MTIME_MISSING { 0 } else { mtime };
        nodedone(state, output, restat && shouldprune(edge, output, old));
    }
    if let Some(rspfile) = crate::env::edgevar(edge, "rspfile", false) {
        if rspfile.n != 0 && !state.options.keeprsp {
            let path = String::from_utf8_lossy(&rspfile.s[..rspfile.n]);
            let _ = fs::remove_file(path.as_ref());
        }
    }
    let command = crate::env::edgevar(edge, "command", true)
        .unwrap_or_else(|| SamuraiString { n: 0, s: vec![0] });
    let rspfile_content = crate::env::edgevar(edge, "rspfile_content", false);
    edgehash(edge, &command, rspfile_content.as_ref());
    let hash = edge.borrow().hash;
    for output in &edge.borrow().out {
        output.borrow_mut().hash = hash;
    }
}

fn jobdone(state: &mut BuildState, job: Job) {
    state.finished += 1;
    if let Some(pool) = &job.edge.borrow().pool {
        let mut pool = pool.borrow_mut();
        pool.numjobs = pool.numjobs.saturating_sub(1);
    }
    if !job.failed {
        edgedone(state, &job.edge);
    }
}

fn jobwork(job: &mut Job, bytes: &[u8]) -> bool {
    job.output.extend_from_slice(bytes);
    !bytes.is_empty()
}

pub(super) fn queryload() -> f64 {
    fs::read_to_string("/proc/loadavg")
        .ok()
        .and_then(|contents| contents.split_whitespace().next()?.parse().ok())
        .unwrap_or(0.0)
}

fn catchsig(signal: i32) -> i32 {
    signal
}

pub fn build(state: &mut BuildState) -> Vec<String> {
    let mut status = Vec::new();
    while !state.work.is_empty() {
        if state.options.maxload > 0.0 && queryload() > state.options.maxload {
            std::thread::yield_now();
        }
        let edge = state.work.remove(0);
        let command = crate::env::edgevar(&edge, "command", true)
            .unwrap_or_else(|| SamuraiString { n: 0, s: vec![0] });
        status.push(printstatus(state, &command));
        let mut job = jobstart(state, edge, command);
        if state.options.dryrun {
            jobwork(&mut job, &[]);
        } else {
            let command = String::from_utf8_lossy(&job.command.s[..job.command.n]);
            match Command::new("/bin/sh")
                .arg("-c")
                .arg(command.as_ref())
                .stdin(Stdio::null())
                .output()
            {
                Ok(result) => {
                    jobwork(&mut job, &result.stdout);
                    jobwork(&mut job, &result.stderr);
                    job.failed = !result.status.success();
                }
                Err(error) => {
                    job.output.extend_from_slice(error.to_string().as_bytes());
                    job.failed = true;
                }
            }
        }
        jobdone(state, job);
    }
    let _ = queryload();
    let _ = catchsig(0);
    status
}
