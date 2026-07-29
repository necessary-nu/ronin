fn main() {
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
            std::process::exit(1);
        }
    }
}
