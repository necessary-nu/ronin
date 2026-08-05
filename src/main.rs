use std::io::Write;

fn write_terminal(
    result: &ronin::RunResult,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> std::io::Result<()> {
    if !result.stdout.is_empty() {
        stdout.write_all(&result.stdout)?;
        stdout.flush()?;
    }
    if !result.stderr.is_empty() {
        stderr.write_all(&result.stderr)?;
        stderr.flush()?;
    }
    Ok(())
}

fn write_diagnostic(message: impl std::fmt::Display) -> std::io::Result<()> {
    let stderr = std::io::stderr();
    let mut stderr = stderr.lock();
    writeln!(stderr, "ronin: {message}")?;
    stderr.flush()
}

fn is_broken_pipe(mut error: &(dyn std::error::Error + 'static)) -> bool {
    loop {
        if error
            .downcast_ref::<std::io::Error>()
            .is_some_and(|error| error.kind() == std::io::ErrorKind::BrokenPipe)
        {
            return true;
        }
        let Some(source) = error.source() else {
            return false;
        };
        error = source;
    }
}

fn main() {
    // [spec:ronin:req:compat.process-integration]
    // [spec:ronin:req:runtime.guarded-signal-boundary]
    let signal_handlers = ronin::install_signal_handlers().unwrap_or_else(|error| {
        let _ = write_diagnostic(format_args!("failed to install signal handlers: {error}"));
        std::process::exit(1);
    });
    let arguments = std::env::args_os().collect::<Vec<_>>();
    let stdout = std::io::stdout();
    let mut stdout = stdout.lock();
    let stderr = std::io::stderr();
    let mut stderr = stderr.lock();
    // [spec:ronin:req:product.ronin-identity]
    // [spec:ronin:req:product.no-samuflags]
    // [spec:ronin:req:product.make-identity]
    // [spec:ronin:req:runtime.explicit-invocation-boundary]
    match ronin::run_process(&arguments, &mut stdout, &mut stderr) {
        Ok(result) => {
            if let Err(error) = write_terminal(&result, &mut stdout, &mut stderr) {
                if error.kind() != std::io::ErrorKind::BrokenPipe {
                    drop(stderr);
                    let _ = write_diagnostic(format_args!("writing terminal output: {error}"));
                    std::process::exit(1);
                }
            }
            if result.exit_code != 0 {
                std::process::exit(result.exit_code);
            }
        }
        Err(error) => {
            drop(stdout);
            drop(stderr);
            if is_broken_pipe(&error) {
                return;
            }
            if let Err(write_error) = write_diagnostic(&error) {
                if write_error.kind() != std::io::ErrorKind::BrokenPipe {
                    std::process::exit(1);
                }
            }
            // [spec:ronin:req:product.build-outcome]
            // Ninja leaves with 130 rather than dying by the signal it caught,
            // so an interrupt reports the same status whether it arrived while
            // the build was running or before it started. C samurai re-raised
            // here; Ninja is the contract.
            if signal_handlers.interrupted().is_some() {
                std::process::exit(ronin::INTERRUPTED_EXIT_CODE);
            }
            std::process::exit(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct BrokenWriter;

    impl Write for BrokenWriter {
        fn write(&mut self, _buffer: &[u8]) -> std::io::Result<usize> {
            Err(std::io::Error::from(std::io::ErrorKind::BrokenPipe))
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    // [spec:ronin:req:runtime.semantic-errors/test]
    #[test]
    fn terminal_writes_expose_broken_pipe_for_deliberate_handling() {
        let result = ronin::RunResult {
            stdout: b"output".to_vec(),
            stderr: Vec::new(),
            exit_code: 0,
        };
        let error = write_terminal(&result, &mut BrokenWriter, &mut Vec::new()).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::BrokenPipe);
    }

    #[test]
    fn broken_pipe_detection_follows_error_sources() {
        #[derive(Debug)]
        struct Wrapped(std::io::Error);

        impl std::fmt::Display for Wrapped {
            fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("wrapped output error")
            }
        }

        impl std::error::Error for Wrapped {
            fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
                Some(&self.0)
            }
        }

        let error = Wrapped(std::io::Error::from(std::io::ErrorKind::BrokenPipe));
        assert!(is_broken_pipe(&error));
    }
}
