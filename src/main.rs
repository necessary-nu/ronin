fn main() {
    // [spec:samurai:req:compat.process-integration]
    if let Err(error) = ronin::subprocess::install_signal_handlers() {
        eprintln!("ronin: failed to install signal handlers: {error}");
        std::process::exit(1);
    }
    let arguments = std::env::args_os().collect::<Vec<_>>();
    // [spec:samurai:req:product.ronin-identity]
    // [spec:samurai:req:product.no-samuflags]
    match ronin::cli::run_os(&arguments) {
        Ok(output) => {
            if !output.is_empty() {
                println!("{output}");
            }
        }
        Err(error) => {
            eprintln!("ronin: {error}");
            if let Some(signal) = ronin::subprocess::interrupted_signal() {
                ronin::subprocess::reraise_signal(signal);
            }
            std::process::exit(1);
        }
    }
}
