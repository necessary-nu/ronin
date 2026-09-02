//! A pseudo-terminal to run a build under.
//!
//! What a build shows a terminal is not what it shows a pipe — a status line
//! is overprinted on one and written whole on the other — and the only way to
//! assert the former is to give the tool a terminal. This opens one, sets its
//! width, turns off the line discipline's own rewriting of newlines so the
//! bytes read back are the tool's own, and gathers everything written to it.
//!
//! Included by `#[path]` for the same reason `support/scratch.rs` is.

use std::fs::File;
use std::io::Read;
use std::process::{Command, ExitStatus, Stdio};

/// `EIO`, which is what reading the master side reports once every holder of
/// the slave side has closed it — the pty's end of file.
const EIO: i32 = 5;

/// What a program wrote while it had the terminal.
pub struct Transcript {
    pub status: ExitStatus,
    /// Every byte written to the terminal, in order.
    pub screen: Vec<u8>,
    /// Standard error, which was a pipe rather than the terminal.
    pub stderr: Vec<u8>,
}

/// Run `command` with a terminal of `columns` width as its standard output.
///
/// `TERM` is set to something that is not `dumb`, and the colour overrides are
/// cleared, so the program sees the terminal Ninja calls smart and decides
/// colour from it alone.
pub fn run_under_terminal(mut command: Command, columns: u16) -> Transcript {
    use rustix::pty::{OpenptFlags, grantpt, openpt, ptsname, unlockpt};
    use rustix::termios::{
        OptionalActions, OutputModes, tcgetattr, tcgetwinsize, tcsetattr, tcsetwinsize,
    };

    let master = openpt(OpenptFlags::RDWR | OpenptFlags::NOCTTY | OpenptFlags::CLOEXEC)
        .expect("a pseudo-terminal");
    grantpt(&master).expect("the slave side is granted");
    unlockpt(&master).expect("the slave side is unlocked");
    let name = ptsname(&master, Vec::new()).expect("the slave side has a name");
    let slave = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(name.to_str().expect("a pty name is text"))
        .expect("the slave side opens");

    let mut size = tcgetwinsize(&slave).expect("the terminal reports a size");
    size.ws_col = columns;
    size.ws_row = 24;
    tcsetwinsize(&slave, size).expect("the terminal takes a size");
    // The line discipline would turn every `\n` into `\r\n`; the bytes under
    // test are the tool's, not the terminal's.
    let mut termios = tcgetattr(&slave).expect("the terminal reports its modes");
    termios.output_modes.remove(OutputModes::ONLCR);
    tcsetattr(&slave, OptionalActions::Now, &termios).expect("the terminal takes its modes");

    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::from(slave))
        .stderr(Stdio::piped())
        .env("TERM", "xterm")
        .env_remove("NO_COLOR")
        .env_remove("CLICOLOR_FORCE")
        .env_remove("FORCE_COLOR")
        .spawn()
        .expect("the program starts");
    // The `Command` keeps its copy of the slave side for as long as it lives,
    // and a slave side still open anywhere means the master never reports the
    // end of file: only the child may hold it now, so that its exit is the
    // end of the transcript.
    drop(command);

    let mut stderr = child.stderr.take().expect("standard error was piped");
    let stderr = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        stderr
            .read_to_end(&mut bytes)
            .expect("standard error reads to its end");
        bytes
    });

    let mut master = File::from(master);
    let mut screen = Vec::new();
    let mut buffer = [0_u8; 4096];
    loop {
        match master.read(&mut buffer) {
            Ok(0) => break,
            Ok(count) => screen.extend_from_slice(&buffer[..count]),
            Err(error) if error.raw_os_error() == Some(EIO) => break,
            Err(error) => panic!("reading the terminal: {error}"),
        }
    }
    let status = child.wait().expect("the program is reaped");
    let stderr = stderr.join().expect("the standard error reader finishes");
    Transcript {
        status,
        screen,
        stderr,
    }
}

/// What a terminal keeps of `bytes` once every overprint has happened.
///
/// A small model of the three controls the status line uses: a newline ends
/// a line, a carriage return goes back to its start, and an erase-to-end
/// discards what stands from the cursor on. Everything else is a character
/// written at the cursor, over whatever was there. The last line is included
/// whether or not it was ended, because it is on screen either way.
pub fn scrollback(bytes: &[u8]) -> Vec<String> {
    let mut lines = Vec::new();
    let mut line: Vec<u8> = Vec::new();
    let mut column = 0;
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'\n' => {
                lines.push(String::from_utf8_lossy(&line).into_owned());
                line.clear();
                column = 0;
            }
            b'\r' => column = 0,
            0x1b if bytes[index..].starts_with(b"\x1b[K") => {
                line.truncate(column);
                index += 2;
            }
            byte => {
                if column < line.len() {
                    line[column] = byte;
                } else {
                    line.push(byte);
                }
                column += 1;
            }
        }
        index += 1;
    }
    if !line.is_empty() {
        lines.push(String::from_utf8_lossy(&line).into_owned());
    }
    lines
}
