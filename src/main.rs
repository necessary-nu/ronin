use std::io::Write;

fn main() {
    // [spec:samurai:req:compat.process-integration]
    if let Err(error) = ronin::install_signal_handlers() {
        eprintln!("ronin: failed to install signal handlers: {error}");
        std::process::exit(1);
    }
    let arguments = std::env::args_os().collect::<Vec<_>>();
    // [spec:samurai:req:product.ronin-identity]
    // [spec:samurai:req:product.no-samuflags]
    match ronin::run_os(&arguments) {
        Ok(result) => {
            if !result.stdout.is_empty() {
                let _ = std::io::stdout().lock().write_all(&result.stdout);
            }
            if !result.stderr.is_empty() {
                let _ = std::io::stderr().lock().write_all(&result.stderr);
            }
            if result.exit_code != 0 {
                std::process::exit(result.exit_code);
            }
        }
        Err(error) => {
            eprintln!("ronin: {error}");
            if let Some(signal) = ronin::interrupted_signal() {
                ronin::reraise_signal(signal);
            }
            std::process::exit(1);
        }
    }
}
